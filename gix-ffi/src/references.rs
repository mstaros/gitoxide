use interoptopus::ffi;

use gix::bstr::ByteSlice;
use gix::refs::{
    FullName, Target, TargetRef,
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
};

use crate::{
    GixError, chain_to_string, commit_from_revision, hex, message, other, parse_id_for_repo,
    path_from_bytes,
};

const LOCAL_BRANCHES: u32 = 1;
const REMOTE_BRANCHES: u32 = 2;
const ALL_BRANCHES: u32 = LOCAL_BRANCHES | REMOTE_BRANCHES;

/// The result of a guarded reference update.
///
/// Replaces a `bool` that collapsed two different refusals into one value.
/// `Mismatch` and `Absent` both mean nothing was changed, but they call for
/// different recovery: a mismatch means another writer moved the reference and
/// the caller must re-read before retrying, while an absent reference means
/// there is nothing left to act on at all.
///
/// Payload-free on purpose. Interoptopus projects a unit-only enum as a plain
/// C# `enum` only when its discriminant is a type C# accepts as an enum base;
/// it then crosses as `ManagedConversion::AsIs` with no unmanaged mirror and no
/// marshaller. Declaring a `repr` here would be inert: `#[ffi]` strips whatever
/// the source wrote and substitutes its own optimal discriminant, and the C# base
/// type is derived from that same choice, so the two sides cannot disagree.
#[ffi]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceUpdateOutcome {
    /// The reference matched the expected value and the change was made.
    Applied,
    /// The reference exists but did not match the expected value. Nothing was
    /// changed.
    Mismatch,
    /// The reference does not exist. Nothing was changed.
    Absent,
}

#[ffi]
#[derive(Debug, Clone)]
pub struct ReferenceRecord {
    pub name: ffi::Vec<u8>,
    pub shorthand: ffi::Vec<u8>,
    pub target: ffi::String,
    pub has_target: bool,
    pub symbolic_target: ffi::Vec<u8>,
    pub has_symbolic_target: bool,
}

#[ffi]
#[derive(Debug, Clone)]
pub struct BranchRecord {
    pub name: ffi::Vec<u8>,
    pub is_remote: bool,
    pub target: ffi::String,
    pub has_target: bool,
}

#[ffi]
#[derive(Debug, Clone)]
pub struct OptionalObjectId {
    pub found: bool,
    pub id: ffi::String,
}

#[ffi(service)]
pub struct ReferenceLockLease {
    _markers: std::vec::Vec<gix::lock::Marker>,
}

#[ffi]
impl ReferenceLockLease {
    /// Acquire cooperative loose-reference locks from an exact repository path.
    ///
    /// A service constructor is required here: returning a service value from
    /// another service method is not FFI-safe in Interoptopus.
    pub fn acquire(
        repository_path: ffi::Slice<u8>,
        encoded_names: ffi::Slice<u8>,
    ) -> ffi::Result<Self, GixError> {
        let path = match path_from_bytes(repository_path.as_slice()) {
            Ok(path) => path,
            Err(error) => return ffi::Err(error),
        };
        let repository = match gix::ThreadSafeRepository::open(path) {
            Ok(repository) => repository,
            Err(error) => return ffi::Err(error.into()),
        };
        let repository = repository.to_thread_local();
        match acquire_reference_locks(&repository, encoded_names.as_slice()) {
            Ok(lease) => ffi::Ok(lease),
            Err(error) => ffi::Err(error),
        }
    }
}

fn empty_string() -> ffi::String {
    ffi::String::from(String::new())
}

fn invalid_reference(value: impl Into<String>) -> GixError {
    GixError::InvalidReference(message(value))
}

fn reference_conflict(value: impl Into<String>) -> GixError {
    GixError::ReferenceConflict(message(value))
}

fn reference_locked(value: impl Into<String>) -> GixError {
    GixError::ReferenceLocked(message(value))
}

fn full_name(value: &[u8]) -> Result<FullName, GixError> {
    FullName::try_from(value.as_bstr())
        .map_err(|err| GixError::InvalidReference(chain_to_string(&err)))
}

fn branch_name(prefix: &[u8], shorthand: &[u8], local: bool) -> Result<FullName, GixError> {
    if shorthand.is_empty() {
        return Err(invalid_reference("branch name must not be empty"));
    }
    let mut name = prefix.to_vec();
    name.extend_from_slice(shorthand);
    if local {
        gix::validate::reference::branch_name(name.as_bstr())
            .map_err(|err| GixError::InvalidReference(chain_to_string(&err)))?;
    }
    full_name(&name)
}

fn local_branch_name(shorthand: &[u8]) -> Result<FullName, GixError> {
    branch_name(b"refs/heads/", shorthand, true)
}

