//! Administrative access to the linked worktrees registered under `.git/worktrees/`.
//!
//! [`Repository::worktree_admin_entries()`](crate::Repository::worktree_admin_entries()) reports
//! every registered directory along with its condition, including the malformed ones.
//! [`Repository::worktrees()`](crate::Repository::worktrees()) deliberately hides those, as it
//! answers "which worktrees can I use"; administration needs the opposite answer.
//!
//! [`Repository::checked_out_branches()`](crate::Repository::checked_out_branches()) answers the
//! question every mutating worktree operation must ask first: is this branch already spoken for?
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use gix_error::{ErrorExt, Exn};

use crate::bstr::{BString, ByteSlice};

/// The error returned by [`Repository::worktree_admin_entries()`](crate::Repository::worktree_admin_entries())
/// and [`Repository::checked_out_branches()`](crate::Repository::checked_out_branches()).
#[derive(Debug, thiserror::Error)]
#[expect(missing_docs)]
pub enum Error {
    #[error("Failed to read or iterate the `worktrees` directory")]
    Listing(#[source] std::io::Error),
    #[error("Could not open a worktree repository")]
    OpenWorktreeRepo(#[source] crate::open::Error),
    #[error("Failed to follow a symbolic reference while inspecting worktrees")]
    FollowSymref(#[source] gix_ref::file::find::existing::Error),
}

/// The structural condition of a single administrative entry.
///
/// Only [`Registered`](Condition::Registered) describes a worktree that is usable; every other
/// variant identifies a specific way in which the registration is broken, which is what
/// `git worktree prune` reports on and removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// The `gitdir` file names a checkout which exists and links back to this entry.
    Registered,
    /// The `gitdir` file is absent or unreadable, so nothing connects this entry to a checkout.
    ///
    /// Git treats such an entry as prunable regardless of age.
    MissingGitdir,
    /// The `gitdir` file names a checkout directory which no longer exists.
    CheckoutMissing,
    /// The checkout directory exists, but its `.git` file is missing, malformed, or points elsewhere.
    CheckoutNotLinked,
}

impl Condition {
    /// Return `true` if this entry describes a usable worktree.
    pub fn is_registered(&self) -> bool {
        matches!(self, Condition::Registered)
    }
}

/// A single directory under `.git/worktrees/`, reported whether or not it is intact.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The administrative identifier, which is the directory name under `worktrees`.
    pub id: BString,
    /// The absolute path of the administrative directory itself.
    pub admin_dir: PathBuf,
    /// What is structurally right or wrong with this entry.
    pub condition: Condition,
    /// The checkout directory named by the `gitdir` file, if it could be read at all.
    ///
    /// Present even when the directory it names does not exist, so callers can report the
    /// path that was expected.
    pub checkout: Option<PathBuf>,
    /// The contents of the `locked` file if the entry is locked, which may be empty.
    ///
    /// A locked entry must not be pruned, moved or removed without `--force`.
    pub lock_reason: Option<BString>,
    /// The modification time of the `gitdir` file, which is the signal Git uses to decide
    /// whether an otherwise broken entry is old enough to remove.
    pub gitdir_modified: Option<std::time::SystemTime>,
}

impl Entry {
    /// Return `true` if a `locked` file is present, whatever its contents.
    pub fn is_locked(&self) -> bool {
        self.lock_reason.is_some()
    }
}

impl crate::Repository {
    /// Return every administrative entry under `.git/worktrees/`, sorted by identifier,
    /// including entries which are malformed or point at a checkout that is gone.
    ///
    /// This is the administrative counterpart to
    /// [`worktrees()`](crate::Repository::worktrees()), which filters out any entry lacking a
    /// `gitdir` file and so cannot see the entries that pruning exists to remove. Prefer
    /// `worktrees()` when only usable worktrees are of interest.
    ///
    /// A missing `worktrees` directory yields an empty list rather than an error, matching Git's
    /// treatment of a repository that has never had a linked worktree.
    ///
    /// Note that this reports structure only. Whether an entry is *stale* additionally depends on
    /// its lock state and on an expiry applied to [`gitdir_modified`](Entry::gitdir_modified).
    pub fn worktree_admin_entries(&self) -> Result<Vec<Entry>, Exn<Error>> {
        let mut out = Vec::new();
        let iter = match std::fs::read_dir(self.common_dir().join("worktrees")) {
            Ok(iter) => iter,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(err) => return Err(Error::Listing(err).raise()),
        };

        for entry in iter {
            let entry = entry.map_err(|err| Error::Listing(err).raise())?;
            let admin_dir = entry.path();
            if !admin_dir.is_dir() {
                continue;
            }
            // `Proxy::new` is used rather than `new_if_gitdir_file_exists` precisely because an
            // entry without a `gitdir` file is one we must report instead of skip.
            let proxy = crate::worktree::Proxy::new(self, admin_dir.clone());

            let (condition, checkout) = match proxy.base() {
                Err(_) => (Condition::MissingGitdir, None),
                Ok(checkout) => {
                    let condition = if !checkout.is_dir() {
                        Condition::CheckoutMissing
                    } else {
                        let linked = gix_discover::path::from_gitdir_file(&checkout.join(".git"))
                            .ok()
                            .and_then(|path| std::fs::canonicalize(path).ok())
                            .zip(std::fs::canonicalize(&admin_dir).ok())
                            .is_some_and(|(actual, expected)| actual == expected);
                        if linked && admin_dir.join("HEAD").is_file() {
                            Condition::Registered
                        } else {
                            Condition::CheckoutNotLinked
                        }
                    };
                    (condition, Some(checkout))
                }
            };

            out.push(Entry {
                id: proxy.id().to_owned(),
                gitdir_modified: std::fs::metadata(admin_dir.join("gitdir"))
                    .and_then(|meta| meta.modified())
                    .ok(),
                // Failure to read a present lock is never evidence that pruning is safe.
                lock_reason: match std::fs::read(admin_dir.join("locked")) {
                    Ok(bytes) => Some(bytes.into()),
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                    Err(err) => return Err(Error::Listing(err).raise()),
                },
                admin_dir,
                condition,
                checkout,
            });
        }

        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Return every reference which is spoken for by this repository's main and linked worktrees,
    /// mapped to the worktree directories which hold it.
    ///
    /// A branch appears here when it is checked out, and also when a worktree has detached its
    /// `HEAD` to rebase or bisect it, matching Git's `is_shared_symref()`. Besides branch names,
    /// `HEAD` is recorded for every worktree with a readable head, along with every reference in
    /// the symbolic chain from `HEAD` to its referent, so a plainly detached head contributes only
    /// a `HEAD` entry.
    ///
    /// Callers use this to refuse operations Git refuses: checking a branch out twice, or deleting
    /// one that another worktree is working on. Bare repositories, and worktrees whose head cannot
    /// be read, are ignored rather than reported as errors.
    pub fn checked_out_branches(&self) -> Result<BTreeMap<gix_ref::FullName, Vec<PathBuf>>, Exn<Error>> {
        let mut map = BTreeMap::new();
        insert_head(self.head().ok(), &mut map)?;
        for proxy in self.worktrees().map_err(|err| Error::Listing(err).raise())? {
            let repo = proxy
                .into_repo_with_possibly_inaccessible_worktree()
                .map_err(|err| Error::OpenWorktreeRepo(err).raise())?;
            insert_head(repo.head().ok(), &mut map)?;
        }
        Ok(map)
    }
}

/// Types for [`Repository::add_worktree()`](crate::Repository::add_worktree()).
pub mod add {
    use std::path::PathBuf;

    use crate::bstr::BString;

    /// What the new worktree's `HEAD` should point at.
    #[derive(Debug, Clone)]
    pub enum Attachment {
        /// Attach `HEAD` to a branch, or use it as the start point for `-b`/`-B`.
        ///
        /// Without branch creation this must name an existing local branch.
        Branch(gix_ref::FullName),
        /// Leave `HEAD` detached at a commit, as `git worktree add --detach` does.
        DetachedAt(gix_hash::ObjectId),
    }

    /// The library-level option surface of `git worktree add`.
    #[derive(Debug, Default, Clone)]
    pub struct Options {
        /// An exact administrative identifier, independent of the checkout's basename.
        ///
        /// It must be a safe single path component. Collisions are errors;
        /// `None` retains Git's basename-derived identifier with numeric suffixes.
        pub name: Option<BString>,
        /// `--force`: permit reuse of a stale registration and a branch already checked out.
        ///
        /// Like Git, this never permits taking over a non-empty directory. A single library flag
        /// authorizes stale locked registrations as well; repeated CLI spelling is caller policy.
        pub force: bool,
        /// `-b <name>`: create a new local branch at the attachment's commit.
        ///
        /// The short branch name is reserved until registration succeeds, then published
        /// with a must-not-exist reference transaction.
        pub new_branch: Option<BString>,
        /// `-B <name>`: create or reset a branch for the worktree.
        pub new_branch_force: Option<BString>,
        /// `--lock`, with the optional `--reason`.
        pub lock: Option<Option<BString>>,
        /// `--checkout`: materialise the working tree.
        ///
        /// Checkout uses exclusive file creation after registration. A checkout error retains the
        /// registration and any partial files, and identifies both in the returned error.
        /// Requires the `worktree-mutation` feature; otherwise the request is refused before registration.
        pub checkout: bool,
        /// `--orphan`: start from an unborn branch.
        pub orphan: bool,
        /// `--track` / `--no-track`; `None` follows `branch.autoSetupMerge`.
        ///
        /// Configured `inherit` mode copies the start branch's upstream settings.
        pub track: Option<bool>,
        /// `--guess-remote`; false still honors `worktree.guessRemote`.
        ///
        /// Ambiguity is resolved through `checkout.defaultRemote` when configured.
        pub guess_remote: bool,
        /// `--relative-paths`: record both directional `gitdir` pointers relative to their files.
        ///
        /// When false, `worktree.useRelativePaths` is consulted.
        pub relative_paths: bool,
        /// `--quiet`: suppress progress reporting, which this method does not emit anyway.
        pub quiet: bool,
    }

