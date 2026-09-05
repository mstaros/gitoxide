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

mod byte_stream;
mod index;
mod ignore;
mod diff;
mod references;
mod notes;
mod status;
mod tags;
pub use byte_stream::ByteReader;
pub use index::IndexEntryRecord;
pub use diff::{DiffRecord, TreeChangeRecord};
pub use references::{
    BranchRecord, OptionalObjectId, ReferenceLockLease, ReferenceRecord, ReferenceUpdateOutcome,
};
pub use status::StatusRecord;
pub use notes::{NoteRecord, NoteEntryRecord};
pub use tags::{TagRecord, TagSignatureRecord};

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
    /// A reference name is not valid for the requested operation.
    InvalidReference(ffi::String),
    /// A reference edit lost a race with another writer.
    ReferenceConflict(ffi::String),
    /// A reference lock file could not be acquired.
    ///
    /// Distinct from [`GixError::ReferenceConflict`], which means an edit lost
    /// a race with a live writer. This can also mean a `.lock` was left behind
    /// by a process that died: `gix::lock::Marker` releases on `Drop` but not
    /// on crash, so there may be no other writer at all and no amount of
    /// retrying will clear it. See `Issues.md` `b66f8f9c`.
    ReferenceLocked(ffi::String),
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

impl From<gix::discover::Error> for GixError {
    fn from(err: gix::discover::Error) -> Self {
        let msg = chain_to_string(&err);
        match err {
            gix::discover::Error::Open(err) => err.into(),
            gix::discover::Error::Discover(err) => match err {
                gix::discover::upwards::Error::CurrentDir(_)
                | gix::discover::upwards::Error::CheckTrust { .. } => Self::Io(msg),
                gix::discover::upwards::Error::InvalidInput { .. } => Self::InvalidPath(msg),
                gix::discover::upwards::Error::InaccessibleDirectory { .. } => Self::Io(msg),
                gix::discover::upwards::Error::NoGitRepository { .. }
                | gix::discover::upwards::Error::NoGitRepositoryWithinCeiling { .. }
                | gix::discover::upwards::Error::NoGitRepositoryWithinFs { .. }
                | gix::discover::upwards::Error::NoMatchingCeilingDir
                | gix::discover::upwards::Error::NoTrustedGitRepository { .. } => {
                    Self::NotARepository(msg)
                }
            },
        }
    }
}

impl From<gix::init::Error> for GixError {
    fn from(err: gix::init::Error) -> Self {
        let msg = chain_to_string(&err);
        match err {
            gix::init::Error::CurrentDir(_) => Self::Io(msg),
            gix::init::Error::Open(err) => err.into(),
            gix::init::Error::Init(err) => match err {
                gix::create::Error::CurrentDir(_)
                | gix::create::Error::IoOpen { .. }
                | gix::create::Error::IoWrite { .. }
                | gix::create::Error::CreateDirectory { .. } => Self::Io(msg),
                gix::create::Error::Span(_) | gix::create::Error::ConfigValue(_) => {
                    Self::Config(msg)
                }
                gix::create::Error::DirectoryExists { .. }
                | gix::create::Error::DirectoryNotEmpty { .. } => Self::Other(msg),
            },
            gix::init::Error::InvalidBranchName { .. } => Self::Config(msg),
            gix::init::Error::EditHeadForDefaultBranch(_) => Self::Other(msg),
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

fn path_from_bytes(bytes: &[u8]) -> Result<std::path::PathBuf, GixError> {
    gix::path::try_from_byte_slice(bytes)
        .map(std::path::Path::to_owned)
        .map_err(|err| GixError::InvalidPath(chain_to_string(&err)))
}

fn path_bytes(path: &std::path::Path) -> ffi::Vec<u8> {
    let bytes = gix::path::into_bstr(path.to_owned()).into_owned();
    ffi::Vec::from(Vec::from(bytes))
}

fn discover_repository(
    start_path: &[u8],
    across_file_systems: bool,
    ceiling_directories: &[u8],
) -> Result<gix::ThreadSafeRepository, GixError> {
    let start_path = path_from_bytes(start_path)?;
    let mut options = gix::discover::upwards::Options {
        cross_fs: across_file_systems,
        ..Default::default()
    };

    if !ceiling_directories.is_empty() {
        let path_list = path_from_bytes(ceiling_directories)?;
        options.ceiling_dirs = std::env::split_paths(path_list.as_os_str()).collect();
        // libgit2 treats non-matching ceilings as an ordinary unsuccessful
        // search instead of an invalid-options error.
        options.match_ceiling_dir_or_error = false;
    }

    gix::ThreadSafeRepository::discover_opts(start_path, options, Default::default())
        .map_err(Into::into)
}

/// Object kind resolved from the object database.
#[ffi]
#[derive(Debug, Clone, Copy)]
pub enum FfiObjectType {
    Commit,
    Tree,
    Blob,
    Tag,
}

/// Lightweight object metadata.
#[ffi]
#[derive(Debug, Clone, Copy)]
pub struct ObjectMetadata {
    pub object_type: FfiObjectType,
    pub size: u64,
}

/// A complete commit snapshot with owned fields.
#[ffi]
#[derive(Debug, Clone)]
pub struct CommitRecord {
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
    pub parent_ids: ffi::Vec<ffi::String>,
}

fn message(value: impl Into<String>) -> ffi::String {
    ffi::String::from(value.into())
}

fn parse_id_for_repo(
    repo: &gix::Repository,
    value: &ffi::String,
) -> Result<gix::ObjectId, GixError> {
    let id = parse_id(value)?;
    if id.kind() != repo.object_hash() {
        return Err(GixError::InvalidId(message(format!(
            "object id uses {:?}, but this repository uses {:?}",
            id.kind(),
            repo.object_hash()
        ))));
    }
    Ok(id)
}

fn commit_from_revision<'repo>(
    repo: &'repo gix::Repository,
    revision: &ffi::String,
) -> Result<gix::Commit<'repo>, GixError> {
    let id = repo
        .rev_parse_single(revision.as_str())
        .map_err(|err| GixError::NotFound(chain_to_string(&err)))?;
    let object = id
        .object()
        .map_err(|err| GixError::NotFound(chain_to_string(&err)))?;
    object.peel_to_commit().map_err(|err| other(&err))
}

fn find_commit_checked<'repo>(
    repo: &'repo gix::Repository,
    id: gix::ObjectId,
) -> Result<gix::Commit<'repo>, GixError> {
    let header = repo
        .find_header(id)
        .map_err(|err| GixError::NotFound(chain_to_string(&err)))?;
    if header.kind() != gix::objs::Kind::Commit {
        return Err(GixError::Other(message(format!(
            "object {id} is {:?}, not a commit",
            header.kind()
        ))));
    }
    repo.find_commit(id).map_err(|err| other(&err))
}

