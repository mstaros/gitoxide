//! Read-only repository diffs and tree deltas.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use gix::bstr::ByteSlice;
use interoptopus::ffi;

use crate::{GixError, chain_to_string, hex, message, other};

#[ffi]
#[derive(Debug, Clone)]
pub struct DiffRecord {
    pub patch: ffi::Vec<u8>,
    pub file_count: u64,
    pub lines_added: u64,
    pub lines_deleted: u64,
}

#[ffi]
#[derive(Debug, Clone)]
pub struct TreeChangeRecord {
    pub path: ffi::Vec<u8>,
    pub old_path: ffi::Option<ffi::Vec<u8>>,
    pub kind: u32,
    pub object_id: ffi::Option<ffi::String>,
    pub old_object_id: ffi::Option<ffi::String>,
    pub mode: u32,
    pub old_mode: u32,
}

type Entry = (gix::ObjectId, u32);

fn tree<'repo>(repo: &'repo gix::Repository, revision: &str) -> Result<gix::Tree<'repo>, GixError> {
    let id = repo.rev_parse_single(revision.as_bytes().as_bstr()).map_err(|err| other(&err))?;
    let object = id.object().map_err(|err| other(&err))?;
    let object = object.peel_to_kind(gix::objs::Kind::Tree).map_err(|err| other(&err))?;
    Ok(object.into_tree())
}

fn head_index(repo: &gix::Repository) -> Result<gix::index::File, GixError> {
    let mut head = repo.head().map_err(|err| other(&err))?;
    match head.try_peel_to_id().map_err(|err| other(&err))? {
        Some(id) => {
            let commit = crate::find_commit_checked(repo, id.detach())?;
            repo.index_from_tree(&commit.tree_id().map_err(|err| other(&err))?).map_err(|err| other(&err))
        }
        None => Ok(gix::index::File::from_state(
            gix::index::State::new(repo.object_hash()), repo.index_path(),
        )),
    }
}

fn entries(index: &gix::index::State) -> Result<BTreeMap<Vec<u8>, Entry>, GixError> {
    let mut out = BTreeMap::new();
    for entry in index.entries() {
        if entry.stage_raw() != 0 {
            return Err(GixError::Other(message("cannot create a diff from an unmerged index")));
        }
        if entry.mode.is_sparse() {
            return Err(GixError::Other(message("sparse directory index entries are not supported by this diff operation")));
        }
        // Intent-to-add carries no staged content.
        if !entry.flags.contains(gix::index::entry::Flags::INTENT_TO_ADD) {
            out.insert(entry.path(index).to_vec(), (entry.id, entry.mode.bits()));
        }
    }
    Ok(out)
}

