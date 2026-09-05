use crate::{
    Target,
    store_impl::file::{Transaction, transaction::PackedRefs},
    transaction::{Change, LogChange, RefEdit, RefLog},
};

impl Transaction<'_, '_> {
    /// Make all [prepared][Transaction::prepare()] permanent and return the performed edits which represent the current
    /// state of the affected refs in the ref store in that instant. Please note that the obtained edits may have been
    /// adjusted to contain more dependent edits or additional information.
    /// `committer` is optional when none of the edits writes a reflog. Reflog requirements, identity encoding and
    /// messages are validated for all edits before publication starts.
    ///
    /// On error the transaction may have been performed partially, depending on the nature of the error, and no attempt to roll back
    /// partial changes is made. [Error::PartialCommit] records prepared edits, completed loose-ref operations,
    /// attempted reflog operations (which may themselves be partial), and whether packed refs were committed.
    ///
    /// In this stage, we perform the following operations:
    ///
    /// * update the ref log
    /// * move updated refs into place
    /// * delete reflogs and empty parent directories
    /// * delete packed refs
    /// * delete their corresponding reference (if applicable)
    ///   along with empty parent directories
    ///
    /// Call [prepare()][Transaction::prepare()] before this method.
    pub fn commit<'a>(self, committer: impl Into<Option<gix_actor::SignatureRef<'a>>>) -> Result<Vec<RefEdit>, Error> {
        self.commit_inner(committer.into())
    }

    fn commit_inner(self, committer: Option<gix_actor::SignatureRef<'_>>) -> Result<Vec<RefEdit>, Error> {
        let mut updates = self.updates.expect("BUG: must call prepare before commit");
        let delete_loose_refs = matches!(
            self.packed_refs,
            PackedRefs::DeletionsAndNonSymbolicUpdatesRemoveLooseSourceReference(_)
        );

        // All input validation precedes publication, including edits without reflogs.
        let mut reflog_updates = Vec::with_capacity(updates.len());
        for change in &updates {
            let write_reflog = match (&change.update.change, change.reflog_update()) {
                (Change::Update { log, .. }, Some((previous, new_oid))) if previous.as_ref() != Some(new_oid) => {
                    self.store.reflog_validate(
                        change.update.name.as_ref(), committer, log.message.as_ref(), log.force_create_reflog,
                    ).map_err(|source| Error::RefLogValidation {
                        source, full_name: change.name(),
                    })?
                }
                _ => false,
            };
            reflog_updates.push(write_reflog);
        }

        let mut completed_refs = Vec::new();
        let mut attempted_reflogs = Vec::new();
        let mut packed_refs_committed = false;
        let result = (|| -> Result<(), Error> {
            // Perform updates first so live commits remain referenced.
            for (change, write_reflog) in updates.iter_mut().zip(reflog_updates) {
                assert!(!change.update.deref, "Deref mode is turned into splits and turned off");
                match &change.update.change {
                    // reflog first, then reference
                    Change::Update { log, new, expected } => {
                        let lock = change.lock.take();
                        let update_ref = log.mode == RefLog::AndReference;
                        if write_reflog {
                            if let Some((previous, new_oid)) = change.reflog_update() {
                                attempted_reflogs.push(change.update.name.clone());
                                self.store.reflog_create_or_append(
                                    change.update.name.as_ref(), previous, new_oid, committer,
                                    log.message.as_ref(), log.force_create_reflog,
                                )?;
                            }
                        }
                        // Don't do anything else while keeping the lock after potentially updating the reflog.
                        // We delay deletion of the reference and dropping the lock to after the packed-refs were
                        // safely written.
                        if delete_loose_refs && matches!(new, Target::Object(_)) {
                            change.lock = lock;
                            continue;
                        }
                        if !update_ref || matches!(expected, PreviousValue::MustExistAndMatch(previous) if previous == new) {
                            // Verification and symbolic-parent locks protect the remaining
                            // edits too. Release them only when the transaction ends.
                            change.lock = lock;
                            continue;
                        }
                        if let Some(lock) = lock {
                            if let Err(err) = lock.commit() {
                                // TODO: when Kind::IsADirectory becomes stable, use that.
                                let err = if err.instance.resource_path().is_dir() {
                                    gix_tempfile::remove_dir::empty_depth_first(err.instance.resource_path())
                                        .map_err(std::io::Error::other)
                                        .and_then(|_| err.instance.commit().map_err(|err| err.error))
                                        .err()
                                } else {
                                    Some(err.error)
                                };

                                if let Some(err) = err {
                                    return Err(Error::LockCommit {
                                        source: err,
                                        full_name: change.name(),
                                    });
                                }
                            }
                            completed_refs.push(change.update.name.clone());
                        }
                    }
                    Change::Delete { .. } => {}
                }
            }

            for change in &mut updates {
                let (reflog_root, relative_name) = self.store.reflog_base_and_relative_path(change.update.name.as_ref());
                match &change.update.change {
                    Change::Update { .. } => {}
                    Change::Delete { .. } => {
                        // Reflog deletion happens first in case it fails a ref without log is less terrible than
                        // a log without a reference.
                        let reflog_path = reflog_root.join(relative_name);
                        attempted_reflogs.push(change.update.name.clone());
                        if let Err(err) = std::fs::remove_file(&reflog_path) {
                            if err.kind() != std::io::ErrorKind::NotFound {
                                return Err(Error::DeleteReflog {
                                    source: err,
                                    full_name: change.name(),
                                });
                            }
                        } else {
                            gix_tempfile::remove_dir::empty_upward_until_boundary(
                                reflog_path.parent().expect("never without parent"),
                                &reflog_root,
                            )
                            .ok();
                        }
                    }
                }
            }

            if let Some(t) = self.packed_transaction {
                t.commit().map_err(Error::PackedTransactionCommit)?;
                packed_refs_committed = true;
                // Always refresh ourselves right away to avoid races. We ignore errors as there may be many reasons this fails, and it's not
                // critical to be done here. In other words, the pack may be refreshed at a later time and then it might work.
                self.store.force_refresh_packed_buffer().ok();
            }

            for change in &mut updates {
                let take_lock_and_delete = match &change.update.change {
                    Change::Update {
                        log: LogChange { mode, .. },
                        new,
                        ..
                    } => delete_loose_refs && *mode == RefLog::AndReference && matches!(new, Target::Object(_)),
                    Change::Delete { log: mode, .. } => *mode == RefLog::AndReference,
                };
                if take_lock_and_delete {
                    let lock = change.lock.take();
                    let reference_path = self.store.reference_path(change.update.name.as_ref());
                    if let Err(err) = std::fs::remove_file(reference_path) {
                        if err.kind() != std::io::ErrorKind::NotFound {
                            return Err(Error::DeleteReference {
                                err,
                                full_name: change.name(),
                            });
                        }
                    }
                    completed_refs.push(change.update.name.clone());
                    drop(lock);
                }
            }
            Ok(())
        })();
        let edits = updates.into_iter().map(|edit| edit.update).collect();
        match result {
            Ok(()) => Ok(edits),
            Err(source) if completed_refs.is_empty() && attempted_reflogs.is_empty() && !packed_refs_committed => {
                Err(source)
            }
            Err(source) => Err(Error::PartialCommit {
                source: Box::new(source), edits, completed_refs, attempted_reflogs, packed_refs_committed,
            }),
        }
    }
}
mod error {
    use gix_object::bstr::BString;

