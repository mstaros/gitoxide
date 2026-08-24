use std::collections::BTreeMap;

use interoptopus::ffi;

use crate::{GixError, message, other};

pub(crate) const SHOW_COMBINED: u32 = 0;
pub(crate) const SHOW_INDEX_ONLY: u32 = 1;
pub(crate) const SHOW_WORKTREE_ONLY: u32 = 2;

pub(crate) const INCLUDE_UNTRACKED: u32 = 1 << 0;
pub(crate) const INCLUDE_IGNORED: u32 = 1 << 1;
pub(crate) const INCLUDE_UNMODIFIED: u32 = 1 << 2;
pub(crate) const EXCLUDE_SUBMODULES: u32 = 1 << 3;
pub(crate) const RECURSE_UNTRACKED_DIRECTORIES: u32 = 1 << 4;
pub(crate) const DISABLE_PATHSPEC_MATCH: u32 = 1 << 5;
pub(crate) const RECURSE_IGNORED_DIRECTORIES: u32 = 1 << 6;
pub(crate) const RENAMES_HEAD_TO_INDEX: u32 = 1 << 7;
pub(crate) const RENAMES_INDEX_TO_WORKTREE: u32 = 1 << 8;
pub(crate) const SORT_CASE_SENSITIVELY: u32 = 1 << 9;
pub(crate) const SORT_CASE_INSENSITIVELY: u32 = 1 << 10;
pub(crate) const RENAMES_FROM_REWRITES: u32 = 1 << 11;
pub(crate) const NO_REFRESH: u32 = 1 << 12;
pub(crate) const UPDATE_INDEX: u32 = 1 << 13;
pub(crate) const INCLUDE_UNREADABLE: u32 = 1 << 14;
pub(crate) const INCLUDE_UNREADABLE_AS_UNTRACKED: u32 = 1 << 15;
const ALL_FLAGS: u32 = (1 << 16) - 1;

pub(crate) const INDEX_NEW: u32 = 1 << 0;
pub(crate) const INDEX_MODIFIED: u32 = 1 << 1;
pub(crate) const INDEX_DELETED: u32 = 1 << 2;
pub(crate) const INDEX_RENAMED: u32 = 1 << 3;
pub(crate) const INDEX_TYPE_CHANGED: u32 = 1 << 4;
pub(crate) const WORKTREE_NEW: u32 = 1 << 7;
pub(crate) const WORKTREE_MODIFIED: u32 = 1 << 8;
pub(crate) const WORKTREE_DELETED: u32 = 1 << 9;
pub(crate) const WORKTREE_TYPE_CHANGED: u32 = 1 << 10;
pub(crate) const WORKTREE_RENAMED: u32 = 1 << 11;
pub(crate) const WORKTREE_UNREADABLE: u32 = 1 << 12;
pub(crate) const IGNORED: u32 = 1 << 14;
pub(crate) const CONFLICTED: u32 = 1 << 15;

/// One repository-relative status result.
///
/// The path is a Git path: raw bytes with slash separators. It is
/// intentionally not validated as UTF-8.
#[ffi]
#[derive(Debug, Clone)]
pub struct StatusRecord {
    pub path: ffi::Vec<u8>,
    pub status: u32,
}

fn invalid_options(reason: impl Into<String>) -> GixError {
    GixError::Other(message(reason))
}

fn validate(show: u32, flags: u32) -> Result<(), GixError> {
    if !matches!(show, SHOW_COMBINED | SHOW_INDEX_ONLY | SHOW_WORKTREE_ONLY) {
        return Err(invalid_options(format!("unknown status show mode {show}")));
    }
    if flags & !ALL_FLAGS != 0 {
        return Err(invalid_options(format!(
            "unknown status option bits 0x{:x}",
            flags & !ALL_FLAGS
        )));
    }
    if flags & SORT_CASE_SENSITIVELY != 0 && flags & SORT_CASE_INSENSITIVELY != 0 {
        return Err(invalid_options(
            "case-sensitive and case-insensitive status sorting are mutually exclusive",
        ));
    }
    if flags & NO_REFRESH != 0 && flags & UPDATE_INDEX != 0 {
        return Err(invalid_options(
            "NoRefresh and UpdateIndex status options are mutually exclusive",
        ));
    }
    Ok(())
}