pub(crate) fn tree_changes(
    repo: &gix::Repository,
    old_revision: &str,
    new_revision: &str,
    pathspec_bytes: &[u8],
) -> Result<Vec<TreeChangeRecord>, GixError> {
    let old = if old_revision.is_empty() { None } else { Some(tree(repo, old_revision)?) };
    let new = tree(repo, new_revision)?;
    let changes = repo.diff_tree_to_tree(old.as_ref(), Some(&new), None).map_err(|err| other(&err))?;
    let index = crate::index::owned_index(repo)?;
    let patterns = crate::status::pathspecs(pathspec_bytes, false)?;
    let mut pathspec = repo.pathspec(
        false, patterns.iter(), false, &index,
        gix::worktree::stack::state::attributes::Source::IdMapping,
    ).map_err(|err| other(&err))?;
    let mut out = Vec::new();
    for change in changes {
        use gix::object::tree::diff::ChangeDetached;
        let (path, old_path, kind, old, new) = match change {
            ChangeDetached::Addition { location, entry_mode, id, .. } =>
                (location, None, 1, None, Some((id, u32::from(entry_mode.value())))),
            ChangeDetached::Deletion { location, entry_mode, id, .. } =>
                (location.clone(), Some(location), 2, Some((id, u32::from(entry_mode.value()))), None),
            ChangeDetached::Modification { location, previous_entry_mode, previous_id, entry_mode, id } => {
                let old_mode = u32::from(previous_entry_mode.value());
                let mode = u32::from(entry_mode.value());
                let kind = if old_mode & 0o170000 == mode & 0o170000 { 3 } else { 8 };
                (location.clone(), Some(location), kind, Some((previous_id, old_mode)), Some((id, mode)))
            }
            ChangeDetached::Rewrite { location, source_location, source_entry_mode, source_id, entry_mode, id, copy, .. } =>
                (location, Some(source_location), if copy { 5 } else { 4 },
                 Some((source_id, u32::from(source_entry_mode.value()))), Some((id, u32::from(entry_mode.value())))),
        };
        if old.is_some_and(|(_, mode)| mode == 0o040000) || new.is_some_and(|(_, mode)| mode == 0o040000) {
            continue;
        }
        if !pathspec.is_included(path.as_bstr(), Some(false))
            && !old_path.as_ref().is_some_and(|path| pathspec.is_included(path.as_bstr(), Some(false)))
        {
            continue;
        }
        out.push((path.to_vec(), TreeChangeRecord {
            path: ffi::Vec::from(path.to_vec()),
            old_path: old_path.map(|path| ffi::Vec::from(path.to_vec())).into(),
            kind,
            object_id: new.map(|(id, _)| hex(id.as_ref())).into(),
            old_object_id: old.map(|(id, _)| hex(id.as_ref())).into(),
            mode: new.map_or(0, |(_, mode)| mode),
            old_mode: old.map_or(0, |(_, mode)| mode),
        }));
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(out.into_iter().map(|(_, record)| record).collect())
}

/// Inspect a worktree resource without following a final symlink or writing objects.
fn worktree_entry(
    repo: &gix::Repository,
    path: &[u8],
    previous: Option<Entry>,
) -> Result<Option<Entry>, GixError> {
    let root = repo.workdir().ok_or_else(|| GixError::Other(message("the repository has no working directory")))?;
    let disk_path = root.join(crate::path_from_bytes(path)?);
    let metadata = match gix::index::fs::Metadata::from_path_no_follow(&disk_path) {
        Ok(value) => value,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(GixError::Io(chain_to_string(&err))),
    };
    let fs = repo.filesystem_options().map_err(|err| other(&err))?;
    let mode = gix::index::entry::Mode::from_bits_truncate(previous.map_or(0o100644, |(_, mode)| mode));
    let mode = mode.change_to_match_fs(&metadata, fs.symlink, fs.executable_bit)
        .map_or(mode, |change| change.apply(mode));
    if metadata.is_dir() {
        // Ordinary directories replace a file with a deletion. A nested repository is a gitlink.
        return match gix::open(&disk_path) {
            Ok(submodule) => match submodule.head_id() {
                Ok(id) => Ok(Some((id.detach(), 0o160000))),
                Err(err) => Err(other(&err)),
            },
            Err(_) if previous.is_some_and(|(_, mode)| mode == 0o160000) => Ok(previous),
            Err(_) => Ok(None),
        };
    }
    if !metadata.is_file() && !metadata.is_symlink() {
        return Err(GixError::Other(message("the diff encountered an unreadable file type")));
    }
    Ok(Some((repo.object_hash().null(), mode.bits())))
}

fn quoted_path(prefix: &[u8], path: &[u8]) -> Vec<u8> {
    let bytes = [prefix, path].concat();
    if bytes.iter().all(|byte| (0x21..0x7f).contains(byte) && !matches!(byte, b'"' | b'\\')) {
        return bytes;
    }
    let mut out = vec![b'"'];
    for byte in bytes {
        match byte {
            b'"' | b'\\' => { out.push(b'\\'); out.push(byte); }
            b'\n' => out.extend_from_slice(br"\n"),
            b'\r' => out.extend_from_slice(br"\r"),
            b'\t' => out.extend_from_slice(br"\t"),
            0x20..=0x7e => out.push(byte),
            _ => out.extend_from_slice(format!("\\{byte:03o}").as_bytes()),
        }
    }
    out.push(b'"');
    out
}

#[derive(Clone, Copy)]
struct Lines<'a>(&'a [u8]);
impl<'a> Iterator for Lines<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() { return None; }
        let end = self.0.iter().position(|byte| *byte == b'\n').map_or(self.0.len(), |i| i + 1);
        let (line, rest) = self.0.split_at(end);
        self.0 = rest;
        Some(line)
    }
}
impl<'a> gix::diff::blob::TokenSource for Lines<'a> {
    type Token = &'a [u8];
    type Tokenizer = Self;
    fn tokenize(&self) -> Self { *self }
    fn estimate_tokens(&self) -> u32 { self.0.iter().filter(|byte| **byte == b'\n').count() as u32 + 1 }
}

struct Hunks(Vec<u8>);
impl gix::diff::blob::unified_diff::ConsumeHunk for Hunks {
    type Out = Vec<u8>;
    fn consume_hunk(
        &mut self,
        header: gix::diff::blob::unified_diff::HunkHeader,
        lines: &[(gix::diff::blob::unified_diff::DiffLineKind, &[u8])],
    ) -> std::io::Result<()> {
        // Empty sides use the line immediately preceding the range, as required by git apply.
        let old_start = header.before_hunk_start - u32::from(header.before_hunk_len == 0);
        let new_start = header.after_hunk_start - u32::from(header.after_hunk_len == 0);
        writeln!(self.0, "@@ -{old_start},{} +{new_start},{} @@", header.before_hunk_len, header.after_hunk_len)?;
        for (kind, line) in lines {
            self.0.push(kind.to_prefix() as u8);
            self.0.extend_from_slice(line);
            if !line.ends_with(b"\n") {
                self.0.extend_from_slice(b"\n\\ No newline at end of file\n");
            }
        }
        Ok(())
    }
    fn finish(self) -> Self::Out { self.0 }
}

