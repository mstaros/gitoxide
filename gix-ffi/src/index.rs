use std::collections::BTreeSet;

use gix::bstr::ByteSlice;
use interoptopus::ffi;

use crate::{GixError, chain_to_string, find_commit_checked, message, other};

/// One repository index entry, including its conflict stage.
///
/// Paths are raw, repository-relative Git path bytes with slash separators.
#[ffi]
#[derive(Debug, Clone)]
pub struct IndexEntryRecord {
    pub path: ffi::Vec<u8>,
    pub stage: u32,
}

fn owned_index(repo: &gix::Repository) -> Result<gix::index::File, GixError> {
    let index = repo.index_or_empty().map_err(|err| other(&err))?;
    Ok(gix::index::File::clone(&index))
}

fn write_index(
    mut index: gix::index::File,
    entries_changed: bool,
) -> Result<(), GixError> {
    if entries_changed {
        // gix-index does not invalidate the TREE extension automatically.
        // Persisting it after entry mutation can make a later write-tree use
        // stale subtree ids.
        index.remove_tree();
    }
    index.write(Default::default()).map_err(|err| other(&err))
}

fn remove_path(index: &mut gix::index::File, path: &[u8]) -> bool {
    let previous_len = index.entries().len();
    index.remove_entries(|_, entry_path, _| entry_path == path.as_bstr());
    index.entries().len() != previous_len
}

fn tracked_paths(index: &gix::index::File) -> BTreeSet<Vec<u8>> {
    index
        .entries()
        .iter()
        .map(|entry| entry.path(index).to_vec())
        .collect()
}

fn stat_for_path(
    repo: &gix::Repository,
    path: &[u8],
) -> Result<gix::index::entry::Stat, GixError> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| GixError::Other(message("the repository has no working directory")))?;
    let disk_path = workdir.join(gix::path::from_bstr(path.as_bstr()).as_ref());
    let metadata = gix::index::fs::Metadata::from_path_no_follow(&disk_path)
        .map_err(|err| GixError::Io(chain_to_string(&err)))?;
    gix::index::entry::Stat::from_fs(&metadata).map_err(|err| other(&err))
}

fn update_from_worktree(
    repo: &gix::Repository,
    pathspecs: &[u8],
    tracked_only: bool,
) -> Result<(), GixError> {
    let records = crate::status::collect(
        repo,
        crate::status::SHOW_WORKTREE_ONLY,
        crate::status::INCLUDE_UNTRACKED
            | crate::status::RECURSE_UNTRACKED_DIRECTORIES,
        pathspecs,
    )?;
    let mut index = owned_index(repo)?;
    let tracked_before = tracked_paths(&index);
    let (mut pipeline, pipeline_index) =
        repo.filter_pipeline(None).map_err(|err| other(&err))?;
    let mut entries_changed = false;

    for record in records {
        let path = record.path.into_vec();
        let was_tracked = tracked_before.contains(&path);
        if tracked_only && !was_tracked {
            continue;
        }

        let converted = pipeline
            .worktree_file_to_object(path.as_bstr(), &pipeline_index)
            .map_err(|err| other(&err))?;
        match converted {
            Some((id, kind, _)) => {
                let stat = stat_for_path(repo, &path)?;
                let _ = remove_path(&mut index, &path);
                index.dangerously_push_entry(
                    stat,
                    id,
                    gix::index::entry::Flags::empty(),
                    kind.into(),
                    path.as_bstr(),
                );
                entries_changed = true;
            }
            None if was_tracked => {
                entries_changed |= remove_path(&mut index, &path);
            }
            None => {}
        }
    }

    if entries_changed {
        index.sort_entries();
    }
    write_index(index, entries_changed)
}

fn head_index(repo: &gix::Repository) -> Result<gix::index::File, GixError> {
    let mut head = repo.head().map_err(|err| other(&err))?;
    let Some(id) = head
        .try_peel_to_id()
        .map_err(|err| other(&err))?
        .map(|id| id.detach())
    else {
        return Ok(gix::index::File::from_state(
            gix::index::State::new(repo.object_hash()),
            repo.index_path(),
        ));
    };

    let commit = find_commit_checked(repo, id)?;
    let tree_id = commit.tree_id().map_err(|err| other(&err))?;
    repo.index_from_tree(&tree_id).map_err(|err| other(&err))
}

