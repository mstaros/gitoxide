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
            "gix-ffi-shallow-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&path).expect("create fixture");
        git(&path, if bare { &["init", "--bare", "-q"] } else { &["init", "-q"] });
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(root).args(args).output().expect("run Git");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).expect("Git object IDs are ASCII").trim().to_owned()
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

fn ids(repo: &Repo) -> Vec<String> {
    ok(repo.shallow_commits()).into_vec().iter().map(|id| id.as_str().to_owned()).collect()
}

fn shallow_path(repo: &Repo) -> PathBuf {
    let bytes = repo.shallow_file().into_vec();
    gix::path::try_from_byte_slice(bytes.as_slice()).expect("platform path bytes").to_owned()
}

#[test]
fn empty_malformed_and_unreadable_shallow_files_are_distinct() {
    for bare in [false, true] {
        let f = Fixture::new(bare);
        let repo = open(&f.0);
        let path = shallow_path(&repo);
        assert_eq!(path, if bare { f.0.join("shallow") } else { f.0.join(".git/shallow") });
        assert!(ids(&repo).is_empty(), "a missing file means complete history");
        std::fs::write(&path, b"").expect("write empty boundary");
        assert!(ids(&repo).is_empty(), "an empty file means complete history");

        let first = "1111111111111111111111111111111111111111";
        let second = "2222222222222222222222222222222222222222";
        std::fs::write(&path, format!("{second}\n{first}\n")).expect("write boundary");
        assert_eq!(ids(&repo), [first, second], "gix returns sorted boundary IDs");
        let timestamp = std::fs::metadata(&path).expect("boundary metadata").modified().expect("modified time");
        std::fs::write(&path, b"not-an-object-id\n").expect("write malformed boundary");
        std::fs::OpenOptions::new().write(true).open(&path).expect("open boundary for timestamp")
            .set_modified(timestamp).expect("preserve timestamp");
        let malformed = repo.shallow_commits();
        assert!(matches!(&malformed, ffi::Err(GixError::InvalidId(_))),
            "malformed data must not look like complete history: {malformed:?}");
        std::fs::remove_file(&path).expect("remove malformed file");
        std::fs::create_dir(&path).expect("make location unreadable as a file");
        assert!(matches!(repo.shallow_commits(), ffi::Err(GixError::Io(_))),
            "read failures remain IO errors");
    }
}

#[test]
fn clone_deepen_unshallow_and_linked_worktrees_expose_current_owned_boundaries() {
    let f = Fixture::new(false);
    git(&f.0, &["config", "user.name", "Shallow Tests"]);
    git(&f.0, &["config", "user.email", "shallow@example.com"]);
    git(&f.0, &["commit", "--allow-empty", "-qm", "first"]);
    let first = git(&f.0, &["rev-parse", "HEAD"]);
    git(&f.0, &["commit", "--allow-empty", "-qm", "second"]);
    let second = git(&f.0, &["rev-parse", "HEAD"]);
    let clone = f.0.join("clone");
    git(&f.0, &["clone", "--no-local", "--depth", "1",
        f.0.to_str().expect("path"), clone.to_str().expect("path")]);
    let repo = open(&clone);
    let snapshot = ids(&repo);
    assert_eq!(snapshot, [second.clone()]);
    git(&clone, &["fetch", "--deepen", "1"]);
    assert_eq!(ids(&repo), [first]);
    git(&clone, &["fetch", "--unshallow"]);
    assert!(ids(&repo).is_empty());
    assert_eq!(snapshot, [second.clone()], "earlier returned IDs remain owned");

    let linked = f.0.join("linked");
    git(&f.0, &["worktree", "add", "--detach", linked.to_str().expect("path"), "HEAD"]);
    let main = open(&f.0);
    let worktree = open(&linked);
    std::fs::write(shallow_path(&main), format!("{second}\n")).expect("write shared boundary");
    assert_eq!(shallow_path(&main).canonicalize().expect("common file"),
        shallow_path(&worktree).canonicalize().expect("linked common file"));
    assert_eq!(ids(&main), ids(&worktree));
    assert_eq!(ids(&worktree), [second]);
}

#[test]
fn configured_shallow_location_preserves_path_bytes() {
    let f = Fixture::new(false);
    let configured = "info/日本語 shallow";
    git(&f.0, &["config", "gitoxide.core.shallowFile", configured]);
    let repo = open(&f.0);
    let path = f.0.join(".git").join(configured);
    let expected = Vec::<u8>::from(gix::path::into_bstr(path.clone()).into_owned());
    assert_eq!(repo.shallow_file().into_vec(), expected);
    assert!(!path.exists(), "location lookup does not create the file");
    let boundary = "1111111111111111111111111111111111111111";
    std::fs::write(&path, format!("{boundary}\n")).expect("write configured boundary");
    assert_eq!(ids(&repo), [boundary]);
}