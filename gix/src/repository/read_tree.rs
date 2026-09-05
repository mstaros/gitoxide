use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use gix_error::{ErrorExt, Exn};

use crate::{
    Repository,
    bstr::{BString, ByteSlice},
    repository::sparse_checkout,
};

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Options for the two-tree form of [Repository::read_tree()].
///
/// The default is Git's two-tree -m operation. Select update_worktree for -u.
/// Reset is destructive and must be selected instead of merge.
#[derive(Clone, Debug)]
pub struct Options {
    /// Carry local index changes forward (-m).
    pub merge: bool,
    /// Also update affected working-tree paths (-u).
    pub update_worktree: bool,
    /// Discard ordinary working-tree modifications and overwrite obstructions (--reset).
    /// Git still checks assumed-valid and skip-worktree entries for modifications.
    /// Staged changes follow the two-tree carry-forward rules.
    pub reset: bool,
    /// Do not inspect or update the worktree (-i).
    pub index_only: bool,
    /// Perform validation without writing the index or working tree.
    pub dry_run: bool,
    /// Unsupported: bind a tree below a prefix.
    pub prefix: Option<BString>,
    /// Unsupported: publish a different index.
    pub index_output: Option<PathBuf>,
    /// Unsupported: aggressive three-tree merge rules.
    pub aggressive: bool,
    /// Unsupported: require a trivial merge.
    pub trivial: bool,
    /// Unsupported: change submodule checkouts recursively.
    pub recurse_submodules: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            merge: true,
            update_worktree: false,
            reset: false,
            index_only: false,
            dry_run: false,
            prefix: None,
            index_output: None,
            aggressive: false,
            trivial: false,
            recurse_submodules: false,
        }
    }
}

/// A successfully validated or applied two-tree transition.
#[derive(Debug)]
pub struct Outcome {
    /// Paths selected by the transition, including deletions already absent from the index.
    pub index_paths: Vec<BString>,
    /// Paths that were, or in a dry run would be, updated in the working tree.
    pub worktree_paths: Vec<BString>,
    /// Whether this outcome describes validation without application.
    pub dry_run: bool,
}

/// Errors from [Repository::read_tree()].
#[derive(Debug, thiserror::Error)]
#[expect(missing_docs)]
pub enum Error {
    #[error("Unsupported read-tree option or form: {option}")]
    Unsupported { option: &'static str },
    #[error("Invalid read-tree options: {reason}")]
    InvalidOptions { reason: &'static str },
    #[error("A working tree is required unless index_only is set")]
    MissingWorktree,
    #[error("Could not lock the index")]
    IndexLock(#[source] gix_lock::acquire::Error),
    #[error("Resolve the unmerged index before a two-tree merge")]
    UnmergedIndex,
    #[error("The transition would overwrite staged changes at {paths:?}")]
    IndexConflict { paths: Vec<BString> },
    #[error("The transition would overwrite working-tree changes at {paths:?}")]
    DirtyWorktree { paths: Vec<BString> },
    #[error("Working-tree paths obstruct the transition: {paths:?}")]
    Obstructed { paths: Vec<BString> },
    #[error("Could not prepare the two-tree transition: {operation}")]
    Prepare {
        operation: &'static str,
        #[source]
        source: BoxError,
    },
    #[error(
        "Transition from {old_tree_id} to {new_tree_id} did not finish; index publication did not complete at {}. Inspect these possibly changed working-tree paths before recovery: {possibly_changed_paths:?}",
        index_path.display()
    )]
    UpdateFailed {
        old_tree_id: gix_hash::ObjectId,
        new_tree_id: gix_hash::ObjectId,
        index_path: PathBuf,
        possibly_changed_paths: Vec<BString>,
        #[source]
        source: BoxError,
    },
}

fn prepare(operation: &'static str, source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Prepare { operation, source: Box::new(source) }
}

fn validate(trees: &[gix_hash::ObjectId], options: &Options) -> Result<(), Error> {
    for (set, option) in [
        (trees.len() != 2, "only the two-tree form is supported"),
        (options.prefix.is_some(), "--prefix"),
        (options.index_output.is_some(), "--index-output"),
        (options.aggressive, "--aggressive"),
        (options.trivial, "--trivial"),
        (options.recurse_submodules, "--recurse-submodules"),
    ] {
        if set {
            return Err(Error::Unsupported { option });
        }
    }
    if options.merge == options.reset {
        return Err(Error::InvalidOptions { reason: "select exactly one of merge and reset" });
    }
    if options.index_only && options.update_worktree {
        return Err(Error::InvalidOptions { reason: "index_only and update_worktree are mutually exclusive" });
    }
    Ok(())
}

fn entries(index: &gix_index::File) -> BTreeMap<BString, gix_index::Entry> {
    index.entries().iter().map(|entry| (entry.path(index).to_owned(), entry.clone())).collect()
}

fn same(left: Option<&gix_index::Entry>, right: Option<&gix_index::Entry>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.id == right.id && left.mode == right.mode,
        _ => false,
    }
}

