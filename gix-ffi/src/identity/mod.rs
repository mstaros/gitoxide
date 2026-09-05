use crate::{GixError, TagSignatureRecord as SignatureRecord, chain_to_string, other};

impl SignatureRecord {
    fn from_owned(signature: gix::actor::Signature) -> Self {
        Self {
            name: signature.name.to_vec().into(),
            email: signature.email.to_vec().into(),
            time_seconds: signature.time.seconds,
            time_offset_seconds: signature.time.offset,
        }
    }
}

pub(crate) fn from_ref(signature: gix::actor::SignatureRef<'_>) -> Result<SignatureRecord, GixError> {
    signature.to_owned().map(SignatureRecord::from_owned).map_err(|error| other(&error))
}

pub(crate) fn configured(
    signature: Option<Result<gix::actor::SignatureRef<'_>, gix::config::time::Error>>,
) -> Result<Option<SignatureRecord>, GixError> {
    match signature {
        None => Ok(None),
        Some(Err(error)) => Err(GixError::Config(chain_to_string(&error))),
        Some(Ok(signature)) => from_ref(signature).map(Some),
    }
}

fn signature(name: &[u8], email: &[u8], seconds: i64, offset: i32) -> gix::actor::Signature {
    // Mailmap resolution accepts arbitrary identity bytes, including empty fields.
    gix::actor::Signature {
        name: name.to_vec().into(),
        email: email.to_vec().into(),
        time: gix::date::Time::new(seconds, offset),
    }
}

pub(crate) fn resolve(
    repo: &gix::Repository, name: &[u8], email: &[u8], seconds: i64, offset: i32,
) -> SignatureRecord {
    let signature = signature(name, email, seconds, offset);
    let mut time = gix::date::parse::TimeBuf::default();
    // open_mailmap deliberately keeps partial results and ignores load errors.
    SignatureRecord::from_owned(repo.open_mailmap().resolve(signature.to_ref(&mut time)))
}

pub(crate) fn try_resolve(
    repo: &gix::Repository, name: &[u8], email: &[u8], seconds: i64, offset: i32,
) -> Option<SignatureRecord> {
    let signature = signature(name, email, seconds, offset);
    let mut time = gix::date::parse::TimeBuf::default();
    repo.open_mailmap().try_resolve(signature.to_ref(&mut time)).map(SignatureRecord::from_owned)
}

#[cfg(test)]
mod tests;
