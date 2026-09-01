use gix_ref::bstr;

/// The buffer length for SHA1 archives.
#[cfg(target_pointer_width = "64")]
#[cfg(feature = "worktree-stream")]
const EXPECTED_BUFFER_LENGTH: usize = 102;
/// The buffer length for SHA1 archives on 32bit machines.
#[cfg(target_pointer_width = "32")]
#[cfg(feature = "worktree-stream")]
const EXPECTED_BUFFER_LENGTH: usize = 86;

#[cfg(feature = "worktree-stream")]
fn expected_buffer_length(repo: &gix::Repository) -> usize {
    EXPECTED_BUFFER_LENGTH + repo.object_hash().len_in_hex() - gix::hash::Kind::Sha1.len_in_hex()
}

#[test]
#[cfg(feature = "worktree-stream")]
fn stream() -> crate::Result {
    let repo = crate::named_repo("make_packed_and_loose.sh")?;
    let mut stream = repo.worktree_stream(repo.head_commit()?.tree_id()?)?.0.into_read();
    assert_eq!(
        std::io::copy(&mut stream, &mut std::io::sink())?,
        expected_buffer_length(&repo) as u64,
        "there is some content in the stream, it works"
    );
    Ok(())
}

#[test]
#[cfg(feature = "worktree-archive")]
fn archive() -> crate::Result {
    let repo = crate::named_repo("make_packed_and_loose.sh")?;
    let (stream, _index) = repo.worktree_stream(repo.head_commit()?.tree_id()?)?;
    let mut buf = Vec::<u8>::new();

    repo.worktree_archive(
        stream,
        std::io::Cursor::new(&mut buf),
        gix_features::progress::Discard,
        &std::sync::atomic::AtomicBool::default(),
        Default::default(),
    )?;
    assert_eq!(buf.len(), expected_buffer_length(&repo), "default format is internal");
    Ok(())
}

/// Pins the fall-through in `gix_ref::file::Store::to_base_dir_and_relative_name`: a name whose
/// category is unknown resolves against the *common* directory, so custom namespaces are shared
/// by every worktree instead of being worktree-private. Tools that key coordination state on
/// namespaces like `refs/guarded/*` depend on this, and a future `Category` variant could
/// silently reroute it.
#[test]
fn custom_ref_namespace_created_in_linked_worktree_is_common() -> crate::Result {
    let fixture = gix_testtools::scripted_fixture_writable("make_worktree_repo.sh")?;
    let linked = gix::open_opts(fixture.path().join("wt-a"), crate::restricted())?;
    assert_eq!(
        linked.kind(),
        gix::repository::Kind::LinkedWorkTree,
        "precondition: the fixture hands us a linked worktree, not the main one"
    );

    let target = linked.head_id()?.detach();
    linked.reference(
        "refs/guarded/marker",
        target,
        gix::refs::transaction::PreviousValue::MustNotExist,
        "create custom-namespace marker",
    )?;

    assert!(
        linked.common_dir().join("refs").join("guarded").join("marker").is_file(),
        "uncategorised names are written to the common ref store"
    );
    assert!(
        !linked.git_dir().join("refs").join("guarded").join("marker").exists(),
        "and never into the worktree-private one"
    );
    assert_eq!(
        linked.main_repo()?.find_reference("refs/guarded/marker")?.id().detach(),
        target,
        "so the main worktree resolves it too"
    );
    Ok(())
}