fn text_hunks(old: &[u8], new: &[u8], algorithm: gix::diff::blob::Algorithm) -> Result<(Vec<u8>, u64, u64), GixError> {
    let input = gix::diff::blob::InternedInput::new(Lines(old), Lines(new));
    let diff = gix::diff::blob::diff_with_slider_heuristics(algorithm, &input);
    let added = u64::from(diff.count_additions());
    let deleted = u64::from(diff.count_removals());
    let hunks = gix::diff::blob::UnifiedDiff::new(&diff, &input, Hunks(Vec::new()), Default::default())
        .consume().map_err(|err| GixError::Io(chain_to_string(&err)))?;
    Ok((hunks, added, deleted))
}

fn patch_file(
    repo: &gix::Repository,
    cache: &mut gix::diff::blob::Platform,
    path: &[u8],
    old: Option<Entry>,
    new: Option<Entry>,
    dirty_gitlink: bool,
) -> Result<Option<(Vec<u8>, u64, u64)>, GixError> {
    if old.is_none() && new.is_none() { return Ok(None); }
    let old_mode = old.map_or(0, |(_, mode)| mode);
    let new_mode = new.map_or(0, |(_, mode)| mode);
    if old_mode != 0 && new_mode != 0 && old_mode & 0o170000 != new_mode & 0o170000 {
        // Git patches represent file-type changes as a deletion followed by an addition.
        let mut bytes = Vec::new();
        let (mut added, mut deleted) = (0, 0);
        for (before, after) in [(old, None), (None, new)] {
            if let Some((part, a, d)) = patch_file(repo, cache, path, before, after, dirty_gitlink)? {
                bytes.extend(part);
                added += a;
                deleted += d;
            }
        }
        return Ok(Some((bytes, added, deleted)));
    }
    let (body, added, deleted, binary) = if old_mode == 0o160000 || new_mode == 0o160000 {
        let content = |entry: Option<Entry>, dirty: bool| entry.map_or_else(Vec::new, |(id, mode)| {
            if mode == 0o160000 {
                format!("Subproject commit {id}{}\n", if dirty { "-dirty" } else { "" }).into_bytes()
            } else { Vec::new() }
        });
        let (body, added, deleted) = text_hunks(&content(old, false), &content(new, dirty_gitlink), Default::default())?;
        (body, added, deleted, false)
    } else {
        use gix::diff::blob::{ResourceKind, platform::{prepare_diff::Operation, resource::Data}};
        let kind = |mode| match mode {
            0o100755 => gix::objs::tree::EntryKind::BlobExecutable,
            0o120000 => gix::objs::tree::EntryKind::Link,
            _ => gix::objs::tree::EntryKind::Blob,
        };
        cache.set_resource(old.map_or(repo.object_hash().null(), |(id, _)| id),
            kind(old_mode), path.as_bstr(), ResourceKind::OldOrSource, repo).map_err(|err| other(&err))?;
        // Missing resources must be read from the object side even when a worktree root is configured.
        let saved_root = if new.is_none() { cache.filter.roots.new_root.take() } else { None };
        let result = cache.set_resource(new.map_or(repo.object_hash().null(), |(id, _)| id),
            kind(new_mode), path.as_bstr(), ResourceKind::NewOrDestination, repo);
        if saved_root.is_some() { cache.filter.roots.new_root = saved_root; }
        result.map_err(|err| other(&err))?;
        let prepared = cache.prepare_diff().map_err(|err| other(&err))?;
        match prepared.operation {
            Operation::SourceOrDestinationIsBinary => {
                // Byte-identical binary resources do not contribute a false change.
                if old == new && old_mode == new_mode { return Ok(None); }
                (Vec::new(), 0, 0, true)
            }
            Operation::InternalDiff { algorithm } => {
                let bytes = |data| match data { Data::Buffer { buf, .. } => buf, _ => &[] };
                let (body, added, deleted) = text_hunks(bytes(prepared.old.data), bytes(prepared.new.data), algorithm)?;
                (body, added, deleted, false)
            }
            Operation::ExternalCommand { .. } => return Err(GixError::Other(message("external diff output is not supported"))),
        }
    };
    cache.clear_resource_cache_keep_allocation();
    if !binary && body.is_empty() && old_mode == new_mode { return Ok(None); }
    let mut patch = b"diff --git ".to_vec();
    patch.extend(quoted_path(b"a/", path));
    patch.push(b' ');
    patch.extend(quoted_path(b"b/", path));
    patch.push(b'\n');
    if old.is_none() {
        writeln!(patch, "new file mode {new_mode:06o}").map_err(|err| other(&err))?;
    } else if new.is_none() {
        writeln!(patch, "deleted file mode {old_mode:06o}").map_err(|err| other(&err))?;
    } else if old_mode != new_mode {
        writeln!(patch, "old mode {old_mode:06o}\nnew mode {new_mode:06o}").map_err(|err| other(&err))?;
    }
    if binary {
        patch.extend_from_slice(b"Binary files ");
        patch.extend(if old.is_some() { quoted_path(b"a/", path) } else { b"/dev/null".to_vec() });
        patch.extend_from_slice(b" and ");
        patch.extend(if new.is_some() { quoted_path(b"b/", path) } else { b"/dev/null".to_vec() });
        patch.extend_from_slice(b" differ\n");
    } else if !body.is_empty() {
        patch.extend_from_slice(b"--- ");
        patch.extend(if old.is_some() { quoted_path(b"a/", path) } else { b"/dev/null".to_vec() });
        patch.extend_from_slice(b"\n+++ ");
        patch.extend(if new.is_some() { quoted_path(b"b/", path) } else { b"/dev/null".to_vec() });
        patch.push(b'\n');
        patch.extend(body);
    }
    Ok(Some((patch, added, deleted)))
}