    /// What [`add_worktree()`](crate::Repository::add_worktree()) created.
    #[derive(Debug, Clone)]
    pub struct Outcome {
        /// The administrative identifier chosen for the worktree.
        pub id: BString,
        /// The administrative directory under `worktrees/`.
        pub admin_dir: PathBuf,
        /// The checkout directory, populated when requested with `Options::checkout`.
        pub checkout: PathBuf,
    }

    /// The error returned by [`add_worktree()`](crate::Repository::add_worktree()).
    #[derive(Debug, thiserror::Error)]
    #[expect(missing_docs)]
    pub enum Error {
        #[error("`{option}` requires a crate feature which is not enabled")]
        Unsupported { option: &'static str },
        #[error("Invalid administrative identifier")]
        InvalidName(#[source] gix_validate::path::component::Error),
        #[error("The administrative identifier {id:?} already exists")]
        IdentifierExists { id: BString },
        #[error("Could not resolve or reserve the worktree branch")]
        Reference {
            #[source]
            source: Box<dyn std::error::Error + Send + Sync + 'static>,
        },
        #[error("Worktree {id:?} remains registered at {path:?}, but checkout was incomplete")]
        Checkout {
            id: BString,
            path: PathBuf,
            #[source]
            source: Box<dyn std::error::Error + Send + Sync + 'static>,
        },
        #[error("Worktree {id:?} remains registered at {path:?}, but branch tracking configuration failed")]
        Configure {
            id: BString,
            path: PathBuf,
            #[source]
            source: Box<dyn std::error::Error + Send + Sync + 'static>,
        },
        #[error("{path:?} is already registered as the worktree {id:?}")]
        AlreadyRegistered { path: PathBuf, id: BString },
        #[error("{path:?} exists and is not empty; refusing to take it over")]
        DirectoryNotEmpty { path: PathBuf },
        #[error("{branch} is already in use by {worktree_dirs:?}")]
        BranchInUse {
            branch: gix_ref::FullName,
            worktree_dirs: Vec<PathBuf>,
        },
        #[error("Could not determine which branches are already in use")]
        Reservation(#[source] super::Error),
        #[error("Could not write the new worktree's HEAD")]
        WriteHead {
            #[source]
            source: Box<dyn std::error::Error + Send + Sync + 'static>,
        },
        #[error("Could not create or write {path:?}")]
        Io {
            path: PathBuf,
            #[source]
            source: std::io::Error,
        },
    }
}

/// Types for [`Repository::remove_worktree()`](crate::Repository::remove_worktree()).
pub mod remove {
    use std::path::PathBuf;

    use crate::bstr::BString;

    /// The options of `git worktree remove`.
    #[derive(Debug, Default, Clone)]
    pub struct Options {
        /// `--force`: remove even when locked, or when the checkout has changes.
        pub force: bool,
    }

    /// What [`remove_worktree()`](crate::Repository::remove_worktree()) actually removed.
    ///
    /// Both fields are `false` when there was nothing left to remove, which is a success rather
    /// than an error; see the method for why.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Outcome {
        /// The administrative directory under `worktrees/` was removed.
        pub registration_removed: bool,
        /// The checkout directory was removed.
        pub checkout_removed: bool,
    }

    /// The error returned by [`remove_worktree()`](crate::Repository::remove_worktree()).
    #[derive(Debug, thiserror::Error)]
    #[expect(missing_docs)]
    pub enum Error {
        #[error("The worktree {id:?} is locked; pass `force` to remove it anyway")]
        Locked { id: BString, reason: BString },
        #[error("The checkout of {id:?} has changes; pass `force` to remove it anyway")]
        Dirty { id: BString },
        #[error("Could not determine whether the checkout of {id:?} has changes")]
        Status {
            id: BString,
            #[source]
            source: Box<dyn std::error::Error + Send + Sync + 'static>,
        },
        #[error("Could not read the registered worktrees")]
        Listing(#[source] super::Error),
        #[error("Could not remove {path:?}")]
        Io {
            path: PathBuf,
            #[source]
            source: std::io::Error,
        },
    }
}

/// Types for [`Repository::prune_worktrees()`](crate::Repository::prune_worktrees()).
pub mod prune {
    use std::path::PathBuf;

    use crate::bstr::BString;

    /// The options of `git worktree prune`.
    #[derive(Debug, Default, Clone)]
    pub struct Options {
        /// `-n`/`--dry-run`: report what would be removed without removing it.
        pub dry_run: bool,
        /// `--expire <time>`: only prune entries whose `gitdir` file is older than this.
        ///
        /// `None` prunes every broken entry regardless of age. Git defaults to three months via
        /// `gc.worktreePruneExpire`; that default belongs to the caller, since this is the
        /// mechanism rather than the policy.
        pub expire: Option<std::time::SystemTime>,
        /// Limit pruning to this exact administrative identifier; `None` considers all entries.
        pub name: Option<BString>,
        /// Permit pruning registrations whose checkout is valid.
        pub include_valid: bool,
        /// Permit pruning locked registrations, without authorizing dirty-file deletion.
        pub include_locked: bool,
        /// Remove the checkout after validating its backpointer and checking for changes.
        ///
        /// Otherwise pruning removes metadata only and preserves checkout bytes.
        pub remove_working_tree: bool,
    }

    /// A single entry considered by [`prune_worktrees()`](crate::Repository::prune_worktrees()).
    #[derive(Debug, Clone)]
    pub struct Candidate {
        /// The administrative identifier.
        pub id: BString,
        /// Its administrative directory.
        pub admin_dir: PathBuf,
        /// Why it is prunable.
        pub reason: super::Condition,
        /// Whether it was actually removed, which is `false` under `dry_run`.
        pub removed: bool,
    }

    /// The error returned by [`prune_worktrees()`](crate::Repository::prune_worktrees()).
    #[derive(Debug, thiserror::Error)]
    #[expect(missing_docs)]
    pub enum Error {
        #[error("Invalid administrative identifier")]
        InvalidName(#[source] gix_validate::path::component::Error),
        #[error("Could not safely remove the selected checkout")]
        Remove(#[source] super::remove::Error),
        #[error("Could not read the registered worktrees")]
        Listing(#[source] super::Error),
        #[error("Could not remove {path:?}")]
        Io {
            path: PathBuf,
            #[source]
            source: std::io::Error,
        },
    }
}

/// Types for [`Repository::repair_worktrees()`](crate::Repository::repair_worktrees()).
pub mod repair {
    use std::path::PathBuf;

    use crate::bstr::BString;

    /// What was done to a single administrative entry.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Action {
        /// The registration was already consistent.
        NothingToDo,
        /// The checkout's `.git` file was missing or wrong, and was rewritten.
        BackPointerRewritten,
        /// The `gitdir` file named a checkout that is gone, and was repointed at a path the
        /// caller supplied.
        GitdirRepointed,
        /// The `gitdir` file names a checkout that is gone, and no replacement path was given.
        ///
        /// Nothing identifies where a vanished checkout went, so this cannot be repaired without
        /// being told. Pass the new location to fix it.
        CheckoutMissing,
    }

    /// What [`repair_worktrees()`](crate::Repository::repair_worktrees()) did to one entry.
    #[derive(Debug, Clone)]
    pub struct Repaired {
        /// The administrative identifier.
        pub id: BString,
        /// Its administrative directory.
        pub admin_dir: PathBuf,
        /// What was done.
        pub action: Action,
    }

    /// The error returned by [`repair_worktrees()`](crate::Repository::repair_worktrees()).
    #[derive(Debug, thiserror::Error)]
    #[expect(missing_docs)]
    pub enum Error {
        #[error("Could not read the registered worktrees")]
        Listing(#[source] super::Error),
        #[error("{path:?} does not look like a worktree checkout of this repository")]
        NotACheckout { path: PathBuf },
        #[error("Could not write {path:?}")]
        Io {
            path: PathBuf,
            #[source]
            source: std::io::Error,
        },
    }
}

/// Types for [`Repository::move_worktree()`](crate::Repository::move_worktree()).
pub mod r#move {
    use std::path::PathBuf;

    use crate::bstr::BString;

    /// The options of `git worktree move`.
    #[derive(Debug, Default, Clone)]
    pub struct Options {
        /// `--force`: move even when the worktree is locked.
        pub force: bool,
    }

    /// The error returned by [`move_worktree()`](crate::Repository::move_worktree()).
    #[derive(Debug, thiserror::Error)]
    #[expect(missing_docs)]
    pub enum Error {
        #[error("There is no linked worktree registered as {id:?}")]
        NotFound { id: BString },
        #[error("The worktree {id:?} is locked; pass `force` to move it anyway")]
        Locked { id: BString, reason: BString },
        #[error("The registration of {id:?} is broken; repair it before moving it")]
        NotRegistered { id: BString },
        #[error("{path:?} already exists")]
        DestinationExists { path: PathBuf },
        #[error("The worktree {id:?} contains submodules, which cannot be moved yet")]
        ContainsSubmodules { id: BString },
        #[error("Could not read the registered worktrees")]
        Listing(#[source] super::Error),
        #[error("Could not move the checkout to {path:?}")]
        Move {
            path: PathBuf,
            #[source]
            source: std::io::Error,
        },
        #[error("The checkout moved to {path:?}, but its pointers could not be updated; `repair` with that path will finish the job")]
        Repoint {
            path: PathBuf,
            #[source]
            source: std::io::Error,
        },
    }
}

/// The error returned by [`Repository::lock_worktree()`](crate::Repository::lock_worktree()) and
/// [`Repository::unlock_worktree()`](crate::Repository::unlock_worktree()).
#[derive(Debug, thiserror::Error)]
#[expect(missing_docs)]
pub enum LockError {
    #[error("There is no linked worktree registered as {id:?}")]
    NotFound { id: BString },
    #[error("The worktree {id:?} is already locked")]
    AlreadyLocked { id: BString },
    #[error("The worktree {id:?} is not locked")]
    NotLocked { id: BString },
    #[error("Could not write the `locked` file")]
    Write(#[source] std::io::Error),
    #[error("Could not remove the `locked` file")]
    Remove(#[source] std::io::Error),
}

impl crate::Repository {
    /// Mark the linked worktree `id` as locked, optionally recording `reason`, so that pruning,
    /// moving and removal refuse to touch it without being forced.
    ///
    /// This is `git worktree lock [--reason <reason>]`. Locking an already-locked worktree is an
    /// error, as it is in Git, rather than silently replacing the reason.
    ///
    /// The main worktree has no administrative directory and so cannot be locked; naming it yields
    /// [`LockError::NotFound`].
    pub fn lock_worktree(&self, id: &crate::bstr::BStr, reason: Option<&crate::bstr::BStr>) -> Result<(), Exn<LockError>> {
        let admin_dir = self.worktree_admin_dir(id).ok_or_else(|| {
            LockError::NotFound {
                id: id.to_owned(),
            }
            .raise()
        })?;
        let lock_path = admin_dir.join("locked");
        if lock_path.exists() {
            return Err(LockError::AlreadyLocked { id: id.to_owned() }.raise());
        }

        // Git terminates the reason with a newline, and writes an empty file when none is given.
        let mut contents = reason.map(ToOwned::to_owned).unwrap_or_default();
        if !contents.is_empty() && !contents.ends_with(b"\n") {
            contents.push(b'\n');
        }
        std::fs::write(&lock_path, &contents).map_err(|err| LockError::Write(err).raise())?;
        Ok(())
    }

    /// Remove the lock from the linked worktree `id`.
    ///
    /// This is `git worktree unlock`. Unlocking a worktree that is not locked is an error, as it is
    /// in Git, so that a caller cannot mistake "nothing to do" for "the lock was mine to drop".
    pub fn unlock_worktree(&self, id: &crate::bstr::BStr) -> Result<(), Exn<LockError>> {
        let admin_dir = self.worktree_admin_dir(id).ok_or_else(|| {
            LockError::NotFound {
                id: id.to_owned(),
            }
            .raise()
        })?;
        let lock_path = admin_dir.join("locked");
        if !lock_path.exists() {
            return Err(LockError::NotLocked { id: id.to_owned() }.raise());
        }
        std::fs::remove_file(&lock_path).map_err(|err| LockError::Remove(err).raise())?;
        Ok(())
    }

    /// Register a new linked worktree at `path` attached per `attach`, optionally materialising
    /// its working tree, and return what was created.
    ///
    /// This is the first of the three steps `git worktree add` performs, and corresponds to
    /// `git worktree add --no-checkout`: it writes the administrative directory under
    /// `worktrees/`, the `.git` file in the checkout, and the pointers linking them. Configuring
    /// sparse checkout can precede materialisation by leaving `options.checkout` disabled.
    /// When checkout is requested, failure leaves the registered checkout available for recovery.
    ///
    /// The target must be absent or an empty directory. A path already registered as a worktree of
    /// this repository is reported as such rather than being taken over, and a branch already in
    /// use by another worktree — including one held by an in-progress rebase or bisect — is
    /// refused, matching Git.
    ///
    /// Administrative directories and the checkout's `.git` file are reserved exclusively.
    /// Derived identifier collisions are retried with a numeric suffix, including incomplete
    /// registrations. Explicit identifiers fail on collisions instead.
    /// On failure, cleanup removes only paths reserved by this call. A newly created checkout is
    /// removed only if it is still empty, preserving files concurrently placed there by others.
    pub fn add_worktree(
        &self,
        path: &std::path::Path,
        attach: add::Attachment,
        options: add::Options,
    ) -> Result<add::Outcome, Exn<add::Error>> {
        use add::{Attachment, Error};

        #[cfg(not(feature = "worktree-mutation"))]
        if options.checkout {
            return Err(Error::Unsupported {
                option: "--checkout (worktree-mutation)",
            }
            .raise());
        }

        let checkout = if path.is_absolute() {
            path.to_owned()
        } else {
            self.workdir().unwrap_or(self.common_dir()).join(path)
        };
        let reference_error = |source: Box<dyn std::error::Error + Send + Sync>| {
            Error::Reference { source }.raise()
        };

        // Classify the target before resolving references. A failed checkout claim must not create
        // or disturb administrative state, even when the requested commit is invalid.
        let mut entries = self.worktree_admin_entries().map_err(|err| {
            Error::Reservation(err.into_inner()).raise()
        })?;
        let replaced_registration = entries
            .iter()
            .position(|entry| entry.checkout.as_deref() == Some(checkout.as_path()))
            .map(|position| entries.remove(position));
        if let Some(entry) = &replaced_registration {
            if !options.force || entry.condition.is_registered() {
                return Err(Error::AlreadyRegistered {
                    path: checkout,
                    id: entry.id.clone(),
                }
                .raise());
            }
        }
        let checkout_existed = match std::fs::metadata(&checkout) {
            Ok(metadata) if metadata.is_dir() => true,
            Ok(_) => {
                return Err(Error::Io {
                    path: checkout,
                    source: std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "the worktree path exists and is not a directory",
                    ),
                }
                .raise());
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(source) => {
                return Err(Error::Io {
                    path: checkout,
                    source,
                }
                .raise());
            }
        };
        if checkout_existed
            && std::fs::read_dir(&checkout)
                .map_err(|source| {
                    Error::Io {
                        path: checkout.clone(),
                        source,
                    }
                    .raise()
                })?
                .next()
                .is_some()
        {
            return Err(Error::DirectoryNotEmpty { path: checkout }.raise());
        }

        let requested_new_branch = match (&options.new_branch, &options.new_branch_force) {
            (Some(_), Some(_)) => {
                return Err(reference_error(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "new_branch and new_branch_force are mutually exclusive",
                ))));
            }
            (Some(name), None) => Some((name.clone(), false)),
            (None, Some(name)) => Some((name.clone(), true)),
            (None, None) => None,
        };
        let guess_remote = options.guess_remote
            || self
                .config_snapshot()
                .boolean("worktree.guessRemote")
                .unwrap_or(false);

        let mut held_names;
        let mut tracking_source = None;
        let mut creating_branch = false;
        let mut resetting_branch = false;
        let mut orphan_branch = false;
        let (attach, tip) = if options.orphan {
            let short_name = match requested_new_branch.as_ref() {
                Some((name, _)) => name.clone(),
                None => checkout
                    .file_name()
                    .map(|name| gix_path::into_bstr(std::path::Path::new(name)).into_owned())
                    .ok_or_else(|| {
                        reference_error(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "an orphan worktree path must have a final component",
                        )))
                    })?,
            };
            let branch = worktree_local_branch_name(short_name.as_bstr())
                .map_err(reference_error)?;
            held_names = vec![branch.clone()];
            orphan_branch = true;
            (Attachment::Branch(branch), None)
        } else {
            let mut guessed_target = None;
            let (start_tip, source_names) = match &attach {
                Attachment::DetachedAt(id) => {
                    self.find_commit(*id)
                        .map_err(|err| reference_error(Box::new(err)))?;
                    (*id, Vec::new())
                }
                Attachment::Branch(name) => {
                    let (tip, names) = self.worktree_branch_tip(name)?;
                    match tip {
                        Some(tip) => {
                            tracking_source = Some(name.clone());
                            (tip, names)
                        }
                        None if requested_new_branch.is_none() && guess_remote => {
                            let (remote_branch, tip) = self
                                .guess_worktree_remote(name)?
                                .ok_or_else(|| {
                                    reference_error(Box::new(std::io::Error::new(
                                        std::io::ErrorKind::NotFound,
                                        format!(
                                            "no unambiguous remote-tracking branch matches {}",
                                            name.shorten()
                                        ),
                                    )))
                                })?;
                            tracking_source = Some(remote_branch);
                            guessed_target = Some(name.clone());
                            (tip, Vec::new())
                        }
                        None => {
                            return Err(reference_error(Box::new(std::io::Error::new(
                                std::io::ErrorKind::NotFound,
                                format!("reference {} does not name a commit", name),
                            ))));
                        }
                    }
                }
            };

            if let Some((short_name, reset)) = requested_new_branch.as_ref() {
                let branch = worktree_local_branch_name(short_name.as_bstr())
                    .map_err(reference_error)?;
                held_names = vec![branch.clone()];
                resetting_branch = *reset;
                creating_branch = !reset;
                (Attachment::Branch(branch), Some(start_tip))
            } else if let Some(branch) = guessed_target {
                held_names = vec![branch.clone()];
                creating_branch = true;
                (Attachment::Branch(branch), Some(start_tip))
            } else {
                held_names = source_names;
                (attach, Some(start_tip))
            }
        };

        let tracking = match &attach {
            Attachment::Branch(branch) => self.worktree_tracking_plan(
                branch,
                tracking_source.as_ref(),
                options.track,
                creating_branch || resetting_branch,
                orphan_branch,
            )?,
            Attachment::DetachedAt(_) => {
                if options.track.is_some() {
                    return Err(reference_error(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "tracking requires a newly created or reset branch",
                    ))));
                }
                None
            }
        };
        let relative_paths = options.relative_paths || self.worktree_paths_relative();

        use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit};
        // Keep the branch name stable until registration is published. New and reset branches are
        // committed only after the owned registration is complete. Orphan branches retain the
        // must-not-exist lock but deliberately never create a reference.
        let publish_branch = creating_branch || resetting_branch;
        let mut branch_guard = match (&attach, tip) {
            (Attachment::Branch(name), tip) => {
                let expected = if orphan_branch || creating_branch {
                    PreviousValue::MustNotExist
                } else if resetting_branch {
                    self.try_find_reference(name.as_ref())
                        .map_err(|err| reference_error(Box::new(err)))?
                        .map_or(PreviousValue::MustNotExist, |reference| {
                            PreviousValue::MustExistAndMatch(reference.target().into_owned())
                        })
                } else {
                    PreviousValue::MustExistAndMatch(gix_ref::Target::Object(
                        tip.expect("an existing branch has a commit"),
                    ))
                };
                let edit = RefEdit {
                    name: name.clone(),
                    deref: !creating_branch && !resetting_branch && !orphan_branch,
                    change: Change::Update {
                        expected,
                        new: gix_ref::Target::Object(
                            tip.unwrap_or_else(|| self.object_hash().null()),
                        ),
                        log: LogChange {
                            message: if resetting_branch {
                                "worktree: reset branch".into()
                            } else {
                                "worktree: create branch".into()
                            },
                            ..Default::default()
                        },
                    },
                };
                Some(
                    self.refs
                        .transaction()
                        .prepare(
                            Some(edit),
                            gix_lock::acquire::Fail::Immediately,
                            gix_lock::acquire::Fail::Immediately,
                        )
                        .map_err(|err| reference_error(Box::new(err)))?,
                )
            }
            _ => None,
        };

        // Recheck branch reservations while the reference transaction is held.
        if let Attachment::Branch(branch) = &attach {
            if !creating_branch && !resetting_branch && !orphan_branch {
                // Preparation may have observed a same-tip alias retarget. Resolve names again
                // under the held guard so reservation checks use that exact symbolic chain.
                held_names = self.worktree_branch_tip(branch)?.1;
            }
            let in_use = self
                .checked_out_branches()
                .map_err(|err| Error::Reservation(err.into_inner()).raise())?;
            if let Some(worktree_dirs) = held_names.iter().find_map(|name| in_use.get(name)) {
                let may_share_existing_branch =
                    options.force && !creating_branch && !resetting_branch && !orphan_branch;
                if !may_share_existing_branch {
                    return Err(Error::BranchInUse {
                        branch: branch.clone(),
                        worktree_dirs: worktree_dirs.clone(),
                    }
                    .raise());
                }
            }
        }

        let dot_git_path = checkout.join(".git");
        let mut checkout_created = false;
        let mut dot_git_created = false;
        let io = |path: &std::path::Path| {
            let path = path.to_owned();
            move |source: std::io::Error| Error::Io { path, source }.raise()
        };
        let result = (|| {
            if !checkout_existed {
                if let Some(parent) = checkout.parent() {
                    std::fs::create_dir_all(parent).map_err(io(parent))?;
                }
                std::fs::create_dir(&checkout).map_err(io(&checkout))?;
                checkout_created = true;
            }
            // Reserve even a caller-provided empty checkout without overwriting another
            // registration which arrived after the preflight inspection.
            let mut dot_git = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&dot_git_path)
                .map_err(io(&dot_git_path))?;
            dot_git_created = true;

            if let Some(entry) = &replaced_registration {
                match std::fs::remove_dir_all(&entry.admin_dir) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => return Err(io(&entry.admin_dir)(source)),
                }
            }

            // Claim the checkout first: losing contenders must not create and remove
            // administrative directories while another contender is reserving a name.
            let (id, admin_dir) =
                self.reserve_worktree_id(&checkout, &entries, options.name.as_ref())?;
            let registration: Result<(), Exn<add::Error>> = (|| {
                self.write_worktree_registration(
                    &admin_dir,
                    &checkout,
                    &attach,
                    tip,
                    options.lock.as_ref(),
                    &mut dot_git,
                    relative_paths,
                )?;
                if publish_branch {
                    if let Some(guard) = branch_guard.take() {
                        let committer = self
                            .committer()
                            .transpose()
                            .map_err(|err| reference_error(Box::new(err)))?;
                        guard
                            .commit(committer)
                            .map_err(|err| reference_error(Box::new(err)))?;
                    }
                }
                Ok(())
            })();
            if let Err(err) = registration {
                // The exclusive mkdir, not a directory listing, established ownership.
                std::fs::remove_dir_all(&admin_dir).ok();
                return Err(err);
            }
            Ok((id, admin_dir))
        })();
        if result.is_err() {
            if dot_git_created {
                std::fs::remove_file(&dot_git_path).ok();
            }
            if checkout_created {
                // Leave any files concurrently added by someone else intact.
                std::fs::remove_dir(&checkout).ok();
            }
        }
        let (id, admin_dir) = result?;
        drop(branch_guard);