/// `worktrees()` answers "which worktrees can I use" and drops anything without a `gitdir` file.
/// Administration needs the opposite: every registered directory, including the broken ones,
/// since those are exactly what pruning removes.
#[test]
fn worktree_admin_entries_report_broken_registrations_that_worktrees_hides() -> crate::Result {
    use gix::repository::worktree_admin::Condition;

    let fixture = gix_testtools::scripted_fixture_writable("make_worktree_repo.sh")?;
    let repo = gix::open_opts(fixture.path().join("repo"), crate::restricted())?;

    let admin = |repo: &gix::Repository| -> crate::Result<Vec<(String, Condition)>> {
        Ok(repo
            .worktree_admin_entries()?
            .into_iter()
            .map(|entry| (entry.id.to_string(), entry.condition))
            .collect())
    };

    // The fixture creates `wt-deleted` and then removes its checkout, leaving the registration
    // behind. That is a prunable entry which the filtered view cannot distinguish from a healthy
    // one, because the `gitdir` file it filters on is still present.
    let baseline = admin(&repo)?;
    assert_eq!(
        baseline
            .iter()
            .find(|(id, _)| id == "wt-deleted")
            .map(|(_, condition)| condition),
        Some(&Condition::CheckoutMissing),
        "a checkout deleted behind git's back is reported as such: {baseline:?}"
    );
    assert!(
        repo.worktrees()?.iter().any(|proxy| proxy.id() == "wt-deleted"),
        "yet worktrees() lists it, since its gitdir file survived"
    );

    let entries = repo.worktree_admin_entries()?;
    let locked = entries
        .iter()
        .find(|entry| entry.is_locked())
        .expect("the fixture locks one worktree");
    assert_eq!(
        locked.id, "wt-c-locked",
        "lock state is reported without needing a second call"
    );
    assert!(
        locked.gitdir_modified.is_some(),
        "the staleness signal is available for every readable entry"
    );

    // Break one entry the way an interrupted removal would: point its `gitdir` at a checkout
    // that is not there. Note we must not delete the real checkout — a linked worktree records an
    // absolute path, so a "writable" copy of this fixture still points back at the shared
    // read-only one, and removing it would poison every other test that uses it.
    let admin_dir = repo.common_dir().join("worktrees").join("wt-a");
    std::fs::write(admin_dir.join("gitdir"), b"/vanished/wt-a/.git\n")?;

    let after_removal = admin(&repo)?;
    assert_eq!(
        after_removal
            .iter()
            .find(|(id, _)| id == "wt-a")
            .map(|(_, condition)| condition),
        Some(&Condition::CheckoutMissing),
        "a checkout that vanished is reported rather than skipped"
    );
    assert!(
        repo.worktrees()?.iter().any(|proxy| proxy.id() == "wt-a"),
        "worktrees() still lists it, because its gitdir file is intact"
    );

    // Break another the way a partial `worktree add` would: no `gitdir` file at all.
    std::fs::remove_file(repo.common_dir().join("worktrees").join("wt-b").join("gitdir"))?;

    let after_gitdir_loss = admin(&repo)?;
    assert_eq!(
        after_gitdir_loss
            .iter()
            .find(|(id, _)| id == "wt-b")
            .map(|(_, condition)| condition),
        Some(&Condition::MissingGitdir),
        "an entry with no gitdir file is reported"
    );
    assert!(
        !repo.worktrees()?.iter().any(|proxy| proxy.id() == "wt-b"),
        "worktrees() silently drops it, which is why administration cannot rely on that view"
    );

    Ok(())
}

