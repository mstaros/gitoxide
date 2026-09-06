use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gix::bstr::ByteSlice;
use gix_ffi::{GixError, Repo, WorktreeRecord};
use interoptopus::ffi;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture { root: PathBuf, main: PathBuf }
impl Fixture {
    fn new(bare: bool) -> Self {
        let root = std::env::temp_dir().join(format!("gix-ffi-worktrees-{}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let main = root.join("main");
        std::fs::create_dir_all(&main).expect("fixture");
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["config", "user.name", "Worktree Tests"]);
        git(&main, &["config", "user.email", "worktree@example.com"]);
        git(&main, &["config", "core.autocrlf", "false"]);
        std::fs::write(main.join("tracked"), b"committed\n").expect("tracked");
        git(&main, &["add", "tracked"]);
        git(&main, &["commit", "-q", "-m", "fixture"]);
        let main = if bare {
            let bare_path = root.join("bare.git");
            git(&root, &["clone", "--bare", "-q", main.to_str().expect("path"), bare_path.to_str().expect("path")]);
            git(&bare_path, &["config", "user.name", "Worktree Tests"]);
            git(&bare_path, &["config", "user.email", "worktree@example.com"]);
            git(&bare_path, &["config", "core.autocrlf", "false"]);
            bare_path
        } else { main };
        Self { root, main }
    }
    fn repo(&self) -> Repo { open(&self.main) }
    fn path(&self, name: &str) -> PathBuf { self.root.join(name) }
    fn head(&self) -> String { text(&self.main, &["rev-parse", "HEAD"]) }
    fn core(&self) -> gix::Repository { gix::open(&self.main).expect("core repository") }
    fn admin(&self, name: &str) -> PathBuf { self.core().common_dir().join("worktrees").join(name) }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); }
}
fn git(path: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git").arg("-C").arg(path).args(args)
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_CONFIG_COUNT")
        .output().expect("run Git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}
fn text(path: &Path, args: &[&str]) -> String {
    String::from_utf8(git(path, args)).expect("ASCII output").trim().to_owned()
}
fn bytes(path: &Path) -> Vec<u8> { gix::path::into_bstr(path).into_owned().into() }
fn open(path: &Path) -> Repo { ok(Repo::open(bytes(path).as_slice().into())) }
fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value,
        ffi::Err(GixError::Other(error)) => panic!("unexpected FFI error: {}", error.as_str()),
        ffi::Err(error) => panic!("unexpected FFI error: {error:?}"),
        _ => panic!("unexpected FFI panic/null"),
    }
}
fn add(repo: &Repo, name: &[u8], path: &Path, reference: &[u8], lock: bool, checkout: bool)
    -> ffi::Result<WorktreeRecord, GixError>
{
    repo.add_worktree(name.into(), bytes(path).as_slice().into(), reference.into(), lock, checkout)
}
fn detached(repo: &Repo, name: &[u8], path: &Path, id: &str, lock: bool)
    -> ffi::Result<WorktreeRecord, GixError>
{
    repo.add_detached_worktree(name.into(), bytes(path).as_slice().into(), id.to_owned().into(), lock)
}
fn prune(repo: &Repo, name: &[u8], valid: bool, locked: bool, files: bool) -> ffi::Result<bool, GixError> {
    repo.prune_worktree(name.into(), valid, locked, files)
}