fn remote_branch_name(shorthand: &[u8]) -> Result<FullName, GixError> {
    branch_name(b"refs/remotes/", shorthand, false)
}

fn head_target_name(value: &[u8]) -> Result<FullName, GixError> {
    if let Some(shorthand) = value.strip_prefix(b"refs/heads/") {
        local_branch_name(shorthand)
    } else if value.starts_with(b"refs/") {
        Err(invalid_reference(
            "HEAD may only point symbolically to a local branch",
        ))
    } else {
        local_branch_name(value)
    }
}

fn reference_record(reference: &gix::Reference<'_>) -> ReferenceRecord {
    let (target, has_target, symbolic_target, has_symbolic_target) = match reference.target() {
        TargetRef::Object(id) => (
            hex(id),
            true,
            ffi::Vec::from(std::vec::Vec::new()),
            false,
        ),
        TargetRef::Symbolic(name) => (
            empty_string(),
            false,
            ffi::Vec::from(name.as_bstr().to_vec()),
            true,
        ),
    };
    ReferenceRecord {
        name: ffi::Vec::from(reference.name().as_bstr().to_vec()),
        shorthand: ffi::Vec::from(reference.name().shorten().to_vec()),
        target,
        has_target,
        symbolic_target,
        has_symbolic_target,
    }
}

fn branch_record(reference: &gix::Reference<'_>, is_remote: bool) -> BranchRecord {
    let (target, has_target) = match reference.target() {
        TargetRef::Object(id) => (hex(id), true),
        TargetRef::Symbolic(_) => (empty_string(), false),
    };
    BranchRecord {
        name: ffi::Vec::from(reference.name().shorten().to_vec()),
        is_remote,
        target,
        has_target,
    }
}

fn edit_precondition_failed(error: &gix::reference::edit::Error) -> bool {
    matches!(
        error,
        gix::reference::edit::Error::FileTransactionPrepare(
            gix::refs::file::transaction::prepare::Error::MustNotExist { .. }
                | gix::refs::file::transaction::prepare::Error::MustExist { .. }
                | gix::refs::file::transaction::prepare::Error::ReferenceOutOfDate { .. }
                | gix::refs::file::transaction::prepare::Error::DeleteReferenceMustExist { .. }
        )
    )
}

/// Classify a failed edit as a refusal the caller can act on, or `None` when it
/// is a real error.
///
/// The two refusals arrive here as already-distinct variants.
/// [`edit_precondition_failed`] answers `bool` and is still correct for callers
/// guarding with `MustNotExist`, where the single refusal is unambiguous; this
/// exists for `MustExistAndMatch`, where it is not.
fn refusal_outcome(error: &gix::reference::edit::Error) -> Option<ReferenceUpdateOutcome> {
    use gix::refs::file::transaction::prepare::Error as Prepare;
    match error {
        gix::reference::edit::Error::FileTransactionPrepare(Prepare::ReferenceOutOfDate { .. }) => {
            Some(ReferenceUpdateOutcome::Mismatch)
        }
        gix::reference::edit::Error::FileTransactionPrepare(
            Prepare::MustExist { .. } | Prepare::DeleteReferenceMustExist { .. },
        ) => Some(ReferenceUpdateOutcome::Absent),
        _ => None,
    }
}

fn map_edit_error(error: gix::reference::edit::Error) -> GixError {
    let detail = chain_to_string(&error);
    match &error {
        gix::reference::edit::Error::NameValidation(_) => GixError::InvalidReference(detail),
        gix::reference::edit::Error::FileTransactionPrepare(inner) => match inner {
            gix::refs::file::transaction::prepare::Error::LockAcquire { .. } => {
                GixError::ReferenceLocked(detail)
            }
            gix::refs::file::transaction::prepare::Error::PackedTransactionAcquire(_) => {
                GixError::ReferenceConflict(detail)
            }
            gix::refs::file::transaction::prepare::Error::Io(_) => GixError::Io(detail),
            _ => GixError::Other(detail),
        },
        gix::reference::edit::Error::LockTimeoutConfiguration(_)
        | gix::reference::edit::Error::ParseCommitterTime(_) => GixError::Config(detail),
        _ => GixError::Other(detail),
    }
}

fn update_edit(name: FullName, new: Target, expected: PreviousValue) -> RefEdit {
    RefEdit {
        change: Change::Update {
            log: LogChange::default(),
            expected,
            new,
        },
        name,
        deref: false,
    }
}

fn delete_edit(name: FullName, expected: PreviousValue) -> RefEdit {
    RefEdit {
        change: Change::Delete {
            expected,
            log: RefLog::AndReference,
        },
        name,
        deref: false,
    }
}