        if let Some((remote, merge)) = tracking {
            let target_branch = match &attach {
                Attachment::Branch(branch) => branch,
                Attachment::DetachedAt(_) => unreachable!("tracking is only planned for a branch"),
            };
            self.write_worktree_tracking_config(
                target_branch,
                remote.as_bstr(),
                merge.as_ref(),
            )
            .map_err(|source| {
                Error::Configure {
                    id: id.clone(),
                    path: checkout.clone(),
                    source,
                }
                .raise()
            })?;
        }

        #[cfg(feature = "worktree-mutation")]
        if options.checkout {
            self.checkout_added_worktree(&admin_dir, &checkout, tip)
                .map_err(|source| {
                    Error::Checkout {
                        id: id.clone(),
                        path: checkout.clone(),
                        source,
                    }
                    .raise()
                })?;
        }

        Ok(add::Outcome {
            id,
            admin_dir,
            checkout,
        })
    }

    /// Remove the linked worktree `id`: its checkout and its administrative directory.
    ///
    /// This is `git worktree remove [--force]`, with one deliberate difference. Git errors when
    /// the worktree is not there; this reports what it removed instead, so removing something
    /// already gone succeeds with both flags `false`. Removal is frequently a retry after an
    /// interruption, and a caller finishing that retry should not have to distinguish "I removed
    /// it" from "it was already gone" by parsing an error.
    ///
    /// Without `force`, a locked worktree is refused, as is a checkout with staged, tracked or
    /// nonignored untracked changes. Status errors are propagated instead of treated as clean.
    /// A registration without an index is removable only when its checkout contains just `.git`.
    /// Untracked files are checked even when `status.showUntrackedFiles` hides them.
    /// Even with `force`, an existing checkout must link back to this registration.
    pub fn remove_worktree(
        &self,
        id: &crate::bstr::BStr,
        options: remove::Options,
    ) -> Result<remove::Outcome, Exn<remove::Error>> {
        use remove::Error;

        let entry = self
            .worktree_admin_entries()
            .map_err(|err| Error::Listing(err.into_inner()).raise())?
            .into_iter()
            .find(|entry| entry.id == id);
        let Some(entry) = entry else {
            // Nothing registered under this name: either it was never here, or a previous attempt
            // finished the job. Both are "removed" as far as the caller is concerned.
            return Ok(remove::Outcome {
                registration_removed: false,
                checkout_removed: false,
            });
        };

        self.remove_worktree_entry(entry, options.force, options.force)
    }