fn require_kind(
    repo: &gix::Repository,
    id: gix::ObjectId,
    expected: gix::objs::Kind,
) -> Result<(), GixError> {
    let header = repo
        .find_header(id)
        .map_err(|err| GixError::NotFound(chain_to_string(&err)))?;
    if header.kind() != expected {
        return Err(GixError::Other(message(format!(
            "object {id} is {:?}, not {expected:?}",
            header.kind()
        ))));
    }
    Ok(())
}

fn commit_record(commit: gix::Commit<'_>) -> Result<CommitRecord, GixError> {
    let author = commit.author().map_err(|err| other(&err))?;
    let committer = commit.committer().map_err(|err| other(&err))?;
    let author_time = author.time().map_err(|err| other(&err))?;
    let committer_time = committer.time().map_err(|err| other(&err))?;
    let commit_message = commit.message_raw().map_err(|err| other(&err))?;
    let parent_ids = commit
        .parent_ids()
        .map(|id| hex(id.as_ref()))
        .collect::<Vec<_>>();

    Ok(CommitRecord {
        id: hex(commit.id.as_ref()),
        message: ffi::Vec::from(commit_message.to_vec()),
        author_name: ffi::Vec::from(author.name.to_vec()),
        author_email: ffi::Vec::from(author.email.to_vec()),
        author_time_seconds: author_time.seconds,
        author_time_offset_seconds: author_time.offset,
        committer_name: ffi::Vec::from(committer.name.to_vec()),
        committer_email: ffi::Vec::from(committer.email.to_vec()),
        committer_time_seconds: committer_time.seconds,
        committer_time_offset_seconds: committer_time.offset,
        parent_ids: ffi::Vec::from(parent_ids),
    })
}

fn explicit_signature(
    name: &[u8],
    email: &[u8],
    time_seconds: i64,
    time_offset_seconds: i32,
) -> Result<gix::actor::Signature, GixError> {
    if name.is_empty() || email.is_empty() {
        return Err(GixError::Other(message(
            "signature name and email must not be empty",
        )));
    }
    Ok(gix::actor::Signature {
        name: name.to_vec().into(),
        email: email.to_vec().into(),
        time: gix::date::Time::new(time_seconds, time_offset_seconds),
    })
}

fn configured_signature(
    repo: &gix::Repository,
    author: bool,
) -> Result<gix::actor::Signature, GixError> {
    let configured = if author {
        repo.author()
    } else {
        repo.committer()
    };
    let role = if author { "author" } else { "committer" };
    match configured {
        None => Err(GixError::Config(message(format!(
            "no {role} identity is configured"
        )))),
        Some(Err(err)) => Err(GixError::Config(chain_to_string(&err))),
        Some(Ok(signature)) => signature.to_owned().map_err(|err| other(&err)),
    }
}

