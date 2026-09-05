use gix::bstr::ByteSlice;
use gix::refs::{
    FullName, Target, TargetRef,
    transaction::{Change, LogChange, PreviousValue, RefEdit},
};
use interoptopus::ffi;

#[cfg(test)]
mod tests;

use crate::{GixError, chain_to_string, explicit_signature, hex, message, other, parse_id_for_repo};

/// One owned note blob with the signatures of its notes commit.
#[ffi]
#[derive(Debug, Clone)]
pub struct NoteRecord {
    pub id: ffi::String,
    pub message: ffi::Vec<u8>,
    pub author_name: ffi::Vec<u8>,
    pub author_email: ffi::Vec<u8>,
    pub author_time_seconds: i64,
    pub author_time_offset_seconds: i32,
    pub committer_name: ffi::Vec<u8>,
    pub committer_email: ffi::Vec<u8>,
    pub committer_time_seconds: i64,
    pub committer_time_offset_seconds: i32,
}

#[ffi]
#[derive(Debug, Clone)]
pub struct NoteEntryRecord {
    pub annotated_object_id: ffi::String,
    pub note: NoteRecord,
}

struct Root {
    name: FullName,
    tree_id: gix::ObjectId,
    parent_commit_id: Option<gix::ObjectId>,
    aliases: Vec<(FullName, FullName)>,
}

fn selected_ref(repo: &gix::Repository, name: &[u8]) -> Result<Option<FullName>, GixError> {
    if name.is_empty() {
        let platform = repo.notes().map_err(|err| other(&err))?;
        return Ok(platform.default_ref().map(ToOwned::to_owned));
    }
    FullName::try_from(name.as_bstr())
        .map(Some)
        .map_err(|err| GixError::InvalidReference(chain_to_string(&err)))
}

fn root(repo: &gix::Repository, mut name: FullName) -> Result<Root, GixError> {
    let mut aliases = Vec::new();
    loop {
        if aliases.len() >= 32 || aliases.iter().any(|(seen, _)| seen == &name) {
            return Err(GixError::InvalidReference(message("the notes reference contains a symbolic cycle or exceeds 32 links")));
        }
        let reference = repo.try_find_reference(name.as_ref()).map_err(|err| other(&err))?;
        match reference.as_ref().map(gix::Reference::target) {
            None => return Ok(Root {
                name,
                tree_id: gix::ObjectId::empty_tree(repo.object_hash()),
                parent_commit_id: None,
                aliases,
            }),
            Some(TargetRef::Symbolic(next)) => {
                let next = next.to_owned();
                aliases.push((name, next.clone()));
                name = next;
            }
            Some(TargetRef::Object(commit_id)) => {
                let commit = repo.find_commit(commit_id).map_err(|err| other(&err))?;
                return Ok(Root {
                    name,
                    tree_id: commit.tree_id().map_err(|err| other(&err))?.detach(),
                    parent_commit_id: Some(commit.id),
                    aliases,
                });
            }
        }
    }
}

fn record(repo: &gix::Repository, root: &Root, note_blob_id: gix::ObjectId) -> Result<NoteRecord, GixError> {
    let commit_id = root.parent_commit_id.ok_or_else(|| GixError::Other(message("a note has no notes commit")))?;
    let commit = repo.find_commit(commit_id).map_err(|err| other(&err))?;
    let author = commit.author().map_err(|err| other(&err))?;
    let committer = commit.committer().map_err(|err| other(&err))?;
    let author_time = author.time().map_err(|err| other(&err))?;
    let committer_time = committer.time().map_err(|err| other(&err))?;
    let mut blob = repo.find_blob(note_blob_id).map_err(|err| other(&err))?;
    Ok(NoteRecord {
        id: hex(note_blob_id.as_ref()),
        message: std::mem::take(&mut blob.data).into(),
        author_name: author.name.to_vec().into(),
        author_email: author.email.to_vec().into(),
        author_time_seconds: author_time.seconds,
        author_time_offset_seconds: author_time.offset,
        committer_name: committer.name.to_vec().into(),
        committer_email: committer.email.to_vec().into(),
        committer_time_seconds: committer_time.seconds,
        committer_time_offset_seconds: committer_time.offset,
    })
}

fn missing() -> GixError {
    GixError::NotFound(message("no note exists for the annotated object in the selected notes reference"))
}

pub(crate) fn read(repo: &gix::Repository, annotated: &ffi::String, name: &[u8]) -> Result<NoteRecord, GixError> {
    let annotated_object_id = parse_id_for_repo(repo, annotated)?;
    let name = selected_ref(repo, name)?.ok_or_else(missing)?;
    let root = root(repo, name)?;
    if root.parent_commit_id.is_none() {
        return Err(missing());
    }
    let note_blob_id = gix::note::plumbing::get(root.tree_id, &annotated_object_id, &repo)
        .map_err(|err| other(&err.into_error()))?.ok_or_else(missing)?;
    record(repo, &root, note_blob_id)
}