/// `lock` and `unlock` are the two administrative writes that do not touch the checkout, so they
/// are checked against a fixture Git itself locked, and read back through the same accessor
/// `worktrees()` uses.
#[test]
fn worktrees_can_be_locked_and_unlocked() -> crate::Result {
    let fixture = gix_testtools::scripted_fixture_writable("make_worktree_repo.sh")?;
    let repo = gix::open_opts(fixture.path().join("repo"), crate::restricted())?;
    let lock_reason = |repo: &gix::Repository, id: &str| -> Option<String> {
        repo.worktree_admin_entries()
            .expect("entries are readable")
            .into_iter()
            .find(|entry| entry.id == id)
            .expect("the worktree is registered")
            .lock_reason
            .map(|reason| reason.to_string())
    };

    // The fixture locks `wt-c-locked` with `git worktree lock --reason`, so we start by reading
    // what Git wrote rather than only what we write ourselves.
    assert_eq!(
        lock_reason(&repo, "wt-c-locked").as_deref(),
        Some("added with --lock"),
        "a reason written by git is read back verbatim"
    );

    repo.unlock_worktree("wt-c-locked".into())?;
    assert_eq!(
        lock_reason(&repo, "wt-c-locked"),
        None,
        "unlocking clears it"
    );
    assert!(
        repo.unlock_worktree("wt-c-locked".into()).is_err(),
        "unlocking twice is an error, as in git, so a caller cannot mistake a no-op for success"
    );

    repo.lock_worktree("wt-a".into(), Some("held for a test".into()))?;
    assert_eq!(
        lock_reason(&repo, "wt-a").as_deref(),
        Some("held for a test"),
        "our own reason round-trips through the same reader"
    );
    assert!(
        repo.lock_worktree("wt-a".into(), None).is_err(),
        "locking an already-locked worktree is an error rather than a silent replacement"
    );

    repo.unlock_worktree("wt-a".into())?;
    repo.lock_worktree("wt-a".into(), None)?;
    assert_eq!(
        lock_reason(&repo, "wt-a").as_deref(),
        Some(""),
        "a lock without a reason is still a lock, with an empty reason"
    );

    assert!(
        repo.lock_worktree("no-such-worktree".into(), None).is_err(),
        "an unregistered name is rejected"
    );
    Ok(())
}

/// The load-bearing direction: Git must accept what we register. Reading our own writes proves
/// only self-consistency, so this shells out to `git worktree list` and to `git status` inside the
/// new checkout.
#[test]
fn added_worktrees_are_accepted_by_git() -> crate::Result {
    use gix::repository::worktree_admin::add;

    let fixture = gix_testtools::scripted_fixture_writable("make_worktree_repo.sh")?;
    let repo = gix::open_opts(fixture.path().join("repo"), crate::restricted())?;
    let target = fixture.path().join("registered-by-gix");

    let outcome = repo.add_worktree(
        &target,
        add::Attachment::DetachedAt(repo.head_id()?.detach()),
        add::Options::default(),
    )?;
    assert_eq!(outcome.id, "registered-by-gix", "the id follows the directory name");
    assert!(
        outcome.admin_dir.join("gitdir").is_file()
            && outcome.admin_dir.join("commondir").is_file()
            && outcome.admin_dir.join("HEAD").is_file(),
        "the administrative files are all written"
    );
    assert!(
        outcome.checkout.join(".git").is_file(),
        "and the checkout points back at them"
    );

    // Git's own view is the real test.
    let listed = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(fixture.path().join("repo"))
        .output()?;
    assert!(listed.status.success(), "git worktree list succeeds");
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("registered-by-gix"),
        "git lists the worktree we registered"
    );

    let status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&outcome.checkout)
        .output()?;
    assert!(
        status.status.success(),
        "git status works inside a checkout we registered: {}",
        String::from_utf8_lossy(&status.stderr)
    );

    // Registering the same path twice is refused rather than silently taking it over.
    assert!(
        repo.add_worktree(
            &target,
            add::Attachment::DetachedAt(repo.head_id()?.detach()),
            add::Options::default(),
        )
        .is_err(),
        "a path already registered here is reported, not reused"
    );

    // A branch another worktree holds is refused, as git does.
    let held: gix::refs::FullName = "refs/heads/wt-a".try_into()?;
    assert!(
        repo.add_worktree(
            &fixture.path().join("would-steal-wt-a"),
            add::Attachment::Branch(held),
            add::Options::default(),
        )
        .is_err(),
        "a branch checked out elsewhere cannot be taken"
    );

    // Options we accept for shape but do not implement must say so rather than be ignored.
    let unsupported = repo.add_worktree(
        &fixture.path().join("unsupported"),
        add::Attachment::DetachedAt(repo.head_id()?.detach()),
        add::Options {
            checkout: true,
            ..Default::default()
        },
    );
    assert!(
        unsupported.is_err(),
        "requesting materialisation from the registration step is refused explicitly"
    );
    assert!(
        !fixture.path().join("unsupported").exists(),
        "and nothing was created before the refusal"
    );
    Ok(())
}

