//! P0a experiment: pull bytes into a caller-owned reusable buffer.
//!
//! This is deliberately gix-specific. It validates the boundary shape without
//! introducing a generic Interoptopus stream abstraction.

use std::io::{Cursor, Read};

use interoptopus::ffi;

use crate::{GixError, Repo, chain_to_string, other, parse_id_for_repo};

enum Source {
    /// Object lookup materialises the complete decompressed object in gix.
    Materialized(Cursor<Vec<u8>>),
    /// The transient worktree representation is produced through gix's pipe-backed Read.
    Worktree(gix::worktree::stream::Stream),
}

/// Stateful native byte reader used by the P0a boundary experiment.
///
/// The caller owns every destination buffer. No pointer from a call is retained,
/// and the source is dropped when the generated service handle is disposed.
#[ffi(service)]
pub struct ByteReader {
    source: Source,
    known_length: Option<u64>,
    retained_native_bytes: u64,
    bytes_delivered: u64,
    read_calls: u64,
    eof_observed: bool,
}

#[ffi]
impl ByteReader {
    /// Materialise one object through gix, then expose it through pull reads.
    pub fn from_object(repo: &Repo, id: ffi::String) -> ffi::Result<Self, GixError> {
        let repo = repo.inner.to_thread_local();
        let id = match parse_id_for_repo(&repo, &id) {
            Ok(id) => id,
            Err(err) => return ffi::Err(err),
        };
        let object = match repo.find_object(id) {
            Ok(object) => object,
            Err(err) => return ffi::Err(GixError::NotFound(chain_to_string(&err))),
        };
        let data = object.detach().data;
        let known_length = data.len() as u64;
        let retained_native_bytes = data.capacity() as u64;
        ffi::Ok(Self {
            source: Source::Materialized(Cursor::new(data)),
            known_length: Some(known_length),
            retained_native_bytes,
            bytes_delivered: 0,
            read_calls: 0,
            eof_observed: false,
        })
    }

    /// Open gix's pipe-backed transient worktree stream for a tree object.
    ///
    /// Unlike object lookup, this source implements Read and is not collected
    /// into one complete native payload before the first boundary read.
    pub fn from_worktree(repo: &Repo, tree_id: ffi::String) -> ffi::Result<Self, GixError> {
        let repo = repo.inner.to_thread_local();
        let tree_id = match parse_id_for_repo(&repo, &tree_id) {
            Ok(id) => id,
            Err(err) => return ffi::Err(err),
        };
        let (stream, _index) = match repo.worktree_stream(tree_id) {
            Ok(value) => value,
            Err(err) => return ffi::Err(other(&err)),
        };
        ffi::Ok(Self {
            source: Source::Worktree(stream),
            known_length: None,
            retained_native_bytes: 0,
            bytes_delivered: 0,
            read_calls: 0,
            eof_observed: false,
        })
    }

    /// Fill as much of destination as is immediately available.
    ///
    /// A zero result for a non-empty destination means EOF. A zero-length
    /// destination is a no-op and does not discover EOF, matching Stream.Read.
    pub fn read(&mut self, mut destination: ffi::SliceMut<u8>) -> ffi::Result<u64, GixError> {
        self.read_calls += 1;
        let destination = destination.as_slice_mut();
        if destination.is_empty() {
            return ffi::Ok(0);
        }
        if self.eof_observed {
            return ffi::Ok(0);
        }

        let result = match &mut self.source {
            Source::Materialized(source) => source.read(destination),
            Source::Worktree(source) => source.as_read_mut().read(destination),
        };
        match result {
            Ok(0) => {
                self.eof_observed = true;
                ffi::Ok(0)
            }
            Ok(read) => {
                self.bytes_delivered += read as u64;
                ffi::Ok(read as u64)
            }
            Err(err) => ffi::Err(other(&err)),
        }
    }

    /// True once a non-empty read has returned zero.
    pub fn eof_observed(&self) -> bool {
        self.eof_observed
    }

    /// Whether length was cheap and known when the reader was opened.
    pub fn has_known_length(&self) -> bool {
        self.known_length.is_some()
    }

    /// Known source length, or zero when the streaming source has no cheap length.
    pub fn known_length(&self) -> u64 {
        self.known_length.unwrap_or(0)
    }

    /// Native bytes retained solely for the materialised object payload.
    ///
    /// Zero for the streaming source; gix's internal pipe/scratch buffers are
    /// intentionally not guessed here and are measured at process level.
    pub fn retained_native_bytes(&self) -> u64 {
        self.retained_native_bytes
    }

    /// Bytes copied into caller-owned buffers so far.
    pub fn bytes_delivered(&self) -> u64 {
        self.bytes_delivered
    }

    /// Number of boundary read calls, including zero-length and repeated EOF reads.
    pub fn read_calls(&self) -> u64 {
        self.read_calls
    }
}