/// Materialize the existing compatibility result; general note cursors remain separate work.
pub(crate) fn enumerate(repo: &gix::Repository, name: &[u8]) -> Result<Vec<NoteEntryRecord>, GixError> {
    let Some(name) = selected_ref(repo, name)? else { return Ok(Vec::new()); };
    let root = root(repo, name)?;
    if root.parent_commit_id.is_none() {
        return Ok(Vec::new());
    }
    let hex_len = repo.object_hash().len_in_hex();
    let mut pending = vec![(root.tree_id, Vec::<u8>::new())];
    let mut notes = std::collections::BTreeMap::new();
    while let Some((tree_id, prefix)) = pending.pop() {
        let tree = repo.find_tree(tree_id).map_err(|err| other(&err))?;
        for entry in tree.iter() {
            let entry = entry.map_err(|err| other(&err))?;
            let mut path = prefix.clone();
            path.extend_from_slice(entry.filename());
            if entry.mode().is_blob() && path.len() == hex_len {
                if let Ok(annotated_object_id) = gix::ObjectId::from_hex(&path) {
                    if notes.insert(annotated_object_id, entry.object_id()).is_some() {
                        return Err(GixError::Other(message("multiple notes map to the same annotated object")));
                    }
                }
            } else if entry.mode().is_tree() && entry.filename().len() == 2
                && path.len() < hex_len && entry.filename().iter().all(u8::is_ascii_hexdigit)
            {
                pending.push((entry.object_id(), path));
            }
        }
    }
    notes.into_iter().map(|(annotated_object_id, note_blob_id)| {
        Ok(NoteEntryRecord {
            annotated_object_id: hex(annotated_object_id.as_ref()),
            note: record(repo, &root, note_blob_id)?,
        })
    }).collect()
}

pub(crate) fn signature(name: &[u8], email: &[u8], seconds: i64, offset: i32) -> Result<gix::actor::Signature, GixError> {
    if name.iter().chain(email).any(|value| matches!(value, 0 | 10 | 13 | 60 | 62)) {
        return Err(GixError::Other(message("a note signature cannot contain NUL, newlines or angle brackets")));
    }
    explicit_signature(name, email, seconds, offset)
}

fn publish(repo: &gix::Repository, root: Root, tree_id: gix::ObjectId,
    author: &gix::actor::Signature, committer: &gix::actor::Signature, text: &str) -> Result<(), GixError>
{
    let mut author_time = gix::date::parse::TimeBuf::default();
    let mut committer_time = gix::date::parse::TimeBuf::default();
    let author = author.to_ref(&mut author_time);
    let committer = committer.to_ref(&mut committer_time);
    let commit = repo.new_commit_as(committer, author, text, tree_id, root.parent_commit_id)
        .map_err(|err| other(&err))?;
    let expected = root.parent_commit_id.map_or(PreviousValue::MustNotExist,
        |id| PreviousValue::MustExistAndMatch(Target::Object(id)));
    let mut edits = root.aliases.into_iter().map(|(name, target)| RefEdit {
        name,
        deref: false,
        change: Change::Update {
            log: LogChange::default(),
            expected: PreviousValue::MustExistAndMatch(Target::Symbolic(target.clone())),
            new: Target::Symbolic(target),
        },
    }).collect::<Vec<_>>();
    edits.push(RefEdit {
        name: root.name.clone(),
        deref: false,
        change: Change::Update {
            log: LogChange { message: format!("notes: {text}").into(), ..Default::default() },
            expected,
            new: Target::Object(commit.id),
        },
    });
    let applied = repo.edit_references_as(edits, Some(committer)).map_err(|error| {
        use gix::refs::file::transaction::prepare::Error as Prepare;
        match error {
            gix::reference::edit::Error::FileTransactionPrepare(
                Prepare::MustExist { .. } | Prepare::MustNotExist { .. }
                | Prepare::ReferenceOutOfDate { .. } | Prepare::DeleteReferenceMustExist { .. }
            ) => GixError::ReferenceConflict(chain_to_string(&error)),
            error => crate::references::map_edit_error(error),
        }
    })?;
    if root.parent_commit_id.is_none() && applied.iter().any(|edit|
        edit.name == root.name && edit.change.previous_value().is_some())
    {
        return Err(GixError::ReferenceConflict(message("another writer created the notes reference")));
    }
    Ok(())
}

pub(crate) fn write(repo: &gix::Repository, annotated: &ffi::String, name: &[u8], data: &[u8],
    author: gix::actor::Signature, committer: gix::actor::Signature, overwrite: bool) -> Result<ffi::String, GixError>
{
    let annotated_object_id = parse_id_for_repo(repo, annotated)?;
    let name = selected_ref(repo, name)?
        .ok_or_else(|| GixError::Config(message("the default notes reference is disabled by core.notesRef")))?;
    let root = root(repo, name)?;
    if !overwrite && root.parent_commit_id.is_some()
        && gix::note::plumbing::get(root.tree_id, &annotated_object_id, &repo)
            .map_err(|err| other(&err.into_error()))?.is_some()
    {
        return Err(GixError::ReferenceConflict(message("a note already exists; enable overwrite to replace it")));
    }
    let note_blob_id = repo.write_blob(data).map_err(|err| other(&err))?.detach();
    let edit = gix::note::plumbing::replace(root.tree_id, annotated_object_id, note_blob_id, &repo)
        .map_err(|err| other(&err.into_error()))?;
    publish(repo, root, edit.tree, &author, &committer, "Notes added by GixSharp")?;
    Ok(hex(note_blob_id.as_ref()))
}

pub(crate) fn remove(repo: &gix::Repository, annotated: &ffi::String, name: &[u8],
    author: gix::actor::Signature, committer: gix::actor::Signature) -> Result<bool, GixError>
{
    let annotated_object_id = parse_id_for_repo(repo, annotated)?;
    let Some(name) = selected_ref(repo, name)? else { return Ok(false); };
    let root = root(repo, name)?;
    if root.parent_commit_id.is_none() {
        return Ok(false);
    }
    let edit = gix::note::plumbing::remove(root.tree_id, annotated_object_id, &repo)
        .map_err(|err| other(&err.into_error()))?;
    if edit.previous.is_none() {
        return Ok(false);
    }
    publish(repo, root, edit.tree, &author, &committer, "Notes removed by GixSharp")?;
    Ok(true)
}