    fn remove_worktree_entry(
        &self,
        entry: Entry,
        force: bool,
        allow_locked: bool,
    ) -> Result<remove::Outcome, Exn<remove::Error>> {
        use remove::Error;
        let status_error = |source: Box<dyn std::error::Error + Send + Sync>| {
            Error::Status {
                id: entry.id.clone(),
                source,
            }
            .raise()
        };
        if let Some(checkout) = entry.checkout.as_deref().filter(|path| path.is_dir()) {
            let back_pointer = gix_discover::path::from_gitdir_file(&checkout.join(".git"))
                .map_err(|err| status_error(Box::new(err)))?;
            let actual = std::fs::canonicalize(back_pointer).map_err(|err| status_error(Box::new(err)))?;
            let expected = std::fs::canonicalize(&entry.admin_dir).map_err(|err| status_error(Box::new(err)))?;
            if actual != expected {
                return Err(status_error(Box::new(std::io::Error::other(
                    "the checkout does not link back to its worktree registration",
                ))));
            }
        }

        if !allow_locked {
            if let Some(reason) = &entry.lock_reason {
                return Err(Error::Locked {
                    id: entry.id.clone(),
                    reason: reason.clone(),
                }
                .raise());
            }
        }
        if !force {
            if let Some(checkout) = entry.checkout.as_deref().filter(|path| path.is_dir()) {
                let has_index = entry
                    .admin_dir
                    .join("index")
                    .try_exists()
                    .map_err(|err| status_error(Box::new(err)))?;
                let dirty = if has_index {
                    let worktree_repo = crate::worktree::Proxy::new(self, entry.admin_dir.clone())
                        .into_repo_with_possibly_inaccessible_worktree()
                        .map_err(|err| status_error(Box::new(err)))?;
                    // Restore the walker even if status.showUntrackedFiles disabled it.
                    // Display preferences must not authorize deleting untracked work.
                    let dirwalk = worktree_repo
                        .dirwalk_options()
                        .map_err(|err| status_error(Box::new(err)))?;
                    let mut changes = worktree_repo
                        .status(gix_features::progress::Discard)
                        .map_err(|err| status_error(Box::new(err)))?
                        .index_worktree_options_mut(|options| options.dirwalk_options = Some(dirwalk))
                        .untracked_files(crate::status::UntrackedFiles::Collapsed)
                        .index_worktree_submodules(crate::status::Submodule::Given {
                            ignore: crate::submodule::config::Ignore::None,
                            check_dirty: true,
                        })
                        .into_iter(Vec::<BString>::new())
                        .map_err(|err| status_error(Box::new(err)))?;
                    // Iterator failures mean unknown cleanliness, never a clean checkout.
                    changes
                        .next()
                        .transpose()
                        .map_err(|err| status_error(Box::new(err)))?
                        .is_some()
                } else {
                    // A no-checkout registration may be empty, but a missing index is not
                    // evidence that files subsequently placed in the checkout are disposable.
                    let mut has_files = false;
                    for child in std::fs::read_dir(checkout).map_err(|err| status_error(Box::new(err)))? {
                        let child = child.map_err(|err| status_error(Box::new(err)))?;
                        if child.file_name() != ".git" {
                            has_files = true;
                            break;
                        }
                    }
                    has_files
                };
                if dirty {
                    return Err(Error::Dirty { id: entry.id.clone() }.raise());
                }
            }
        }

        // Remove the checkout first: if that fails we still have a registration pointing at it,
        // which is a state `prune` and `repair` both understand. The reverse would strand a
        // directory nothing refers to.
        let mut checkout_removed = false;
        if let Some(checkout) = entry.checkout.as_deref() {
            if checkout.is_dir() {
                std::fs::remove_dir_all(checkout).map_err(|source| {
                    Error::Io {
                        path: checkout.to_owned(),
                        source,
                    }
                    .raise()
                })?;
                checkout_removed = true;
            }
        }
        std::fs::remove_dir_all(&entry.admin_dir).map_err(|source| {
            Error::Io {
                path: entry.admin_dir.clone(),
                source,
            }
            .raise()
        })?;

        Ok(remove::Outcome {
            registration_removed: true,
            checkout_removed,
        })
    }