#[test]
fn named_branch_checkout_and_common_directory_enumeration_match_git() {
    let f = Fixture::new(false);
    let repo = f.repo();
    assert!(ok(repo.get_worktrees()).is_empty(), "main is not a linked registration");
    let path = f.path("different-checkout-basename");
    let info = ok(add(&repo, b"feature", &path, b"", false, true));
    assert_eq!(info.name.into_vec(), b"feature");
    assert!(info.is_valid && !info.is_locked);
    assert_eq!(PathBuf::from(gix::path::from_bstr(info.path.into_vec().as_bstr()).as_ref()), path);
    assert_eq!(std::fs::read(path.join("tracked")).expect("checkout"), b"committed\n");
    assert_eq!(text(&path, &["symbolic-ref", "HEAD"]), "refs/heads/feature");
    assert_eq!(text(&path, &["rev-parse", "HEAD"]), f.head());
    assert_eq!(text(&path, &["status", "--porcelain"]), "");
    assert_eq!(text(&path, &["rev-parse", "HEAD@{0}"]), f.head(), "HEAD reflog records initial tip");
    let linked = open(&path);
    assert_eq!(ok(linked.get_worktrees()).len(), 1);
    assert_eq!(ok(repo.get_worktrees()).len(), 1);
    assert!(text(&f.main, &["worktree", "list", "--porcelain"]).contains("refs/heads/feature"));
}

#[test]
fn explicit_branch_no_checkout_lock_and_reservations_are_preserved() {
    let f = Fixture::new(false);
    git(&f.main, &["branch", "topic"]);
    let repo = f.repo();
    let path = f.path("empty-checkout");
    let info = ok(add(&repo, b"admin-name", &path, b"topic", true, false));
    assert!(info.is_valid && info.is_locked);
    assert!(info.lock_reason.is_empty(), "empty lock still protects the registration");
    assert!(!path.join("tracked").exists());
    assert_eq!(std::fs::read_dir(&path).expect("checkout").count(), 1);
    assert_eq!(text(&path, &["symbolic-ref", "HEAD"]), "refs/heads/topic");
    assert!(matches!(add(&repo, b"would-steal", &f.path("steal"), b"refs/heads/topic", false, true),
        ffi::Err(GixError::ReferenceConflict(_))));
    assert!(matches!(add(&repo, b"main-steal", &f.path("steal-main"), b"main", false, true),
        ffi::Err(GixError::ReferenceConflict(_))));
    assert!(!ok(prune(&repo, b"admin-name", true, false, true)));
    assert!(ok(prune(&repo, b"admin-name", true, true, true)), "lock override permits a clean no-checkout registration");
    assert!(!path.exists());
}

#[test]
fn detached_exact_commits_are_checked_out_in_normal_and_bare_repositories() {
    for bare in [false, true] {
        let f = Fixture::new(bare);
        let repo = f.repo();
        let path = f.path("detached-checkout");
        let branches = git(&f.main, &["for-each-ref", "--format=%(refname)", "refs/heads/"]);
        let info = ok(detached(&repo, b"detached", &path, &f.head(), false));
        assert!(info.is_valid);
        assert_eq!(text(&path, &["rev-parse", "HEAD"]), f.head());
        assert!(gix::open(&path).expect("linked").head().expect("head").is_detached());
        assert_eq!(std::fs::read(path.join("tracked")).expect("checkout"), b"committed\n");
        assert_eq!(git(&f.main, &["for-each-ref", "--format=%(refname)", "refs/heads/"]), branches,
            "detached creation does not create transient branches");
        let blob = ok(repo.write_blob(b"blob".as_slice().into()));
        assert!(matches!(detached(&repo, b"not-commit", &f.path("not-commit"), blob.as_str(), false), ffi::Err(_)));
        assert!(!f.path("not-commit").exists());
    }
}

#[test]
fn metadata_only_pruning_is_exact_and_keeps_dirty_checkout_bytes() {
    let f = Fixture::new(false);
    let repo = f.repo();
    for name in ["one", "two"] {
        ok(detached(&repo, name.as_bytes(), &f.path(name), &f.head(), false));
    }
    let path = f.path("one");
    std::fs::write(path.join("tracked"), b"modified\0bytes").expect("modify");
    std::fs::write(path.join("untracked"), b"valuable").expect("untracked");
    let pointer = std::fs::read(path.join(".git")).expect("pointer");
    assert!(!ok(prune(&repo, b"one", false, false, false)), "valid registrations need explicit permission");
    assert!(ok(prune(&repo, b"one", true, false, false)));
    assert!(!ok(prune(&repo, b"one", true, false, false)), "exact pruning is idempotent");
    assert_eq!(std::fs::read(path.join("tracked")).expect("preserved"), b"modified\0bytes");
    assert_eq!(std::fs::read(path.join("untracked")).expect("preserved"), b"valuable");
    assert_eq!(std::fs::read(path.join(".git")).expect("preserved"), pointer);
    assert!(!f.admin("one").exists());
    assert!(f.admin("two").is_dir());
    assert_eq!(ok(repo.get_worktrees()).len(), 1);
}