fn selected_paths(
    repo: &gix::Repository,
    index: &gix::index::File,
    candidates: BTreeSet<Vec<u8>>,
    pathspec_bytes: &[u8],
) -> Result<BTreeSet<Vec<u8>>, GixError> {
    let patterns = crate::status::pathspecs(pathspec_bytes, false)?;
    if patterns.is_empty() {
        return Ok(candidates);
    }

    let mut pathspec = repo
        .pathspec(
            false,
            patterns.iter(),
            false,
            index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
        )
        .map_err(|err| other(&err))?;
    Ok(candidates
        .into_iter()
        .filter(|path| pathspec.is_included(path.as_bstr(), Some(false)))
        .collect())
}

pub(crate) fn stage(
    repo: &gix::Repository,
    pathspecs: &[u8],
) -> Result<(), GixError> {
    update_from_worktree(repo, pathspecs, false)
}

pub(crate) fn update(
    repo: &gix::Repository,
    pathspecs: &[u8],
) -> Result<(), GixError> {
    update_from_worktree(repo, pathspecs, true)
}

pub(crate) fn unstage(
    repo: &gix::Repository,
    pathspecs: &[u8],
) -> Result<(), GixError> {
    let mut index = owned_index(repo)?;
    let head = head_index(repo)?;
    let mut candidates = tracked_paths(&index);
    candidates.extend(tracked_paths(&head));
    let selected = selected_paths(repo, &index, candidates, pathspecs)?;

    let replacements = head
        .entries()
        .iter()
        .filter_map(|entry| {
            let path = entry.path(&head);
            selected.contains::<[u8]>(&path[..]).then(|| {
                (
                    entry.stat,
                    entry.id,
                    entry.flags,
                    entry.mode,
                    path.to_vec(),
                )
            })
        })
        .collect::<Vec<_>>();

    let previous_len = index.entries().len();
    index.remove_entries(|_, path, _| selected.contains::<[u8]>(&path[..]));
    let mut entries_changed = index.entries().len() != previous_len;
    for (stat, id, flags, mode, path) in replacements {
        index.dangerously_push_entry(stat, id, flags, mode, path.as_bstr());
        entries_changed = true;
    }

    if entries_changed {
        index.sort_entries();
    }
    write_index(index, entries_changed)
}

pub(crate) fn refresh(
    repo: &gix::Repository,
    _force: bool,
) -> Result<(), GixError> {
    let exists = repo
        .index_path()
        .try_exists()
        .map_err(|err| GixError::Io(chain_to_string(&err)))?;
    if exists {
        let size = std::fs::metadata(repo.index_path())
            .map_err(|err| GixError::Io(chain_to_string(&err)))?
            .len();
        if size < repo.object_hash().len_in_bytes() as u64 {
            return Err(GixError::Other(message(format!(
                "the index file is too short: {size} bytes"
            ))));
        }
        let _ = repo.open_index().map_err(|err| other(&err))?;
    }
    Ok(())
}

pub(crate) fn entries(
    repo: &gix::Repository,
) -> Result<Vec<IndexEntryRecord>, GixError> {
    let index = repo.index_or_empty().map_err(|err| other(&err))?;
    Ok(index
        .entries()
        .iter()
        .map(|entry| IndexEntryRecord {
            path: ffi::Vec::from(entry.path(&index).to_vec()),
            stage: entry.stage_raw(),
        })
        .collect())
}

pub(crate) fn resolve_conflict_as_deleted(
    repo: &gix::Repository,
    path: &[u8],
) -> Result<(), GixError> {
    if path.is_empty() || path.contains(&0) {
        return Err(GixError::InvalidPath(message(
            "an index path must be non-empty and contain no NUL bytes",
        )));
    }

    let mut index = owned_index(repo)?;
    if !remove_path(&mut index, path) {
        return Err(GixError::NotFound(message(format!(
            "the index has no entry at '{}'",
            String::from_utf8_lossy(path)
        ))));
    }
    write_index(index, true)
}