    use crate::store_impl::{file, packed};

    /// The error returned by various [`Transaction`][super::Transaction] methods.
    #[derive(Debug, thiserror::Error)]
    #[expect(missing_docs)]
    pub enum Error {
        #[error("Reflog input for reference {full_name:?} is invalid")]
        RefLogValidation {
            #[source]
            source: file::log::create_or_update::Error,
            full_name: BString,
        },
        #[error("The ref transaction failed during publication: {source}")]
        PartialCommit {
            #[source]
            source: Box<Error>,
            /// All prepared edits, including the prior values checked under lock.
            edits: Vec<crate::transaction::RefEdit>,
            /// Loose ref writes or deletions that completed successfully.
            completed_refs: Vec<crate::FullName>,
            /// Reflog operations attempted; inspect these files for partial writes.
            attempted_reflogs: Vec<crate::FullName>,
            /// Whether the packed-ref file was successfully committed.
            packed_refs_committed: bool,
        },
        #[error("The packed-ref transaction could not be committed")]
        PackedTransactionCommit(#[source] packed::transaction::commit::Error),
        #[error("Edit preprocessing failed with error")]
        PreprocessingFailed { source: std::io::Error },
        #[error("The change for reference {full_name:?} could not be committed")]
        LockCommit { source: std::io::Error, full_name: BString },
        #[error("The reference {full_name} could not be deleted")]
        DeleteReference { full_name: BString, err: std::io::Error },
        #[error("The reflog of reference {full_name:?} could not be deleted")]
        DeleteReflog { full_name: BString, source: std::io::Error },
        #[error("The reflog could not be created or updated")]
        CreateOrUpdateRefLog(#[from] file::log::create_or_update::Error),
    }
}
pub use error::Error;

use crate::transaction::PreviousValue;