fn tree_from_index(repo: &gix::Repository) -> Result<gix::ObjectId, GixError> {
    // Fresh from disk: a commit created immediately after `stage` must not build its tree
    // from a cached pre-stage index. See `index::owned_index`.
    let index = crate::index::owned_index(repo)?;
    let mut editor = repo
        .edit_tree(gix::ObjectId::empty_tree(repo.object_hash()))
        .map_err(|err| other(&err))?;

    for entry in index.entries() {
        if entry.stage() != gix::index::entry::Stage::Unconflicted {
            return Err(GixError::Other(message(
                "cannot create a commit while the index contains conflicts",
            )));
        }
        let mode = entry.mode.to_tree_entry_mode().ok_or_else(|| {
            GixError::Other(message("the index contains an unsupported entry mode"))
        })?;
        editor
            .upsert(entry.path(&index), mode.kind(), entry.id)
            .map_err(|err| other(&err))?;
    }

    editor.write().map(|id| id.detach()).map_err(|err| other(&err))
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

/// Stable repository location metadata, returned as owned data.
#[ffi]
#[derive(Debug, Clone)]
pub struct RepositoryInfo {
    /// Repository-private git directory.
    pub repository_path: ffi::Vec<u8>,
    /// Worktree directory, empty when the repository is bare.
    pub working_directory: ffi::Vec<u8>,
    /// Whether `working_directory` is present.
    pub has_working_directory: bool,
    /// Common git directory shared by linked worktrees.
    pub common_directory: ffi::Vec<u8>,
    /// Whether the repository has no worktree.
    pub is_bare: bool,
    /// Whether this handle belongs to a linked worktree.
    pub is_worktree: bool,
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
    /// Initialize a repository at `path`.
    pub fn create(path: ffi::Slice<u8>, bare: bool) -> ffi::Result<Self, GixError> {
        let path = match path_from_bytes(path.as_slice()) {
            Ok(path) => path,
            Err(err) => return ffi::Err(err),
        };
        let kind = if bare {
            gix::create::Kind::Bare
        } else {
            gix::create::Kind::WithWorktree
        };
        match gix::ThreadSafeRepository::init(path, kind, Default::default()) {
            Ok(inner) => ffi::Ok(Self { inner }),
            Err(err) => ffi::Err(err.into()),
        }
    }

    /// Open an existing repository at `path`.
    ///
    /// `path` is raw bytes in the platform's native encoding, not UTF-8.
    /// On Windows it must still round-trip through UTF-16, hence the
    /// fallible conversion.
    pub fn open(path: ffi::Slice<u8>) -> ffi::Result<Self, GixError> {
        let path = match path_from_bytes(path.as_slice()) {
            Ok(path) => path,
            Err(err) => return ffi::Err(err),
        };
        match gix::ThreadSafeRepository::open(path) {
            Ok(inner) => ffi::Ok(Self { inner }),
            Err(err) => ffi::Err(err.into()),
        }
    }

    /// Discover and open a repository upwards from `start_path`.
    ///
    /// `ceiling_directories` is a platform path-list string. An empty slice
    /// means no ceiling. If `across_file_systems` is false, discovery stops
    /// at a filesystem boundary on platforms that expose device ids.
    pub fn discover(
        start_path: ffi::Slice<u8>,
        across_file_systems: bool,
        ceiling_directories: ffi::Slice<u8>,
    ) -> ffi::Result<Self, GixError> {
        match discover_repository(
            start_path.as_slice(),
            across_file_systems,
            ceiling_directories.as_slice(),
        ) {
            Ok(inner) => ffi::Ok(Self { inner }),
            Err(err) => ffi::Err(err),
        }
    }

    /// Stable repository location metadata.
    pub fn info(&self) -> RepositoryInfo {
        let repo = self.inner.to_thread_local();
        let repository_path = repo.git_dir();
        let common_directory = repo.common_dir();
        let working_directory = repo.workdir();

        RepositoryInfo {
            repository_path: path_bytes(repository_path),
            working_directory: working_directory
                .map(path_bytes)
                .unwrap_or_else(|| ffi::Vec::from(Vec::new())),
            has_working_directory: working_directory.is_some(),
            common_directory: path_bytes(common_directory),
            is_bare: repo.is_bare(),
            is_worktree: common_directory != repository_path,
        }
    }
    /// Absolute path of the `.git` directory, as raw platform bytes.
    pub fn git_dir(&self) -> ffi::Result<ffi::Vec<u8>, GixError> {
        let repo = self.inner.to_thread_local();
        let bytes = gix::path::into_bstr(repo.git_dir().to_owned()).into_owned();
        ffi::Ok(ffi::Vec::from(Vec::from(bytes)))
    }

    /// Match repository, local and global ignore rules against a relative Git path.
    pub fn is_path_ignored(
        &self,
        path: ffi::Slice<u8>,
        is_directory: bool,
    ) -> ffi::Result<bool, GixError> {
        let repo = self.inner.to_thread_local();
        match ignore::is_path_ignored(&repo, path.as_slice(), is_directory) {
            Ok(ignored) => ffi::Ok(ignored),
            Err(err) => ffi::Err(err),
        }
    }

    /// Add an exact, root-anchored rule to the common info/exclude file.
    pub fn ensure_local_exclude(
        &self,
        path: ffi::Slice<u8>,
        is_directory: bool,
    ) -> ffi::Result<(), GixError> {
        let repo = self.inner.to_thread_local();
        match ignore::ensure_local_exclude(&repo, path.as_slice(), is_directory) {
            Ok(()) => ffi::Ok(()),
            Err(err) => ffi::Err(err),
        }
    }

    /// Whether this repository has no working tree.
    pub fn is_bare(&self) -> bool {
        self.inner.to_thread_local().is_bare()
    }

    /// Whether this repository currently has a shallow history boundary.
    pub fn is_shallow(&self) -> bool {
        self.inner.to_thread_local().is_shallow()
    }

    /// Owned shallow-boundary object IDs in gix order, empty for complete history.
    pub fn shallow_commits(&self) -> ffi::Result<ffi::Vec<ffi::String>, GixError> {
        let repo = self.inner.to_thread_local();
        // Managed calls return fresh owned snapshots. An mtime-only cache can hide
        // a replacement or newly malformed file whose timestamp did not advance.
        let commits = match gix_shallow::read(&repo.shallow_file()) {
            Ok(commits) => commits,
            Err(err) => {
                let message = chain_to_string(&err);
                return ffi::Err(match err {
                    gix::shallow::read::Error::Io(_) => GixError::Io(message),
                    gix::shallow::read::Error::DecodeHash(_) => GixError::InvalidId(message),
                });
            }
        };
        let ids: Vec<ffi::String> = commits
            .map(|commits| commits.iter().map(|id| hex(id.as_ref())).collect())
            .unwrap_or_default();
        ffi::Ok(ffi::Vec::from(ids))
    }

    /// Configured shallow-file location as owned platform bytes; the file may not exist.
    pub fn shallow_file(&self) -> ffi::Vec<u8> {
        path_bytes(&self.inner.to_thread_local().shallow_file())
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

    /// Resolve a revision, peel annotated tags, and return a complete commit.
    pub fn lookup_commit(
        &self,
        revision: ffi::String,
    ) -> ffi::Result<CommitRecord, GixError> {
        let repo = self.inner.to_thread_local();
        match commit_from_revision(&repo, &revision).and_then(commit_record) {
            Ok(commit) => ffi::Ok(commit),
            Err(err) => ffi::Err(err),
        }
    }

    /// Return whether a valid object id exists in this repository.
    ///
    /// Malformed ids and ids for a different object format remain errors.
    pub fn has_object(&self, id: ffi::String) -> ffi::Result<bool, GixError> {
        let repo = self.inner.to_thread_local();
        match parse_id_for_repo(&repo, &id) {
            Ok(id) => ffi::Ok(repo.has_object(id)),
            Err(err) => ffi::Err(err),
        }
    }

    /// Write exact blob bytes and return their content-addressed object id.
    pub fn write_blob(&self, bytes: ffi::Slice<u8>) -> ffi::Result<ffi::String, GixError> {
        let repo = self.inner.to_thread_local();
        match repo.write_blob(bytes.as_slice()) {
            Ok(id) => ffi::Ok(hex(id.as_ref())),
            Err(err) => ffi::Err(other(&err)),
        }
    }

    /// Return object kind and uncompressed size without loading the body.
    pub fn object_metadata(
        &self,
        id: ffi::String,
    ) -> ffi::Result<ObjectMetadata, GixError> {
        let repo = self.inner.to_thread_local();
        let id = match parse_id_for_repo(&repo, &id) {
            Ok(id) => id,
            Err(err) => return ffi::Err(err),
        };
        let header = match repo.find_header(id) {
            Ok(header) => header,
            Err(err) => return ffi::Err(GixError::NotFound(chain_to_string(&err))),
        };
        let object_type = match header.kind() {
            gix::objs::Kind::Commit => FfiObjectType::Commit,
            gix::objs::Kind::Tree => FfiObjectType::Tree,
            gix::objs::Kind::Blob => FfiObjectType::Blob,
            gix::objs::Kind::Tag => FfiObjectType::Tag,
        };
        ffi::Ok(ObjectMetadata {
            object_type,
            size: header.size(),
        })
    }

    /// Walk commits with libgit2-compatible sort flags.
    ///
    /// Bit 0 is topological, bit 1 is commit time, and bit 2 reverses the
    /// complete result. An empty exclusion means no hidden revision.
    pub fn commit_history(
        &self,
        revision: ffi::String,
        excluded_revision: ffi::String,
        max_count: u64,
        sort_flags: u32,
    ) -> ffi::Result<ffi::Vec<ffi::String>, GixError> {
        const TOPOLOGICAL: u32 = 1;
        const TIME: u32 = 2;
        const REVERSE: u32 = 4;
        if sort_flags & !(TOPOLOGICAL | TIME | REVERSE) != 0 {
            return ffi::Err(GixError::Other(message(
                "unknown commit history sort flag",
            )));
        }
        if max_count == 0 {
            return ffi::Ok(ffi::Vec::from(Vec::new()));
        }

        let repo = self.inner.to_thread_local();
        let tip = match commit_from_revision(&repo, &revision) {
            Ok(commit) => commit.id,
            Err(err) => return ffi::Err(err),
        };
        let excluded = if excluded_revision.as_str().is_empty() {
            None
        } else {
            match commit_from_revision(&repo, &excluded_revision) {
                Ok(commit) => Some(commit.id),
                Err(err) => return ffi::Err(err),
            }
        };

        let mut ids = Vec::<gix::ObjectId>::new();
        if sort_flags & TOPOLOGICAL != 0 {
            let mut builder =
                gix::traverse::commit::topo::Builder::new(&repo.objects).with_tips([tip]);
            if let Some(excluded) = excluded {
                builder = builder.with_ends([excluded]);
            }
            let sorting = if sort_flags & TIME != 0 {
                gix::traverse::commit::topo::Sorting::DateOrder
            } else {
                gix::traverse::commit::topo::Sorting::TopoOrder
            };
            let walk = match builder.sorting(sorting).build() {
                Ok(walk) => walk,
                Err(err) => return ffi::Err(other(&err)),
            };
            for item in walk {
                match item {
                    Ok(info) => ids.push(info.id),
                    Err(err) => return ffi::Err(other(&err)),
                }
            }
        } else {
            let sorting = if sort_flags & TIME != 0 {
                gix::revision::walk::Sorting::ByCommitTime(Default::default())
            } else {
                gix::revision::walk::Sorting::BreadthFirst
            };
            let mut platform = repo.rev_walk(Some(tip)).sorting(sorting);
            if let Some(excluded) = excluded {
                platform = platform.with_hidden([excluded]);
            }
            let walk = match platform.all() {
                Ok(walk) => walk,
                Err(err) => return ffi::Err(other(&err)),
            };
            for item in walk {
                match item {
                    Ok(info) => ids.push(info.id),
                    Err(err) => return ffi::Err(other(&err)),
                }
            }
        }

        if sort_flags & REVERSE != 0 {
            ids.reverse();
        }
        let limit = usize::try_from(max_count).unwrap_or(usize::MAX);
        ids.truncate(limit);
        ffi::Ok(ffi::Vec::from(
            ids.into_iter().map(|id| hex(id.as_ref())).collect::<Vec<_>>(),
        ))
    }

    /// Return the tree id referenced by a commit revision.
    pub fn commit_tree_id(
        &self,
        revision: ffi::String,
    ) -> ffi::Result<ffi::String, GixError> {
        let repo = self.inner.to_thread_local();
        let commit = match commit_from_revision(&repo, &revision) {
            Ok(commit) => commit,
            Err(err) => return ffi::Err(err),
        };
        match commit.tree_id() {
            Ok(id) => ffi::Ok(hex(id.as_ref())),
            Err(err) => ffi::Err(other(&err)),
        }
    }

    /// Create a commit with explicit tree and ordered parents.
    ///
    /// Empty update-reference bytes write only the object. Each signature can
    /// independently come from repository configuration or explicit fields.
    #[allow(clippy::too_many_arguments)]
    pub fn create_commit_object(
        &self,
        message_text: ffi::String,
        tree_id: ffi::String,
        parent_ids: ffi::Vec<ffi::String>,
        update_reference: ffi::Slice<u8>,
        author_is_explicit: bool,
        author_name: ffi::Slice<u8>,
        author_email: ffi::Slice<u8>,
        author_time_seconds: i64,
        author_time_offset_seconds: i32,
        committer_is_explicit: bool,
        committer_name: ffi::Slice<u8>,
        committer_email: ffi::Slice<u8>,
        committer_time_seconds: i64,
        committer_time_offset_seconds: i32,
    ) -> ffi::Result<ffi::String, GixError> {
        let repo = self.inner.to_thread_local();
        let tree_id = match parse_id_for_repo(&repo, &tree_id) {
            Ok(id) => id,
            Err(err) => return ffi::Err(err),
        };
        if let Err(err) = require_kind(&repo, tree_id, gix::objs::Kind::Tree) {
            return ffi::Err(err);
        }

        let mut parents = Vec::with_capacity(parent_ids.len());
        for parent in parent_ids.into_vec() {
            let id = match parse_id_for_repo(&repo, &parent) {
                Ok(id) => id,
                Err(err) => return ffi::Err(err),
            };
            if let Err(err) = find_commit_checked(&repo, id) {
                return ffi::Err(err);
            }
            parents.push(id);
        }

        let author = if author_is_explicit {
            explicit_signature(
                author_name.as_slice(),
                author_email.as_slice(),
                author_time_seconds,
                author_time_offset_seconds,
            )
        } else {
            configured_signature(&repo, true)
        };
        let author = match author {
            Ok(signature) => signature,
            Err(err) => return ffi::Err(err),
        };
        let committer = if committer_is_explicit {
            explicit_signature(
                committer_name.as_slice(),
                committer_email.as_slice(),
                committer_time_seconds,
                committer_time_offset_seconds,
            )
        } else {
            configured_signature(&repo, false)
        };
        let committer = match committer {
            Ok(signature) => signature,
            Err(err) => return ffi::Err(err),
        };

        let mut author_time = gix::date::parse::TimeBuf::default();
        let mut committer_time = gix::date::parse::TimeBuf::default();
        let author = author.to_ref(&mut author_time);
        let committer = committer.to_ref(&mut committer_time);

        if update_reference.as_slice().is_empty() {
            match repo.new_commit_as(
                committer,
                author,
                message_text.as_str(),
                tree_id,
                parents,
            ) {
                Ok(commit) => ffi::Ok(hex(commit.id.as_ref())),
                Err(err) => ffi::Err(other(&err)),
            }
        } else {
            let reference: gix::bstr::BString = update_reference.as_slice().to_vec().into();
            match repo.commit_as(
                committer,
                author,
                reference,
                message_text.as_str(),
                tree_id,
                parents,
            ) {
                Ok(id) => ffi::Ok(hex(id.as_ref())),
                Err(err) => ffi::Err(other(&err)),
            }
        }
    }

    /// Create a commit from the current index and atomically advance HEAD.
    pub fn create_commit_from_index(
        &self,
        message_text: ffi::String,
        has_explicit_identity: bool,
        author_name: ffi::Slice<u8>,
        author_email: ffi::Slice<u8>,
        allow_empty: bool,
    ) -> ffi::Result<ffi::String, GixError> {
        let repo = self.inner.to_thread_local();
        let tree_id = match tree_from_index(&repo) {
            Ok(id) => id,
            Err(err) => return ffi::Err(err),
        };

        let mut head = match repo.head() {
            Ok(head) => head,
            Err(err) => return ffi::Err(other(&err)),
        };
        let parent = match head.try_peel_to_id() {
            Ok(parent) => parent.map(|id| id.detach()),
            Err(err) => return ffi::Err(other(&err)),
        };

        if !allow_empty {
            let unchanged = match parent {
                Some(parent_id) => {
                    let commit = match find_commit_checked(&repo, parent_id) {
                        Ok(commit) => commit,
                        Err(err) => return ffi::Err(err),
                    };
                    match commit.tree_id() {
                        Ok(parent_tree) => parent_tree.detach() == tree_id,
                        Err(err) => return ffi::Err(other(&err)),
                    }
                }
                None => tree_id == gix::ObjectId::empty_tree(repo.object_hash()),
            };
            if unchanged {
                return ffi::Err(GixError::Other(message(
                    "refusing to create an empty commit",
                )));
            }
        }

        let parents = parent.into_iter().collect::<Vec<_>>();
        if has_explicit_identity {
            let now = gix::date::Time::now_local_or_utc();
            let signature = match explicit_signature(
                author_name.as_slice(),
                author_email.as_slice(),
                now.seconds,
                now.offset,
            ) {
                Ok(signature) => signature,
                Err(err) => return ffi::Err(err),
            };
            let mut author_time = gix::date::parse::TimeBuf::default();
            let mut committer_time = gix::date::parse::TimeBuf::default();
            match repo.commit_as(
                signature.to_ref(&mut committer_time),
                signature.to_ref(&mut author_time),
                "HEAD",
                message_text.as_str(),
                tree_id,
                parents,
            ) {
                Ok(id) => ffi::Ok(hex(id.as_ref())),
                Err(err) => ffi::Err(other(&err)),
            }
        } else {
            match repo.commit(
                "HEAD",
                message_text.as_str(),
                tree_id,
                parents,
            ) {
                Ok(id) => ffi::Ok(hex(id.as_ref())),
                Err(err) => ffi::Err(other(&err)),
            }
        }
    }

    /// Return a read-only unified diff: 0 = HEAD/index, 1 = index/worktree,
    /// 2 = HEAD/worktree including non-ignored untracked files.
    pub fn diff(&self, target: u32, pathspecs: ffi::Slice<u8>) -> ffi::Result<DiffRecord, GixError> {
        let repo = self.inner.to_thread_local();
        match diff::collect(&repo, target, pathspecs.as_slice()) {
            Ok(record) => ffi::Ok(record),
            Err(error) => ffi::Err(error),
        }
    }

    /// Compare tree-resolving revisions. Empty old_revision denotes the empty tree.
    /// Pathspecs are NUL-separated raw Git bytes; paths and IDs in results are owned.
    pub fn tree_changes(
        &self,
        old_revision: ffi::String,
        new_revision: ffi::String,
        pathspecs: ffi::Slice<u8>,
    ) -> ffi::Result<ffi::Vec<TreeChangeRecord>, GixError> {
        let repo = self.inner.to_thread_local();
        match diff::tree_changes(&repo, old_revision.as_str(), new_revision.as_str(), pathspecs.as_slice()) {
            Ok(records) => ffi::Ok(records.into()),
            Err(error) => ffi::Err(error),
        }
    }

    /// Return repository status with LibGit2.Native-compatible numeric flags.
    ///
    /// `pathspecs` is a NUL-separated list of raw Git pathspec bytes.
    pub fn status(
        &self,
        show: u32,
        flags: u32,
        pathspecs: ffi::Slice<u8>,
    ) -> ffi::Result<ffi::Vec<StatusRecord>, GixError> {
        let repo = self.inner.to_thread_local();
        match status::collect(&repo, show, flags, pathspecs.as_slice()) {
            Ok(records) => ffi::Ok(ffi::Vec::from(records)),
            Err(err) => ffi::Err(err),
        }
    }

    /// Add matching working-directory changes to the index and write it.
    ///
    /// Pathspecs are a NUL-separated list of raw Git pathspec bytes.
    pub fn stage(&self, pathspecs: ffi::Slice<u8>) -> ffi::Result<(), GixError> {
        let repo = self.inner.to_thread_local();
        match index::stage(&repo, pathspecs.as_slice()) {
            Ok(()) => ffi::Ok(()),
            Err(err) => ffi::Err(err),
        }
    }

    /// Restore matching index entries from HEAD without changing the worktree.
    ///
    /// An empty pathspec list selects every index path. In an unborn
    /// repository the selected entries are removed.
    pub fn unstage(&self, pathspecs: ffi::Slice<u8>) -> ffi::Result<(), GixError> {
        let repo = self.inner.to_thread_local();
        match index::unstage(&repo, pathspecs.as_slice()) {
            Ok(()) => ffi::Ok(()),
            Err(err) => ffi::Err(err),
        }
    }

    /// Physically reload and validate the repository index from disk.
    ///
    /// Each FFI operation creates a fresh thread-local repository, so this
    /// observes the physical file instead of a retained snapshot.
    ///
    /// This does not restat the worktree against cached index entries; that is
    /// `git update-index --refresh` and is a separate operation.
    pub fn refresh_index(&self) -> ffi::Result<(), GixError> {
        let repo = self.inner.to_thread_local();
        match index::refresh(&repo) {
            Ok(()) => ffi::Ok(()),
            Err(err) => ffi::Err(err),
        }
    }

    /// Update matching tracked entries from the worktree and write the index.
    ///
    /// Untracked paths are never added. Pathspecs are NUL-separated raw Git
    /// pathspec bytes.
    pub fn update_index(&self, pathspecs: ffi::Slice<u8>) -> ffi::Result<(), GixError> {
        let repo = self.inner.to_thread_local();
        match index::update(&repo, pathspecs.as_slice()) {
            Ok(()) => ffi::Ok(()),
            Err(err) => ffi::Err(err),
        }
    }

    /// Return every index entry, including separate conflict stages.
    pub fn index_entries(&self) -> ffi::Result<ffi::Vec<IndexEntryRecord>, GixError> {
        let repo = self.inner.to_thread_local();
        match index::entries(&repo) {
            Ok(entries) => ffi::Ok(ffi::Vec::from(entries)),
            Err(err) => ffi::Err(err),
        }
    }

    /// Resolve a conflicted path as deleted by removing all of its stages.
    pub fn resolve_conflict_as_deleted(
        &self,
        path: ffi::Slice<u8>,
    ) -> ffi::Result<(), GixError> {
        let repo = self.inner.to_thread_local();
        match index::resolve_conflict_as_deleted(&repo, path.as_slice()) {
            Ok(()) => ffi::Ok(()),
            Err(err) => ffi::Err(err),
        }
    }

    /// Write the conflict-free index as a tree object without changing the
    /// index, worktree, or any reference.
    pub fn write_index_tree(&self) -> ffi::Result<ffi::String, GixError> {
        let repo = self.inner.to_thread_local();
        match tree_from_index(&repo) {
            Ok(id) => ffi::Ok(hex(id.as_ref())),
            Err(err) => ffi::Err(err),
        }
    }

    /// Return whether ancestor is reachable from descendant.
    pub fn is_ancestor_of(
        &self,
        ancestor: ffi::String,
        descendant: ffi::String,
    ) -> ffi::Result<bool, GixError> {
        let repo = self.inner.to_thread_local();
        let ancestor = match parse_id_for_repo(&repo, &ancestor) {
            Ok(id) => id,
            Err(err) => return ffi::Err(err),
        };
        let descendant = match parse_id_for_repo(&repo, &descendant) {
            Ok(id) => id,
            Err(err) => return ffi::Err(err),
        };
        if let Err(err) = find_commit_checked(&repo, ancestor) {
            return ffi::Err(err);
        }
        if let Err(err) = find_commit_checked(&repo, descendant) {
            return ffi::Err(err);
        }
        if ancestor == descendant {
            return ffi::Ok(true);
        }

        let walk = match repo.rev_walk(Some(descendant)).all() {
            Ok(walk) => walk,
            Err(err) => return ffi::Err(other(&err)),
        };
        for item in walk {
            match item {
                Ok(info) if info.id == ancestor => return ffi::Ok(true),
                Ok(_) => {}
                Err(err) => return ffi::Err(other(&err)),
            }
        }
        ffi::Ok(false)
    }

    /// Enumerate references, optionally filtered by a raw-byte glob.
    ///
    /// An empty glob enumerates every ordinary reference.
    pub fn references(
        &self,
        glob: ffi::Slice<u8>,
    ) -> ffi::Result<ffi::Vec<ReferenceRecord>, GixError> {
        let repo = self.inner.to_thread_local();
        match references::references(&repo, glob.as_slice()) {
            Ok(records) => ffi::Ok(ffi::Vec::from(records)),
            Err(error) => ffi::Err(error),
        }
    }

    /// Create an annotated tag with exact Git bytes. An absent tagger writes no tagger header.
    pub fn create_annotated_tag(
        &self, name: ffi::Slice<u8>, target_id: ffi::String, data: ffi::Slice<u8>,
        tagger: ffi::Option<TagSignatureRecord>, force: bool,
    ) -> ffi::Result<ffi::String, GixError> {
        let repo = self.inner.to_thread_local();
        let tagger = match tagger.into_option() {
            Some(value) => {
                let name = value.name.into_vec();
                let email = value.email.into_vec();
                match tags::signature(&name, &email, value.time_seconds, value.time_offset_seconds) {
                    Ok(value) => Some(value), Err(error) => return ffi::Err(error),
                }
            }
            None => None,
        };
        match tags::create(&repo, name.as_slice(), &target_id, tagger, data.as_slice(), force) {
            Ok(id) => ffi::Ok(id), Err(error) => ffi::Err(error),
        }
    }

    /// Create or replace a lightweight tag without peeling its supplied object.
    pub fn create_tag_reference(&self, name: ffi::Slice<u8>, target_id: ffi::String, force: bool)
        -> ffi::Result<(), GixError>
    {
        let repo = self.inner.to_thread_local();
        match tags::create_reference(&repo, name.as_slice(), &target_id, force) {
            Ok(()) => ffi::Ok(()), Err(error) => ffi::Err(error),
        }
    }

    /// Read an owned annotated-tag snapshot by exact object id.
    pub fn read_tag(&self, tag_id: ffi::String) -> ffi::Result<TagRecord, GixError> {
        let repo = self.inner.to_thread_local();
        match tags::read(&repo, &tag_id) {
            Ok(record) => ffi::Ok(record), Err(error) => ffi::Err(error),
        }
    }

    /// Follow tag objects to the first non-tag object; other object kinds return their own id.
    pub fn peel_tags(&self, object_id: ffi::String) -> ffi::Result<ffi::String, GixError> {
        let repo = self.inner.to_thread_local();
        match tags::peel(&repo, &object_id) {
            Ok(id) => ffi::Ok(id), Err(error) => ffi::Err(error),
        }
    }

    /// Read an owned note from an exact reference, or the configured default when empty.
    pub fn read_note(&self, annotated_object_id: ffi::String, notes_ref: ffi::Slice<u8>)
        -> ffi::Result<NoteRecord, GixError>
    {
        let repo = self.inner.to_thread_local();
        match notes::read(&repo, &annotated_object_id, notes_ref.as_slice()) {
            Ok(note) => ffi::Ok(note), Err(error) => ffi::Err(error),
        }
    }

    /// Materialize the notes compatibility result in annotated-object ID order.
    pub fn enumerate_notes(&self, notes_ref: ffi::Slice<u8>)
        -> ffi::Result<ffi::Vec<NoteEntryRecord>, GixError>
    {
        let repo = self.inner.to_thread_local();
        match notes::enumerate(&repo, notes_ref.as_slice()) {
            Ok(notes) => ffi::Ok(notes.into()), Err(error) => ffi::Err(error),
        }
    }

    /// Write exact note bytes and atomically update the notes reference.
    pub fn write_note(
        &self,
        annotated_object_id: ffi::String,
        notes_ref: ffi::Slice<u8>,
        data: ffi::Slice<u8>,
        author_name: ffi::Slice<u8>,
        author_email: ffi::Slice<u8>,
        author_time_seconds: i64,
        author_time_offset_seconds: i32,
        committer_name: ffi::Slice<u8>,
        committer_email: ffi::Slice<u8>,
        committer_time_seconds: i64,
        committer_time_offset_seconds: i32,
        overwrite: bool,
    ) -> ffi::Result<ffi::String, GixError> {
        let repo = self.inner.to_thread_local();
        let author = match notes::signature(author_name.as_slice(), author_email.as_slice(),
            author_time_seconds, author_time_offset_seconds) {
            Ok(value) => value, Err(error) => return ffi::Err(error),
        };
        let committer = match notes::signature(committer_name.as_slice(), committer_email.as_slice(),
            committer_time_seconds, committer_time_offset_seconds) {
            Ok(value) => value, Err(error) => return ffi::Err(error),
        };
        match notes::write(&repo, &annotated_object_id, notes_ref.as_slice(), data.as_slice(),
            author, committer, overwrite) {
            Ok(id) => ffi::Ok(id), Err(error) => ffi::Err(error),
        }
    }

    /// Remove an existing note without deleting its notes reference.
    pub fn remove_note(
        &self,
        annotated_object_id: ffi::String,
        notes_ref: ffi::Slice<u8>,
        author_name: ffi::Slice<u8>,
        author_email: ffi::Slice<u8>,
        author_time_seconds: i64,
        author_time_offset_seconds: i32,
        committer_name: ffi::Slice<u8>,
        committer_email: ffi::Slice<u8>,
        committer_time_seconds: i64,
        committer_time_offset_seconds: i32,
    ) -> ffi::Result<bool, GixError> {
        let repo = self.inner.to_thread_local();
        let author = match notes::signature(author_name.as_slice(), author_email.as_slice(),
            author_time_seconds, author_time_offset_seconds) {
            Ok(value) => value, Err(error) => return ffi::Err(error),
        };
        let committer = match notes::signature(committer_name.as_slice(), committer_email.as_slice(),
            committer_time_seconds, committer_time_offset_seconds) {
            Ok(value) => value, Err(error) => return ffi::Err(error),
        };
        match notes::remove(&repo, &annotated_object_id, notes_ref.as_slice(), author, committer) {
            Ok(removed) => ffi::Ok(removed), Err(error) => ffi::Err(error),
        }
    }

    /// Enumerate local and/or remote-tracking branches.
    pub fn branches(&self, filter: u32) -> ffi::Result<ffi::Vec<BranchRecord>, GixError> {
        let repo = self.inner.to_thread_local();
        match references::branches(&repo, filter) {
            Ok(records) => ffi::Ok(ffi::Vec::from(records)),
            Err(error) => ffi::Err(error),
        }
    }

    /// Create or replace a local branch at a commit-resolving revision.
    pub fn create_branch(
        &self,
        name: ffi::Slice<u8>,
        target_revision: ffi::String,
        force: bool,
    ) -> ffi::Result<BranchRecord, GixError> {
        let repo = self.inner.to_thread_local();
        match references::create_branch(&repo, name.as_slice(), &target_revision, force) {
            Ok(record) => ffi::Ok(record),
            Err(error) => ffi::Err(error),
        }
    }

    /// Delete a local or remote-tracking branch.
    pub fn delete_branch(
        &self,
        name: ffi::Slice<u8>,
        remote: bool,
    ) -> ffi::Result<(), GixError> {
        let repo = self.inner.to_thread_local();
        match references::delete_branch(&repo, name.as_slice(), remote) {
            Ok(()) => ffi::Ok(()),
            Err(error) => ffi::Err(error),
        }
    }

    /// Point HEAD symbolically at a reference without checking out files.
    pub fn set_head(&self, branch_name: ffi::Slice<u8>) -> ffi::Result<(), GixError> {
        let repo = self.inner.to_thread_local();
        match references::set_head(&repo, branch_name.as_slice()) {
            Ok(()) => ffi::Ok(()),
            Err(error) => ffi::Err(error),
        }
    }

    /// Resolve an exact reference through symbolic links to its first object id.
    pub fn try_get_reference_target(
        &self,
        name: ffi::Slice<u8>,
    ) -> ffi::Result<OptionalObjectId, GixError> {
        let repo = self.inner.to_thread_local();
        match references::try_get_reference_target(&repo, name.as_slice()) {
            Ok(result) => ffi::Ok(result),
            Err(error) => ffi::Err(error),
        }
    }

    /// Create an exact direct reference only when it is absent.
    pub fn try_create_reference(
        &self,
        name: ffi::Slice<u8>,
        target: ffi::String,
    ) -> ffi::Result<bool, GixError> {
        let repo = self.inner.to_thread_local();
        match references::try_create_reference(&repo, name.as_slice(), &target) {
            Ok(created) => ffi::Ok(created),
            Err(error) => ffi::Err(error),
        }
    }

    /// Atomically replace an exact direct reference when expected-old matches.
    pub fn compare_exchange_reference(
        &self,
        name: ffi::Slice<u8>,
        target: ffi::String,
        expected: ffi::String,
    ) -> ffi::Result<ReferenceUpdateOutcome, GixError> {
        let repo = self.inner.to_thread_local();
        match references::compare_exchange_reference(
            &repo,
            name.as_slice(),
            &target,
            &expected,
        ) {
            Ok(outcome) => ffi::Ok(outcome),
            Err(error) => ffi::Err(error),
        }
    }

    /// Delete an exact direct reference when expected-old matches.
    pub fn delete_reference(
        &self,
        name: ffi::Slice<u8>,
        expected: ffi::String,
    ) -> ffi::Result<ReferenceUpdateOutcome, GixError> {
        let repo = self.inner.to_thread_local();
        match references::delete_reference(&repo, name.as_slice(), &expected) {
            Ok(outcome) => ffi::Ok(outcome),
            Err(error) => ffi::Err(error),
        }
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
        .register(builtins_vec!(ReferenceRecord))
        .register(builtins_vec!(BranchRecord))
        .register(builtins_vec!(IndexEntryRecord))
        .register(builtins_vec!(StatusRecord))
        .register(builtins_vec!(TreeChangeRecord))
        .register(builtins_vec!(NoteEntryRecord))
        .register(service!(ReferenceLockLease))
        .register(service!(Repo))
        .register(service!(ByteReader))
        .validate()
}
