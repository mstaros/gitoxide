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
//! 3. Paths and other git data cross as BYTES, not UTF-8 strings. Git
//!    stores paths, ref names, author names and messages as `BString`.
//!    Taking `ffi::String` would validate as UTF-8 and reject repositories
//!    that git itself handles, and widening the type later would break
//!    every consumer.
//! 4. One error type for now - see `GixError`.

use interoptopus::ffi;
use interoptopus::inventory::RustInventory;
use interoptopus::{builtins_string, builtins_vec, guard, service};

/// The single error type crossing the boundary.
///
/// Deliberately coarse for now. `gix` error enums are mostly *struct*
/// variants (`LockCommit { source, full_name }`), and interoptopus payload
/// enums support single-field tuple variants only - struct variants and
/// multi-field tuple variants are explicitly unsupported. Rather than
/// introduce a companion `#[ffi]` struct per fielded variant across
/// hundreds of `gix` variants, the whole `Display`/`source` chain is
/// flattened into one message string here.
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
            Err(err) => {
                return ffi::Err(GixError::InvalidPath(chain_to_string(&err)));
            }
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
}

/// The exported surface.
///
/// `guard!` emits an API hash checked by the generated C# at load time;
/// it is the defence against bindings drifting from the DLL, which is
/// otherwise silent and undetectable at the ABI level.
///
/// `builtins_vec!(u8)` is required, not optional. Without it the backend
/// still emits *references* to `VecByte` for `ffi::Vec<u8>` returns but
/// never emits the type itself. The Rust side compiles either way, so
/// only the C# compiler catches the omission. Note it takes the element
/// type, unlike `builtins_string!()`.
pub fn ffi_inventory() -> RustInventory {
    RustInventory::new()
        .register(guard!(ffi_inventory))
        .register(builtins_string!())
        .register(builtins_vec!(u8))
        .register(service!(Repo))
        .validate()
}
