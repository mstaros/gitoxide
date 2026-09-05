use gix::bstr::ByteSlice;
use gix::refs::{FullName, Target, transaction::{Change, LogChange, PreviousValue, RefEdit}};
use interoptopus::ffi;

use crate::{FfiObjectType, GixError, chain_to_string, explicit_signature, hex, message, other, parse_id_for_repo};

/// Owned tagger fields.
#[ffi]
#[derive(Debug, Clone)]
pub struct TagSignatureRecord {
    pub name: ffi::Vec<u8>,
    pub email: ffi::Vec<u8>,
    pub time_seconds: i64,
    pub time_offset_seconds: i32,
}

/// An owned annotated tag. Message retains the entire body, including any signature.
#[ffi]
#[derive(Debug, Clone)]
pub struct TagRecord {
    pub id: ffi::String,
    pub target_id: ffi::String,
    pub target_type: FfiObjectType,
    pub name: ffi::Vec<u8>,
    pub message: ffi::Vec<u8>,
    pub tagger: ffi::Option<TagSignatureRecord>,
    pub signature: ffi::Option<ffi::Vec<u8>>,
}

fn tag_name(name: &[u8]) -> Result<FullName, GixError> {
    gix::validate::tag::name(name.as_bstr())
        .map_err(|err| GixError::InvalidReference(chain_to_string(&err)))?;
    if name.starts_with(b"-") {
        return Err(GixError::InvalidReference(message("tag names must not start with '-'")));
    }
    let mut full = b"refs/tags/".to_vec();
    full.extend_from_slice(name);
    // Ref names become filesystem paths; Windows cannot represent arbitrary Git bytes.
    crate::path_from_bytes(&full)?;
    FullName::try_from(full.as_bstr())
        .map_err(|err| GixError::InvalidReference(chain_to_string(&err)))
}

fn find_object<'repo>(repo: &'repo gix::Repository, id: &ffi::String)
    -> Result<gix::Object<'repo>, GixError>
{
    let object_id = parse_id_for_repo(repo, id)?;
    repo.try_find_object(object_id).map_err(|err| other(&err))?
        .ok_or_else(|| GixError::NotFound(message(format!("object {object_id} does not exist"))))
}

fn target_metadata(repo: &gix::Repository, value: &ffi::String)
    -> Result<(gix::ObjectId, gix::objs::Kind), GixError>
{
    let target_id = parse_id_for_repo(repo, value)?;
    // Creating a tag only needs the target's kind, not a potentially large blob payload.
    let header = repo.try_find_header(target_id).map_err(|err| other(&err))?
        .ok_or_else(|| GixError::NotFound(message(format!("object {target_id} does not exist"))))?;
    Ok((target_id, header.kind()))
}

pub(crate) fn signature(name: &[u8], email: &[u8], seconds: i64, offset: i32)
    -> Result<gix::actor::Signature, GixError>
{
    if name.iter().chain(email).any(|byte| matches!(byte, 0 | 10 | 13 | 60 | 62)) {
        return Err(GixError::Other(message("a tagger identity cannot contain NUL, newlines or angle brackets")));
    }
    explicit_signature(name, email, seconds, offset)
}

fn publish(repo: &gix::Repository, name: FullName, target_id: gix::ObjectId, force: bool)
    -> Result<(), GixError>
{
    if !force {
        if !crate::references::try_create_reference(repo, name.as_bstr(), &hex(target_id.as_ref()))? {
            return Err(GixError::ReferenceConflict(message(format!("tag {} already exists", name.as_bstr()))));
        }
        return Ok(());
    }
    // Like Repository::tag_reference(), replace the tag itself, without dereferencing a symbolic tag.
    repo.edit_reference(RefEdit {
        name,
        deref: false,
        change: Change::Update {
            log: LogChange::default(),
            expected: PreviousValue::Any,
            new: Target::Object(target_id),
        },
    }).map_err(crate::references::map_edit_error)?;
    Ok(())
}

pub(crate) fn create(repo: &gix::Repository, name: &[u8], target_id: &ffi::String,
    tagger: Option<gix::actor::Signature>, data: &[u8], force: bool)
    -> Result<ffi::String, GixError>
{
    let reference_name = tag_name(name)?;
    let (target_id, target_kind) = target_metadata(repo, target_id)?;
    // Repository::tag() accepts str. Its underlying owned object and reference plumbing
    // also handles Git's non-UTF-8 names, messages and identities without re-encoding.
    let tag = gix::objs::Tag {
        target: target_id,
        target_kind,
        name: name.to_vec().into(),
        tagger,
        message: data.to_vec().into(),
        signature: None,
    };
    let tag_id = repo.write_object(&tag).map_err(|err| other(&err))?.detach();
    publish(repo, reference_name, tag_id, force)?;
    Ok(hex(tag_id.as_ref()))
}

pub(crate) fn create_reference(repo: &gix::Repository, name: &[u8], target_id: &ffi::String, force: bool)
    -> Result<(), GixError>
{
    let reference_name = tag_name(name)?;
    let (target_id, _) = target_metadata(repo, target_id)?;
    publish(repo, reference_name, target_id, force)
}

pub(crate) fn read(repo: &gix::Repository, tag_id: &ffi::String) -> Result<TagRecord, GixError> {
    let object = find_object(repo, tag_id)?;
    if object.kind != gix::objs::Kind::Tag {
        return Err(GixError::Other(message(format!("object {} is {:?}, not an annotated tag", object.id, object.kind))));
    }
    let tag = object.into_tag();
    let decoded = tag.decode().map_err(|err| other(&err))?;
    // Decode without TagRef::tagger()'s trimming so raw identity bytes remain intact.
    let tagger = decoded.tagger.map(|raw| {
        let mut input: &[u8] = raw.as_ref();
        let signature = gix::actor::SignatureRef::from_bytes_consuming(&mut input).map_err(|err| other(&err))?;
        let time = signature.time().map_err(|err| other(&err))?;
        Ok::<_, GixError>(TagSignatureRecord {
            name: signature.name.to_vec().into(),
            email: signature.email.to_vec().into(),
            time_seconds: time.seconds,
            time_offset_seconds: time.offset,
        })
    }).transpose()?;
    // gix separates a signed body and strips its joining newline. Copy the original
    // body instead so MessageBytes remains byte-for-byte equal to what Git stores.
    let message = if decoded.signature.is_some() {
        let separator = tag.data.windows(2).position(|bytes| bytes == b"\n\n")
            .ok_or_else(|| GixError::Other(message("signed tag has no header/body separator")))?;
        &tag.data[separator + 2..]
    } else {
        decoded.message.as_ref()
    };
    Ok(TagRecord {
        id: hex(tag.id.as_ref()),
        target_id: hex(decoded.target().as_ref()),
        target_type: match decoded.target_kind {
            gix::objs::Kind::Commit => FfiObjectType::Commit,
            gix::objs::Kind::Tree => FfiObjectType::Tree,
            gix::objs::Kind::Blob => FfiObjectType::Blob,
            gix::objs::Kind::Tag => FfiObjectType::Tag,
        },
        name: decoded.name.to_vec().into(),
        message: message.to_vec().into(),
        tagger: tagger.into(),
        signature: decoded.signature.map(|value| ffi::Vec::from(value.to_vec())).into(),
    })
}

pub(crate) fn peel(repo: &gix::Repository, object_id: &ffi::String) -> Result<ffi::String, GixError> {
    let object = find_object(repo, object_id)?;
    let peeled = object.peel_tags_to_end().map_err(|err| other(&err))?;
    Ok(hex(peeled.id.as_ref()))
}