/// `prune` must find exactly what is broken and leave everything else alone, and `remove` must be
/// safe to retry. The fixture ships one already-broken entry and one locked one, which is the
/// distinction that matters: a lock outranks brokenness.
#[test]
fn broken_worktrees_are_pruned_and_removal_is_idempotent() -> crate::Result {
    use gix::repository::worktree_admin::{add, prune, remove};

    let fixture = gix_testtools::scripted_fixture_writable("make_worktree_repo.sh")?;
    let repo = gix::open_opts(fixture.path().join("repo"), crate::restricted())?;

    // Lock the entry the fixture already broke, so it is both prunable and protected.
    repo.lock_worktree("wt-deleted".into(), Some("keep me".into()))?;
    let dry = repo.prune_worktrees(prune::Options {
        dry_run: true,
        ..Default::default()
    })?;
    assert!(
        dry.iter().all(|candidate| candidate.id != "wt-deleted"),
        "a locked entry is never a candidate, however broken: {dry:?}"
    );
    repo.unlock_worktree("wt-deleted".into())?;

    let dry = repo.prune_worktrees(prune::Options {
        dry_run: true,
        ..Default::default()
    })?;
    assert_eq!(
        dry.iter().map(|c| c.id.to_string()).collect::<Vec<_>>(),
        vec!["wt-deleted".to_string()],
        "unlocked, it is the only broken entry the fixture has"
    );
    assert!(
        dry.iter().all(|candidate| !candidate.removed),
        "a dry run reports without removing"
    );
    assert!(
        repo.common_dir().join("worktrees").join("wt-deleted").is_dir(),
        "and really did not remove it"
    );

    // An expiry in the distant past protects everything, since nothing is that old.
    let ancient = std::time::SystemTime::UNIX_EPOCH;
    assert!(
        repo.prune_worktrees(prune::Options {
            dry_run: true,
            expire: Some(ancient),
        })?
        .is_empty(),
        "an expiry older than every entry excludes them all"
    );

    let pruned = repo.prune_worktrees(prune::Options::default())?;
    assert_eq!(pruned.len(), 1, "one entry was actually removed");
    assert!(pruned[0].removed);
    assert!(
        !repo.common_dir().join("worktrees").join("wt-deleted").is_dir(),
        "and it is gone from disk"
    );

    // Removal of a healthy worktree we registered ourselves, then a retry.
    let target = fixture.path().join("to-be-removed");
    let outcome = repo.add_worktree(
        &target,
        add::Attachment::DetachedAt(repo.head_id()?.detach()),
        add::Options::default(),
    )?;
    let removed = repo.remove_worktree(outcome.id.as_ref(), remove::Options::default())?;
    assert_eq!(
        removed,
        remove::Outcome {
            registration_removed: true,
            checkout_removed: true
        },
        "both halves go"
    );
    assert!(!target.exists() && !outcome.admin_dir.exists());

    assert_eq!(
        repo.remove_worktree(outcome.id.as_ref(), remove::Options::default())?,
        remove::Outcome {
            registration_removed: false,
            checkout_removed: false
        },
        "removing again succeeds and reports that nothing was left, so a retry after an \
         interruption does not have to parse an error to know it is done"
    );

    // A locked worktree is refused without force, and yields to it.
    let locked = repo.add_worktree(
        &fixture.path().join("locked-one"),
        add::Attachment::DetachedAt(repo.head_id()?.detach()),
        add::Options {
            lock: Some(Some("held".into())),
            ..Default::default()
        },
    )?;
    assert!(
        repo.remove_worktree(locked.id.as_ref(), remove::Options::default())
            .is_err(),
        "a lock is honoured"
    );
    assert!(
        repo.remove_worktree(locked.id.as_ref(), remove::Options { force: true })?
            .registration_removed,
        "and force overrides it"
    );
    Ok(())
}

mod with_core_worktree_config {
    use std::io::BufRead;