impl Repository {
    /// Carry the index from the first tree to the second using Git's two-tree merge rules.
    ///
    /// Both inputs must be tree object IDs. Local index changes on unaffected paths,
    /// and changes already matching the new tree, are retained. Otherwise the index
    /// must match the old tree and affected tracked files must be clean.
    ///
    /// The index is opened from disk after acquiring its lock. All overlap checks
    /// precede worktree changes, and index publication follows successful application.
    /// A dry run leaves index and worktree bytes unchanged. No reference is moved.
    ///
    /// An I/O or filter failure during application can leave a partial checkout;
    /// [Error::UpdateFailed] records both trees, the index path and possibly changed
    /// paths. Callers must inspect that evidence before retrying or restoring files.
    /// Coordinate other writers through the caller's repository lease: the index
    /// lock cannot serialize editors which write working-tree files directly.
    ///
    /// One-tree and three-tree forms, compressed sparse indexes and the explicitly
    /// unsupported options in [Options] are rejected. Cone and pattern sparse checkout
    /// with an ordinary index are supported.
    pub fn read_tree(&self, trees: &[gix_hash::ObjectId], options: Options) -> Result<Outcome, Exn<Error>> {
        self.read_tree_inner(trees, &options).map_err(ErrorExt::raise)
    }

    fn read_tree_inner(&self, trees: &[gix_hash::ObjectId], options: &Options) -> Result<Outcome, Error> {
        validate(trees, options)?;
        let workdir = if options.index_only {
            None
        } else {
            Some(self.workdir().ok_or(Error::MissingWorktree)?)
        };
        let index_path = self.index_path();
        let mut lock = gix_lock::File::acquire_to_update_resource(
            &index_path, gix_lock::acquire::Fail::Immediately, None,
        ).map_err(Error::IndexLock)?;
        let current = match self.open_index() {
            Ok(index) => index,
            Err(crate::worktree::open_index::Error::IndexFile(gix_index::file::init::Error::Io(err)))
                if err.kind() == std::io::ErrorKind::NotFound => {
                    gix_index::File::from_state(gix_index::State::new(self.object_hash()), index_path.clone())
                }
            Err(err) => return Err(prepare("open index", err)),
        };
        if current.is_sparse() {
            return Err(Error::Unsupported { option: "compressed sparse index" });
        }
        let unmerged: BTreeSet<_> = current.entries().iter()
            .filter(|entry| entry.stage_raw() != 0)
            .map(|entry| entry.path(&current).to_owned()).collect();
        if !options.reset && !unmerged.is_empty() {
            return Err(Error::UnmergedIndex);
        }
        let old = self.index_from_tree(&trees[0]).map_err(|err| prepare("read old tree", err))?;
        let new = self.index_from_tree(&trees[1]).map_err(|err| prepare("read new tree", err))?;
        let old_entries = entries(&old);
        let new_entries = entries(&new);
        let current_entries = entries(&current);
        let paths: BTreeSet<_> = old_entries.keys().chain(new_entries.keys())
            .chain(current_entries.keys()).cloned().collect();
        let initial = current.entries().is_empty();
        let mut changes = BTreeMap::new();
        let mut conflicts = Vec::new();

        // Compare entry values independently of path traversal and filesystem state.
        // Unchanged target paths and already-staged target values carry forward.
        for path in paths {
            let before = old_entries.get(&path);
            let after = new_entries.get(&path);
            let staged = current_entries.get(&path);
            if unmerged.contains(&path) {
                changes.insert(path, after.cloned());
            } else if staged.is_some() {
                if same(before, after) || same(staged, after) {
                    continue;
                }
                if same(staged, before) {
                    changes.insert(path, after.cloned());
                } else {
                    conflicts.push(path);
                }
            } else if after.is_some() && before.is_some() && !initial {
                if !same(before, after) {
                    conflicts.push(path);
                }
            } else if before.is_some() || after.is_some() {
                changes.insert(path, after.cloned());
            }
        }
        if !conflicts.is_empty() {
            return Err(Error::IndexConflict { paths: conflicts });
        }

        let mut result = current.clone();
        result.remove_entries(|_, path, _| changes.contains_key(path));
        let sparse = if current.entries().is_empty() {
            None
        } else {
            self.list_sparse_checkout().map_err(|err| prepare("read sparse definition", err))?
        };
        let case = if self.config.ignore_case {
            gix_glob::pattern::Case::Fold
        } else {
            gix_glob::pattern::Case::Sensitive
        };
        let mut pattern_search = gix_ignore::Search::default();
        if let Some((gix_index::sparse::Mode::IncludeByIgnorePatternStoreAllEntriesSkipUnmatched, patterns)) = &sparse {
            let mut bytes = Vec::new();
            for pattern in patterns {
                bytes.extend_from_slice(pattern);
                bytes.push(b'\n');
            }
            pattern_search.add_patterns_buffer(
                &bytes, self.git_dir().join("info/sparse-checkout"), None, Default::default(),
            );
        }
        for (path, replacement) in &mut changes {
            let Some(entry) = replacement else { continue };
            entry.flags = gix_index::entry::Flags::empty();
            entry.stat = Default::default();
            let included = match &sparse {
                None => true,
                Some((gix_index::sparse::Mode::IncludeByIgnorePatternStoreAllEntriesSkipUnmatched, _)) => {
                    sparse_checkout::pattern_includes(path.as_bstr(), &pattern_search, case)
                }
                Some((_, directories)) => sparse_checkout::cone_includes(path.as_bstr(), directories, case),
            };
            if !included {
                entry.flags.insert(gix_index::entry::Flags::EXTENDED | gix_index::entry::Flags::SKIP_WORKTREE);
            }
            result.dangerously_push_entry(entry.stat, entry.id, entry.flags, entry.mode, path.as_bstr());
        }
        result.sort_entries();
        result.remove_tree();
        reject_colliding_paths(&result, self.config.ignore_case)?;
        let mut worktree_paths = Vec::new();
        if let Some(workdir) = workdir {
            let mut candidates = Vec::new();
            let removable: BTreeSet<BString> = changes.keys()
                .filter(|path| current_entries.contains_key(*path))
                .map(|path| if self.config.ignore_case { path.to_ascii_lowercase().into() } else { path.clone() })
                .collect();
            let mut excludes = self.excludes(
                &current,
                None,
                gix_worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            ).map_err(|err| prepare("read ignore rules", err))?;
            let positions: BTreeMap<_, _> = current.entries().iter().enumerate()
                .map(|(idx, entry)| (entry.path(&current), idx)).collect();
            for (path, replacement) in &changes {
                let before = current_entries.get(path);
                let both_skipped = before.is_some_and(skipped) && replacement.as_ref().is_none_or(skipped);
                if both_skipped {
                    continue;
                }
                let mode = replacement.as_ref().or(before).or_else(|| old_entries.get(path))
                    .expect("a changed path occurs in at least one input").mode;
                let disk_path = sparse_checkout::entry_worktree_path(self, workdir, path.as_bstr(), mode)
                    .map_err(|err| prepare("validate worktree path", err))?;
                if before.is_some_and(|entry| entry.mode == gix_index::entry::Mode::COMMIT) {
                    if replacement.as_ref().is_some_and(|entry| entry.mode != gix_index::entry::Mode::COMMIT) {
                        return Err(Error::Unsupported { option: "replacing a submodule working directory" });
                    }
                    continue;
                }
                if let Some(before) = before {
                    let presence = sparse_checkout::path_presence(self, workdir, path.as_bstr(), before.mode)
                        .map_err(|err| prepare("inspect tracked path", err))?;
                    let check_despite_reset = before.flags.intersects(
                        gix_index::entry::Flags::ASSUME_VALID | gix_index::entry::Flags::SKIP_WORKTREE,
                    );
                    if (!options.reset || check_despite_reset) && presence != sparse_checkout::Presence::Missing {
                        if let Some(idx) = positions.get(path.as_bstr()) {
                            candidates.push(*idx);
                        }
                    }
                }
                if options.update_worktree && replacement.as_ref().is_none_or(|entry| !skipped(entry)) {
                    if before.is_none() {
                        let extra = safe_destination(
                            workdir, &disk_path, &removable, self.config.ignore_case,
                            &mut excludes, options.reset,
                        ).map_err(|err| prepare("inspect checkout obstruction", err))?;
                        let Some(extra) = extra else {
                            return Err(Error::Obstructed { paths: vec![path.clone()] });
                        };
                        // An already-staged deletion only verifies absence; it must
                        // not remove a file the caller subsequently left untracked.
                        if replacement.is_some() {
                            worktree_paths.extend(extra);
                        }
                    }
                    if (before.is_some() || replacement.is_some())
                        && replacement.as_ref().is_none_or(|entry| entry.mode != gix_index::entry::Mode::COMMIT)
                    {
                        worktree_paths.push(path.clone());
                    }
                }
            }
            let dirty = sparse_checkout::dirty_entries(self, workdir, &current, &candidates)
                .map_err(|err| prepare("inspect tracked modifications", err))?;
            if !dirty.is_empty() {
                return Err(Error::DirtyWorktree {
                    paths: dirty.into_iter().map(|idx| current.entries()[idx].path(&current).to_owned()).collect(),
                });
            }
        }
        worktree_paths.sort();
        worktree_paths.dedup();
        let outcome = Outcome {
            index_paths: changes.keys().cloned().collect(),
            worktree_paths,
            dry_run: options.dry_run,
        };
        if options.dry_run {
            return Ok(outcome);
        }
        let recovery = |source: BoxError| Error::UpdateFailed {
            old_tree_id: trees[0],
            new_tree_id: trees[1],
            index_path: index_path.clone(),
            possibly_changed_paths: outcome.worktree_paths.clone(),
            source,
        };
        if options.update_worktree {
            apply_worktree(
                self, workdir.expect("validated worktree"), &mut result,
                &outcome.worktree_paths, options.reset,
            ).map_err(&recovery)?;
        }
        result.write_to(&mut lock, Default::default())
            .map_err(|err| recovery(Box::new(err)))?;
        lock.commit().map_err(|err| recovery(Box::new(err)))?;
        Ok(outcome)
    }
}

