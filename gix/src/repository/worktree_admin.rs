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
    /// On any failure after the first directory is created, everything created here is removed
    /// again, so a failed call leaves no registration behind.
    ///
    /// ### Note
    ///
    /// `HEAD` is written unconditionally, as no guarded symbolic-reference update exists yet. Two
    /// concurrent calls racing for the same identifier can therefore both proceed; the identifier
    /// is derived to avoid collisions, but that derivation is not itself atomic.
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

        let id = self.unused_worktree_id(&checkout, &entries);
        let admin_dir = self
            .common_dir()
            .join("worktrees")
            .join(gix_path::from_bstr(id.as_bstr()).as_ref());

        // Everything below can fail partway; undo it rather than leaving a broken registration.
        let result = self.write_worktree_registration(&admin_dir, &checkout, &attach, options.lock.as_ref());
        if result.is_err() {
            std::fs::remove_dir_all(&admin_dir).ok();
            if !checkout_existed {
                std::fs::remove_dir_all(&checkout).ok();
            } else {
                std::fs::remove_file(checkout.join(".git")).ok();
            }
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
    /// Without `force`, a locked worktree is refused, and so is one whose checkout has changes.
    ///
    /// ### Divergence from Git
    ///
    /// The cleanliness check uses [`is_dirty()`](crate::Repository::is_dirty()), which compares
    /// index, tree and worktree but **ignores untracked files**. Git additionally refuses to
    /// remove a worktree containing untracked files. A worktree holding only untracked files is
    /// therefore removed here without `force` where Git would refuse.
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

        if !options.force {
            if let Some(reason) = &entry.lock_reason {
                return Err(Error::Locked {
                    id: entry.id.clone(),
                    reason: reason.clone(),
                }
                .raise());
            }
            // A registered worktree that was never materialised has no index, and comparing it
            // against HEAD would report every tracked file as deleted — "unpopulated" is not
            // "dirty", and there is nothing there to lose. Registration and materialisation are
            // separate steps here, so this intermediate state is normal rather than exceptional.
            if entry.condition.is_registered() && entry.admin_dir.join("index").exists() {
                let worktree_repo = crate::worktree::Proxy::new(self, entry.admin_dir.clone())
                    .into_repo_with_possibly_inaccessible_worktree()
                    .map_err(|err| {
                        Error::Status {
                            id: entry.id.clone(),
                            source: Box::new(err),
                        }
                        .raise()
                    })?;
                let dirty = worktree_repo.is_dirty().map_err(|err| {
                    Error::Status {
                        id: entry.id.clone(),
                        source: Box::new(err),
                    }
                    .raise()
                })?;
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

    /// Write every file that links `admin_dir` and `checkout` together, creating both directories.
    fn write_worktree_registration(
        &self,
        admin_dir: &std::path::Path,
        checkout: &std::path::Path,
        attach: &add::Attachment,
        lock: Option<&Option<BString>>,
    ) -> Result<(), Exn<add::Error>> {
        use add::{Attachment, Error};
        let io = |path: &std::path::Path| {
            let path = path.to_owned();
            move |source: std::io::Error| Error::Io { path, source }.raise()
        };

        std::fs::create_dir_all(admin_dir).map_err(io(admin_dir))?;
        std::fs::create_dir_all(checkout).map_err(io(checkout))?;

        // Git records an absolute path to the `.git` file, with forward slashes even on Windows.
        let dot_git = checkout.join(".git");
        let mut gitdir = gix_path::to_unix_separators_on_windows(gix_path::into_bstr(dot_git.as_path())).into_owned();
        gitdir.push(b'\n');
        std::fs::write(admin_dir.join("gitdir"), &gitdir).map_err(io(&admin_dir.join("gitdir")))?;

        // `commondir` is relative to the administrative directory, which is always two levels down.
        std::fs::write(admin_dir.join("commondir"), b"../..\n").map_err(io(&admin_dir.join("commondir")))?;

        let head = match attach {
            Attachment::Branch(name) => {
                let mut head = BString::from("ref: ");
                head.extend_from_slice(name.as_bstr());
                head.push(b'\n');
                head
            }
            Attachment::DetachedAt(id) => {
                let mut head = BString::from(id.to_string());
                head.push(b'\n');
                head
            }
        };
        std::fs::write(admin_dir.join("HEAD"), &head).map_err(io(&admin_dir.join("HEAD")))?;

        if let Some(reason) = lock {
            let mut contents = reason.clone().unwrap_or_default();
            if !contents.is_empty() && !contents.ends_with(b"\n") {
                contents.push(b'\n');
            }
            std::fs::write(admin_dir.join("locked"), &contents).map_err(io(&admin_dir.join("locked")))?;
        }

        // The checkout points back with an absolute path, completing the two-way link.
        let mut back_pointer = BString::from("gitdir: ");
        back_pointer.extend_from_slice(&gix_path::to_unix_separators_on_windows(gix_path::into_bstr(admin_dir)));
        back_pointer.push(b'\n');
        std::fs::write(&dot_git, &back_pointer).map_err(io(&dot_git))?;
        Ok(())
    }

    /// Derive an administrative identifier from `checkout` which no existing entry uses.
    ///
    /// Git names the entry after the checkout's final component and disambiguates with a counter.
    /// Comparison is case-insensitive so that two worktrees differing only in case cannot collide
    /// on a case-insensitive filesystem.
    fn unused_worktree_id(&self, checkout: &std::path::Path, entries: &[Entry]) -> BString {
        let base: BString = checkout
            .file_name()
            .map(|name| gix_path::into_bstr(std::path::Path::new(name)).into_owned())
            .unwrap_or_else(|| "worktree".into());
        let taken = |candidate: &BString| {
            entries
                .iter()
                .any(|entry| entry.id.to_ascii_lowercase() == candidate.to_ascii_lowercase())
        };
        if !taken(&base) {
            return base;
        }
        for suffix in 1u32.. {
            let mut candidate = base.clone();
            candidate.extend_from_slice(format!("{suffix}").as_bytes());
            if !taken(&candidate) {
                return candidate;
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
    // holds a commit id when the bisect started from a detached head.
    if git_dir.join("BISECT_LOG").is_file() {
        if let Ok(contents) = std::fs::read(git_dir.join("BISECT_START")) {
            let start = contents.trim();
            if !start.is_empty() {
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