    #[test]
    #[cfg(feature = "index")]
    fn relative() -> crate::Result {
        for (name, is_relative) in [("absolute-worktree", false), ("relative-worktree", true)] {
            let repo = repo(name);

            if is_relative {
                assert_eq!(
                    repo.workdir().unwrap(),
                    repo.git_dir().parent().unwrap().parent().unwrap().join("worktree"),
                    "{name}|{is_relative}: work_dir is set to core.worktree config value, relative paths are appended to `git_dir() and made absolute`"
                );
            } else {
                assert_eq!(
                    repo.workdir().unwrap(),
                    gix_path::realpath(repo.git_dir().parent().unwrap().parent().unwrap().join("worktree"))?,
                    "absolute workdirs are left untouched"
                );
            }

            assert_eq!(
                repo.worktree().expect("present").base(),
                repo.workdir().unwrap(),
                "current worktree is based on work-tree dir"
            );

            let baseline = crate::repository::worktree::Baseline::collect(repo.git_dir())?;
            assert_eq!(baseline.len(), 1, "git lists the main worktree");
            assert_eq!(
                baseline[0].root,
                gix_path::realpath(repo.git_dir().parent().unwrap())?,
                "git lists the original worktree, to which we have no access anymore"
            );
            assert_eq!(
                repo.worktrees()?.len(),
                0,
                "we only list linked worktrees, and there are none"
            );
            assert_eq!(
                repo.index()?.entries().len(),
                count_deleted(repo.git_dir()),
                "git considers all worktree entries missing as the overridden worktree is an empty dir"
            );
            assert_eq!(repo.index()?.entries().len(), 3, "just to be sure");
        }
        Ok(())
    }

    #[test]
    fn non_existing_relative() {
        let repo = repo("relative-nonexisting-worktree");
        assert_eq!(
            count_deleted(repo.git_dir()),
            0,
            "git can't chdir into missing worktrees, has no error handling there"
        );

        assert!(
            !repo.workdir().expect("configured").exists(),
            "non-existing or invalid worktrees (this one is a file) are taken verbatim and \
            may lead to errors later - just like in `git` and we explicitly do not try to be smart about it"
        );
    }

    #[test]
    fn relative_file() {
        let repo = repo("relative-worktree-file");
        assert_eq!(count_deleted(repo.git_dir()), 0, "git can't chdir into a file");

        assert!(
            repo.workdir().expect("configured").is_file(),
            "non-existing or invalid worktrees (this one is a file) are taken verbatim and \
            may lead to errors later - just like in `git` and we explicitly do not try to be smart about it"
        );
    }

    #[test]
    #[cfg(feature = "index")]
    fn bare_relative() -> crate::Result {
        let repo = repo("bare-relative-worktree");

        assert_eq!(
            count_deleted(repo.git_dir()),
            0,
            "git refuses to mix bare with core.worktree"
        );
        assert!(
            repo.workdir().is_none(),
            "we simply don't load core.worktree in bare repos either to match this behaviour"
        );
        assert!(repo.try_index()?.is_none());
        assert!(repo.index_or_empty()?.entries().is_empty());
        Ok(())
    }

    #[test]
    #[cfg(unix)] // symlinks are used here, let's not try our luck on Windows.
    fn relative_through_symlinked_ancestor_keeps_callers_path_namespace() -> crate::Result {
        let link = gix_testtools::scripted_fixture_read_only("make_core_worktree_repo.sh")?.join("symlinked-ancestor");

        let repo = gix::open_opts(link.join("relative-worktree"), crate::restricted())?;
        assert_eq!(
            repo.workdir(),
            Some(link.join("worktree").as_path()),
            "if a symlink in an ancestor changes nothing about how the relative worktree resolves, \
             the caller's path namespace is kept instead of jumping to the canonicalized one"
        );
        Ok(())
    }

