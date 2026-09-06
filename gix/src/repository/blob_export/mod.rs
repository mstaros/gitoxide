//! Verified, bounded raw-object export. No checkout filters or replacement refs are applied.
//! See [Platform] for the supported tree-listing and blob-output contract.
use std::{
    collections::BTreeSet,
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use gix_error::{ErrorExt, Exn};
use gix_hash::ObjectId;
use gix_object::{Kind, tree::EntryMode};
use crate::{Repository, bstr::{BString, ByteSlice}};

/// Resource limits enforced independently of repository configuration.
/// These bound individual object/delta allocations, not total process RSS.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Maximum decoded object or delta allocation in bytes.
    pub object_bytes: usize,
    /// Maximum number of entries visited, including directories.
    pub entries: usize,
    /// Maximum nested directory depth.
    pub depth: usize,
    /// Maximum repository-relative path length in bytes.
    pub path_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { object_bytes: 64 * 1024 * 1024, entries: 100_000, depth: 128, path_bytes: 4096 }
    }
}

/// Output forms for full-OID, repository-root-relative tree listings.
#[derive(Clone, Debug, Default)]
pub enum Format {
    /// Mode, object kind, OID, tab and path.
    #[default]
    Default,
    /// Default fields with blob size, like ls-tree -l.
    Long,
    /// Only paths.
    NameOnly,
    /// Only object IDs.
    ObjectOnly,
    /// NUL-delimited output using Git's documented ls-tree format fields:
    /// objectmode, objecttype, objectname, objectsize[:padded], and path.
    /// Literal bytes, %% and %xNN escapes are supported. Generic path fields use
    /// core.quotePath=true; exact built-in formats retain raw -z paths, like Git.
    /// Invalid formats fail
    /// before any output, even for an empty tree.
    Custom(BString),
    /// Default fields with newline termination and Git C-style path quoting,
    /// equivalent to core.quotePath=true, independently of repository config.
    Quoted,
}
/// Supported ls-tree options. Paths are root-relative; Format selects quoting and termination.
#[derive(Clone, Debug, Default)]
pub struct TreeOptions {
    /// Recurse into trees (-r).
    pub recursive: bool,
    /// Include trees when recursing (-t).
    pub show_trees: bool,
    /// Emit trees and gitlinks (-d, which implies -t).
    pub trees_only: bool,
    /// Select an output form.
    pub format: Format,
}
/// A byte-preserving tree record. Gitlink objects need not exist locally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Raw path relative to the root tree.
    pub path: BString,
    /// Canonical Git tree mode.
    pub mode: EntryMode,
    /// Referenced object ID.
    pub id: ObjectId,
    /// Blob size, including symbolic-link blobs; absent for trees and gitlinks.
    pub size: Option<u64>,
}
/// Errors from verified export.
#[derive(Debug, thiserror::Error)]
#[expect(missing_docs)]
pub enum Error {
    #[error("Invalid export limits")]
    InvalidLimits,
    #[error("Unsupported or invalid export output format")]
    Unsupported,
    #[error("Export interrupted")]
    Interrupted,
    #[error("Export resource limit exceeded: {0}")]
    Limit(&'static str),
    #[error("Object {id} has the wrong hash kind")]
    HashKind { id: ObjectId },
    #[error("Object {id} failed content identity or size verification")]
    Integrity { id: ObjectId },
    #[error("Object {id} must be {expected}, got {actual}")]
    Kind { id: ObjectId, expected: Kind, actual: Kind },
    #[error("Invalid tree {id}: {reason}")]
    Tree { id: ObjectId, reason: &'static str },
    #[error("Export operation failed: {operation}")]
    Source {
        operation: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}
fn source(operation: &'static str, err: impl std::error::Error + Send + Sync + 'static) -> Exn<Error> {
    Error::Source { operation, source: Box::new(err) }.raise()
}
fn check_interrupt(cancel: &AtomicBool) -> Result<(), Exn<Error>> {
    if cancel.load(Ordering::Relaxed) { return Err(Error::Interrupted.raise()); }
    Ok(())
}

/// An isolated reader with mandatory allocation limits and replacements disabled.
///
/// list_tree/write_tree cover raw full-tree ls-tree -z output and its recursion,
/// tree-only, long, name-only, object-only, custom and quoted forms. read_object returns verified
/// kind/size/content (cat-file -t/-s/raw equivalents). write_blob writes raw
/// cat-file blob bytes. No command parser, pathspec, abbreviation, textconv,
/// filter, mailmap or batch protocol is implied.
pub struct Platform {
    repo: Repository,
    limits: Limits,
}
impl Repository {
    /// Prepare an isolated, allocation-bounded raw object exporter for this repository.
    ///
    /// Reopening avoids inheriting unbounded decoded caches or replacement-ref
    /// settings. Object and delta allocations use gitoxide.objects.allocLimit.
    pub fn blob_export(&self, limits: Limits) -> Result<Platform, Exn<Error>> {
        if limits.object_bytes == 0 || limits.entries == 0 || limits.depth == 0 || limits.path_bytes == 0 {
            return Err(Error::InvalidLimits.raise());
        }
        let mut repo = crate::open_opts(
            self.current_dir().join(self.git_dir()),
            crate::open::Options::isolated().with(self.git_dir_trust()).config_overrides([
                format!("gitoxide.objects.allocLimit={}", limits.object_bytes),
            ]),
        ).map_err(|err| source("open bounded object reader", err))?;
        repo.objects.ignore_replacements = true;
        Ok(Platform { repo, limits })
    }
}
impl Platform {
    /// Read an object, checking its hash kind, header size, decoded size and OID.
    /// No output is written on failure. One object is buffered at a time.
    pub fn read_object(&self, id: ObjectId, cancel: &AtomicBool) -> Result<crate::Object<'_>, Exn<Error>> {
        check_interrupt(cancel)?;
        if id.kind() != self.repo.object_hash() { return Err(Error::HashKind { id }.raise()); }
        let header = self.repo.find_header(id).map_err(|err| source("read object header", err))?;
        if header.size() > self.limits.object_bytes as u64 { return Err(Error::Limit("object bytes").raise()); }
        let object = self.repo.find_object(id).map_err(|err| source("decode object", err))?;
        check_interrupt(cancel)?;
        if object.data.len() as u64 != header.size() || object.kind != header.kind() {
            return Err(Error::Integrity { id }.raise());
        }
        let actual_id = gix_object::compute_hash(id.kind(), object.kind, &object.data)
            .map_err(|err| source("hash object", err))?;
        if actual_id != id { return Err(Error::Integrity { id }.raise()); }
        Ok(object)
    }

    /// Enumerate fully decoded, verified trees in Git depth-first order.
    ///
    /// Every tree visited is consumed completely before its entries are used.
    /// Rejects duplicate/unsorted names, illegal modes/components, null IDs and
    /// malformed trailing data. Blob headers are checked; use read_object or
    /// write_blob to verify their content. Gitlinks are metadata, never traversed.
    pub fn list_tree(
        &self, tree_id: ObjectId, options: &TreeOptions, cancel: &AtomicBool,
    ) -> Result<Vec<Entry>, Exn<Error>> {
        if let Format::Custom(format) = &options.format {
            write_custom(format, None, &mut std::io::sink(), cancel)?;
        }
        let mut pending = vec![(tree_id, BString::default(), EntryMode::from(gix_object::tree::EntryKind::Tree), 0usize)];
        let mut records = Vec::new();
        let mut count = 0usize;
        while let Some((id, path, mode, depth)) = pending.pop() {
            check_interrupt(cancel)?;
            if mode.is_tree() {
                if depth > self.limits.depth { return Err(Error::Limit("tree depth").raise()); }
                let object = self.read_object(id, cancel)?;
                if object.kind != Kind::Tree { return Err(Error::Kind { id, expected: Kind::Tree, actual: object.kind }.raise()); }
                let mut tree_entries = Vec::new();
                let mut names = BTreeSet::new();
                let mut previous = None;
                let mut iter = gix_object::TreeRefIter::from_bytes(&object.data, id.kind());
                loop {
                    let offset = iter.offset_to_next_entry(&object.data);
                    let Some(parsed) = iter.next() else { break };
                    let entry = parsed.map_err(|err| source("decode complete tree", err))?;
                    let encoded_mode = object.data[offset..].split(|b| *b == b' ').next().unwrap_or_default();
                    if !matches!(encoded_mode, b"40000" | b"040000" | b"100644" | b"100755" | b"120000" | b"160000") {
                        return Err(Error::Tree { id, reason: "noncanonical mode encoding" }.raise());
                    }
                    check_interrupt(cancel)?;
                    count = count.checked_add(1).ok_or_else(|| Error::Limit("entry count").raise())?;
                    if count > self.limits.entries { return Err(Error::Limit("entry count").raise()); }
                    if !matches!(entry.mode.value(), 0o40000 | 0o100644 | 0o100755 | 0o120000 | 0o160000)
                        || entry.filename.is_empty() || entry.filename.contains(&b'/')
                        || entry.filename == b".".as_bstr() || entry.filename == b"..".as_bstr()
                        || entry.oid.is_null()
                    { return Err(Error::Tree { id, reason: "invalid component, mode or null ID" }.raise()); }
                    if !names.insert(entry.filename) || previous.is_some_and(|prev| prev >= entry) {
                        return Err(Error::Tree { id, reason: "duplicate or unsorted entry" }.raise());
                    }
                    previous = Some(entry);
                    tree_entries.push(entry);
                }
                if !path.is_empty() && (options.show_trees || options.trees_only || !options.recursive) {
                    records.push(Entry { path: path.clone(), mode, id, size: None });
                }
                if depth > 0 && !options.recursive { continue; }
                for entry in tree_entries.iter().rev() {
                    let extra = usize::from(!path.is_empty());
                    let length = path.len().checked_add(extra).and_then(|n| n.checked_add(entry.filename.len()))
                        .ok_or_else(|| Error::Limit("path bytes").raise())?;
                    if length > self.limits.path_bytes { return Err(Error::Limit("path bytes").raise()); }
                    let mut child = path.clone();
                    if extra != 0 { child.push(b'/'); }
                    child.extend_from_slice(entry.filename);
                    pending.push((entry.oid.to_owned(), child, entry.mode, depth + 1));
                }
            } else if !options.trees_only || mode.is_commit() {
                let size = if mode.is_blob_or_symlink() {
                    let header = self.repo.find_header(id).map_err(|err| source("read tree entry header", err))?;
                    if header.kind() != Kind::Blob { return Err(Error::Kind { id, expected: Kind::Blob, actual: header.kind() }.raise()); }
                    Some(header.size())
                } else { None };
                records.push(Entry { path, mode, id, size });
            }
        }
        Ok(records)
    }

    /// Write a validated listing. Invalid options/trees produce no output;
    /// writer failure or cancellation during output may leave a partial stream.
    pub fn write_tree(
        &self, tree_id: ObjectId, options: &TreeOptions, out: &mut dyn Write, cancel: &AtomicBool,
    ) -> Result<(), Exn<Error>> {
        let records = self.list_tree(tree_id, options, cancel)?;
        // Git dispatches these exact custom strings to its built-in formatters.
        // Their -z paths are raw; generic custom %(path) fields are quoted.
        let format = match &options.format {
            Format::Custom(format) => match format.as_slice() {
                b"%(objectmode) %(objecttype) %(objectname)%x09%(path)" => &Format::Default,
                b"%(objectmode) %(objecttype) %(objectname) %(objectsize:padded)%x09%(path)" => &Format::Long,
                b"%(path)" => &Format::NameOnly,
                b"%(objectname)" => &Format::ObjectOnly,
                _ => &options.format,
            },
            format => format,
        };
        for entry in records {
            check_interrupt(cancel)?;
            if let Format::Custom(format) = format {
                write_custom(format, Some(&entry), out, cancel)?;
            } else {
                let kind = entry_kind(&entry);
                let prefix = match format {
                    Format::Default | Format::Quoted => format!("{:06o} {kind} {}\t", entry.mode.value(), entry.id),
                    Format::Long => {
                        let size = entry.size.map_or_else(|| "-".to_owned(), |n| n.to_string());
                        format!("{:06o} {kind} {} {size:>7}\t", entry.mode.value(), entry.id)
                    }
                    Format::NameOnly => String::new(),
                    Format::ObjectOnly => entry.id.to_string(),
                    Format::Custom(_) => unreachable!("custom formats are handled above"),
                };
                write_listing_bytes(prefix.as_bytes(), out, cancel)?;
                if matches!(format, Format::Quoted) {
                    write_listing_bytes(gix_quote::ansi_c::quote(entry.path.as_bstr()).as_ref(), out, cancel)?;
                } else if !matches!(format, Format::ObjectOnly) {
                    write_listing_bytes(&entry.path, out, cancel)?;
                }
            }
            let terminator = if matches!(format, Format::Quoted) { b"\n" } else { b"\0" };
            write_listing_bytes(terminator, out, cancel)?;
        }
        check_interrupt(cancel)
    }

    /// Write verified raw blob bytes in 64 KiB chunks. No filters are run.
    ///
    /// expected_size binds a manifest lock's size before writing. Cancellation
    /// or writer failure can leave a partial caller-owned stream; export_blob
    /// provides publication without partial destination files.
    pub fn write_blob(
        &self, blob_id: ObjectId, expected_size: Option<u64>, out: &mut dyn Write, cancel: &AtomicBool,
    ) -> Result<u64, Exn<Error>> {
        let object = self.read_object(blob_id, cancel)?;
        if object.kind != Kind::Blob { return Err(Error::Kind { id: blob_id, expected: Kind::Blob, actual: object.kind }.raise()); }
        let size = object.data.len() as u64;
        if expected_size.is_some_and(|expected| expected != size) { return Err(Error::Integrity { id: blob_id }.raise()); }
        for chunk in object.data.chunks(64 * 1024) {
            check_interrupt(cancel)?;
            out.write_all(chunk).map_err(|err| source("write blob", err))?;
        }
        check_interrupt(cancel)?;
        Ok(size)
    }

    /// Publish one raw verified blob as a new file, never replacing an existing
    /// file, directory or symlink. The existing parent must be stable and trusted.
    /// A temporary sibling is synced before atomic no-clobber publication.
    /// This promises atomic visibility, not parent-directory crash durability.
    pub fn export_blob(
        &self, blob_id: ObjectId, expected_size: Option<u64>, destination: &Path, cancel: &AtomicBool,
    ) -> Result<u64, Exn<Error>> {
        check_interrupt(cancel)?;
        let parent = destination.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        let mut temporary = gix_tempfile::new(parent, gix_tempfile::ContainingDirectory::Exists, gix_tempfile::AutoRemove::Tempfile)
            .map_err(|err| source("create blob staging file", err))?;
        let size = self.write_blob(blob_id, expected_size, &mut temporary, cancel)?;
        temporary.with_mut(|file| file.as_file().sync_all())
            .map_err(|err| source("access staging file", err))?
            .map_err(|err| source("sync staging file", err))?;
        check_interrupt(cancel)?;
        let file = temporary.take().ok_or_else(|| Error::Interrupted.raise())?;
        file.persist_noclobber(destination).map_err(|err| source("publish blob without replacement", err.error))?;
        Ok(size)
    }
}
fn entry_kind(entry: &Entry) -> &'static str {
    if entry.mode.is_tree() { "tree" } else if entry.mode.is_commit() { "commit" } else { "blob" }
}

fn write_listing_bytes(bytes: &[u8], out: &mut dyn Write, cancel: &AtomicBool) -> Result<(), Exn<Error>> {
    for chunk in bytes.chunks(64 * 1024) {
        check_interrupt(cancel)?;
        out.write_all(chunk).map_err(|err| source("write listing", err))?;
    }
    check_interrupt(cancel)
}

// None validates the entire format without emitting data. Parse directly from
// the caller's bytes to avoid allocating a token list or an expanded record.
fn write_custom(
    mut format: &[u8], entry: Option<&Entry>, out: &mut dyn Write, cancel: &AtomicBool,
) -> Result<(), Exn<Error>> {
    check_interrupt(cancel)?;
    while !format.is_empty() {
        check_interrupt(cancel)?;
        let literal_len = format.find_byte(b'%').unwrap_or(format.len());
        if entry.is_some() {
            write_listing_bytes(&format[..literal_len], out, cancel)?;
        }
        format = &format[literal_len..];
        if format.is_empty() { break; }
        format = &format[1..];
        match format.first() {
            Some(b'%') => {
                if entry.is_some() { write_listing_bytes(b"%", out, cancel)?; }
                format = &format[1..];
            }
            Some(b'x') => {
                let hex = format.get(1..3).ok_or_else(|| Error::Unsupported.raise())?;
                let digit = |byte: u8| match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                };
                let byte = digit(hex[0]).zip(digit(hex[1]))
                    .map(|(high, low)| high * 16 + low)
                    .ok_or_else(|| Error::Unsupported.raise())?;
                if entry.is_some() { write_listing_bytes(&[byte], out, cancel)?; }
                format = &format[3..];
            }
            Some(b'(') => {
                let end = format.find_byte(b')').ok_or_else(|| Error::Unsupported.raise())?;
                let field = &format[1..end];
                if !matches!(field, b"objectmode" | b"objecttype" | b"objectname" |
                    b"objectsize" | b"objectsize:padded" | b"path") {
                    return Err(Error::Unsupported.raise());
                }
                if let Some(entry) = entry {
                    if field == b"path" {
                        write_listing_bytes(gix_quote::ansi_c::quote(entry.path.as_bstr()).as_ref(), out, cancel)?;
                    } else {
                        let value = match field {
                            b"objectmode" => format!("{:06o}", entry.mode.value()),
                            b"objecttype" => entry_kind(entry).to_owned(),
                            b"objectname" => entry.id.to_string(),
                            b"objectsize" | b"objectsize:padded" => {
                                let size = entry.size.map_or_else(|| "-".to_owned(), |n| n.to_string());
                                if field == b"objectsize:padded" { format!("{size:>7}") } else { size }
                            }
                            _ => unreachable!("field was validated above"),
                        };
                        write_listing_bytes(value.as_bytes(), out, cancel)?;
                    }
                }
                format = &format[end + 1..];
            }
            _ => return Err(Error::Unsupported.raise()),
        }
    }
    check_interrupt(cancel)
}