    /// Remove administrative entries whose registration is broken, and report every candidate.
    ///
    /// This is `git worktree prune [-n] [--expire <time>]`. An entry is a candidate when its
    /// [`Condition`] is anything but [`Registered`](Condition::Registered) — the `gitdir` file is
    /// missing, or names a checkout that is gone or no longer links back. Locked and valid entries
    /// are excluded unless explicitly included. Checkout removal must be separately requested and
    /// still refuses dirty, untracked or foreign data; including locks does not imply force.
    ///
    /// With [`expire`](prune::Options::expire), only entries whose `gitdir` file is older than the
    /// given time are removed; entries whose age cannot be determined are left alone rather than
    /// assumed old. With [`dry_run`](prune::Options::dry_run) nothing is removed and every
    /// candidate is returned with `removed: false`, which is also how a caller reconciles this
    /// against its own record of which worktrees are live before allowing any deletion.
    pub fn prune_worktrees(&self, options: prune::Options) -> Result<Vec<prune::Candidate>, Exn<prune::Error>> {
        if let Some(name) = &options.name {
            gix_validate::path::component(name.as_bstr(), None, Default::default())
                .map_err(|err| prune::Error::InvalidName(err).raise())?;
        }
        let mut out = Vec::new();
        for entry in self
            .worktree_admin_entries()
            .map_err(|err| prune::Error::Listing(err.into_inner()).raise())?
        {
            if options.name.as_ref().is_some_and(|name| *name != entry.id)
                || (entry.condition.is_registered() && !options.include_valid)
                || (entry.is_locked() && !options.include_locked)
            {
                continue;
            }
            if let Some(expire) = options.expire {
                // No timestamp means no evidence of age, and pruning on no evidence is how a live
                // worktree gets deleted.
                match entry.gitdir_modified {
                    Some(modified) if modified < expire => {}
                    _ => continue,
                }
            }

            let removed = if options.dry_run {
                false
            } else if options.remove_working_tree {
                self.remove_worktree_entry(entry.clone(), false, options.include_locked)
                    .map_err(|err| prune::Error::Remove(err.into_inner()).raise())?
                    .registration_removed
            } else {
                std::fs::remove_dir_all(&entry.admin_dir).map_err(|source| {
                    prune::Error::Io {
                        path: entry.admin_dir.clone(),
                        source,
                    }
                    .raise()
                })?;
                true
            };
            out.push(prune::Candidate {
                id: entry.id,
                admin_dir: entry.admin_dir,
                reason: entry.condition,
                removed,
            });
        }
        Ok(out)
    }

    /// Repair the two-way link between each registered checkout and its administrative directory.
    ///
    /// This is `git worktree repair [<path>...]`. There are two directions of breakage and they
    /// are not symmetric:
    ///
    /// - A checkout whose `.git` file is missing or names the wrong administrative directory is
    ///   repaired without help, because the registration still says where the checkout is.
    /// - A registration whose `gitdir` names a checkout that is gone can only be repaired by being
    ///   told where it went. Pass the new location in `moved_checkouts`; without it, such an entry
    ///   is reported as [`Action::CheckoutMissing`](repair::Action::CheckoutMissing) and left alone.
    ///
    /// Every entry is reported, including the ones that needed nothing, so a caller can tell
    /// "repaired" from "was already fine" from "cannot repair without more information".
    pub fn repair_worktrees(
        &self,
        moved_checkouts: &[&std::path::Path],
    ) -> Result<Vec<repair::Repaired>, Exn<repair::Error>> {
        use repair::{Action, Error};

        // Index the supplied paths by the administrative *identifier* their `.git` file names, so
        // a moved checkout can be matched to the registration that lost track of it. The identifier
        // is used rather than the full path because the two spellings need not match byte for byte:
        // the `.git` file records forward slashes even on Windows, while entries come from
        // `read_dir` with native separators, and either side may differ in prefix or normalisation.
        let key = |path: &std::path::Path| -> Option<BString> {
            path.file_name()
                .map(|name| gix_path::into_bstr(std::path::Path::new(name)).into_owned())
        };
        let mut relocated = std::collections::BTreeMap::new();
        for path in moved_checkouts {
            let dot_git = path.join(".git");
            let admin_dir = gix_discover::path::from_plain_file(dot_git.as_ref())
                .transpose()
                .ok()
                .flatten()
                .ok_or_else(|| {
                    Error::NotACheckout {
                        path: path.to_path_buf(),
                    }
                    .raise()
                })?;
            let id = key(&admin_dir).ok_or_else(|| {
                Error::NotACheckout {
                    path: path.to_path_buf(),
                }
                .raise()
            })?;
            relocated.insert(id, path.to_path_buf());
        }

        let mut out = Vec::new();
        for entry in self
            .worktree_admin_entries()
            .map_err(|err| Error::Listing(err.into_inner()).raise())?
        {
            let action = match entry.condition {
                Condition::Registered => Action::NothingToDo,
                Condition::MissingGitdir => Action::CheckoutMissing,
                Condition::CheckoutNotLinked => {
                    let checkout = entry.checkout.clone().expect("a checkout path was read");
                    write_dot_git_back_pointer(
                        &checkout,
                        &entry.admin_dir,
                        registration_paths_are_relative(&entry.admin_dir)
                            || self.worktree_paths_relative(),
                    )
                        .map_err(|source| Error::Io {
                            path: checkout.join(".git"),
                            source,
                        })
                        .map_err(ErrorExt::raise)?;
                    Action::BackPointerRewritten
                }
                Condition::CheckoutMissing => match relocated.get(&entry.id) {
                    None => Action::CheckoutMissing,
                    Some(new_checkout) => {
                        write_gitdir_pointer(
                            &entry.admin_dir,
                            new_checkout,
                            registration_paths_are_relative(&entry.admin_dir)
                                || self.worktree_paths_relative(),
                        )
                            .map_err(|source| Error::Io {
                                path: entry.admin_dir.join("gitdir"),
                                source,
                            })
                            .map_err(ErrorExt::raise)?;
                        Action::GitdirRepointed
                    }
                },
            };
            out.push(repair::Repaired {
                id: entry.id,
                admin_dir: entry.admin_dir,
                action,
            });
        }
        Ok(out)
    }