    #[test]
    #[cfg(unix)] // symlinks are used here, let's not try our luck on Windows.
    fn relative_from_symlinked_git_dir() -> crate::Result {
        let fixture = gix_testtools::scripted_fixture_read_only("make_core_worktree_repo.sh")?;
        let root = fixture.join("linked-git-dir-detached-worktree");
        let repo = gix::open_opts(root.join("home"), crate::restricted())?;
        let git_worktree = std::fs::read_to_string(root.join("worktree.baseline"))?;

        assert_eq!(
            gix_path::realpath(repo.workdir().expect("core.worktree is configured"))?,
            gix_path::realpath(git_worktree.trim_end())?,
            "relative core.worktree values from repository config are resolved against the real git dir"
        );
        Ok(())
    }

    fn repo(name: &str) -> gix::Repository {
        let dir = gix_testtools::scripted_fixture_read_only("make_core_worktree_repo.sh").unwrap();
        gix::open_opts(dir.join(name), crate::restricted()).unwrap()
    }

    fn count_deleted(git_dir: &std::path::Path) -> usize {
        std::fs::read(git_dir.join("status.baseline"))
            .unwrap()
            .lines()
            .map_while(Result::ok)
            .filter(|line| line.contains(" D "))
            .count()
    }
}

struct Baseline<'a> {
    lines: bstr::Lines<'a>,
}

mod baseline {
    use std::{
        borrow::Cow,
        path::{Path, PathBuf},
    };

    use gix::bstr::{BString, ByteSlice};
    use gix_object::bstr::BStr;

    use super::Baseline;

    impl Baseline<'_> {
        pub fn collect(dir: impl AsRef<Path>) -> std::io::Result<Vec<Worktree>> {
            let content = std::fs::read(dir.as_ref().join("worktree-list.baseline"))?;
            Ok(Baseline { lines: content.lines() }.collect())
        }
    }

    pub type Reason = BString;

    #[derive(Debug)]
    pub struct Worktree {
        pub root: PathBuf,
        pub bare: bool,
        pub locked: Option<Reason>,
        pub peeled: gix_hash::ObjectId,
        pub branch: Option<BString>,
        pub prunable: Option<Reason>,
    }

    impl Iterator for Baseline<'_> {
        type Item = Worktree;

        fn next(&mut self) -> Option<Self::Item> {
            let root = gix_path::from_bstr(Cow::Borrowed(fields(self.lines.next()?).1)).into_owned();
            let mut bare = false;
            let mut branch = None;
            let mut peeled = gix_hash::ObjectId::null(gix_hash::Kind::Sha1);
            let mut locked = None;
            let mut prunable = None;
            for line in self.lines.by_ref() {
                if line.is_empty() {
                    break;
                }
                if line == b"bare" {
                    bare = true;
                    continue;
                } else if line == b"detached" {
                    continue;
                }
                let (field, value) = fields(line);
                match field {
                    f if f == "HEAD" => peeled = gix_hash::ObjectId::from_hex(value).expect("valid hash"),
                    f if f == "branch" => branch = Some(value.to_owned()),
                    f if f == "locked" => locked = Some(value.to_owned()),
                    f if f == "prunable" => prunable = Some(value.to_owned()),
                    _ => unreachable!("unknown field: {}", field),
                }
            }
            Some(Worktree {
                root,
                bare,
                locked,
                peeled,
                branch,
                prunable,
            })
        }
    }

    fn fields(line: &[u8]) -> (&BStr, &BStr) {
        let (a, b) = line.split_at(line.find_byte(b' ').expect("at least a space"));
        (a.as_bstr(), b[1..].as_bstr())
    }
}

#[test]
fn from_bare_parent_repo() {
    let Some(dir) = gix_testtools::scripted_fixture_read_only_with_args_with_git_version(
        "make_worktree_repo.sh",
        ["bare"],
        |version| version >= (2, 31, 0),
    )
    .unwrap() else {
        return;
    };
    let repo = gix::open_opts(dir.join("repo.git"), crate::restricted()).expect("fixture repository opens");

    run_assertions(repo, true /* bare */);
}