fn skipped(entry: &gix_index::Entry) -> bool {
    entry.flags.contains(gix_index::entry::Flags::SKIP_WORKTREE)
}

fn reject_colliding_paths(index: &gix_index::File, ignore_case: bool) -> Result<(), Error> {
    let mut seen = BTreeSet::new();
    for entry in index.entries() {
        let path = entry.path(index);
        let key = if ignore_case { path.to_ascii_lowercase() } else { path.to_vec() };
        if !seen.insert(key.clone()) {
            return Err(Error::IndexConflict { paths: vec![path.to_owned()] });
        }
    }
    // Case folding can change lexical order, so check ancestors against the
    // complete set, including parents encountered later in the index.
    for entry in index.entries() {
        let path = entry.path(index);
        let key = if ignore_case { path.to_ascii_lowercase() } else { path.to_vec() };
        for slash in key.iter().enumerate().filter_map(|(idx, byte)| (*byte == b'/').then_some(idx)) {
            if seen.contains(&key[..slash]) {
                return Err(Error::IndexConflict { paths: vec![path.to_owned()] });
            }
        }
    }
    Ok(())
}

/// Check obstructions without following symlinks, collecting ignored paths that
/// Git permits checkout to replace. None means an untracked path must be retained.
fn safe_destination(
    workdir: &Path,
    path: &Path,
    removable: &BTreeSet<BString>,
    ignore_case: bool,
    excludes: &mut crate::AttributeStack<'_>,
    reset: bool,
) -> std::io::Result<Option<Vec<BString>>> {
    let relative = path.strip_prefix(workdir).map_err(std::io::Error::other)?;
    let mut cursor = workdir.to_owned();
    for component in relative.components() {
        cursor.push(component);
        match std::fs::symlink_metadata(&cursor) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Some(Vec::new())),
            Err(err) => return Err(err),
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return removable_file(workdir, &cursor, removable, ignore_case, excludes, reset);
                }
                if cursor == path {
                    let mut extra = Vec::new();
                    for item in std::fs::read_dir(path)? {
                        let Some(paths) = safe_destination(
                            workdir, &item?.path(), removable, ignore_case, excludes, reset,
                        )? else {
                            return Ok(None);
                        };
                        extra.extend(paths);
                    }
                    return Ok(Some(extra));
                }
            }
        }
    }
    Ok(Some(Vec::new()))
}

