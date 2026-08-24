//! FFI facade over `gix`, exposed to C# via interoptopus.
//!
//! Design rules for everything added here:
//!
//! 1. No borrowed data crosses the boundary. `gix`'s handle types
//!    (`Commit<'repo>`, `Tree<'repo>`, ...) are lifetime-bound to a
//!    `Repository` and cannot be represented in the ABI. Every exposed
//!    handle owns what it needs.
//! 2. `ThreadSafeRepository` is the stored form. `gix::Repository` holds
//!    `Option<RefCell<Vec<Vec<u8>>>>` and is therefore never `Sync`;
//!    `to_thread_local()` is called per operation.
//! 3. Paths, ref names and message text cross as BYTES, not UTF-8 strings.
//!    Git stores them as `BString`. Taking `ffi::String` would validate as
//!    UTF-8 and reject repositories that git itself handles, and widening
//!    the type later would break every consumer.
//! 4. Object ids cross as lowercase HEX strings. They are ASCII by
//!    construction, so UTF-8 validation is free, and they are readable in a
//!    debugger. The 2x size over raw bytes is irrelevant next to the cost of
//!    an FFI crossing.
//! 5. Small read-once things are records; only types with many operations
//!    or lazy sub-access become services.
//! 6. One error type for now - see `GixError`.

use interoptopus::ffi;
use interoptopus::inventory::RustInventory;
use interoptopus::{builtins_string, builtins_vec, guard, service};

/// The single error type crossing the boundary.
///
/// Deliberately coarse for now. `gix` error enums are mostly *struct*
/// variants (`LockCommit { source, full_name }`), and interoptopus payload
/// enums support single-field tuple variants only. Rather than introduce a
/// companion `#[ffi]` struct per fielded variant across hundreds of `gix`
/// variants, the whole `Display`/`source` chain is flattened into one
/// message string here.
///
/// Splitting this into per-domain error types is a breaking change for
/// anyone matching on it, so it should happen once there are enough
/// operations to design a real taxonomy against - not from one call site.
#[ffi]
#[derive(Debug, Clone)]
pub enum GixError {
    /// Path exists but is not a repository, or could not be discovered.
    NotARepository(ffi::String),
    /// Filesystem or OS-level failure.
    Io(ffi::String),
    /// Repository configuration could not be read or is invalid.
    Config(ffi::String),
    /// A path supplied by the caller could not be represented on this
    /// platform. Only reachable on Windows, where paths must round-trip
    /// through UTF-16.
    InvalidPath(ffi::String),
    /// An object id supplied by the caller was not valid hex.
    InvalidId(ffi::String),
    /// A reference or object does not exist.
    NotFound(ffi::String),
    /// Anything not yet categorised.
    Other(ffi::String),
}

/// Flatten a `std::error::Error` chain into one human-readable string.
///
/// `gix` errors nest via `#[source]`, and that context is where the
/// actionable detail usually lives, so it must not be dropped.
fn chain_to_string(err: &dyn std::error::Error) -> ffi::String {
    let mut out = err.to_string();
    let mut cursor = err.source();
    while let Some(inner) = cursor {
        out.push_str(": ");
        out.push_str(&inner.to_string());
        cursor = inner.source();
    }
    ffi::String::from(out)
}

/// Convenience for the common `Other` case where there is no better variant.
fn other(err: &dyn std::error::Error) -> GixError {
    GixError::Other(chain_to_string(err))
}

impl From<gix::open::Error> for GixError {
    fn from(err: gix::open::Error) -> Self {
        let msg = chain_to_string(&err);
        match err {
            gix::open::Error::NotARepository { .. } => Self::NotARepository(msg),
            gix::open::Error::Config(_) => Self::Config(msg),
            gix::open::Error::Io(_) => Self::Io(msg),
            _ => Self::Other(msg),
        }
    }
}

/// Format an object id as lowercase hex.
fn hex(id: &gix::hash::oid) -> ffi::String {
    ffi::String::from(id.to_string())
}

/// Parse a caller-supplied hex object id.
fn parse_id(value: &ffi::String) -> Result<gix::ObjectId, GixError> {
    gix::ObjectId::from_hex(value.as_str().as_bytes())
        .map_err(|err| GixError::InvalidId(chain_to_string(&err)))
}

/// Where `HEAD` points, as a snapshot.
///
/// A record rather than a service: `Head` is small and read once, so a
/// handle would mean three FFI crossings to learn what one record carries.
#[ffi]
#[derive(Debug, Clone)]
pub struct HeadInfo {
    /// Hex object id `HEAD` resolves to. Empty when unborn.
    pub target: ffi::String,
    /// Full ref name, e.g. `refs/heads/main`. Empty when detached.
    /// Bytes, because ref names are `BString`.
    pub referent: ffi::Vec<u8>,
    /// `HEAD` points straight at an object rather than a branch.
    pub is_detached: bool,
    /// `HEAD` names a branch that has no commits yet.
    pub is_unborn: bool,
}

/// A commit, as owned data.
#[ffi]
#[derive(Debug, Clone)]
pub struct CommitInfo {
    /// Hex object id of this commit.
    pub id: ffi::String,
    /// Author name, raw bytes.
    pub author_name: ffi::Vec<u8>,
    /// Author email, raw bytes.
    pub author_email: ffi::Vec<u8>,
    /// Author time, seconds since the unix epoch.
    pub time_seconds: i64,
    /// Author timezone offset in seconds east of UTC.
    pub time_offset_seconds: i32,
    /// Full commit message, raw bytes.
    pub message: ffi::Vec<u8>,
}

/// An open repository.
///
/// Stores the `Sync`-capable form; each method derives a thread-local
/// `Repository` for the duration of the call.
#[ffi(service)]
pub struct Repo {
    inner: gix::ThreadSafeRepository,
}