    /// Move the checkout of the linked worktree `id` to `destination`, updating both pointers.
    ///
    /// This is `git worktree move`. A locked worktree is refused without `force`, an existing
    /// destination is refused outright, and the main worktree has no administrative directory so
    /// naming it yields [`NotFound`](r#move::Error::NotFound).
    ///
    /// The directory is moved first and the pointers updated second. If the move succeeds and the
    /// pointer update does not, the result is exactly the state
    /// [`repair_worktrees()`](crate::Repository::repair_worktrees()) fixes when given the new
    /// path — so a failure here is recoverable rather than a puzzle. The error says so.
    ///
    /// ### Divergence from Git
    ///
    /// Git refuses to move a worktree containing submodules, because their `gitdir` pointers would
    /// also need rewriting. This refuses the same case rather than moving one and leaving the
    /// submodules broken.
    pub fn move_worktree(
        &self,
        id: &crate::bstr::BStr,
        destination: &std::path::Path,
        options: r#move::Options,
    ) -> Result<PathBuf, Exn<r#move::Error>> {
        use r#move::Error;

        let entry = self
            .worktree_admin_entries()
            .map_err(|err| Error::Listing(err.into_inner()).raise())?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| Error::NotFound { id: id.to_owned() }.raise())?;

        if !options.force {
            if let Some(reason) = &entry.lock_reason {
                return Err(Error::Locked {
                    id: entry.id.clone(),
                    reason: reason.clone(),
                }
                .raise());
            }
        }
        if !entry.condition.is_registered() {
            return Err(Error::NotRegistered { id: entry.id.clone() }.raise());
        }
        let source = entry.checkout.clone().expect("a registered entry has a checkout");
        if destination.exists() {
            return Err(Error::DestinationExists {
                path: destination.to_owned(),
            }
            .raise());
        }
        if source.join(".gitmodules").is_file() {
            return Err(Error::ContainsSubmodules { id: entry.id.clone() }.raise());
        }

        std::fs::rename(&source, destination).map_err(|err| {
            Error::Move {
                path: destination.to_owned(),
                source: err,
            }
            .raise()
        })?;

        // From here the checkout has already moved, so a failure is repairable rather than lost.
        let relative_paths =
            registration_paths_are_relative(&entry.admin_dir) || self.worktree_paths_relative();
        write_gitdir_pointer(&entry.admin_dir, destination, relative_paths)
            .and_then(|()| {
                write_dot_git_back_pointer(destination, &entry.admin_dir, relative_paths)
            })
            .map_err(|err| {
                Error::Repoint {
                    path: destination.to_owned(),
                    source: err,
                }
                .raise()
            })?;