fn removable_file(
    workdir: &Path,
    path: &Path,
    removable: &BTreeSet<BString>,
    ignore_case: bool,
    excludes: &mut crate::AttributeStack<'_>,
    reset: bool,
) -> std::io::Result<Option<Vec<BString>>> {
    let relative = path.strip_prefix(workdir).map_err(std::io::Error::other)?;
    let key = gix_path::to_unix_separators_on_windows(gix_path::into_bstr(relative.to_owned()));
    let owned = if ignore_case {
        removable.contains(key.to_ascii_lowercase().as_bstr())
    } else {
        removable.contains(key.as_ref())
    };
    if owned {
        Ok(Some(Vec::new()))
    } else if reset || excludes.at_path(relative, None)?.is_excluded() {
        Ok(Some(vec![key.into_owned()]))
    } else {
        Ok(None)
    }
}

/// Remove only empty directory trees. A file appearing after preflight causes
/// application to fail with recovery evidence instead of being deleted.
fn remove_empty_directory_tree(path: &Path) -> std::io::Result<()> {
    for item in std::fs::read_dir(path)? {
        let item = item?;
        if item.file_type()?.is_dir() {
            remove_empty_directory_tree(&item.path())?;
        }
    }
    std::fs::remove_dir(path)
}

/// Never follow an obstructing ancestor while removing a planned destination.
/// A tracked ancestor is removed by its own entry; checkout then creates the directory.
fn parents_are_directories(workdir: &Path, path: &Path) -> std::io::Result<bool> {
    let parent = path.parent().expect("validated repository-relative file");
    let relative = parent.strip_prefix(workdir).map_err(std::io::Error::other)?;
    let mut cursor = workdir.to_owned();
    for component in relative.components() {
        cursor.push(component);
        match std::fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Ok(false),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err),
        }
    }
    Ok(true)
}

