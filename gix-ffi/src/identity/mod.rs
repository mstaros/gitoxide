use interoptopus::ffi;

use crate::{GixError, chain_to_string, other};

/// An optional owned signature. Absence is separate from empty name/email bytes.
#[ffi]
#[derive(Debug, Clone)]
pub struct SignatureRecord {
    pub is_present: bool,
    pub name: ffi::Vec<u8>,
    pub email: ffi::Vec<u8>,
    pub time_seconds: i64,
    pub time_offset_seconds: i32,
}

impl SignatureRecord {
    fn absent() -> Self {
        Self {
            is_present: false,
            name: Vec::new().into(),
            email: Vec::new().into(),
            time_seconds: 0,
            time_offset_seconds: 0,
        }
    }

    fn from_owned(signature: gix::actor::Signature) -> Self {
        Self {
            is_present: true,
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
) -> Result<SignatureRecord, GixError> {
    match signature {
        None => Ok(SignatureRecord::absent()),
        Some(Err(error)) => Err(GixError::Config(chain_to_string(&error))),
        Some(Ok(signature)) => from_ref(signature),
    }
}

pub(crate) fn resolve(
    repo: &gix::Repository,
    name: &[u8],
    email: &[u8],
    time_seconds: i64,
    time_offset_seconds: i32,
    only_mapped: bool,
) -> SignatureRecord {
    // Mailmap resolution accepts arbitrary identity bytes, including empty fields.
    // Repository::open_mailmap deliberately keeps partial results and ignores load errors.
    let signature = gix::actor::Signature {
        name: name.to_vec().into(),
        email: email.to_vec().into(),
        time: gix::date::Time::new(time_seconds, time_offset_seconds),
    };
    let mut time = gix::date::parse::TimeBuf::default();
    let signature = signature.to_ref(&mut time);
    let mailmap = repo.open_mailmap();
    if only_mapped {
        mailmap.try_resolve(signature)
            .map(SignatureRecord::from_owned)
            .unwrap_or_else(SignatureRecord::absent)
    } else {
        SignatureRecord::from_owned(mailmap.resolve(signature))
    }
}

#[cfg(test)]
mod tests;