        Ok(destination.to_owned())
    }

    /// Write every file that links `admin_dir` and `checkout` together, creating both directories.
    fn write_worktree_registration(
        &self,
        admin_dir: &std::path::Path,
        checkout: &std::path::Path,
        attach: &add::Attachment,
        tip: Option<gix_hash::ObjectId>,
        lock: Option<&Option<BString>>,
        dot_git_file: &mut std::fs::File,
        relative_paths: bool,
    ) -> Result<(), Exn<add::Error>> {
        use add::Error;
        use std::io::Write;
        let io = |path: &std::path::Path| {
            let path = path.to_owned();
            move |source: std::io::Error| Error::Io { path, source }.raise()
        };

        let dot_git = checkout.join(".git");
        write_gitdir_pointer(admin_dir, checkout, relative_paths)
            .map_err(io(&admin_dir.join("gitdir")))?;

        // `commondir` is relative to the administrative directory, which is always two levels down.
        std::fs::write(admin_dir.join("commondir"), b"../..\n").map_err(io(&admin_dir.join("commondir")))?;

        self.write_worktree_head(admin_dir, attach, tip)?;

        if let Some(reason) = lock {
            let mut contents = reason.clone().unwrap_or_default();
            if !contents.is_empty() && !contents.ends_with(b"\n") {
                contents.push(b'\n');
            }
            std::fs::write(admin_dir.join("locked"), &contents).map_err(io(&admin_dir.join("locked")))?;
        }

        // The checkout points back at us, completing the two-way link.
        let contents = dot_git_back_pointer_contents(checkout, admin_dir, relative_paths)
            .map_err(io(&dot_git))?;
        dot_git_file.write_all(&contents).map_err(io(&dot_git))?;
        Ok(())
    }

    /// Write the new worktree's `HEAD` through a reference store scoped to `admin_dir`.
    ///
    /// Git does the same: it sets up the worktree's own reference store and writes `HEAD` through
    /// it, which is what produces `worktrees/<id>/logs/HEAD`. Writing the file directly skips the
    /// reflog, and reimplementing the reflog by hand would mean reimplementing
    /// `core.logAllRefUpdates` and the rule that bare repositories keep none. The store already
    /// knows both.
    ///
    /// The reflog message is empty because Git's is: `builtin/worktree.c` passes none to either
    /// `refs_update_ref` or `refs_update_symref`. The `checkout: moving from ...` text one might
    /// expect is written by the checkout Git runs afterwards, which this `--no-checkout` shape
    /// never performs.
    ///
    /// `gix-ref` logs a *symbolic* update only when the expectation names an object, which is the
    /// mechanism `clone` uses to record its initial `HEAD`, so the branch case supplies the branch
    /// tip. An unborn branch has no tip and gets no reflog, as in Git.
    fn write_worktree_head(
        &self,
        admin_dir: &std::path::Path,
        attach: &add::Attachment,
        tip: Option<gix_hash::ObjectId>,
    ) -> Result<(), Exn<add::Error>> {
        use add::{Attachment, Error};
        use gix_ref::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};

        let store = crate::RefStore::for_linked_worktree_opts(
            admin_dir.to_owned(),
            self.common_dir().to_owned(),
            self.object_hash(),
            gix_ref::store::init::Options {
                write_reflog: self.refs.write_reflog,
                precompose_unicode: self.refs.precompose_unicode,
                prohibit_windows_device_names: self.refs.prohibit_windows_device_names,
            },
        );

        let (new, expected) = match attach {
            Attachment::Branch(name) => {
                // The tip is what lets `gix-ref` log a symbolic update at all. Its absence means an
                // unborn branch rather than an error; `add_worktree` has already established that
                // the branch is not in use elsewhere.
                let expected = match tip {
                    Some(tip) => PreviousValue::ExistingMustMatch(gix_ref::Target::Object(tip)),
                    None => PreviousValue::MustNotExist,
                };
                (gix_ref::Target::Symbolic(name.clone()), expected)
            }
            Attachment::DetachedAt(id) => (gix_ref::Target::Object(*id), PreviousValue::MustNotExist),
        };

        let edit = RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: Default::default(),
                },
                expected,
                new,
            },
            name: "HEAD".try_into().expect("HEAD is always a valid reference name"),
            deref: false,
        };

        let write_head =
            |source: Box<dyn std::error::Error + Send + Sync + 'static>| Error::WriteHead { source }.raise();
        let committer = self.committer().transpose().map_err(|err| write_head(Box::new(err)))?;
        store
            .transaction()
            .prepare(
                Some(edit),
                gix_lock::acquire::Fail::Immediately,
                gix_lock::acquire::Fail::Immediately,
            )
            .map_err(|err| write_head(Box::new(err)))?
            .commit(committer)
            .map_err(|err| write_head(Box::new(err)))?;
        Ok(())
    }

    fn worktree_branch_tip(
        &self,
        name: &gix_ref::FullName,
    ) -> Result<(Option<gix_hash::ObjectId>, Vec<gix_ref::FullName>), Exn<add::Error>> {
        let error = |source: Box<dyn std::error::Error + Send + Sync>| {
            add::Error::Reference { source }.raise()
        };
        let mut names = Vec::new();
        let mut current = name.clone();
        loop {
            if names.contains(&current) {
                return Err(error(Box::new(std::io::Error::other("cyclic branch symbolic reference"))));
            }
            names.push(current.clone());
            match self.try_find_reference(current.as_ref()).map_err(|err| error(Box::new(err)))? {
                None => return Ok((None, names)),
                Some(reference) => match reference.target() {
                    gix_ref::TargetRef::Symbolic(next) => current = next.to_owned(),
                    gix_ref::TargetRef::Object(id) => {
                        self.find_commit(id.to_owned()).map_err(|err| error(Box::new(err)))?;
                        return Ok((Some(id.to_owned()), names));
                    }
                },
            }
        }
    }

    fn guess_worktree_remote(
        &self,
        local_branch: &gix_ref::FullName,
    ) -> Result<Option<(gix_ref::FullName, gix_hash::ObjectId)>, Exn<add::Error>> {
        let error = |source: Box<dyn std::error::Error + Send + Sync>| {
            add::Error::Reference { source }.raise()
        };
        let platform = self.references().map_err(|err| error(Box::new(err)))?;
        let references = platform
            .remote_branches()
            .map_err(|err| error(Box::new(err)))?;
        let mut candidates = Vec::new();
        for reference in references {
            let mut reference = reference.map_err(error)?;
            let tracking_branch = reference.name().to_owned();
            let mapping = self
                .upstream_branch_and_remote_for_tracking_branch(tracking_branch.as_ref())
                .map_err(|err| error(Box::new(err)))?;
            let Some((upstream, remote)) = mapping else {
                continue;
            };
            if upstream.shorten() != local_branch.shorten() {
                continue;
            }
            let Some(remote_name) = remote.name() else {
                continue;
            };
            let tip = reference
                .peel_to_id()
                .map_err(|err| error(Box::new(err)))?
                .detach();
            self.find_commit(tip)
                .map_err(|err| error(Box::new(err)))?;
            candidates.push((tracking_branch, tip, remote_name.as_bstr().to_owned()));
        }

        if candidates.len() == 1 {
            return Ok(candidates.pop().map(|(branch, tip, _)| (branch, tip)));
        }
        if let Some(preferred) = self.config_snapshot().string("checkout.defaultRemote") {
            let mut preferred_candidates = candidates
                .into_iter()
                .filter(|(_, _, remote)| remote == &preferred);
            if let Some((branch, tip, _)) = preferred_candidates.next() {
                if preferred_candidates.next().is_none() {
                    return Ok(Some((branch, tip)));
                }
            }
        }
        Ok(None)
    }

    fn worktree_tracking_plan(
        &self,
        target_branch: &gix_ref::FullName,
        source_branch: Option<&gix_ref::FullName>,
        requested: Option<bool>,
        branch_is_created_or_reset: bool,
        orphan: bool,
    ) -> Result<Option<(BString, gix_ref::FullName)>, Exn<add::Error>> {
        let error = |source: Box<dyn std::error::Error + Send + Sync>| {
            add::Error::Reference { source }.raise()
        };
        if requested.is_some() && (!branch_is_created_or_reset || orphan) {
            return Err(error(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "tracking requires a non-orphan newly created or reset branch",
            ))));
        }
        if requested == Some(false) {
            return Ok(None);
        }
        let Some(source_branch) = source_branch else {
            if requested == Some(true) {
                return Err(error(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "tracking requires a branch start point",
                ))));
            }
            return Ok(None);
        };

        let config = self.config_snapshot();
        let auto_setup = config.string("branch.autoSetupMerge");
        if requested.is_none()
            && auto_setup
                .as_ref()
                .is_some_and(|value| value.as_bstr().eq_ignore_ascii_case(b"inherit"))
        {
            if source_branch.category() != Some(gix_ref::Category::LocalBranch) {
                return Ok(None);
            }
            let remote = config.string_by("branch", Some(source_branch.shorten()), "remote");
            let merge = config.string_by("branch", Some(source_branch.shorten()), "merge");
            return match remote.zip(merge) {
                Some((remote, merge)) => {
                    let merge = gix_ref::FullName::try_from(merge)
                        .map_err(|err| error(Box::new(err)))?;
                    Ok(Some((remote, merge)))
                }
                None => Ok(None),
            };
        }

        let (candidate, source_is_remote) = match source_branch.category() {
            Some(gix_ref::Category::RemoteBranch) => {
                let mapping = self
                    .upstream_branch_and_remote_for_tracking_branch(source_branch.as_ref())
                    .map_err(|err| error(Box::new(err)))?;
                let Some((upstream, remote)) = mapping else {
                    if requested == Some(true) {
                        return Err(error(Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "the remote-tracking branch has no unambiguous configured remote",
                        ))));
                    }
                    return Ok(None);
                };
                let remote = remote.name().ok_or_else(|| {
                    error(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "an anonymous remote cannot configure branch tracking",
                    )))
                })?;
                ((remote.as_bstr().to_owned(), upstream), true)
            }
            Some(gix_ref::Category::LocalBranch) => {
                ((BString::from("."), source_branch.clone()), false)
            }
            _ => {
                if requested == Some(true) {
                    return Err(error(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "tracking requires a local or remote-tracking branch",
                    ))));
                }
                return Ok(None);
            }
        };

        if requested == Some(true) {
            return Ok(Some(candidate));
        }
        let mode = auto_setup.as_ref().map(|value| value.as_bstr());
        let enabled = if mode.is_some_and(|value| value.eq_ignore_ascii_case(b"always")) {
            true
        } else if mode.is_some_and(|value| value.eq_ignore_ascii_case(b"simple")) {
            source_is_remote && target_branch.shorten() == candidate.1.shorten()
        } else {
            let configured = config
                .try_boolean("branch.autoSetupMerge")
                .map_err(|err| error(Box::new(err)))?
                .unwrap_or(true);
            configured && source_is_remote
        };
        Ok(enabled.then_some(candidate))
    }

    fn write_worktree_tracking_config(
        &self,
        branch: &gix_ref::FullName,
        remote: &crate::bstr::BStr,
        merge: &gix_ref::FullNameRef,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let config_path = self.common_dir().join("config");
        let mut config =
            gix_config::File::from_path_no_includes(config_path.clone(), gix_config::Source::Local)?;
        {
            let mut section =
                config.section_mut_or_create_new("branch", Some(branch.shorten()))?;
            while section.remove("remote").is_some() {}
            while section.remove("merge").is_some() {}
            section.set("remote", remote)?;
            section.set("merge", merge.as_bstr())?;
        }
        let mut lock = gix_lock::File::acquire_to_update_resource(
            &config_path,
            gix_lock::acquire::Fail::Immediately,
            None,
        )?;
        config.write_to_filter(&mut lock, |section| {
            section.meta().source == gix_config::Source::Local
        })?;
        lock.commit()?;
        Ok(())
    }

    fn worktree_paths_relative(&self) -> bool {
        self.config_snapshot()
            .boolean("worktree.useRelativePaths")
            .unwrap_or(false)
    }

    #[cfg(feature = "worktree-mutation")]
    fn checkout_added_worktree(
        &self,
        admin_dir: &Path,
        checkout: &Path,
        tip: Option<gix_hash::ObjectId>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let repo = crate::worktree::Proxy::new(self, admin_dir.to_owned()).into_repo()?;
        let index_path = repo.index_path();
        let mut index_lock = gix_lock::File::acquire_to_update_resource(
            &index_path, gix_lock::acquire::Fail::Immediately, None,
        )?;
        if index_path.try_exists()? {
            return Err(std::io::Error::other("another writer already created the worktree index").into());
        }
        let mut index = match tip {
            Some(id) => repo.index_from_tree(&repo.find_commit(id)?.tree_id()?)?,
            None => gix_index::File::from_state(gix_index::State::new(repo.object_hash()), repo.index_path()),
        };
        let mut options = repo.checkout_options(gix_worktree::stack::state::attributes::Source::IdMapping)?;
        options.destination_is_initially_empty = true;
        options.overwrite_existing = false;
        options.keep_going = false;
        let outcome = gix_worktree_state::checkout(
            &mut index,
            checkout,
            repo.objects.clone().into_arc()?,
            &gix_features::progress::Discard,
            &gix_features::progress::Discard,
            &std::sync::atomic::AtomicBool::new(false),
            options,
        )?;
        // Persist recovery information even when a collision prevented complete checkout.
        index.write_to(&mut index_lock, Default::default())?;
        index_lock.commit()?;
        if !outcome.collisions.is_empty() || !outcome.errors.is_empty()
            || !outcome.delayed_paths_unknown.is_empty() || !outcome.delayed_paths_unprocessed.is_empty()
        {
            return Err(std::io::Error::other(format!(
                "checkout reported {} collisions, {} errors and {} unprocessed paths",
                outcome.collisions.len(), outcome.errors.len(),
                outcome.delayed_paths_unknown.len() + outcome.delayed_paths_unprocessed.len(),
            )).into());
        }
        Ok(())
    }

    /// Reserve an administrative directory derived from the checkout's final component.
    ///
    /// As in Git, only a successful exclusive mkdir establishes ownership. A collided entry,
    /// even an incomplete one, belongs to someone else; pruning is a separate operation.
    fn reserve_worktree_id(
        &self,
        checkout: &std::path::Path,
        entries: &[Entry],
        name: Option<&BString>,
    ) -> Result<(BString, PathBuf), Exn<add::Error>> {
        if let Some(name) = name {
            gix_validate::path::component(name.as_bstr(), None, Default::default())
                .map_err(|err| add::Error::InvalidName(err).raise())?;
        }
        let base: BString = name.cloned().unwrap_or_else(|| {
            checkout.file_name()
                .map(|name| gix_path::into_bstr(std::path::Path::new(name)).into_owned())
                .unwrap_or_else(|| "worktree".into())
        });
        let parent = self.common_dir().join("worktrees");
        std::fs::create_dir_all(&parent).map_err(|source| {
            add::Error::Io {
                path: parent.clone(),
                source,
            }
            .raise()
        })?;
        for suffix in 0u32.. {
            let mut id = base.clone();
            if suffix != 0 {
                id.extend_from_slice(format!("{suffix}").as_bytes());
            }
            if entries
                .iter()
                .any(|entry| entry.id.to_ascii_lowercase() == id.to_ascii_lowercase())
            {
                if name.is_some() {
                    return Err(add::Error::IdentifierExists { id }.raise());
                }
                continue;
            }
            let component = gix_path::try_from_byte_slice(id.as_slice()).map_err(|error| {
                add::Error::Io {
                    path: parent.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidInput, error),
                }.raise()
            })?;
            let admin_dir = parent.join(component);
            match std::fs::create_dir(&admin_dir) {
                Ok(()) => return Ok((id, admin_dir)),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if name.is_some() {
                        return Err(add::Error::IdentifierExists { id }.raise());
                    }
                    continue;
                }
                Err(source) => {
                    return Err(add::Error::Io {
                        path: admin_dir,
                        source,
                    }
                    .raise());
                }
            }
        }
        unreachable!("the counter is exhausted only after 4 billion identically named worktrees")
    }

    /// Return the administrative directory of the linked worktree `id`, or `None` if no such
    /// directory exists.
    ///
    /// Note this reports presence of the directory only, not that the registration is intact; see
    /// [`worktree_admin_entries()`](crate::Repository::worktree_admin_entries()) for its condition.
    fn worktree_admin_dir(&self, id: &crate::bstr::BStr) -> Option<PathBuf> {
        let dir = self
            .common_dir()
            .join("worktrees")
            .join(gix_path::from_bstr(id).as_ref());
        dir.is_dir().then_some(dir)
    }
}