pub(crate) fn pathspecs(
    bytes: &[u8],
    literal: bool,
) -> Result<Vec<gix::bstr::BString>, GixError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }

    bytes
        .split(|byte| *byte == 0)
        .map(|pattern| {
            if pattern.is_empty() {
                return Err(invalid_options("status pathspecs must not be empty"));
            }
            let mut out = Vec::with_capacity(pattern.len() + if literal { 10 } else { 0 });
            if literal {
                out.extend_from_slice(b":(literal)");
            }
            out.extend_from_slice(pattern);
            Ok(out.into())
        })
        .collect()
}

fn entry_kind(mode: gix::index::entry::Mode) -> u32 {
    mode.bits() & 0o170000
}

fn is_type_change(previous: gix::index::entry::Mode, current: gix::index::entry::Mode) -> bool {
    entry_kind(previous) != entry_kind(current)
}

fn is_submodule(mode: gix::index::entry::Mode) -> bool {
    mode == gix::index::entry::Mode::COMMIT
}

fn add(entries: &mut BTreeMap<Vec<u8>, u32>, path: &[u8], status: u32) {
    *entries.entry(path.to_vec()).or_default() |= status;
}

fn tree_index_change(
    entries: &mut BTreeMap<Vec<u8>, u32>,
    change: gix::diff::index::Change,
    exclude_submodules: bool,
) {
    use gix::diff::index::ChangeRef;

    match change {
        ChangeRef::Addition {
            location,
            entry_mode,
            ..
        } => {
            if !exclude_submodules || !is_submodule(entry_mode) {
                add(entries, location.as_ref(), INDEX_NEW);
            }
        }
        ChangeRef::Deletion {
            location,
            entry_mode,
            ..
        } => {
            if !exclude_submodules || !is_submodule(entry_mode) {
                add(entries, location.as_ref(), INDEX_DELETED);
            }
        }
        ChangeRef::Modification {
            location,
            previous_entry_mode,
            entry_mode,
            ..
        } => {
            let type_change = is_type_change(previous_entry_mode, entry_mode);
            if !exclude_submodules
                || type_change
                || (!is_submodule(previous_entry_mode) && !is_submodule(entry_mode))
            {
                add(
                    entries,
                    location.as_ref(),
                    if type_change {
                        INDEX_TYPE_CHANGED
                    } else {
                        INDEX_MODIFIED
                    },
                );
            }
        }
        ChangeRef::Rewrite {
            location,
            source_entry_mode,
            entry_mode,
            ..
        } => {
            let type_change = is_type_change(source_entry_mode, entry_mode);
            if !exclude_submodules
                || type_change
                || (!is_submodule(source_entry_mode) && !is_submodule(entry_mode))
            {
                add(
                    entries,
                    location.as_ref(),
                    if type_change {
                        INDEX_TYPE_CHANGED
                    } else {
                        INDEX_RENAMED
                    },
                );
            }
        }
    }
}

fn tracked_worktree_status(
    status: gix::status::plumbing::index_as_worktree::EntryStatus<(), gix::submodule::Status>,
) -> u32 {
    use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};

    match status {
        EntryStatus::Conflict { .. } => CONFLICTED,
        EntryStatus::Change(Change::Removed) => WORKTREE_DELETED,
        EntryStatus::Change(Change::Type { .. }) => WORKTREE_TYPE_CHANGED,
        EntryStatus::Change(Change::Modification { .. })
        | EntryStatus::Change(Change::SubmoduleModification(_)) => WORKTREE_MODIFIED,
        EntryStatus::NeedsUpdate(_) => 0,
        EntryStatus::IntentToAdd => WORKTREE_NEW,
    }
}

fn maybe_directory_path(mut path: Vec<u8>, is_directory: bool, collapse: bool) -> Vec<u8> {
    if is_directory && collapse && !path.ends_with(b"/") {
        path.push(b'/');
    }
    path
}

