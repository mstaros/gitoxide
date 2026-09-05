use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gix_ffi::{GixError, Repo};
use interoptopus::ffi;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new(bare: bool) -> Self {
        let path = std::env::temp_dir().join(format!(
            "gix-ffi-ignore-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&path).expect("create fixture");
        git(&path, if bare { &["init", "--bare", "-q"] } else { &["init", "-q"] });
        Self(path)
    }

    fn repo(&self) -> Repo {
        open(&self.0)
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let destination = self.0.join(path);
        std::fs::create_dir_all(destination.parent().expect("parent")).expect("create parent");
        std::fs::write(destination, bytes).expect("write fixture");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git").arg("-C").arg(root).args(args).output().expect("run Git");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

fn open(root: &Path) -> Repo {
    let path = Vec::<u8>::from(gix::path::into_bstr(root.to_owned()).into_owned());
    ok(Repo::open(ffi::Slice::from_slice(&path)))
}

fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value,
        ffi::Err(err) => panic!("unexpected FFI error: {err:?}"),
        _ => panic!("unexpected FFI panic/null"),
    }
}

fn ignored(repo: &Repo, path: &[u8], directory: bool) -> bool {
    ok(repo.is_path_ignored(ffi::Slice::from_slice(path), directory))
}

fn ensure(repo: &Repo, path: &[u8], directory: bool) {
    ok(repo.ensure_local_exclude(ffi::Slice::from_slice(path), directory));
}

fn git_ignored(root: &Path, path: &str, directory: bool) -> bool {
    let path = if directory { format!("{path}/") } else { path.to_owned() };
    let output = Command::new("git")
        .arg("-C").arg(root).args(["check-ignore", "--no-index", "-q", "--", &path])
        .output().expect("run Git ignore check");
    match output.status.code() {
        Some(0) => true,
        Some(1) => false,
        _ => panic!("Git ignore check failed: {}", String::from_utf8_lossy(&output.stderr)),
    }
}

#[test]
fn ignore_precedence_matches_git_for_tracked_files_negations_and_directories() {
    let f = Fixture::new(false);
    f.write("global.ignore", b"*.global\n*.tmp\n");
    git(&f.0, &["config", "core.excludesFile", f.0.join("global.ignore").to_str().expect("path")]);
    f.write(".git/info/exclude", b"*.local\n!keep.global\n");
    f.write(".gitignore", b"*.tmp\n!keep.tmp\n!keep.local\ncache/\nblocked/\n!blocked/child\n");
    f.write("nested/.gitignore", b"!nested.tmp\n");
    f.write("tracked.tmp", b"tracked");
    git(&f.0, &["add", "-f", "tracked.tmp"]);
    let repo = f.repo();
    for (path, directory) in [
        ("tracked.tmp", false), ("keep.tmp", false), ("x.global", false),
        ("keep.global", false), ("x.local", false), ("keep.local", false),
        ("cache", true), ("cache", false), ("cache/file", false),
        ("blocked/child", false), ("nested/nested.tmp", false), ("nested/other.tmp", false),
    ] {
        assert_eq!(ignored(&repo, path.as_bytes(), directory), git_ignored(&f.0, path, directory), "{path}");
    }
    f.write(".gitignore", b"*.tmp\n!live.tmp\n");
    assert!(!ignored(&repo, b"live.tmp", false), "same handle sees updated ignore files");
}

#[test]
fn local_excludes_preserve_bytes_and_duplicate_calls_release_their_lock() {
    for bare in [false, true] {
        let f = Fixture::new(bare);
        let exclude = if bare { "info/exclude" } else { ".git/info/exclude" };
        let original = b"# raw \xff\r\n/previous\r\n# no final newline";
        f.write(exclude, original);
        let repo = f.repo();
        ensure(&repo, b".csharpmpc", true);
        let mut expected = original.to_vec();
        expected.extend_from_slice(b"\n/.csharpmpc/\n");
        assert_eq!(std::fs::read(f.0.join(exclude)).expect("exclude"), expected);
        ensure(&repo, b".csharpmpc", true);
        ensure(&repo, b".csharpmpc", true);
        assert_eq!(std::fs::read(f.0.join(exclude)).expect("exclude"), expected);
        assert!(!f.0.join(format!("{exclude}.lock")).exists());
        assert!(ignored(&repo, b".csharpmpc", true));
        assert!(!ignored(&repo, b".csharpmpc", false));
        assert!(!ignored(&repo, b"nested/.csharpmpc", true), "rule is root anchored");
        ensure(&repo, b"file-only", false);
        assert!(ignored(&repo, b"file-only", false));
    }
}

#[test]
fn literal_paths_are_escaped_without_expanding_the_exclusion() {
    let f = Fixture::new(false);
    let repo = f.repo();
    for path in ["name[ab]", "file name ", "#literal", "!literal"] {
        ensure(&repo, path.as_bytes(), false);
        assert!(git_ignored(&f.0, path, false), "{path}");
    }
    assert!(!ignored(&repo, b"namea", false));
    let bytes = std::fs::read(f.0.join(".git/info/exclude")).expect("exclude");
    assert!(bytes.windows(b"/name\\[ab\\]\n".len()).any(|part| part == b"/name\\[ab\\]\n"));
    assert!(bytes.windows(b"/file\\ name\\ \n".len()).any(|part| part == b"/file\\ name\\ \n"));
}

#[test]
fn linked_worktrees_share_common_excludes() {
    let f = Fixture::new(false);
    git(&f.0, &["-c", "user.name=Ignore Tests", "-c", "user.email=ignore@example.com", "commit", "--allow-empty", "-qm", "initial"]);
    let linked = f.0.join("linked");
    git(&f.0, &["worktree", "add", "-q", "-b", "linked", linked.to_str().expect("path")]);
    let linked_repo = open(&linked);
    ensure(&linked_repo, b".shared-cache", true);
    assert!(ignored(&f.repo(), b".shared-cache", true));
    assert!(ignored(&linked_repo, b".shared-cache", true));
    let direct = gix::open(&linked).expect("linked gix repository");
    assert!(!direct.git_dir().join("info/exclude").exists(), "no worktree-private exclude file");
    assert!(std::fs::read(f.0.join(".git/info/exclude")).expect("common exclude")
        .windows(b"/.shared-cache/\n".len()).any(|part| part == b"/.shared-cache/\n"));
}

#[test]
fn locked_excludes_and_overriding_rules_report_failure_without_stealing_locks() {
    let f = Fixture::new(false);
    f.write(".git/info/exclude", b"# original\n");
    f.write(".git/info/exclude.lock", b"another writer");
    let repo = f.repo();
    assert!(matches!(repo.ensure_local_exclude(b"blocked".as_slice().into(), false), ffi::Err(GixError::Io(_))));
    assert_eq!(std::fs::read(f.0.join(".git/info/exclude.lock")).expect("foreign lock"), b"another writer");
    assert_eq!(std::fs::read(f.0.join(".git/info/exclude")).expect("exclude"), b"# original\n");
    std::fs::remove_file(f.0.join(".git/info/exclude.lock")).expect("release test lock");
    f.write(".gitignore", b"!visible\n");
    assert!(matches!(repo.ensure_local_exclude(b"visible".as_slice().into(), false), ffi::Err(GixError::Other(_))));
    assert!(!ignored(&repo, b"visible", false), "higher-priority negation wins");
    assert!(!f.0.join(".git/info/exclude.lock").exists(), "our lock is released on failure");
    assert!(std::fs::read(f.0.join(".git/info/exclude")).expect("exclude").ends_with(b"/visible\n"));
}

#[test]
fn invalid_paths_do_not_write_rules() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let before = std::fs::read(f.0.join(".git/info/exclude")).expect("exclude");
    for path in [b"".as_slice(), b"/root", b"trailing/", b"two//parts", b".", b"a/../b", b"a\\b", b"a\nb", b"a\0b"] {
        assert!(matches!(repo.is_path_ignored(path.into(), false), ffi::Err(GixError::InvalidPath(_))), "{path:?}");
        assert!(matches!(repo.ensure_local_exclude(path.into(), false), ffi::Err(GixError::InvalidPath(_))), "{path:?}");
    }
    assert_eq!(std::fs::read(f.0.join(".git/info/exclude")).expect("exclude"), before);
}