#[test]
fn from_nonbare_parent_repo() {
    let Some(dir) = gix_testtools::scripted_fixture_read_only_with_git_version("make_worktree_repo.sh", |version| {
        version >= (2, 31, 0)
    })
    .unwrap() else {
        return;
    };
    let repo = gix::open_opts(dir.join("repo"), crate::restricted()).expect("fixture repository opens");

    run_assertions(repo, false /* bare */);
}

#[test]
fn linked_worktree_proxy_base_with_relative_linking_files() -> crate::Result {
    let fixture = gix_testtools::scripted_fixture_read_only_needs_archive("make_worktree_relative_linking.sh")?;
    let main = fixture.join("main");
    let linked = fixture.join("linked");
    let private_git_dir = main.join(".git/worktrees/linked");
    let repo = gix::open_opts(&main, crate::restricted())?;
    let worktrees = repo.worktrees()?;
    assert_eq!(worktrees.len(), 1, "the relative-path fixture has one linked worktree");
    let proxy = worktrees.into_iter().next().expect("one worktree");

    assert_eq!(
        gix_path::realpath(proxy.base()?)?,
        gix_path::realpath(&linked)?,
        "proxy bases resolve relative worktrees/<id>/gitdir paths against the private git dir"
    );
    let linked_repo = proxy.into_repo()?;
    assert_eq!(
        linked_repo.workdir().map(gix_path::realpath).transpose()?,
        Some(gix_path::realpath(&linked)?)
    );
    assert_eq!(linked_repo.git_dir(), private_git_dir);

    Ok(())
}

#[test]
#[cfg(unix)]
fn linked_worktree_proxy_base_with_symlinked_main_repo() -> crate::Result {
    let fixture = gix_testtools::scripted_fixture_read_only_needs_archive("make_worktree_relative_linking.sh")?;
    let linked = fixture.join("actual/linked");
    let main_symlink = fixture.join("main-symlink");

    let repo = gix::open_opts(&main_symlink, crate::restricted())?;
    let worktrees = repo.worktrees()?;
    assert_eq!(worktrees.len(), 1, "the relative-path fixture has one linked worktree");
    let proxy = worktrees.into_iter().next().expect("one worktree");

    assert_eq!(
        gix_path::realpath(proxy.base()?)?,
        gix_path::realpath(&linked)?,
        "proxy bases preserve symlink semantics when resolving relative worktrees/<id>/gitdir paths"
    );
    let repo = proxy.into_repo()?;
    assert_eq!(
        repo.workdir().map(gix_path::realpath).transpose()?,
        Some(gix_path::realpath(&linked)?)
    );

    Ok(())
}

#[test]
fn from_nonbare_parent_repo_set_workdir() -> gix_testtools::Result {
    let Some(dir) = gix_testtools::scripted_fixture_read_only_with_git_version("make_worktree_repo.sh", |version| {
        version >= (2, 31, 0)
    })?
    else {
        return Ok(());
    };
    let mut repo = gix::open_opts(dir.join("repo"), crate::restricted()).expect("fixture repository opens");

    assert!(repo.worktree().is_some_and(|wt| wt.is_main()), "we have main worktree");

    let worktrees = repo.worktrees()?;
    assert_eq!(worktrees.len(), 6);

    let linked_wt_dir = worktrees.first().unwrap().base().expect("this linked worktree exists");
    repo.set_workdir(linked_wt_dir).expect("works as the dir exists");

    assert!(
        repo.worktree().is_some_and(|wt| wt.is_main()),
        "it's still the main worktree as that depends on the git_dir"
    );

    let mut wt_repo = repo.worktrees()?.first().unwrap().clone().into_repo()?;
    assert!(
        wt_repo.worktree().is_some_and(|wt| !wt.is_main()),
        "linked worktrees are never main"
    );

    wt_repo.set_workdir(Some(repo.workdir().unwrap().to_owned()))?;
    assert!(
        wt_repo.worktree().is_some_and(|wt| !wt.is_main()),
        "it's still the linked worktree as that depends on the git_dir"
    );

    Ok(())
}