#[test]
fn stale_entries_match_git_pruning_and_locked_entries_survive() {
    let f = Fixture::new(false);
    let repo = f.repo();
    ok(detached(&repo, b"stale", &f.path("stale"), &f.head(), false));
    ok(detached(&repo, b"locked", &f.path("locked"), &f.head(), true));
    std::fs::remove_dir_all(f.path("stale")).expect("remove checkout");
    std::fs::remove_dir_all(f.path("locked")).expect("remove checkout");
    let entries = ok(repo.get_worktrees()).into_vec();
    assert!(entries.iter().all(|entry| !entry.is_valid));
    let output = Command::new("git").arg("-C").arg(&f.main)
        .args(["worktree", "prune", "--dry-run", "--verbose", "--expire=now"])
        .output().expect("Git prune");
    assert!(output.status.success());
    let dry = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(dry.contains("stale"));
    assert!(!dry.contains("locked"));
    assert_eq!(ok(repo.prune_worktrees(false, false, false)), 1);
    assert!(f.admin("locked").is_dir());
    assert!(!ok(prune(&repo, b"locked", false, false, false)));
    assert!(ok(prune(&repo, b"locked", false, true, false)));
    assert!(ok(repo.get_worktrees()).is_empty());
}

#[test]
fn removing_checkout_refuses_dirty_untracked_and_foreign_data_even_with_lock_override() {
    for dirty in ["tracked", "untracked"] {
        let f = Fixture::new(false);
        let repo = f.repo();
        let path = f.path("dirty");
        ok(detached(&repo, b"dirty", &path, &f.head(), true));
        std::fs::write(path.join(dirty), b"keep").expect("dirty");
        git(&f.main, &["config", "status.showUntrackedFiles", "no"]);
        assert!(matches!(prune(&repo, b"dirty", true, true, true), ffi::Err(_)));
        assert_eq!(std::fs::read(path.join(dirty)).expect("preserved"), b"keep");
        assert!(f.admin("dirty").is_dir());
    }
    let f = Fixture::new(false);
    let repo = f.repo();
    let path = f.path("foreign");
    ok(detached(&repo, b"foreign", &path, &f.head(), false));
    std::fs::write(path.join(".git"), format!("gitdir: {}\n", f.core().common_dir().display())).expect("foreign pointer");
    let info = ok(repo.get_worktrees()).into_vec().remove(0);
    assert!(!info.is_valid, "an existing foreign backpointer is invalid");
    assert!(matches!(prune(&repo, b"foreign", true, true, true), ffi::Err(_)));
    assert!(path.join("tracked").is_file());
    assert!(f.admin("foreign").is_dir());
}

#[test]
fn raw_lock_bytes_and_missing_gitdir_are_owned_and_fail_closed() {
    let f = Fixture::new(false);
    let repo = f.repo();
    ok(detached(&repo, b"raw", &f.path("raw"), &f.head(), false));
    let raw = b" \xfflock\0reason\r\n";
    std::fs::write(f.admin("raw").join("locked"), raw).expect("lock");
    let info = ok(repo.get_worktrees()).into_vec().remove(0);
    drop(repo);
    assert_eq!(info.lock_reason.into_vec(), raw);
    assert!(info.is_locked);
    std::fs::remove_file(f.admin("raw").join("gitdir")).expect("malformed registration");
    let repo = f.repo();
    let info = ok(repo.get_worktrees()).into_vec().remove(0);
    assert!(!info.is_valid && info.path.is_empty() && info.is_locked);
    assert_eq!(ok(repo.prune_worktrees(false, false, false)), 0);
    std::fs::remove_file(f.admin("raw").join("locked")).expect("remove lock");
    std::fs::create_dir(f.admin("raw").join("locked")).expect("unreadable lock");
    assert!(matches!(repo.get_worktrees(), ffi::Err(_)));
    assert!(matches!(repo.prune_worktrees(false, false, false), ffi::Err(_)));
    assert!(f.admin("raw").is_dir());
}