/// Write admin_dir/gitdir, naming the .git file inside checkout.
fn write_gitdir_pointer(
    admin_dir: &std::path::Path,
    checkout: &std::path::Path,
    relative: bool,
) -> std::io::Result<()> {
    let target = checkout.join(".git");
    let target = if relative {
        path_relative_to(admin_dir, &target)?
    } else {
        target
    };
    let mut contents =
        gix_path::to_unix_separators_on_windows(gix_path::into_bstr(target)).into_owned();
    contents.push(b'\n');
    std::fs::write(admin_dir.join("gitdir"), &contents)
}

/// Write the .git file inside checkout, naming admin_dir.
///
/// This is the other half of the two-way link, and the half git worktree repair restores when a
/// checkout has been copied or its .git file lost.
fn write_dot_git_back_pointer(
    checkout: &std::path::Path,
    admin_dir: &std::path::Path,
    relative: bool,
) -> std::io::Result<()> {
    let contents = dot_git_back_pointer_contents(checkout, admin_dir, relative)?;
    std::fs::write(checkout.join(".git"), &contents)
}

fn dot_git_back_pointer_contents(
    checkout: &std::path::Path,
    admin_dir: &std::path::Path,
    relative: bool,
) -> std::io::Result<BString> {
    let target = if relative {
        path_relative_to(checkout, admin_dir)?
    } else {
        admin_dir.to_owned()
    };
    let mut contents = BString::from("gitdir: ");
    contents.extend_from_slice(
        &gix_path::to_unix_separators_on_windows(gix_path::into_bstr(target)),
    );
    contents.push(b'\n');
    Ok(contents)
}

fn registration_paths_are_relative(admin_dir: &std::path::Path) -> bool {
    std::fs::read(admin_dir.join("gitdir"))
        .ok()
        .map(|contents| {
            gix_path::from_bstr(contents.trim().as_bstr())
                .as_ref()
                .is_relative()
        })
        .unwrap_or(false)
}

fn path_relative_to(
    from_directory: &std::path::Path,
    target: &std::path::Path,
) -> std::io::Result<PathBuf> {
    use std::path::Component;

    let from = from_directory.components().collect::<Vec<_>>();
    let to = target.components().collect::<Vec<_>>();
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(left, right)| left == right)
        .count();
    if from_directory.is_absolute() != target.is_absolute()
        || (from_directory.is_absolute() && common == 0)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "cannot make paths on different roots relative",
        ));
    }

    let mut out = PathBuf::new();
    for component in &from[common..] {
        match component {
            Component::Normal(_) | Component::ParentDir => out.push(".."),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "cannot make paths on different roots relative",
                ));
            }
        }
    }
    for component in &to[common..] {
        out.push(component.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    Ok(out)
}

fn worktree_local_branch_name(
    short_name: &crate::bstr::BStr,
) -> Result<gix_ref::FullName, Box<dyn std::error::Error + Send + Sync>> {
    let mut full = BString::from("refs/heads/");
    full.extend_from_slice(short_name);
    gix_validate::reference::branch_name(full.as_bstr())?;
    Ok(gix_ref::FullName::try_from(full)?)
}

/// Record the worktree directory under `HEAD` and every reference in its symbolic referent chain.
///
/// Do nothing if `head` is absent or its repository has no worktree, and fail if a symbolic
/// reference in the chain cannot be followed.
fn insert_head(
    head: Option<crate::Head<'_>>,
    out: &mut BTreeMap<gix_ref::FullName, Vec<PathBuf>>,
) -> Result<(), Exn<Error>> {
    let Some((head, workdir)) = head.and_then(|head| head.repo.workdir().map(|workdir| (head, workdir))) else {
        return Ok(());
    };
    out.entry("HEAD".try_into().expect("valid reference name"))
        .or_default()
        .push(workdir.to_owned());

    // A rebase or bisect detaches `HEAD` while still holding the branch it started from, so the
    // branch is unavailable even though no symbolic reference points at it. Git applies this rule
    // only to detached worktrees, and so do we.
    if head.is_detached() {
        if let Some(held) = branch_held_by_in_progress_operation(head.repo.git_dir()) {
            out.entry(held).or_default().push(workdir.to_owned());
        }
    }

    let mut cursor = head.try_into_referent();
    while let Some(reference) = cursor {
        out.entry(reference.name().to_owned())
            .or_default()
            .push(workdir.to_owned());
        cursor = reference
            .follow()
            .transpose()
            .map_err(|err| Error::FollowSymref(err).raise())?;
    }
    Ok(())
}

/// Return the branch a rebase or bisect in `git_dir` started from, if either is in progress.
///
/// Unreadable or malformed state is treated as "nothing held" rather than as an error, since a
/// half-written operation directory must not make an unrelated branch permanently unusable.
fn branch_held_by_in_progress_operation(git_dir: &Path) -> Option<gix_ref::FullName> {
    // Interactive and merge-based rebases use `rebase-merge`, `git rebase --apply` uses
    // `rebase-apply`. Both record the original branch in `head-name`; `git am` also uses
    // `rebase-apply` but writes no `head-name`, so it correctly holds nothing.
    for dir in ["rebase-merge", "rebase-apply"] {
        if let Ok(contents) = std::fs::read(git_dir.join(dir).join("head-name")) {
            if let Ok(name) = gix_ref::FullName::try_from(contents.trim().as_bstr()) {
                return Some(name);
            }
        }
    }

    // `BISECT_LOG` marks a bisect as running; `BISECT_START` names the branch it began on, or
    // holds a commit id when the bisect started from a detached head — which git's `get_branch`
    // renders as an abbreviated hash rather than a branch, so a commit id holds nothing.
    if git_dir.join("BISECT_LOG").is_file() {
        if let Ok(contents) = std::fs::read(git_dir.join("BISECT_START")) {
            let start = contents.trim();
            if !start.is_empty() && gix_hash::ObjectId::from_hex(start).is_err() {
                let mut full = BString::from("refs/heads/");
                full.extend_from_slice(start);
                if let Ok(name) = gix_ref::FullName::try_from(full.as_bstr()) {
                    return Some(name);
                }
            }
        }
    }
    None
}