fn previous_value_was_present(edits: &[RefEdit], name: &FullName) -> bool {
    edits
        .iter()
        .find(|edit| &edit.name == name)
        .and_then(|edit| edit.change.previous_value())
        .is_some()
}

pub(crate) fn references(
    repo: &gix::Repository,
    glob: &[u8],
) -> Result<std::vec::Vec<ReferenceRecord>, GixError> {
    let platform = repo.references().map_err(|err| other(&err))?;
    let iter = platform.all().map_err(|err| other(&err))?;
    let pattern = (!glob.is_empty()).then_some(glob.as_bstr());
    let mut records = std::vec::Vec::new();
    for item in iter {
        let reference = item.map_err(|err| other(err.as_ref()))?;
        if pattern.is_some_and(|pattern| {
            !gix::glob::wildmatch(
                pattern,
                reference.name().as_bstr(),
                gix::glob::wildmatch::Mode::empty(),
            )
        }) {
            continue;
        }
        records.push((reference.name().as_bstr().to_vec(), reference_record(&reference)));
    }
    records.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(records.into_iter().map(|(_, record)| record).collect())
}

pub(crate) fn branches(
    repo: &gix::Repository,
    filter: u32,
) -> Result<std::vec::Vec<BranchRecord>, GixError> {
    if filter == 0 || filter & !ALL_BRANCHES != 0 {
        return Err(GixError::Other(message(format!(
            "invalid branch filter value {filter}"
        ))));
    }

    let platform = repo.references().map_err(|err| other(&err))?;
    let mut records = std::vec::Vec::new();
    if filter & LOCAL_BRANCHES != 0 {
        for item in platform.local_branches().map_err(|err| other(&err))? {
            let reference = item.map_err(|err| other(err.as_ref()))?;
            records.push((reference.name().shorten().to_vec(), false, branch_record(&reference, false)));
        }
    }
    if filter & REMOTE_BRANCHES != 0 {
        for item in platform.remote_branches().map_err(|err| other(&err))? {
            let reference = item.map_err(|err| other(err.as_ref()))?;
            records.push((reference.name().shorten().to_vec(), true, branch_record(&reference, true)));
        }
    }
    records.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(records.into_iter().map(|(_, _, record)| record).collect())
}

pub(crate) fn create_branch(
    repo: &gix::Repository,
    shorthand: &[u8],
    target_revision: &ffi::String,
    force: bool,
) -> Result<BranchRecord, GixError> {
    let name = local_branch_name(shorthand)?;
    let target = commit_from_revision(repo, target_revision)?.id;
    let expected = if force {
        PreviousValue::Any
    } else {
        PreviousValue::MustNotExist
    };
    let edit = update_edit(name.clone(), Target::Object(target), expected);
    match repo.edit_reference(edit) {
        Ok(edits) => {
            if !force && previous_value_was_present(&edits, &name) {
                return Err(reference_conflict(format!(
                    "branch {} already exists",
                    name.as_bstr()
                )));
            }
        }
        Err(error) if !force && edit_precondition_failed(&error) => {
            return Err(reference_conflict(format!(
                "branch {} already exists",
                name.as_bstr()
            )));
        }
        Err(error) => return Err(map_edit_error(error)),
    }
    Ok(BranchRecord {
        name: ffi::Vec::from(shorthand.to_vec()),
        is_remote: false,
        target: hex(target.as_ref()),
        has_target: true,
    })
}

pub(crate) fn delete_branch(
    repo: &gix::Repository,
    shorthand: &[u8],
    remote: bool,
) -> Result<(), GixError> {
    let name = if remote {
        remote_branch_name(shorthand)?
    } else {
        local_branch_name(shorthand)?
    };

    if !remote {
        let current = repo.head_name().map_err(|err| other(&err))?;
        if current.as_ref().is_some_and(|current| current == &name) {
            return Err(reference_conflict(format!(
                "cannot delete checked-out branch {}",
                name.as_bstr()
            )));
        }
    }

    let reference = repo
        .try_find_reference(name.as_ref())
        .map_err(|err| other(&err))?
        .ok_or_else(|| GixError::NotFound(message(format!("branch {} was not found", name.as_bstr()))))?;
    let expected = match reference.target() {
        TargetRef::Object(id) => Target::Object(id.to_owned()),
        TargetRef::Symbolic(target) => Target::Symbolic(target.to_owned()),
    };
    drop(reference);

    match repo.edit_reference(delete_edit(
        name.clone(),
        PreviousValue::MustExistAndMatch(expected),
    )) {
        Ok(_) => Ok(()),
        Err(error) if edit_precondition_failed(&error) => Err(reference_conflict(format!(
            "branch {} changed while it was being deleted",
            name.as_bstr()
        ))),
        Err(error) => Err(map_edit_error(error)),
    }
}

