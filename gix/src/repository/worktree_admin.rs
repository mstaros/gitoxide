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
    /// The checkout directory exists, but its `.git` file does not, so the link is one-way.
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
                    } else if !checkout.join(".git").exists() {
                        Condition::CheckoutNotLinked
                    } else {
                        Condition::Registered
                    };
                    (condition, Some(checkout))
                }
            };

            out.push(Entry {
                id: proxy.id().to_owned(),
                gitdir_modified: std::fs::metadata(admin_dir.join("gitdir"))
                    .and_then(|meta| meta.modified())
                    .ok(),
                lock_reason: proxy.lock_reason(),
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
        /// Attach `HEAD` to an existing local branch, which must not be in use by another worktree.
        Branch(gix_ref::FullName),
        /// Leave `HEAD` detached at a commit, as `git worktree add --detach` does.
        DetachedAt(gix_hash::ObjectId),
    }

    /// The full option surface of `git worktree add`.
    ///
    /// Options which this phase does not yet implement are present but rejected with
    /// [`Error::Unsupported`] rather than silently ignored, so a caller can never believe it asked
    /// for something it did not get.
    #[derive(Debug, Default, Clone)]
    pub struct Options {
        /// `--force`: permit a non-empty target directory, and a branch already checked out.
        pub force: bool,
        /// `-b <name>`: create a new branch for the worktree.
        pub new_branch: Option<BString>,
        /// `-B <name>`: create or reset a branch for the worktree.
        pub new_branch_force: Option<BString>,
        /// `--lock`, with the optional `--reason`.
        pub lock: Option<Option<BString>>,
        /// `--checkout`: materialise the working tree.
        ///
        /// Registration and materialisation are separate calls here, mirroring `git worktree add
        /// --no-checkout` followed by `git checkout`, so requesting it from this method is refused.
        pub checkout: bool,
        /// `--orphan`: start from an unborn branch.
        pub orphan: bool,
        /// `--track` / `--no-track`.
        pub track: Option<bool>,
        /// `--guess-remote`.
        pub guess_remote: bool,
        /// `--relative-paths`: record `gitdir` and `commondir` relative rather than absolute.
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
        /// The checkout directory, which exists but has not been populated.
        pub checkout: PathBuf,
    }

    /// The error returned by [`add_worktree()`](crate::Repository::add_worktree()).
    #[derive(Debug, thiserror::Error)]
    #[expect(missing_docs)]
    pub enum Error {
        #[error("`{option}` is accepted for compatibility with `git worktree add` but is not implemented yet")]
        Unsupported { option: &'static str },
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

    /// Register a new linked worktree at `path` attached per `attach`, **without** materialising
    /// its working tree, and return what was created.
    ///
    /// This is the first of the three steps `git worktree add` performs, and corresponds to
    /// `git worktree add --no-checkout`: it writes the administrative directory under
    /// `worktrees/`, the `.git` file in the checkout, and the pointers linking them. Configuring
    /// sparse checkout and materialising the tree are separate calls, so that a caller can
    /// configure a cone before any file is written — and so an interrupted sequence leaves a
    /// registered but empty worktree that can be finished rather than a half-populated one.
    ///
    /// The target must be absent or an empty directory. A path already registered as a worktree of
    /// this repository is reported as such rather than being taken over, and a branch already in
    /// use by another worktree — including one held by an in-progress rebase or bisect — is
    /// refused, matching Git.
    ///
    /// Administrative directories and the checkout's `.git` file are reserved exclusively.
    /// Identifier collisions are retried with a numeric suffix, including incomplete registrations.
    /// On failure, cleanup removes only paths reserved by this call. A newly created checkout is
    /// removed only if it is still empty, preserving files concurrently placed there by others.
    pub fn add_worktree(
        &self,
        path: &std::path::Path,
        attach: add::Attachment,
        options: add::Options,
    ) -> Result<add::Outcome, Exn<add::Error>> {
        use add::{Attachment, Error};

        for (requested, option) in [
            (options.force, "--force"),
            (options.new_branch.is_some(), "-b"),
            (options.new_branch_force.is_some(), "-B"),
            (options.checkout, "--checkout"),
            (options.orphan, "--orphan"),
            (options.track.is_some(), "--track/--no-track"),
            (options.guess_remote, "--guess-remote"),
            (options.relative_paths, "--relative-paths"),
        ] {
            if requested {
                return Err(Error::Unsupported { option }.raise());
            }
        }

        let checkout = if path.is_absolute() {
            path.to_owned()
        } else {
            self.workdir().unwrap_or(self.common_dir()).join(path)
        };

        // Classify the target: registered here, foreign, or usable.
        let entries = self.worktree_admin_entries().map_err(|err| {
            Error::Reservation(err.into_inner()).raise()
        })?;
        if let Some(entry) = entries
            .iter()
            .find(|entry| entry.checkout.as_deref() == Some(checkout.as_path()))
        {
            return Err(Error::AlreadyRegistered {
                path: checkout,
                id: entry.id.clone(),
            }
            .raise());
        }
        let checkout_existed = checkout.is_dir();
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

        // Refuse a branch another worktree is using, exactly as `git worktree add` does.
        if let Attachment::Branch(branch) = &attach {
            let in_use = self
                .checked_out_branches()
                .map_err(|err| Error::Reservation(err.into_inner()).raise())?;
            if let Some(worktree_dirs) = in_use.get(branch) {
                return Err(Error::BranchInUse {
                    branch: branch.clone(),
                    worktree_dirs: worktree_dirs.clone(),
                }
                .raise());
            }
        }

        let (id, admin_dir) = self.reserve_worktree_id(&checkout, &entries)?;
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
            self.write_worktree_registration(&admin_dir, &checkout, &attach, options.lock.as_ref(), &mut dot_git)
        })();
        if result.is_err() {
            if dot_git_created {
                std::fs::remove_file(&dot_git_path).ok();
            }
            if checkout_created {
                // Leave any files concurrently added by someone else intact.
                std::fs::remove_dir(&checkout).ok();
            }
            // The exclusive mkdir above, not a directory listing, established ownership.
            std::fs::remove_dir_all(&admin_dir).ok();
        }
        result?;

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

        if !options.force {
            if let Some(reason) = &entry.lock_reason {
                return Err(Error::Locked {
                    id: entry.id.clone(),
                    reason: reason.clone(),
                }
                .raise());
            }
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
    /// missing, or names a checkout that is gone or no longer links back. Locked entries are never
    /// candidates, whatever their condition, since a lock exists precisely to survive this.
    ///
    /// With [`expire`](prune::Options::expire), only entries whose `gitdir` file is older than the
    /// given time are removed; entries whose age cannot be determined are left alone rather than
    /// assumed old. With [`dry_run`](prune::Options::dry_run) nothing is removed and every
    /// candidate is returned with `removed: false`, which is also how a caller reconciles this
    /// against its own record of which worktrees are live before allowing any deletion.
    pub fn prune_worktrees(&self, options: prune::Options) -> Result<Vec<prune::Candidate>, Exn<prune::Error>> {
        let mut out = Vec::new();
        for entry in self
            .worktree_admin_entries()
            .map_err(|err| prune::Error::Listing(err.into_inner()).raise())?
        {
            if entry.condition.is_registered() || entry.is_locked() {
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
                    write_dot_git_back_pointer(&checkout, &entry.admin_dir)
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
                        write_gitdir_pointer(&entry.admin_dir, new_checkout)
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
        write_gitdir_pointer(&entry.admin_dir, destination)
            .and_then(|()| write_dot_git_back_pointer(destination, &entry.admin_dir))
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
        lock: Option<&Option<BString>>,
        dot_git_file: &mut std::fs::File,
    ) -> Result<(), Exn<add::Error>> {
        use add::Error;
        use std::io::Write;
        let io = |path: &std::path::Path| {
            let path = path.to_owned();
            move |source: std::io::Error| Error::Io { path, source }.raise()
        };

        let dot_git = checkout.join(".git");
        write_gitdir_pointer(admin_dir, checkout).map_err(io(&admin_dir.join("gitdir")))?;

        // `commondir` is relative to the administrative directory, which is always two levels down.
        std::fs::write(admin_dir.join("commondir"), b"../..\n").map_err(io(&admin_dir.join("commondir")))?;

        self.write_worktree_head(admin_dir, attach)?;

        if let Some(reason) = lock {
            let mut contents = reason.clone().unwrap_or_default();
            if !contents.is_empty() && !contents.ends_with(b"\n") {
                contents.push(b'\n');
            }
            std::fs::write(admin_dir.join("locked"), &contents).map_err(io(&admin_dir.join("locked")))?;
        }

        // The checkout points back at us, completing the two-way link.
        let mut contents = BString::from("gitdir: ");
        contents.extend_from_slice(&gix_path::to_unix_separators_on_windows(gix_path::into_bstr(admin_dir)));
        contents.push(b'\n');
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
                let tip = self
                    .find_reference(name.as_ref())
                    .ok()
                    .and_then(|mut reference| reference.peel_to_id().ok())
                    .map(|id| id.detach());
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

    /// Reserve an administrative directory derived from the checkout's final component.
    ///
    /// As in Git, only a successful exclusive mkdir establishes ownership. A collided entry,
    /// even an incomplete one, belongs to someone else; pruning is a separate operation.
    fn reserve_worktree_id(
        &self,
        checkout: &std::path::Path,
        entries: &[Entry],
    ) -> Result<(BString, PathBuf), Exn<add::Error>> {
        let base: BString = checkout
            .file_name()
            .map(|name| gix_path::into_bstr(std::path::Path::new(name)).into_owned())
            .unwrap_or_else(|| "worktree".into());
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
                continue;
            }
            let admin_dir = parent.join(gix_path::from_bstr(id.as_bstr()).as_ref());
            match std::fs::create_dir(&admin_dir) {
                Ok(()) => return Ok((id, admin_dir)),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
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

/// Write `admin_dir/gitdir`, naming the `.git` file inside `checkout`.
///
/// Git records an absolute path here, with forward slashes even on Windows.
fn write_gitdir_pointer(admin_dir: &std::path::Path, checkout: &std::path::Path) -> std::io::Result<()> {
    let mut contents =
        gix_path::to_unix_separators_on_windows(gix_path::into_bstr(checkout.join(".git"))).into_owned();
    contents.push(b'\n');
    std::fs::write(admin_dir.join("gitdir"), &contents)
}

/// Write the `.git` file inside `checkout`, naming `admin_dir`.
///
/// This is the other half of the two-way link, and the half `git worktree repair` restores when a
/// checkout has been copied or its `.git` file lost.
fn write_dot_git_back_pointer(checkout: &std::path::Path, admin_dir: &std::path::Path) -> std::io::Result<()> {
    let mut contents = BString::from("gitdir: ");
    contents.extend_from_slice(&gix_path::to_unix_separators_on_windows(gix_path::into_bstr(admin_dir)));
    contents.push(b'\n');
    std::fs::write(checkout.join(".git"), &contents)
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
