//! Owned linked-worktree views and compatibility operations over core administration.
use gix::bstr::ByteSlice;
use gix::repository::worktree_admin::{self, add, prune};
use interoptopus::ffi;

use crate::{GixError, chain_to_string, message, other};

/// An owned administrative snapshot. Main worktrees have no linked registration.
#[ffi]
#[derive(Debug, Clone)]
pub struct WorktreeRecord {
    pub name: ffi::Vec<u8>,
    pub path: ffi::Vec<u8>,
    pub is_valid: bool,
    pub is_locked: bool,
    pub lock_reason: ffi::Vec<u8>,
}

fn record(entry: worktree_admin::Entry) -> WorktreeRecord {
    WorktreeRecord {
        name: Vec::<u8>::from(entry.id).into(),
        path: entry.checkout.as_deref().map(crate::path_bytes).unwrap_or_else(|| Vec::<u8>::new().into()),
        is_valid: entry.condition.is_registered(),
        is_locked: entry.lock_reason.is_some(),
        lock_reason: entry.lock_reason.map(Vec::<u8>::from).unwrap_or_default().into(),
    }
}

pub(crate) fn list(repo: &gix::Repository) -> Result<Vec<WorktreeRecord>, GixError> {
    Ok(repo.worktree_admin_entries().map_err(|err| other(&err.into_inner()))?
        .into_iter().map(record).collect())
}

fn added(repo: &gix::Repository, outcome: add::Outcome) -> Result<WorktreeRecord, GixError> {
    repo.worktree_admin_entries().map_err(|err| other(&err.into_inner()))?
        .into_iter().find(|entry| entry.id == outcome.id).map(record)
        .ok_or_else(|| GixError::Other(message(format!(
            "worktree {:?} was created at {:?}, but its registration disappeared before it could be read",
            outcome.id, outcome.checkout,
        ))))
}

fn map_add_error(error: gix::error::Exn<add::Error>) -> GixError {
    let error = error.into_inner();
    let text = chain_to_string(&error);
    match error {
        add::Error::InvalidName(_) => GixError::InvalidPath(text),
        add::Error::Io { .. } => GixError::Io(text),
        add::Error::BranchInUse { .. } | add::Error::IdentifierExists { .. }
        | add::Error::AlreadyRegistered { .. } => GixError::ReferenceConflict(text),
        _ => GixError::Other(text),
    }
}

fn options(name: &[u8], lock: bool, checkout: bool) -> add::Options {
    add::Options {
        name: Some(name.into()),
        lock: lock.then_some(None),
        checkout,
        ..Default::default()
    }
}

pub(crate) fn add(
    repo: &gix::Repository,
    name: &[u8],
    path: &[u8],
    reference: &[u8],
    lock: bool,
    checkout: bool,
) -> Result<WorktreeRecord, GixError> {
    crate::path_from_bytes(name)?;
    let mut options = options(name, lock, checkout);
    let attach = if reference.is_empty() {
        options.new_branch = Some(name.into());
        add::Attachment::DetachedAt(repo.head_commit().map_err(|err| other(&err))?.id)
    } else {
        let reference = repo.find_reference(reference.as_bstr()).map_err(|err| {
            GixError::InvalidReference(chain_to_string(&err))
        })?;
        if !reference.name().as_bstr().starts_with(b"refs/heads/") {
            return Err(GixError::InvalidReference(message("a worktree reference must resolve to a local branch")));
        }
        add::Attachment::Branch(reference.name().to_owned())
    };
    let outcome = repo.add_worktree(&crate::path_from_bytes(path)?, attach, options).map_err(map_add_error)?;
    added(repo, outcome)
}

pub(crate) fn add_detached(
    repo: &gix::Repository, name: &[u8], path: &[u8], commit: &ffi::String, lock: bool,
) -> Result<WorktreeRecord, GixError> {
    crate::path_from_bytes(name)?;
    let id = crate::parse_id_for_repo(repo, commit)?;
    repo.find_commit(id).map_err(|err| other(&err))?;
    let outcome = repo.add_worktree(
        &crate::path_from_bytes(path)?, add::Attachment::DetachedAt(id), options(name, lock, true),
    ).map_err(map_add_error)?;
    added(repo, outcome)
}

pub(crate) fn prune(
    repo: &gix::Repository, name: Option<&[u8]>, valid: bool, locked: bool, working_tree: bool,
) -> Result<u64, GixError> {
    if let Some(name) = name { crate::path_from_bytes(name)?; }
    let removed = repo.prune_worktrees(prune::Options {
        name: name.map(Into::into),
        include_valid: valid,
        include_locked: locked,
        remove_working_tree: working_tree,
        ..Default::default()
    }).map_err(|error| {
        let error = error.into_inner();
        let text = chain_to_string(&error);
        match error {
            prune::Error::InvalidName(_) => GixError::InvalidPath(text),
            _ => GixError::Other(text),
        }
    })?;
    Ok(removed.into_iter().filter(|candidate| candidate.removed).count() as u64)
}