pub(crate) fn collect(repo: &gix::Repository, target: u32, pathspec_bytes: &[u8]) -> Result<DiffRecord, GixError> {
    if target > 2 { return Err(GixError::Other(message("unknown diff target"))); }
    let index = crate::index::owned_index(repo)?;
    let current = entries(&index)?;
    let baseline_index = if target == 1 { index.clone() } else { head_index(repo)? };
    let old = entries(&baseline_index)?;
    let patterns = crate::status::pathspecs(pathspec_bytes, false)?;
    let mut pathspec = repo.pathspec(false, patterns.iter(), false, &index,
        gix::worktree::stack::state::attributes::Source::IdMapping).map_err(|err| other(&err))?;
    let mut changed_paths = BTreeSet::new();
    let candidates = if target == 0 {
        old.keys().chain(current.keys()).cloned().collect::<BTreeSet<_>>()
    } else {
        let flags = if target == 2 {
            crate::status::INCLUDE_UNTRACKED | crate::status::RECURSE_UNTRACKED_DIRECTORIES
        } else { 0 };
        for record in crate::status::collect_with_index(repo, crate::status::SHOW_WORKTREE_ONLY,
            flags, pathspec_bytes, baseline_index.clone())? {
            changed_paths.insert(record.path.into_vec());
        }
        if target == 2 {
            // An explicitly staged new file remains tracked even when ignore rules would hide it.
            changed_paths.extend(current.keys().filter(|path| !old.contains_key(*path)).cloned());
        }
        changed_paths
    };
    let roots = gix::diff::blob::pipeline::WorktreeRoots {
        old_root: None, new_root: if target == 0 { None } else { repo.workdir().map(ToOwned::to_owned) },
    };
    let attrs = repo.attributes_only(&index, if target == 0 {
        gix::worktree::stack::state::attributes::Source::IdMapping
    } else { gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping })
        .map_err(|err| other(&err))?;
    let mut cache = gix::diff::resource_cache(repo, gix::diff::blob::pipeline::Mode::ToGit, attrs.detach(), roots)
        .map_err(|err| other(&err))?;
    let mut patch = Vec::new();
    let (mut file_count, mut lines_added, mut lines_deleted) = (0, 0, 0);
    for path in candidates {
        if !pathspec.is_included(path.as_bstr(), Some(false)) { continue; }
        let before = old.get(&path).copied();
        let after = if target == 0 { current.get(&path).copied() }
            else { worktree_entry(repo, &path, current.get(&path).copied().or(before))? };
        if target == 0 && before == after { continue; }
        let dirty_gitlink = if target != 0 && after.is_some_and(|(_, mode)| mode == 0o160000) {
            let root = repo.workdir().ok_or_else(|| GixError::Other(message("the repository has no working directory")))?;
            match gix::open(root.join(crate::path_from_bytes(&path)?)) {
                Ok(submodule) => !crate::status::collect(&submodule, crate::status::SHOW_COMBINED,
                    crate::status::INCLUDE_UNTRACKED | crate::status::RECURSE_UNTRACKED_DIRECTORIES, &[])?.is_empty(),
                Err(_) => false,
            }
        } else { false };
        if let Some((bytes, added, deleted)) = patch_file(repo, &mut cache, &path, before, after, dirty_gitlink)? {
            patch.extend(bytes);
            file_count += 1;
            lines_added += added;
            lines_deleted += deleted;
        }
    }
    Ok(DiffRecord { patch: patch.into(), file_count, lines_added, lines_deleted })
}