fn directory_status(
    entry: &gix::dir::Entry,
    flags: u32,
    exclude_submodules: bool,
) -> Option<(Vec<u8>, u32)> {
    use gix::dir::entry::{Kind, Status};

    if exclude_submodules && entry.index_kind == Some(Kind::Repository) {
        return None;
    }

    let is_directory = matches!(entry.disk_kind, Some(Kind::Directory | Kind::Repository));
    let untrackable = entry.disk_kind == Some(Kind::Untrackable);
    match entry.status {
        Status::Tracked => Some((entry.rela_path.to_vec(), 0)),
        Status::Ignored(_) if flags & INCLUDE_IGNORED != 0 => Some((
            maybe_directory_path(
                entry.rela_path.to_vec(),
                is_directory,
                flags & RECURSE_IGNORED_DIRECTORIES == 0,
            ),
            IGNORED,
        )),
        Status::Untracked if untrackable => {
            if flags & INCLUDE_UNREADABLE_AS_UNTRACKED != 0 {
                Some((entry.rela_path.to_vec(), WORKTREE_NEW))
            } else if flags & INCLUDE_UNREADABLE != 0 {
                Some((entry.rela_path.to_vec(), WORKTREE_UNREADABLE))
            } else {
                None
            }
        }
        Status::Untracked if flags & INCLUDE_UNTRACKED != 0 => Some((
            maybe_directory_path(
                entry.rela_path.to_vec(),
                is_directory,
                flags & RECURSE_UNTRACKED_DIRECTORIES == 0,
            ),
            WORKTREE_NEW,
        )),
        Status::Pruned | Status::Ignored(_) | Status::Untracked => None,
    }
}

fn index_worktree_change(
    entries: &mut BTreeMap<Vec<u8>, u32>,
    item: gix::status::index_worktree::Item,
    show: u32,
    flags: u32,
) {
    use gix::status::index_worktree::{Item, RewriteSource};
    use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};

    let exclude_submodules = flags & EXCLUDE_SUBMODULES != 0;
    let include_unmodified = flags & INCLUDE_UNMODIFIED != 0;

    match item {
        Item::Modification {
            entry,
            rela_path,
            status,
            ..
        } => {
            let is_conflict = matches!(status, EntryStatus::Conflict { .. });
            let type_change = matches!(status, EntryStatus::Change(Change::Type { .. }));
            if exclude_submodules && is_submodule(entry.mode) && !type_change && !is_conflict {
                return;
            }

            if show != SHOW_INDEX_ONLY || is_conflict {
                add(entries, &rela_path, tracked_worktree_status(status));
            } else if include_unmodified {
                add(entries, &rela_path, 0);
            }
        }
        Item::DirectoryContents { entry, .. } => {
            if show == SHOW_INDEX_ONLY {
                if include_unmodified
                    && entry.status == gix::dir::entry::Status::Tracked
                    && !(exclude_submodules
                        && entry.index_kind == Some(gix::dir::entry::Kind::Repository))
                {
                    add(entries, &entry.rela_path, 0);
                }
                return;
            }
            if let Some((path, status)) = directory_status(&entry, flags, exclude_submodules) {
                add(entries, &path, status);
            }
        }
        Item::Rewrite {
            source,
            dirwalk_entry,
            ..
        } => {
            if show == SHOW_INDEX_ONLY {
                if include_unmodified {
                    if let RewriteSource::RewriteFromIndex {
                        source_entry,
                        source_rela_path,
                        ..
                    } = source
                    {
                        if !exclude_submodules || !is_submodule(source_entry.mode) {
                            add(entries, &source_rela_path, 0);
                        }
                    }
                }
                return;
            }

            let source_mode = match &source {
                RewriteSource::RewriteFromIndex { source_entry, .. } => Some(source_entry.mode),
                RewriteSource::CopyFromDirectoryEntry { .. } => None,
            };
            let destination_is_submodule =
                dirwalk_entry.disk_kind == Some(gix::dir::entry::Kind::Repository);
            if exclude_submodules
                && source_mode.is_some_and(is_submodule)
                && destination_is_submodule
            {
                return;
            }
            add(entries, &dirwalk_entry.rela_path, WORKTREE_RENAMED);
        }
    }
}

