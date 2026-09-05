//! Git ignore matching and atomic additions to the common info/exclude file.
use std::io::Write;

use gix::bstr::ByteSlice;

use crate::{GixError, chain_to_string, message};

fn validate_path(path: &[u8]) -> Result<(), GixError> {
    if path.is_empty()
        || path.iter().any(|byte| matches!(byte, 0 | b'\n' | b'\r' | b'\\'))
        || path.split(|byte| *byte == b'/').any(|part| part.is_empty() || part == b"." || part == b"..")
    {
        return Err(GixError::InvalidPath(message(
            "expected a non-empty repository-relative Git path with '/' separators, no leading/trailing slash, no dot segments, and no NUL or line breaks",
        )));
    }
    Ok(())
}

pub(crate) fn is_path_ignored(
    repo: &gix::Repository,
    path: &[u8],
    is_directory: bool,
) -> Result<bool, GixError> {
    validate_path(path)?;
    let index = crate::index::owned_index(repo)?;
    let source = gix::worktree::stack::state::ignore::Source::default().adjust_for_bare(repo.is_bare());
    let mut excludes = repo
        .excludes(&index, None, source)
        .map_err(|err| GixError::Config(chain_to_string(&err)))?;
    let mode = if is_directory { gix::index::entry::Mode::DIR } else { gix::index::entry::Mode::FILE };
    let platform = excludes
        .at_entry(path.as_bstr(), Some(mode))
        .map_err(|err| GixError::Io(chain_to_string(&err)))?;
    Ok(platform.is_excluded())
}

/// Escape an exact path as a root-anchored ignore rule, never a caller-supplied glob.
fn literal_rule(path: &[u8], is_directory: bool) -> Vec<u8> {
    let mut rule = Vec::with_capacity(path.len() + 2);
    rule.push(b'/');
    for byte in path {
        if matches!(byte, b'*' | b'?' | b'[' | b']' | b' ' | b'\\') {
            rule.push(b'\\');
        }
        rule.push(*byte);
    }
    if is_directory {
        rule.push(b'/');
    }
    rule
}

fn append_rule(repo: &gix::Repository, rule: &[u8]) -> Result<(), GixError> {
    let common = repo.common_dir();
    let exclude = common.join("info").join("exclude");
    // gix owns only the lock it successfully acquired. In particular, acquisition
    // failure must never unlink another process's lock.
    let mut lock = gix::lock::File::acquire_to_update_resource(
        &exclude,
        gix::lock::acquire::Fail::AfterDurationWithBackoff(std::time::Duration::from_secs(1)),
        Some(common.to_owned()),
    )
    .map_err(|err| GixError::Io(chain_to_string(&err)))?;
    let existing = match std::fs::read(&exclude) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => return Err(GixError::Io(chain_to_string(&err))),
    };
    if existing.split(|byte| *byte == b'\n').any(|line| line.strip_suffix(b"\r").unwrap_or(line) == rule) {
        return Ok(());
    }
    lock.write_all(&existing).map_err(|err| GixError::Io(chain_to_string(&err)))?;
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        lock.write_all(b"\n").map_err(|err| GixError::Io(chain_to_string(&err)))?;
    }
    lock.write_all(rule).map_err(|err| GixError::Io(chain_to_string(&err)))?;
    lock.write_all(b"\n").map_err(|err| GixError::Io(chain_to_string(&err)))?;
    lock.with_mut(|file| file.sync_all()).map_err(|err| GixError::Io(chain_to_string(&err)))?;
    lock.commit().map_err(|err| GixError::Io(chain_to_string(&err)))?;
    Ok(())
}

pub(crate) fn ensure_local_exclude(
    repo: &gix::Repository,
    path: &[u8],
    is_directory: bool,
) -> Result<(), GixError> {
    validate_path(path)?;
    let rule = literal_rule(path, is_directory);
    append_rule(repo, &rule)?;
    // Every lookup creates a fresh stack, so same-timestamp updates cannot reuse
    // stale ignore data. An overriding .gitignore rule must remain authoritative.
    if !is_path_ignored(repo, path, is_directory)? {
        return Err(GixError::Other(message(
            "the local exclude rule is present, but higher-priority ignore rules still include the path",
        )));
    }
    Ok(())
}