#[ffi]
impl Repo {
    /// Open an existing repository at `path`.
    ///
    /// `path` is raw bytes in the platform's native encoding, not UTF-8.
    /// On Windows it must still round-trip through UTF-16, hence the
    /// fallible conversion.
    pub fn open(path: ffi::Slice<u8>) -> ffi::Result<Self, GixError> {
        let path = match gix::path::try_from_byte_slice(path.as_slice()) {
            Ok(path) => path,
            Err(err) => return ffi::Err(GixError::InvalidPath(chain_to_string(&err))),
        };
        match gix::ThreadSafeRepository::open(path) {
            Ok(inner) => ffi::Ok(Self { inner }),
            Err(err) => ffi::Err(err.into()),
        }
    }

    /// Absolute path of the `.git` directory, as raw platform bytes.
    pub fn git_dir(&self) -> ffi::Result<ffi::Vec<u8>, GixError> {
        let repo = self.inner.to_thread_local();
        let bytes = gix::path::into_bstr(repo.git_dir().to_owned()).into_owned();
        ffi::Ok(ffi::Vec::from(Vec::from(bytes)))
    }

    /// Whether this repository has no working tree.
    pub fn is_bare(&self) -> bool {
        self.inner.to_thread_local().is_bare()
    }

    /// Where `HEAD` currently points.
    pub fn head(&self) -> ffi::Result<HeadInfo, GixError> {
        let repo = self.inner.to_thread_local();
        let mut head = match repo.head() {
            Ok(head) => head,
            Err(err) => return ffi::Err(GixError::NotFound(chain_to_string(&err))),
        };

        let is_detached = head.is_detached();
        let is_unborn = head.is_unborn();

        let referent = head
            .referent_name()
            .map(|name| name.as_bstr().to_vec())
            .unwrap_or_default();

        let target = match head.try_peel_to_id() {
            Ok(Some(id)) => hex(id.as_ref()),
            Ok(None) => ffi::String::from(String::new()),
            Err(err) => return ffi::Err(other(&err)),
        };

        ffi::Ok(HeadInfo {
            target,
            referent: ffi::Vec::from(referent),
            is_detached,
            is_unborn,
        })
    }

    /// Walk commit ancestry from `tip`, newest first.
    ///
    /// Returns hex ids only. Ids are small and fixed-size, so materialising
    /// them is bounded and cheap, while the expensive part - loading commit
    /// bodies - stays lazy via `commit_info`.
    ///
    /// `max_count` of 0 means unlimited.
    pub fn rev_walk(
        &self,
        tip: ffi::String,
        max_count: u64,
    ) -> ffi::Result<ffi::Vec<ffi::String>, GixError> {
        let repo = self.inner.to_thread_local();
        let tip = match parse_id(&tip) {
            Ok(id) => id,
            Err(err) => return ffi::Err(err),
        };

        let walk = match repo.rev_walk(Some(tip)).all() {
            Ok(walk) => walk,
            Err(err) => return ffi::Err(other(&err)),
        };

        let mut ids: Vec<ffi::String> = Vec::new();
        for item in walk {
            match item {
                Ok(info) => ids.push(hex(info.id.as_ref())),
                Err(err) => return ffi::Err(other(&err)),
            }
            if max_count != 0 && ids.len() as u64 >= max_count {
                break;
            }
        }

        ffi::Ok(ffi::Vec::from(ids))
    }

    /// Load one commit by hex id.
    pub fn commit_info(&self, id: ffi::String) -> ffi::Result<CommitInfo, GixError> {
        let repo = self.inner.to_thread_local();
        let oid = match parse_id(&id) {
            Ok(oid) => oid,
            Err(err) => return ffi::Err(err),
        };

        let commit = match repo.find_commit(oid) {
            Ok(commit) => commit,
            Err(err) => return ffi::Err(GixError::NotFound(chain_to_string(&err))),
        };

        let author = match commit.author() {
            Ok(author) => author,
            Err(err) => return ffi::Err(other(&err)),
        };

        // `SignatureRef::time` is a raw &str: gix keeps it undecoded so the
        // header round-trips losslessly, since parsing can be lossy.
        let time = match author.time() {
            Ok(time) => time,
            Err(err) => return ffi::Err(other(&err)),
        };

        let message = match commit.message_raw() {
            Ok(message) => message.to_vec(),
            Err(err) => return ffi::Err(other(&err)),
        };

        ffi::Ok(CommitInfo {
            id: hex(oid.as_ref()),
            author_name: ffi::Vec::from(author.name.to_vec()),
            author_email: ffi::Vec::from(author.email.to_vec()),
            time_seconds: time.seconds,
            time_offset_seconds: time.offset,
            message: ffi::Vec::from(message),
        })
    }
}

/// The exported surface.
///
/// `guard!` emits an API hash checked by the generated C# at load time;
/// it is the defence against bindings drifting from the DLL, which is
/// otherwise silent and undetectable at the ABI level.
///
/// EVERY `ffi::Vec<T>` used anywhere in the surface needs its own
/// `builtins_vec!(T)` here. Without it the backend still emits *references*
/// to the C# type (`VecByte`, `VecUtf8String`, ...) but never emits the type
/// itself. The Rust side compiles cleanly either way, so the omission
/// surfaces only as a C# compile error in generated code - once for
/// `ffi::Vec<u8>` and again for `ffi::Vec<ffi::String>`. Note the macro takes
/// the element type, unlike `builtins_string!()`.
pub fn ffi_inventory() -> RustInventory {
    RustInventory::new()
        .register(guard!(ffi_inventory))
        .register(builtins_string!())
        .register(builtins_vec!(u8))
        .register(builtins_vec!(ffi::String))
        .register(service!(Repo))
        .validate()
}
