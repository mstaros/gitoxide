//! Coverage for `checked_out_branches()` reporting branches held by an in-progress
//! rebase or bisect, and for `delete_local_branches` refusing them.
//!
//! A rebase or bisect detaches `HEAD` while still holding the branch it started from,
//! so the branch is unavailable even though no symbolic reference points at it. Git
//! applies this only to detached worktrees, and so does `insert_head`.
//!
//! Fixtures are opened writable: `delete_local_branches` takes `&mut self`, and a test
//! that mutated a shared read-only fixture would corrupt every later run.

use gix::refs::FullName;

fn full_name(name: &str) -> FullName {
    name.try_into().expect("valid reference name")
}

/// Every branch reported as checked out, excluding the synthetic `HEAD` entry.
fn held_branches(repo: &gix::Repository) -> crate::Result<Vec<String>> {
    Ok(repo
        .checked_out_branches()?
        .into_keys()
        .filter(|name| name.as_bstr() != "HEAD")
        .map(|name| name.as_bstr().to_string())
        .collect())
}

fn assert_refuses_deletion(repo: &mut gix::Repository, branch: &str) {
    let err = repo
        .delete_local_branches([full_name(branch)])
        .expect_err("the branch is held, so deletion must be refused");
    match err {
        gix::repository::branch::delete::Error::CheckedOut { name, worktree_dirs } => {
            assert_eq!(name.as_bstr(), branch);
            assert!(
                !worktree_dirs.is_empty(),
                "the refusal must say where the branch is held, or it cannot be acted on"
            );
        }
        other => panic!("expected CheckedOut, got {other:?}"),
    }
}

#[test]
fn interactive_rebase_holds_the_branch_it_started_from() -> crate::Result {
    let fixture = gix_testtools::scripted_fixture_writable("make_rebase_i_repo.sh")?;
    let mut repo = gix::open_opts(fixture.path(), crate::restricted())?;

    assert!(
        repo.head()?.is_detached(),
        "precondition: an interactive rebase detaches HEAD"
    );
    assert_eq!(held_branches(&repo)?, ["refs/heads/main"]);
    assert_refuses_deletion(&mut repo, "refs/heads/main");
    Ok(())
}

#[test]
fn rebase_apply_holds_the_branch_it_started_from() -> crate::Result {
    let fixture = gix_testtools::scripted_fixture_writable("make_rebase_apply_repo.sh")?;
    let mut repo = gix::open_opts(fixture.path().join("rebase-apply-conflict"), crate::restricted())?;

    assert!(repo.head()?.is_detached(), "precondition: the rebase detached HEAD");
    assert_eq!(
        held_branches(&repo)?,
        ["refs/heads/topic"],
        "`rebase-apply/head-name` is read as well as `rebase-merge/head-name`"
    );
    assert_refuses_deletion(&mut repo, "refs/heads/topic");
    Ok(())
}

#[test]
fn bisect_started_on_a_branch_holds_it() -> crate::Result {
    let fixture = gix_testtools::scripted_fixture_writable("make_bisect_holding_repo.sh")?;
    let mut repo = gix::open_opts(fixture.path().join("started-on-branch"), crate::restricted())?;

    assert!(
        repo.head()?.is_detached(),
        "precondition: a stepping bisect checks out a midpoint"
    );
    assert_eq!(
        held_branches(&repo)?,
        ["refs/heads/topic"],
        "`BISECT_START` names the branch the bisect began on"
    );
    assert_refuses_deletion(&mut repo, "refs/heads/topic");
    Ok(())
}

#[test]
fn bisect_started_detached_holds_nothing() -> crate::Result {
    let fixture = gix_testtools::scripted_fixture_writable("make_bisect_holding_repo.sh")?;
    let repo = gix::open_opts(fixture.path().join("started-detached"), crate::restricted())?;

    assert!(repo.head()?.is_detached(), "precondition: the bisect began detached");
    assert_eq!(
        held_branches(&repo)?,
        Vec::<String>::new(),
        "`BISECT_START` holds an object id, not a branch. Prefixing it with `refs/heads/` \
         produces a syntactically valid name for a branch that does not exist, which git's \
         `get_branch` avoids by rendering an abbreviated hash instead"
    );
    Ok(())
}
