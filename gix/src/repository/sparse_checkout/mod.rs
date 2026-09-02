use std::{
    collections::{BTreeSet, HashSet},
    convert::Infallible,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use crate::bstr::{BStr, BString, ByteSlice};

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Errors returned by sparse-checkout porcelain operations.
#[derive(Debug, thiserror::Error)]
#[expect(missing_docs)]
pub enum Error {
    #[error("A worktree is required for sparse checkout")]
    MissingWorktree,
    #[error("The sparse-checkout mode {0:?} cannot be set by this API")]
    UnsupportedMode(gix_index::sparse::Mode),
    #[error("Could not expand the sparse index")]
    SparseIndexExpand(#[source] gix_index::sparse::expand::Error),
    #[error("Could not compress the sparse index")]
    SparseIndexCompress(#[source] gix_index::sparse::compress::Error),
    #[error("Sparse checkout cannot be changed while the index contains unmerged entries")]
    UnmergedIndex,
    #[error("Invalid sparse-checkout pattern {pattern:?}: {reason}")]
    InvalidPattern { pattern: BString, reason: &'static str },
    #[error("Invalid cone-mode directory {directory:?}: {reason}")]
    InvalidConeDirectory { directory: BString, reason: String },
    #[error("Invalid path in the index: {path:?}: {reason}")]
    InvalidIndexPath { path: BString, reason: String },
    #[error("The cone-mode sparse-checkout definition is not canonical: {line:?}")]
    InvalidConeDefinition { line: BString },
    #[error("Could not read the sparse-checkout definition at {}", path.display())]
    DefinitionRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Could not create or write the sparse-checkout definition at {}", path.display())]
    DefinitionWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Could not acquire a lock for the sparse-checkout definition at {}", path.display())]
    DefinitionLock {
        path: PathBuf,
        #[source]
        source: gix_lock::acquire::Error,
    },
    #[error("Could not open the index")]
    OpenIndex(#[source] BoxError),
    #[error("Could not prepare checkout options")]
    CheckoutOptions(#[source] BoxError),
    #[error("Could not prepare the object database for checkout")]
    ObjectStore(#[source] std::io::Error),
    #[error("Could not materialize sparse-checkout paths")]
    Checkout(#[source] gix_worktree_state::checkout::Error),
    #[error("Sparse checkout did not materialize every requested path: {outcome:?}")]
    CheckoutIncomplete {
        outcome: Box<gix_worktree_state::checkout::Outcome>,
    },
    #[error("Could not write the updated index")]
    IndexWrite(#[source] gix_index::file::write::Error),
    #[error("Could not inspect worktree paths")]
    WorktreeInspect(#[source] std::io::Error),
    #[error("Could not prepare status detection for paths leaving the sparse definition")]
    StatusSetup(#[source] BoxError),
    #[error("Could not compare paths leaving the sparse definition with the index")]
    Status(#[source] gix_status::index_as_worktree::Error),
    #[error("Could not read configuration at {}", path.display())]
    ConfigRead {
        path: PathBuf,
        #[source]
        source: gix_config::file::init::from_paths::Error,
    },
    #[error("Could not update configuration")]
    ConfigSet(#[from] gix_config::file::set_raw_value::Error),
    #[error("Could not acquire a configuration lock at {}", path.display())]
    ConfigLock {
        path: PathBuf,
        #[source]
        source: gix_lock::acquire::Error,
    },
    #[error("Could not write configuration at {}", path.display())]
    ConfigWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Could not commit configuration at {}", path.display())]
    ConfigCommit {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Could not reload the repository after changing sparse-checkout configuration")]
    Reload(#[source] crate::open::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Presence {
    Missing,
    Present,
    Obstructed,
}

enum Definition {
    Cone {
        directories: Vec<BString>,
        bytes: Vec<u8>,
    },
    Patterns {
        bytes: Vec<u8>,
    },
}

impl crate::Repository {
    /// Set the sparse-checkout definition and update the worktree and index to match it.
    ///
    /// Cone mode accepts repository-relative directory names. Pattern mode accepts one
    /// gitignore-syntax pattern per item, with sparse-checkout polarity: a positive match
    /// includes a path and a leading exclamation mark excludes it. The last matching pattern
    /// wins and unmatched paths are excluded.
    ///
    /// Use IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs for Git-compatible
    /// `--sparse-index` cone mode, IncludeDirectoriesStoreAllEntriesSkipUnmatched for
    /// a full cone-mode index, or IncludeByIgnorePatternStoreAllEntriesSkipUnmatched
    /// for pattern mode.
    pub fn set_sparse_checkout(
        &mut self,
        mode: gix_index::sparse::Mode,
        items: impl IntoIterator<Item = impl Into<BString>>,
    ) -> Result<(), Error> {
        let workdir = self.workdir().ok_or(Error::MissingWorktree)?.to_owned();
        let mut index = self
            .open_index()
            .map_err(|err| Error::OpenIndex(Box::new(err)))?;
        let validate = self
            .config
            .protect_options()
            .map_err(|err| Error::StatusSetup(Box::new(err)))?;
        index
            .expand_sparse_index(&*self, validate)
            .map_err(Error::SparseIndexExpand)?;
        reject_unrepresentable_index(&index)?;

        let definition = match mode {
            gix_index::sparse::Mode::IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs
            | gix_index::sparse::Mode::IncludeDirectoriesStoreAllEntriesSkipUnmatched => {
                let directories = normalize_cone_directories(self, items.into_iter().map(Into::into))?;
                let bytes = cone_definition(&directories);
                Definition::Cone { directories, bytes }
            }
            gix_index::sparse::Mode::IncludeByIgnorePatternStoreAllEntriesSkipUnmatched => {
                let patterns = normalize_patterns(items.into_iter().map(Into::into))?;
                let bytes = pattern_definition(&patterns);
                Definition::Patterns { bytes }
            }
            other => return Err(Error::UnsupportedMode(other)),
        };

        let definition_path = sparse_checkout_path(self);
        write_locked_bytes(&definition_path, definition.bytes())?;

        let paths = index_paths(&index);
        let case = if self.config.ignore_case {
            gix_glob::pattern::Case::Fold
        } else {
            gix_glob::pattern::Case::Sensitive
        };
        let included = match &definition {
            Definition::Cone { directories, .. } => paths
                .iter()
                .map(|path| cone_includes(path.as_bstr(), directories, case))
                .collect::<Vec<_>>(),
            Definition::Patterns { bytes } => {
                let mut search = gix_ignore::Search::default();
                search.add_patterns_buffer(
                    bytes,
                    definition_path.clone(),
                    None,
                    gix_ignore::search::Ignore::default(),
                );
                paths
                    .iter()
                    .map(|path| pattern_includes(path.as_bstr(), &search, case))
                    .collect::<Vec<_>>()
            }
        };

        let mut presence = Vec::with_capacity(paths.len());
        for (idx, path) in paths.iter().enumerate() {
            presence.push(path_presence(
                self,
                &workdir,
                path.as_bstr(),
                index.entries()[idx].mode,
            )?);
        }

        let materialize: Vec<usize> = included
            .iter()
            .zip(&presence)
            .zip(index.entries())
            .enumerate()
            .filter_map(|(idx, ((included, presence), entry))| {
                (*included
                    && *presence == Presence::Missing
                    && entry.mode != gix_index::entry::Mode::COMMIT
                    && !entry.flags.contains(gix_index::entry::Flags::INTENT_TO_ADD))
                .then_some(idx)
            })
            .collect();
        materialize_entries(self, &workdir, &mut index, &materialize)?;

        let removable: Vec<usize> = included
            .iter()
            .zip(&presence)
            .zip(index.entries())
            .enumerate()
            .filter_map(|(idx, ((included, presence), entry))| {
                (!*included
                    && *presence == Presence::Present
                    && entry.mode != gix_index::entry::Mode::COMMIT
                    && !entry.flags.contains(gix_index::entry::Flags::INTENT_TO_ADD))
                .then_some(idx)
            })
            .collect();
        let dirty = dirty_entries(self, &workdir, &index, &removable)?;

        let removable: BTreeSet<usize> = removable
            .into_iter()
            .filter(|idx| !dirty.contains(idx))
            .collect();
        for (idx, entry) in index.entries_mut().iter_mut().enumerate() {
            if included[idx] {
                clear_skip_worktree(entry);
            } else {
                match presence[idx] {
                    Presence::Missing => set_skip_worktree(entry),
                    Presence::Present if removable.contains(&idx) => set_skip_worktree(entry),
                    Presence::Present | Presence::Obstructed => clear_skip_worktree(entry),
                }
            }
        }
        write_index(&mut index)?;

        let mut removal_failed = Vec::new();
        for idx in removable {
            let path = entry_worktree_path(
                self,
                &workdir,
                paths[idx].as_bstr(),
                index.entries()[idx].mode,
            )?;
            if !remove_tracked_path(&path) {
                removal_failed.push(idx);
            } else {
                remove_empty_parents(path.parent(), &workdir);
            }
        }
        if !removal_failed.is_empty() {
            for idx in removal_failed {
                clear_skip_worktree(&mut index.entries_mut()[idx]);
            }
            write_index(&mut index)?;
        }

        if mode == gix_index::sparse::Mode::IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs {
            index
                .convert_to_sparse_index(|tree| gix_object::Write::write(&*self, tree))
                .map_err(Error::SparseIndexCompress)?;
            enable_sparse_config(self, mode)?;
            write_index(&mut index)?;
        } else {
            enable_sparse_config(self, mode)?;
        }
        Ok(())
    }

    /// Disable sparse checkout, materialize all missing tracked files, and clear every
    /// SKIP_WORKTREE bit in the index.
    ///
    /// The sparse-checkout definition is intentionally retained on disk. Cone-mode
    /// configuration and extensions.worktreeConfig are left unchanged.
    pub fn disable_sparse_checkout(&mut self) -> Result<(), Error> {
        let workdir = self.workdir().ok_or(Error::MissingWorktree)?.to_owned();
        let mut index = self
            .open_index()
            .map_err(|err| Error::OpenIndex(Box::new(err)))?;
        let validate = self
            .config
            .protect_options()
            .map_err(|err| Error::StatusSetup(Box::new(err)))?;
        index
            .expand_sparse_index(&*self, validate)
            .map_err(Error::SparseIndexExpand)?;
        reject_unrepresentable_index(&index)?;

        let paths = index_paths(&index);
        let mut materialize = Vec::new();
        for (idx, entry) in index.entries().iter().enumerate() {
            if entry.mode == gix_index::entry::Mode::COMMIT
                || entry.flags.contains(gix_index::entry::Flags::INTENT_TO_ADD)
            {
                continue;
            }
            if path_presence(self, &workdir, paths[idx].as_bstr(), entry.mode)? == Presence::Missing {
                materialize.push(idx);
            }
        }
        materialize_entries(self, &workdir, &mut index, &materialize)?;

        for entry in index.entries_mut() {
            clear_skip_worktree(entry);
        }
        write_index(&mut index)?;

        set_sparse_checkout_enabled(self, false)?;
        Ok(())
    }

    /// Return the active sparse-checkout mode and its user-facing definition.
    ///
    /// This is gated by core.sparseCheckout; a retained definition file does not make
    /// sparse checkout active after disable_sparse_checkout().
    pub fn list_sparse_checkout(
        &self,
    ) -> Result<Option<(gix_index::sparse::Mode, Vec<BString>)>, Error> {
        let index = self
            .open_index()
            .map_err(|err| Error::OpenIndex(Box::new(err)))?;
        let mode = index.sparse_mode();
        if mode == gix_index::sparse::Mode::Disabled {
            return Ok(None);
        }

        let path = sparse_checkout_path(self);
        let bytes = std::fs::read(&path).map_err(|source| Error::DefinitionRead {
            path: path.clone(),
            source,
        })?;
        let items = match mode {
            gix_index::sparse::Mode::IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs
            | gix_index::sparse::Mode::IncludeDirectoriesStoreAllEntriesSkipUnmatched => {
                parse_cone_definition(&bytes)?
            }
            gix_index::sparse::Mode::IncludeByIgnorePatternStoreAllEntriesSkipUnmatched => {
                active_pattern_lines(&bytes)
            }
            gix_index::sparse::Mode::Disabled => return Ok(None),
        };
        Ok(Some((mode, items)))
    }
}

impl Definition {
    fn bytes(&self) -> &[u8] {
        match self {
            Definition::Cone { bytes, .. } | Definition::Patterns { bytes } => bytes,
        }
    }
}

fn reject_unrepresentable_index(index: &gix_index::File) -> Result<(), Error> {
    if index.entries().iter().any(|entry| entry.stage_raw() != 0) {
        return Err(Error::UnmergedIndex);
    }
    Ok(())
}

fn index_paths(index: &gix_index::File) -> Vec<BString> {
    index
        .entries()
        .iter()
        .map(|entry| entry.path_in(index.path_backing()).to_owned())
        .collect()
}

fn set_skip_worktree(entry: &mut gix_index::Entry) {
    entry.flags.insert(
        gix_index::entry::Flags::EXTENDED | gix_index::entry::Flags::SKIP_WORKTREE,
    );
}

fn clear_skip_worktree(entry: &mut gix_index::Entry) {
    entry.flags.remove(gix_index::entry::Flags::SKIP_WORKTREE);
    if !entry.flags.intersects(
        gix_index::entry::Flags::SKIP_WORKTREE | gix_index::entry::Flags::INTENT_TO_ADD,
    ) {
        entry.flags.remove(gix_index::entry::Flags::EXTENDED);
    }
}

fn write_index(index: &mut gix_index::File) -> Result<(), Error> {
    index.remove_tree();
    index.write(Default::default()).map_err(Error::IndexWrite)
}

fn normalize_patterns(patterns: impl Iterator<Item = BString>) -> Result<Vec<BString>, Error> {
    let mut out = Vec::new();
    for pattern in patterns {
        if pattern.is_empty() {
            return Err(Error::InvalidPattern {
                pattern,
                reason: "patterns must not be empty",
            });
        }
        if pattern.iter().any(|byte| matches!(*byte, 0 | b'\r' | b'\n')) {
            return Err(Error::InvalidPattern {
                pattern,
                reason: "patterns must contain exactly one line and no NUL",
            });
        }
        let mut parsed = gix_ignore::parse(&pattern, false);
        if parsed.next().is_none() || parsed.next().is_some() {
            return Err(Error::InvalidPattern {
                pattern,
                reason: "the item is a comment or is not one active ignore pattern",
            });
        }
        out.push(pattern);
    }
    Ok(out)
}

fn pattern_definition(patterns: &[BString]) -> Vec<u8> {
    let mut out = Vec::new();
    for pattern in patterns {
        out.extend_from_slice(pattern);
        out.push(b'\n');
    }
    out
}

fn pattern_includes(
    path: &BStr,
    search: &gix_ignore::Search,
    case: gix_glob::pattern::Case,
) -> bool {
    let mut best = None;
    for slash in path.find_iter(b"/") {
        let directory = path[..slash].as_bstr();
        if let Some(matched) = search.pattern_matching_relative_path(directory, Some(true), case) {
            if best
                .as_ref()
                .is_none_or(|(_, sequence)| matched.sequence_number >= *sequence)
            {
                best = Some((!matched.pattern.is_negative(), matched.sequence_number));
            }
        }
    }
    if let Some(matched) = search.pattern_matching_relative_path(path, Some(false), case) {
        if best
            .as_ref()
            .is_none_or(|(_, sequence)| matched.sequence_number >= *sequence)
        {
            best = Some((!matched.pattern.is_negative(), matched.sequence_number));
        }
    }
    best.is_some_and(|(included, _)| included)
}

fn normalize_cone_directories(
    repo: &crate::Repository,
    directories: impl Iterator<Item = BString>,
) -> Result<Vec<BString>, Error> {
    let options = repo
        .config
        .protect_options()
        .map_err(|err| Error::StatusSetup(Box::new(err)))?;
    let mut normalized = BTreeSet::new();
    for original in directories {
        if original.iter().any(|byte| matches!(*byte, 0 | b'\r' | b'\n')) {
            return Err(Error::InvalidConeDirectory {
                directory: original,
                reason: "directory names must contain one line and no NUL".into(),
            });
        }
        if original.starts_with(b"/") {
            return Err(Error::InvalidConeDirectory {
                directory: original,
                reason: "directory names must be repository-relative".into(),
            });
        }
        let mut directory = original.as_bstr();
        while directory.ends_with(b"/") {
            directory = directory[..directory.len() - 1].as_bstr();
        }
        if directory == "." {
            directory = BStr::new(b"");
        }
        if !directory.is_empty() {
            for component in directory.split(|byte| *byte == b'/') {
                if let Err(err) = gix_validate::path::component(component.as_bstr(), None, options) {
                    return Err(Error::InvalidConeDirectory {
                        directory: original,
                        reason: err.to_string(),
                    });
                }
            }
        }
        normalized.insert(directory.to_owned());
    }

    let all: Vec<BString> = normalized.into_iter().collect();
    Ok(all
        .iter()
        .enumerate()
        .filter(|(idx, candidate)| {
            !all.iter().enumerate().any(|(other_idx, parent)| {
                *idx != other_idx
                    && is_component_prefix(
                        parent.as_bstr(),
                        candidate.as_bstr(),
                        gix_glob::pattern::Case::Sensitive,
                    )
            })
        })
        .map(|(_, directory)| directory.clone())
        .collect())
}

fn cone_definition(directories: &[BString]) -> Vec<u8> {
    let mut ancestors = BTreeSet::new();
    for directory in directories {
        let mut offset = 0;
        while let Some(slash) = directory[offset..].find_byte(b'/') {
            let end = offset + slash;
            ancestors.insert(directory[..end].as_bstr().to_owned());
            offset = end + 1;
        }
    }

    let selected: BTreeSet<&BStr> = directories.iter().map(|directory| directory.as_bstr()).collect();
    let mut out = b"/*\n!/*/\n".to_vec();
    for ancestor in ancestors {
        if ancestor.is_empty() || selected.contains(ancestor.as_bstr()) {
            continue;
        }
        out.push(b'/');
        escape_cone_path(ancestor.as_bstr(), &mut out);
        out.extend_from_slice(b"/\n!/");
        escape_cone_path(ancestor.as_bstr(), &mut out);
        out.extend_from_slice(b"/*/\n");
    }
    for directory in directories {
        if directory.is_empty() {
            continue;
        }
        out.push(b'/');
        escape_cone_path(directory.as_bstr(), &mut out);
        out.extend_from_slice(b"/\n");
    }
    out
}

fn escape_cone_path(path: &BStr, out: &mut Vec<u8>) {
    for (idx, byte) in path.iter().enumerate() {
        if matches!(*byte, b'*' | b'?' | b'[' | b'\\') || (*byte == b' ' && idx + 1 == path.len()) {
            out.push(b'\\');
        }
        out.push(*byte);
    }
}

fn unescape_cone_path(path: &[u8]) -> BString {
    let mut out = BString::default();
    let mut bytes = path.iter();
    while let Some(byte) = bytes.next() {
        if *byte == b'\\' {
            if let Some(escaped) = bytes.next() {
                out.push(*escaped);
            } else {
                out.push(*byte);
            }
        } else {
            out.push(*byte);
        }
    }
    out
}

fn cone_includes(
    path: &BStr,
    directories: &[BString],
    case: gix_glob::pattern::Case,
) -> bool {
    let Some(parent_end) = path.rfind_byte(b'/') else {
        return true;
    };
    let parent = path[..parent_end].as_bstr();
    directories.iter().any(|selected| {
        let selected = selected.as_bstr();
        !selected.is_empty()
            && (is_component_prefix(selected, path, case)
                || is_component_prefix(parent, selected, case))
    })
}

fn is_component_prefix(prefix: &BStr, path: &BStr, case: gix_glob::pattern::Case) -> bool {
    if prefix.is_empty() {
        return true;
    }
    if path.len() <= prefix.len() || path.get(prefix.len()) != Some(&b'/') {
        return false;
    }
    match case {
        gix_glob::pattern::Case::Sensitive => path[..prefix.len()] == *prefix,
        gix_glob::pattern::Case::Fold => path[..prefix.len()].eq_ignore_ascii_case(prefix),
    }
}

fn parse_cone_definition(bytes: &[u8]) -> Result<Vec<BString>, Error> {
    let mut positive = BTreeSet::new();
    let mut ancestors = BTreeSet::new();
    for line in active_pattern_lines(bytes) {
        let line = line.as_bstr();
        if line == "/*" || line == "!/*/" {
            continue;
        }
        if line.starts_with(b"!/") && line.ends_with(b"/*/") {
            ancestors.insert(unescape_cone_path(&line[2..line.len() - 3]));
        } else if line.starts_with(b"/") && line.ends_with(b"/") {
            positive.insert(unescape_cone_path(&line[1..line.len() - 1]));
        } else {
            return Err(Error::InvalidConeDefinition {
                line: line.to_owned(),
            });
        }
    }
    for ancestor in ancestors {
        positive.remove(&ancestor);
    }
    Ok(positive.into_iter().collect())
}

fn active_pattern_lines(bytes: &[u8]) -> Vec<BString> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let lines: Vec<&[u8]> = bytes.lines().collect();
    gix_ignore::parse(bytes, false)
        .filter_map(|(_, line_number, _)| {
            lines
                .get(line_number.saturating_sub(1))
                .map(|line| BString::from(*line))
        })
        .collect()
}

fn sparse_checkout_path(repo: &crate::Repository) -> PathBuf {
    repo.git_dir().join("info").join("sparse-checkout")
}

fn write_locked_bytes(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::DefinitionWrite {
            path: path.to_owned(),
            source,
        })?;
    }
    let mut lock = gix_lock::File::acquire_to_update_resource(
        path,
        gix_lock::acquire::Fail::Immediately,
        None,
    )
    .map_err(|source| Error::DefinitionLock {
        path: path.to_owned(),
        source,
    })?;
    std::io::Write::write_all(&mut lock, bytes).map_err(|source| Error::DefinitionWrite {
        path: path.to_owned(),
        source,
    })?;
    lock.commit().map_err(|err| Error::DefinitionWrite {
        path: path.to_owned(),
        source: err.error,
    })?;
    Ok(())
}

fn entry_worktree_path(
    repo: &crate::Repository,
    workdir: &Path,
    path: &BStr,
    mode: gix_index::entry::Mode,
) -> Result<PathBuf, Error> {
    let options = repo
        .config
        .protect_options()
        .map_err(|err| Error::StatusSetup(Box::new(err)))?;
    if path.is_empty() || path.starts_with(b"/") {
        return Err(Error::InvalidIndexPath {
            path: path.to_owned(),
            reason: "paths must be non-empty and repository-relative".into(),
        });
    }
    let components: Vec<_> = path.split(|byte| *byte == b'/').collect();
    for (idx, component) in components.iter().enumerate() {
        let component_mode = (idx + 1 == components.len() && mode == gix_index::entry::Mode::SYMLINK)
            .then_some(gix_validate::path::component::Mode::Symlink);
        if let Err(err) = gix_validate::path::component(component.as_bstr(), component_mode, options) {
            return Err(Error::InvalidIndexPath {
                path: path.to_owned(),
                reason: err.to_string(),
            });
        }
    }
    let relative = gix_path::try_from_bstr(path).map_err(|err| Error::InvalidIndexPath {
        path: path.to_owned(),
        reason: err.to_string(),
    })?;
    Ok(workdir.join(relative))
}

fn path_presence(
    repo: &crate::Repository,
    workdir: &Path,
    path: &BStr,
    mode: gix_index::entry::Mode,
) -> Result<Presence, Error> {
    let destination = entry_worktree_path(repo, workdir, path, mode)?;
    let relative = destination
        .strip_prefix(workdir)
        .map_err(|err| Error::InvalidIndexPath {
            path: path.to_owned(),
            reason: err.to_string(),
        })?;
    let mut cursor = workdir.to_owned();
    let components: Vec<_> = relative.components().collect();
    for (idx, component) in components.iter().enumerate() {
        cursor.push(component);
        match std::fs::symlink_metadata(&cursor) {
            Ok(metadata) if idx + 1 == components.len() => {
                return Ok(if metadata.is_dir() && mode != gix_index::entry::Mode::COMMIT {
                    Presence::Obstructed
                } else {
                    Presence::Present
                });
            }
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Ok(Presence::Obstructed);
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Presence::Missing),
            Err(err) if err.kind() == std::io::ErrorKind::NotADirectory => {
                return Ok(Presence::Obstructed);
            }
            Err(err) => return Err(Error::WorktreeInspect(err)),
        }
    }
    Ok(Presence::Missing)
}

fn materialize_entries(
    repo: &crate::Repository,
    workdir: &Path,
    index: &mut gix_index::File,
    materialize: &[usize],
) -> Result<(), Error> {
    if materialize.is_empty() {
        return Ok(());
    }
    let selected: HashSet<usize> = materialize.iter().copied().collect();
    let mut checkout_index = index.clone();
    for (idx, entry) in checkout_index.entries_mut().iter_mut().enumerate() {
        if selected.contains(&idx) {
            clear_skip_worktree(entry);
        } else {
            set_skip_worktree(entry);
        }
    }

    let mut options = repo
        .checkout_options(gix_worktree::stack::state::attributes::Source::WorktreeThenIdMapping)
        .map_err(|err| Error::CheckoutOptions(Box::new(err)))?;
    options.destination_is_initially_empty = false;
    options.overwrite_existing = false;
    options.keep_going = false;

    let files = gix_features::progress::Discard;
    let bytes = gix_features::progress::Discard;
    let should_interrupt = AtomicBool::default();
    let outcome = gix_worktree_state::checkout(
        &mut checkout_index,
        workdir,
        repo.objects
            .clone()
            .into_arc()
            .map_err(Error::ObjectStore)?,
        &files,
        &bytes,
        &should_interrupt,
        options,
    )
    .map_err(Error::Checkout)?;
    if !outcome.collisions.is_empty()
        || !outcome.errors.is_empty()
        || !outcome.delayed_paths_unknown.is_empty()
        || !outcome.delayed_paths_unprocessed.is_empty()
        || materialize.iter().any(|idx| {
            let entry = &checkout_index.entries()[*idx];
            let path = entry.path_in(checkout_index.path_backing());
            path_presence(repo, workdir, path, entry.mode)
                .map(|presence| presence != Presence::Present)
                .unwrap_or(true)
        })
    {
        return Err(Error::CheckoutIncomplete {
            outcome: Box::new(outcome),
        });
    }
    for idx in materialize {
        index.entries_mut()[*idx].stat = checkout_index.entries()[*idx].stat;
    }
    Ok(())
}

#[derive(Clone)]
struct NoSubmodule;

impl gix_status::index_as_worktree::traits::SubmoduleStatus for NoSubmodule {
    type Output = ();
    type Error = Infallible;

    fn status(
        &mut self,
        _entry: &gix_index::Entry,
        _rela_path: &BStr,
    ) -> Result<Option<Self::Output>, Self::Error> {
        Ok(None)
    }
}

fn dirty_entries(
    repo: &crate::Repository,
    workdir: &Path,
    index: &gix_index::File,
    candidates: &[usize],
) -> Result<HashSet<usize>, Error> {
    if candidates.is_empty() {
        return Ok(HashSet::new());
    }
    let selected: HashSet<usize> = candidates.iter().copied().collect();
    let mut status_index = index.clone();
    for (idx, entry) in status_index.entries_mut().iter_mut().enumerate() {
        if selected.contains(&idx) {
            clear_skip_worktree(entry);
            entry.flags.remove(
                gix_index::entry::Flags::UPTODATE
                    | gix_index::entry::Flags::ASSUME_VALID
                    | gix_index::entry::Flags::FSMONITOR_VALID,
            );
        } else {
            set_skip_worktree(entry);
        }
    }

    let attributes = repo
        .attributes(
            &status_index,
            gix_worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
            gix_worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped,
            None,
        )
        .map_err(|err| Error::StatusSetup(Box::new(err)))?;
    let pathspec = gix_pathspec::Search::from_specs([], None, Path::new(""))
        .map_err(|err| Error::StatusSetup(Box::new(err)))?;
    let filter = gix_filter::Pipeline::new(
        repo.command_context()
            .map_err(|err| Error::StatusSetup(Box::new(err)))?,
        crate::filter::Pipeline::options(repo)
            .map_err(|err| Error::StatusSetup(Box::new(err)))?,
    );
    let mut recorder = gix_status::index_as_worktree::Recorder::default();
    let should_interrupt = AtomicBool::default();
    gix_status::index_as_worktree(
        &status_index,
        workdir,
        &mut recorder,
        gix_status::index_as_worktree::traits::FastEq,
        NoSubmodule,
        repo.objects
            .clone()
            .into_arc()
            .map_err(Error::ObjectStore)?,
        &mut gix_features::progress::Discard,
        gix_status::index_as_worktree::Context {
            pathspec,
            stack: attributes.inner,
            filter,
            should_interrupt: &should_interrupt,
        },
        gix_status::index_as_worktree::Options {
            fs: repo
                .filesystem_options()
                .map_err(|err| Error::StatusSetup(Box::new(err)))?,
            thread_limit: Some(1),
            stat: repo
                .stat_options()
                .map_err(|err| Error::StatusSetup(Box::new(err)))?,
            fscache: false,
        },
    )
    .map_err(Error::Status)?;

    Ok(recorder
        .records
        .into_iter()
        .filter_map(|record| {
            matches!(
                record.status,
                gix_status::index_as_worktree::EntryStatus::Change(_)
                    | gix_status::index_as_worktree::EntryStatus::Conflict { .. }
            )
            .then_some(record.entry_index)
        })
        .collect())
}

fn remove_tracked_path(path: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if metadata.file_type().is_symlink() {
        gix_fs::symlink::remove(path).is_ok()
    } else if metadata.is_file() {
        std::fs::remove_file(path).is_ok()
    } else {
        false
    }
}

fn remove_empty_parents(mut parent: Option<&Path>, workdir: &Path) {
    while let Some(directory) = parent {
        if directory == workdir || std::fs::remove_dir(directory).is_err() {
            break;
        }
        parent = directory.parent();
    }
}

fn read_config(path: &Path, source: gix_config::Source) -> Result<gix_config::File, Error> {
    match gix_config::File::from_path_no_includes(path.to_owned(), source) {
        Ok(config) => Ok(config),
        Err(gix_config::file::init::from_paths::Error::Io { source: err, .. })
            if err.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(gix_config::File::new(gix_config::file::Metadata::from(source)))
        }
        Err(source) => Err(Error::ConfigRead {
            path: path.to_owned(),
            source,
        }),
    }
}

fn write_config(path: &Path, config: &gix_config::File) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| Error::ConfigWrite {
            path: path.to_owned(),
            source,
        })?;
    }
    let mut lock = gix_lock::File::acquire_to_update_resource(
        path,
        gix_lock::acquire::Fail::Immediately,
        None,
    )
    .map_err(|source| Error::ConfigLock {
        path: path.to_owned(),
        source,
    })?;
    config
        .write_to(&mut lock)
        .map_err(|source| Error::ConfigWrite {
            path: path.to_owned(),
            source,
        })?;
    lock.commit().map_err(|err| Error::ConfigCommit {
        path: path.to_owned(),
        source: err.error,
    })?;
    Ok(())
}

fn enable_sparse_config(
    repo: &mut crate::Repository,
    mode: gix_index::sparse::Mode,
) -> Result<(), Error> {
    let common_path = repo.common_dir().join("config");
    let worktree_path = repo.git_dir().join("config.worktree");
    let mut common = read_config(&common_path, gix_config::Source::Local)?;
    let mut worktree = read_config(&worktree_path, gix_config::Source::Worktree)?;

    if let Some(value) = common.string_by("core", None, "worktree") {
        worktree.set_raw_value_by("core", None, "worktree", value)?;
    }
    let core_ids: Vec<_> = common
        .sections_and_ids_by_name("core")
        .into_iter()
        .flatten()
        .map(|(_, id)| id)
        .collect();
    for id in core_ids {
        if let Some(mut section) = common.section_mut_by_id(id) {
            while section.remove("worktree").is_some() {}
        }
    }

    worktree.set_raw_value_by("core", None, "sparseCheckout", "true")?;
    worktree.set_raw_value_by(
        "core",
        None,
        "sparseCheckoutCone",
        if matches!(
            mode,
            gix_index::sparse::Mode::IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs
                | gix_index::sparse::Mode::IncludeDirectoriesStoreAllEntriesSkipUnmatched
        ) {
            "true"
        } else {
            "false"
        },
    )?;
    worktree.set_raw_value_by(
        "index",
        None,
        "sparse",
        if mode == gix_index::sparse::Mode::IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs {
            "true"
        } else {
            "false"
        },
    )?;
    common.set_raw_value_by("extensions", None, "worktreeConfig", "true")?;

    write_config(&worktree_path, &worktree)?;
    write_config(&common_path, &common)?;
    repo.reload().map_err(Error::Reload)?;
    Ok(())
}

fn set_sparse_checkout_enabled(repo: &mut crate::Repository, enabled: bool) -> Result<(), Error> {
    let path = repo.git_dir().join("config.worktree");
    let mut config = read_config(&path, gix_config::Source::Worktree)?;
    config.set_raw_value_by(
        "core",
        None,
        "sparseCheckout",
        if enabled { "true" } else { "false" },
    )?;
    if !enabled {
        config.set_raw_value_by("index", None, "sparse", "false")?;
    }
    write_config(&path, &config)?;
    repo.reload().map_err(Error::Reload)?;
    Ok(())
}