fn configure_platform<'repo>(
    repo: &'repo gix::Repository,
    flags: u32,
) -> Result<gix::status::Platform<'repo, gix::progress::Discard>, GixError> {
    let mut platform = repo.status(gix::progress::Discard).map_err(|err| other(&err))?;

    let untracked_mode = if flags & RECURSE_UNTRACKED_DIRECTORIES != 0 {
        gix::dir::walk::EmissionMode::Matching
    } else {
        gix::dir::walk::EmissionMode::CollapseDirectory
    };
    let ignored_mode = if flags & RECURSE_IGNORED_DIRECTORIES != 0 {
        gix::dir::walk::EmissionMode::Matching
    } else {
        gix::dir::walk::EmissionMode::CollapseDirectory
    };
    let need_dirwalk = flags
        & (INCLUDE_UNTRACKED
            | INCLUDE_IGNORED
            | INCLUDE_UNMODIFIED
            | INCLUDE_UNREADABLE
            | INCLUDE_UNREADABLE_AS_UNTRACKED
            | RENAMES_INDEX_TO_WORKTREE)
        != 0;

    platform = platform.index_worktree_options_mut(|options| {
        if need_dirwalk {
            if let Some(dirwalk) = options.dirwalk_options.as_mut() {
                dirwalk
                    .set_emit_untracked(untracked_mode)
                    .set_emit_ignored(
                        (flags & INCLUDE_IGNORED != 0).then_some(ignored_mode),
                    )
                    .set_for_deletion(
                        (flags & RECURSE_IGNORED_DIRECTORIES != 0).then_some(
                            gix::dir::walk::ForDeletionMode::FindNonBareRepositoriesInIgnoredDirectories,
                        ),
                    )
                    .set_emit_tracked(flags & INCLUDE_UNMODIFIED != 0)
                    .set_symlinks_to_directories_are_ignored_like_directories(true);
            }
        } else {
            options.dirwalk_options = None;
        }
    });

    if flags & EXCLUDE_SUBMODULES != 0 {
        platform = platform.index_worktree_submodules(None);
    }

    let mut rewrites = gix::diff::Rewrites::default();
    if flags & RENAMES_FROM_REWRITES != 0 {
        rewrites.copies = Some(gix::diff::rewrites::Copies::default());
    }
    platform = platform.index_worktree_rewrites(
        (flags & RENAMES_INDEX_TO_WORKTREE != 0).then_some(rewrites),
    );
    platform = platform.tree_index_track_renames(
        if flags & RENAMES_HEAD_TO_INDEX != 0 {
            gix::status::tree_index::TrackRenames::Given(rewrites)
        } else {
            gix::status::tree_index::TrackRenames::Disabled
        },
    );

    Ok(platform)
}

pub(crate) fn collect(
    repo: &gix::Repository,
    show: u32,
    flags: u32,
    pathspec_bytes: &[u8],
) -> Result<Vec<StatusRecord>, GixError> {
    validate(show, flags)?;
    let patterns = pathspecs(pathspec_bytes, flags & DISABLE_PATHSPEC_MATCH != 0)?;
    let platform = configure_platform(repo, flags)?;
    let mut iter = platform.into_iter(patterns).map_err(|err| other(&err))?;
    let mut entries = BTreeMap::new();

    while let Some(item) = iter.next() {
        match item.map_err(|err| other(&err))? {
            gix::status::Item::TreeIndex(change) if show != SHOW_WORKTREE_ONLY => {
                tree_index_change(
                    &mut entries,
                    change,
                    flags & EXCLUDE_SUBMODULES != 0,
                );
            }
            gix::status::Item::TreeIndex(_) => {}
            gix::status::Item::IndexWorktree(change) => {
                index_worktree_change(&mut entries, change, show, flags);
            }
        }
    }

    if flags & UPDATE_INDEX != 0 {
        if let Some(outcome) = iter.outcome_mut() {
            if let Some(result) = outcome.write_changes() {
                result.map_err(|err| other(&err))?;
            }
        }
    }

    let include_unmodified = flags & INCLUDE_UNMODIFIED != 0;
    let mut entries = entries
        .into_iter()
        .filter(|(_, status)| *status != 0 || include_unmodified)
        .collect::<Vec<_>>();

    if flags & SORT_CASE_INSENSITIVELY != 0 {
        entries.sort_by(|left, right| {
            let left_folded = left
                .0
                .iter()
                .map(u8::to_ascii_lowercase)
                .collect::<Vec<_>>();
            let right_folded = right
                .0
                .iter()
                .map(u8::to_ascii_lowercase)
                .collect::<Vec<_>>();
            left_folded
                .cmp(&right_folded)
                .then_with(|| left.0.cmp(&right.0))
        });
    } else {
        entries.sort_by(|left, right| left.0.cmp(&right.0));
    }

    Ok(entries
        .into_iter()
        .map(|(path, status)| StatusRecord {
            path: ffi::Vec::from(path),
            status,
        })
        .collect())
}