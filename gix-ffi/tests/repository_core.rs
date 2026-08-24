use gix_ffi::{GixError, Repo, RepositoryInfo};
use interoptopus::ffi;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gix-ffi-{label}-{}-{sequence}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("remove stale test directory");
        }
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bytes(path: &std::path::Path) -> Vec<u8> {
    Vec::from(gix::path::into_bstr(path.to_owned()).into_owned())
}

fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value,
        ffi::Err(error) => panic!("unexpected FFI error: {error:?}"),
        ffi::Result::Panic => panic!("unexpected panic marker"),
        ffi::Result::Null => panic!("unexpected null marker"),
    }
}

#[test]
fn init_reports_normal_and_bare_repository_metadata() {
    let worktree = TempDir::new("worktree");
    let worktree_path = bytes(&worktree.0);
    let repo = ok(Repo::create(
        ffi::Slice::from_slice(&worktree_path),
        false,
    ));
    let RepositoryInfo {
        repository_path,
        working_directory,
        has_working_directory,
        common_directory,
        is_bare,
        is_worktree,
    } = repo.info();

    assert!(!is_bare);
    assert!(has_working_directory);
    assert!(!is_worktree);
    assert!(!repository_path.is_empty());
    assert!(!working_directory.is_empty());
    assert_eq!(repository_path.into_vec(), common_directory.into_vec());

    let bare = TempDir::new("bare");
    let bare_path = bytes(&bare.0);
    let repo = ok(Repo::create(ffi::Slice::from_slice(&bare_path), true));
    let info = repo.info();

    assert!(info.is_bare);
    assert!(!info.has_working_directory);
    assert!(!info.is_worktree);
    assert!(info.working_directory.is_empty());
}

#[test]
fn discover_opens_the_repository_from_a_nested_path() {
    let root = TempDir::new("discover");
    let root_path = bytes(&root.0);
    let repo = ok(Repo::create(ffi::Slice::from_slice(&root_path), false));
    let expected = repo.info().repository_path.into_vec();

    let nested = root.0.join("one").join("two");
    std::fs::create_dir_all(&nested).expect("create nested directory");
    let nested_path = bytes(&nested);

    let discovered = ok(Repo::discover(
        ffi::Slice::from_slice(&nested_path),
        false,
        ffi::Slice::empty(),
    ));
    let actual = discovered.info().repository_path.into_vec();

    assert_eq!(actual, expected);
}

#[test]
fn discover_reports_not_a_repository() {
    let root = TempDir::new("missing");
    std::fs::create_dir_all(&root.0).expect("create search directory");
    let root_path = bytes(&root.0);

    let result = Repo::discover(
        ffi::Slice::from_slice(&root_path),
        false,
        ffi::Slice::empty(),
    );

    assert!(matches!(result, ffi::Err(GixError::NotARepository(_))));
}