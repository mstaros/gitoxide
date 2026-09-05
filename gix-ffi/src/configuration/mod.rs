//! Repository configuration reads and atomic local configuration edits.

use gix::bstr::{BString, ByteSlice};
use gix::config::{File, KeyRef, Source};

use crate::{GixError, chain_to_string, message};

fn key(name: &[u8]) -> Result<KeyRef<'_>, GixError> {
    let invalid = || GixError::Config(message("expected a valid section[.subsection].name configuration key"));
    if name.iter().any(|byte| matches!(byte, 0 | b'\n' | b'\r')) {
        return Err(invalid());
    }
    let key = KeyRef::parse_unvalidated(name.as_bstr()).ok_or_else(invalid)?;
    let valid_part = |part: &str| !part.is_empty()
        && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if !valid_part(key.section_name)
        || !key.section_name.as_bytes()[0].is_ascii_alphanumeric()
        || !valid_part(key.value_name)
        || !key.value_name.as_bytes()[0].is_ascii_alphabetic()
    {
        return Err(invalid());
    }
    Ok(key)
}

fn matches(section: gix::config::file::SectionRef<'_>, key: KeyRef<'_>) -> bool {
    section.header().name().eq_ignore_ascii_case(key.section_name.as_bytes())
        && section.header().subsection_name() == key.subsection_name
}

pub(crate) fn get(repo: &gix::Repository, name: &[u8]) -> Result<Vec<u8>, GixError> {
    let key = key(name)?;
    let config = repo.config_snapshot().reload().map_err(|err| GixError::Config(chain_to_string(&err)))?;
    // The string convenience accessor skips implicit values. Here an implicit
    // key is present with an empty string, matching git_config_get_string_buf.
    let mut value = None;
    for section in config.sections() {
        if matches(section, key) {
            if let Some(found) = section.value_implicit(key.value_name) {
                value = Some(found.unwrap_or_default());
            }
        }
    }
    value.map(Vec::from).ok_or_else(|| GixError::NotFound(message("configuration key was not found")))
}

/// Check the writable backend, including its includes, as libgit2's single-value
/// set/delete methods do. Included or repeated keys require an explicit multivar API.
fn local_value(config: &File, key: KeyRef<'_>) -> Result<Option<Option<BString>>, GixError> {
    let mut count = 0;
    let mut value = None;
    let mut included = false;
    for section in config.sections() {
        if section.meta().source != Source::Local || !matches(section, key) {
            continue;
        }
        let occurrences = section.value_names().filter(|name| name.eq_ignore_ascii_case(key.value_name)).count();
        if occurrences != 0 {
            count += occurrences;
            included |= section.meta().level != 0;
            value = section.value_implicit(key.value_name);
        }
    }
    if count > 1 || included {
        return Err(GixError::Config(message(
            "single-value configuration edits cannot modify repeated or included keys",
        )));
    }
    Ok(value)
}

fn no_change(existing: &Option<Option<BString>>, value: Option<&[u8]>) -> bool {
    match (existing, value) {
        (None, None) => true,
        (Some(Some(existing)), Some(value)) => existing.as_slice() == value,
        _ => false,
    }
}

/// Refresh valid typed settings after a successful raw edit. Repository::reload
/// leaves the previous repository intact on failure. Generic config operations
/// remain usable to read and repair malformed typed values through raw reloads.
fn refresh_typed_settings(repo: &mut gix::Repository) {
    // Persistence has succeeded already. A typed-validation failure must not turn
    // this into a reported write failure or prevent a later repair on this handle.
    let _ = repo.reload();
}

/// Returns whether a local value existed (the delete result).
pub(crate) fn edit(
    repo: &mut gix::Repository,
    name: &[u8],
    value: Option<&[u8]>,
) -> Result<bool, GixError> {
    let key = key(name)?;
    if value.is_some_and(|value| value.contains(&0)) {
        return Err(GixError::Config(message("configuration string values cannot contain NUL")));
    }
    let config = repo.config_snapshot().reload().map_err(|err| GixError::Config(chain_to_string(&err)))?;
    let existing = local_value(&config, key)?;
    if no_change(&existing, value) {
        refresh_typed_settings(repo);
        return Ok(existing.is_some());
    }
    // Snapshot::reload records the common config path anchored to the open-time
    // cwd, including while typed reopening is impossible. Use the same path for
    // locking and persistence so a later cwd change cannot redirect the write.
    let path = config.meta().path.clone().ok_or_else(|| {
        GixError::Config(message("fresh configuration is missing its repository-local path"))
    })?;
    let directory = path.parent().ok_or_else(|| {
        GixError::Config(message("repository-local configuration path has no directory"))
    })?;
    let mut lock = gix::lock::File::acquire_to_update_resource(
        &path,
        gix::lock::acquire::Fail::AfterDurationWithBackoff(std::time::Duration::from_secs(1)),
        Some(directory.to_owned()),
    ).map_err(|err| GixError::Io(chain_to_string(&err)))?;
    // Re-read after acquiring the cooperative writer lock to avoid overwriting a
    // change that arrived between the initial compatibility check and acquisition.
    let config = repo.config_snapshot().reload().map_err(|err| GixError::Config(chain_to_string(&err)))?;
    let existing = local_value(&config, key)?;
    if no_change(&existing, value) {
        refresh_typed_settings(repo);
        return Ok(existing.is_some());
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(GixError::Io(chain_to_string(&err))),
    };
    let mut config = File::from_bytes_no_includes(
        &bytes,
        gix::config::file::Metadata::from(Source::Local).at(&path),
        Default::default(),
    ).map_err(|err| GixError::Config(chain_to_string(&err)))?;
    let section_id = config.sections()
        .find(|section| matches(*section, key) && section.contains_value_name(key.value_name))
        .map(|section| section.id());
    match (section_id, value) {
        (Some(id), value) => {
            let mut section = config.section_mut_by_id(id).ok_or_else(|| {
                GixError::Config(message("configuration section disappeared while editing"))
            })?;
            if let Some(value) = value {
                if section.value_implicit(key.value_name) == Some(None) {
                    section.remove(key.value_name);
                    section.push(key.value_name, Some(value.as_bstr()))
                        .map_err(|err| GixError::Config(chain_to_string(&err)))?;
                } else {
                    section.set(key.value_name, value.as_bstr())
                        .map_err(|err| GixError::Config(chain_to_string(&err)))?;
                }
            } else {
                section.remove(key.value_name);
            }
        }
        (None, Some(value)) => {
            config.set_raw_value(key, value.as_bstr())
                .map_err(|err| GixError::Config(chain_to_string(&err)))?;
        }
        (None, None) => return Ok(false),
    }
    config.write_to(&mut lock).map_err(|err| GixError::Io(chain_to_string(&err)))?;
    lock.with_mut(|file| file.sync_all()).map_err(|err| GixError::Io(chain_to_string(&err)))?;
    lock.commit().map_err(|err| GixError::Io(chain_to_string(&err)))?;
    refresh_typed_settings(repo);
    Ok(existing.is_some())
}