#[test]
fn explicit_names_refuse_collisions_and_aliases_without_leaking_branches() {
    let f = Fixture::new(false);
    let repo = f.repo();
    ok(detached(&repo, b"occupied", &f.path("first"), &f.head(), false));
    assert!(matches!(add(&repo, b"occupied", &f.path("second"), b"", false, false),
        ffi::Err(GixError::ReferenceConflict(_))));
    assert!(!f.path("second").exists());
    assert!(f.core().try_find_reference("refs/heads/occupied").expect("query").is_none());
    for name in [b"../escape".as_slice(), b".", b"..", b"alias.", b"alias ", b"CON", b"x\\y", b""] {
        assert!(matches!(detached(&repo, name, &f.path("invalid"), &f.head(), false), ffi::Err(_)));
        assert!(!f.path("invalid").exists());
        assert!(matches!(prune(&repo, name, true, true, false), ffi::Err(_)));
    }
}


#[test]
fn concurrent_branch_creations_and_existing_branch_registrations_keep_one_owner() {
    for create in [true, false] {
        let f = Fixture::new(false);
        if !create { git(&f.main, &["branch", "shared"]); }
        let barrier = std::sync::Barrier::new(12);
        let winners = std::thread::scope(|scope| {
            let handles = (0..12).map(|index| {
                let barrier = &barrier;
                let f = &f;
                scope.spawn(move || {
                    let repo = f.repo();
                    let name = if create { b"shared".to_vec() } else { format!("admin-{index}").into_bytes() };
                    barrier.wait();
                    match add(&repo, &name, &f.path(&format!("contender-{index}")),
                        if create { b"" } else { b"refs/heads/shared" }, false, true)
                    {
                        ffi::Ok(_) => true,
                        ffi::Err(_) => false,
                        _ => panic!("no panics or null FFI results"),
                    }
                })
            }).collect::<Vec<_>>();
            handles.into_iter().map(|handle| handle.join().expect("contender")).filter(|won| *won).count()
        });
        assert_eq!(winners, 1, "a branch can have only one registered checkout");
        assert_eq!(ok(f.repo().get_worktrees()).len(), 1, "losers clean only their owned registration");
        assert_eq!(text(&f.main, &["rev-parse", "refs/heads/shared"]), f.head());
        assert!(!f.core().common_dir().join("refs/heads/shared.lock").exists());
    }
}

#[test]
fn checkout_failure_reports_retained_registration_and_partial_path() {
    let f = Fixture::new(false);
    std::fs::write(f.main.join(".gitattributes"), b"tracked filter=fail\n").expect("attributes");
    git(&f.main, &["add", ".gitattributes"]);
    git(&f.main, &["commit", "-q", "-m", "required filter"]);
    // Enable the failure only after seeding the tree, so Git can refresh the original index.
    git(&f.main, &["config", "filter.fail.required", "true"]);
    git(&f.main, &["config", "filter.fail.smudge", "gix-worktree-nonexistent-command"]);
    let repo = f.repo();
    let path = f.path("incomplete-checkout");
    match add(&repo, b"incomplete", &path, b"", false, true) {
        ffi::Err(GixError::Other(error)) => {
            assert!(error.as_str().contains("remains registered"));
            assert!(error.as_str().contains("incomplete"));
            assert!(error.as_str().contains("incomplete-checkout"));
        }
        _ => panic!("a failed required filter must report incomplete checkout"),
    }
    assert!(f.admin("incomplete").join("HEAD").is_file());
    assert!(path.join(".git").is_file());
    assert!(f.core().try_find_reference("refs/heads/incomplete").expect("branch").is_some());
}