fn apply_worktree(
    repo: &Repository,
    workdir: &Path,
    index: &mut gix_index::File,
    paths: &[BString],
    reset: bool,
) -> Result<(), BoxError> {
    if paths.is_empty() {
        return Ok(());
    }
    // Delete deepest paths first so directory/file transitions can remove empty parents.
    let mut removal = paths.to_vec();
    removal.sort_by_key(|path| std::cmp::Reverse(path.len()));
    for path in &removal {
        let disk_path = sparse_checkout::entry_worktree_path(repo, workdir, path.as_bstr(), gix_index::entry::Mode::FILE)?;
        if !parents_are_directories(workdir, &disk_path)? {
            continue;
        }
        match std::fs::symlink_metadata(&disk_path) {
            Ok(metadata) if metadata.is_dir() => {
                if reset {
                    std::fs::remove_dir_all(&disk_path)?;
                } else {
                    remove_empty_directory_tree(&disk_path)?;
                }
            }
            Ok(_) => std::fs::remove_file(&disk_path)?,
            Err(err) if matches!(err.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {}
            Err(err) => return Err(Box::new(err)),
        }
        sparse_checkout::remove_empty_parents(disk_path.parent(), workdir);
    }
    let selected: BTreeSet<_> = paths.iter().cloned().collect();
    let mut checkout_index = index.clone();
    let mut materialized = Vec::new();
    for (idx, (entry, path)) in checkout_index.entries_mut_with_paths().enumerate() {
        if selected.contains(path) && entry.mode != gix_index::entry::Mode::COMMIT && !skipped(entry) {
            entry.flags.remove(gix_index::entry::Flags::UPTODATE);
            materialized.push(idx);
        } else {
            entry.flags.insert(gix_index::entry::Flags::EXTENDED | gix_index::entry::Flags::SKIP_WORKTREE);
        }
    }
    let mut options = repo.checkout_options(gix_worktree::stack::state::attributes::Source::IdMapping)?;
    options.destination_is_initially_empty = false;
    options.overwrite_existing = reset;
    options.keep_going = false;
    options.thread_limit = Some(1);
    let outcome = gix_worktree_state::checkout(
        &mut checkout_index, workdir, repo.objects.clone().into_arc()?,
        &gix_features::progress::Discard, &gix_features::progress::Discard,
        &AtomicBool::default(), options,
    )?;
    if !outcome.collisions.is_empty() || !outcome.errors.is_empty()
        || !outcome.delayed_paths_unknown.is_empty() || !outcome.delayed_paths_unprocessed.is_empty()
    {
        return Err(Box::new(std::io::Error::other(format!("checkout incomplete: {outcome:?}"))));
    }
    for idx in materialized {
        index.entries_mut()[idx].stat = checkout_index.entries()[idx].stat;
    }
    Ok(())
}