fn run_assertions(main_repo: gix::Repository, should_be_bare: bool) {
    assert_eq!(main_repo.is_bare(), should_be_bare);
    assert_eq!(main_repo.kind(), gix::repository::Kind::Common);
    let mut baseline = Baseline::collect(
        main_repo
            .workdir()
            .map_or_else(|| main_repo.git_dir().parent(), std::path::Path::parent)
            .expect("a temp dir as parent"),
    )
    .unwrap();
    let expected_main = baseline.remove(0);
    assert_eq!(expected_main.bare, should_be_bare);

    if should_be_bare {
        assert!(main_repo.worktree().is_none());
    } else {
        assert_eq!(
            main_repo.workdir().expect("non-bare").canonicalize().unwrap(),
            expected_main.root.canonicalize().unwrap()
        );
        assert_eq!(main_repo.head_id().unwrap(), expected_main.peeled);
        assert_eq!(
            main_repo.head_name().unwrap().expect("no detached head"),
            expected_main.branch.unwrap()
        );
        let worktree = main_repo.worktree().expect("not bare");
        assert!(
            worktree.lock_reason().is_none(),
            "main worktrees, bare or not, are never locked"
        );
        assert!(!worktree.is_locked());
        assert!(worktree.is_main());
    }
    assert_eq!(main_repo.main_repo().unwrap(), main_repo, "main repo stays main repo");

    let actual = main_repo.worktrees().unwrap();
    assert_eq!(actual.len(), baseline.len());

    for actual in actual {
        let base = actual.base().unwrap();
        let expected = baseline
            .iter()
            .find(|exp| exp.root == base)
            .expect("we get the same root and it matches");
        assert!(
            !expected.bare,
            "only the main worktree can be bare, and we don't see it in this loop"
        );
        let proxy_lock_reason = actual.lock_reason();
        assert_eq!(proxy_lock_reason, expected.locked);
        let proxy_is_locked = actual.is_locked();
        assert_eq!(proxy_is_locked, proxy_lock_reason.is_some());
        // TODO: check id of expected worktree, but need access to .gitdir from worktree base
        let proxy_id = actual.id().to_owned();
        assert_eq!(
            base.is_dir(),
            expected.prunable.is_none(),
            "in our case prunable repos have no worktree base"
        );

        assert_eq!(
            main_repo.worktree_proxy_by_id(actual.id()).expect("exists").git_dir(),
            actual.git_dir(),
            "we can basically get the same proxy by its ID explicitly"
        );

        let repo = if base.is_dir() {
            let repo = actual.clone().into_repo().unwrap();
            assert_eq!(
                &gix::open_opts(base, crate::restricted()).expect("linked worktree repository opens"),
                &repo,
                "repos are considered the same no matter if opened from worktree or from git dir"
            );
            repo
        } else {
            assert!(
                matches!(
                    actual.clone().into_repo(),
                    Err(gix::worktree::proxy::into_repo::Error::MissingWorktree { .. })
                ),
                "missing bases are detected"
            );
            actual.clone().into_repo_with_possibly_inaccessible_worktree().unwrap()
        };
        let worktree = repo.worktree().expect("linked worktrees have at least a base path");
        assert!(!worktree.is_main());
        assert_eq!(worktree.lock_reason(), proxy_lock_reason);
        assert_eq!(worktree.is_locked(), proxy_is_locked);
        assert_eq!(worktree.id(), Some(proxy_id.as_ref()));
        assert_eq!(
            repo.main_repo().unwrap(),
            main_repo,
            "main repo from worktree repo is the actual main repo"
        );

        let proxy_by_id = repo
            .worktree_proxy_by_id(actual.id())
            .expect("can get the proxy from a linked repo as well");
        assert_ne!(
            proxy_by_id.git_dir(),
            actual.git_dir(),
            "The git directories might not look the same…"
        );
        assert_eq!(
            gix_path::realpath(proxy_by_id.git_dir()).ok(),
            gix_path::realpath(actual.git_dir()).ok(),
            "…but they are the same effectively"
        );
    }
}