pub(crate) fn set_head(repo: &gix::Repository, branch_name: &[u8]) -> Result<(), GixError> {
    let target = head_target_name(branch_name)?;
    let head = full_name(b"HEAD")?;
    repo.edit_reference(update_edit(
        head,
        Target::Symbolic(target),
        PreviousValue::Any,
    ))
    .map(|_| ())
    .map_err(map_edit_error)
}

pub(crate) fn try_get_reference_target(
    repo: &gix::Repository,
    name: &[u8],
) -> Result<OptionalObjectId, GixError> {
    let name = full_name(name)?;
    let Some(mut reference) = repo
        .try_find_reference(name.as_ref())
        .map_err(|err| other(&err))?
    else {
        return Ok(OptionalObjectId {
            found: false,
            id: empty_string(),
        });
    };

    let id = match reference.target() {
        TargetRef::Object(id) => id.to_owned(),
        TargetRef::Symbolic(_) => reference
            .follow_to_object()
            .map_err(|err| GixError::NotFound(chain_to_string(&err)))?
            .detach(),
    };
    Ok(OptionalObjectId {
        found: true,
        id: hex(id.as_ref()),
    })
}

pub(crate) fn try_create_reference(
    repo: &gix::Repository,
    name: &[u8],
    target: &ffi::String,
) -> Result<bool, GixError> {
    let name = full_name(name)?;
    let target = parse_id_for_repo(repo, target)?;
    match repo.edit_reference(update_edit(
        name.clone(),
        Target::Object(target),
        PreviousValue::MustNotExist,
    )) {
        Ok(edits) => Ok(!previous_value_was_present(&edits, &name)),
        Err(error) if edit_precondition_failed(&error) => Ok(false),
        Err(error) => Err(map_edit_error(error)),
    }
}

pub(crate) fn compare_exchange_reference(
    repo: &gix::Repository,
    name: &[u8],
    target: &ffi::String,
    expected: &ffi::String,
) -> Result<ReferenceUpdateOutcome, GixError> {
    let name = full_name(name)?;
    let target = parse_id_for_repo(repo, target)?;
    let expected = parse_id_for_repo(repo, expected)?;
    match repo.edit_reference(update_edit(
        name,
        Target::Object(target),
        PreviousValue::MustExistAndMatch(Target::Object(expected)),
    )) {
        Ok(_) => Ok(ReferenceUpdateOutcome::Applied),
        Err(error) => match refusal_outcome(&error) {
            Some(outcome) => Ok(outcome),
            None => Err(map_edit_error(error)),
        },
    }
}

pub(crate) fn delete_reference(
    repo: &gix::Repository,
    name: &[u8],
    expected: &ffi::String,
) -> Result<ReferenceUpdateOutcome, GixError> {
    let name = full_name(name)?;
    let expected = parse_id_for_repo(repo, expected)?;
    match repo.edit_reference(delete_edit(
        name,
        PreviousValue::MustExistAndMatch(Target::Object(expected)),
    )) {
        Ok(_) => Ok(ReferenceUpdateOutcome::Applied),
        Err(error) => match refusal_outcome(&error) {
            Some(outcome) => Ok(outcome),
            None => Err(map_edit_error(error)),
        },
    }
}

pub(crate) fn acquire_reference_locks(
    repo: &gix::Repository,
    encoded_names: &[u8],
) -> Result<ReferenceLockLease, GixError> {
    if encoded_names.is_empty() {
        return Err(invalid_reference("at least one reference name is required"));
    }

    let mut names = std::vec::Vec::new();
    for raw_name in encoded_names.split(|byte| *byte == 0) {
        if raw_name.is_empty() {
            return Err(invalid_reference("reference names must not be empty"));
        }
        names.push(raw_name.to_vec());
    }
    names.sort();
    names.dedup();

    let mut locations = std::vec::Vec::with_capacity(names.len());
    for raw_name in &names {
        let name = full_name(raw_name)?;
        let (base, relative) = repo.refs.reference_path_with_base(name.as_ref());
        let resource = base.join(relative);
        locations.push((resource, base.into_owned()));
    }

    let mut markers = std::vec::Vec::with_capacity(locations.len());
    for (resource, boundary) in locations {
        match gix::lock::Marker::acquire_to_hold_resource(
            resource,
            gix::lock::acquire::Fail::Immediately,
            Some(boundary),
        ) {
            Ok(marker) => markers.push(marker),
            Err(gix::lock::acquire::Error::PermanentlyLocked { .. }) => {
                return Err(reference_locked(
                    "one or more requested references are already locked",
                ));
            }
            Err(gix::lock::acquire::Error::Io(error)) => {
                return Err(GixError::Io(chain_to_string(&error)));
            }
        }
    }
    Ok(ReferenceLockLease { _markers: markers })
}