use gix_ffi::{GixError, Repo};
use interoptopus::ffi;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const SHOW_COMBINED: u32 = 0;
const SHOW_INDEX_ONLY: u32 = 1;
const SHOW_WORKTREE_ONLY: u32 = 2;

const INCLUDE_UNTRACKED: u32 = 1 << 0;
const INCLUDE_IGNORED: u32 = 1 << 1;
const INCLUDE_UNMODIFIED: u32 = 1 << 2;
const EXCLUDE_SUBMODULES: u32 = 1 << 3;
const RECURSE_UNTRACKED_DIRECTORIES: u32 = 1 << 4;
const DISABLE_PATHSPEC_MATCH: u32 = 1 << 5;
const RECURSE_IGNORED_DIRECTORIES: u32 = 1 << 6;
const RENAMES_HEAD_TO_INDEX: u32 = 1 << 7;
const RENAMES_INDEX_TO_WORKTREE: u32 = 1 << 8;
const SORT_CASE_SENSITIVELY: u32 = 1 << 9;
const SORT_CASE_INSENSITIVELY: u32 = 1 << 10;
const RENAMES_FROM_REWRITES: u32 = 1 << 11;
const NO_REFRESH: u32 = 1 << 12;
const UPDATE_INDEX: u32 = 1 << 13;
const INCLUDE_UNREADABLE: u32 = 1 << 14;
const INCLUDE_UNREADABLE_AS_UNTRACKED: u32 = 1 << 15;

const CURRENT: u32 = 0;
const INDEX_NEW: u32 = 1 << 0;
const INDEX_MODIFIED: u32 = 1 << 1;
const INDEX_DELETED: u32 = 1 << 2;
const INDEX_RENAMED: u32 = 1 << 3;
const INDEX_TYPE_CHANGED: u32 = 1 << 4;
const WORKTREE_NEW: u32 = 1 << 7;
const WORKTREE_MODIFIED: u32 = 1 << 8;
const WORKTREE_DELETED: u32 = 1 << 9;
const WORKTREE_TYPE_CHANGED: u32 = 1 << 10;
const WORKTREE_RENAMED: u32 = 1 << 11;
#[cfg(target_family = "unix")]
const WORKTREE_UNREADABLE: u32 = 1 << 12;
const IGNORED: u32 = 1 << 14;
const CONFLICTED: u32 = 1 << 15;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gix-ffi-status-{label}-{}-{sequence}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("remove stale test directory");
        }
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bytes(path: &Path) -> Vec<u8> {
    Vec::from(gix::path::into_bstr(path.to_owned()).into_owned())
}

fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value,
        ffi::Err(error) => {
            let message = match error {
                GixError::NotARepository(value)
                | GixError::Io(value)
                | GixError::Config(value)
                | GixError::InvalidPath(value)
                | GixError::InvalidId(value)
                | GixError::NotFound(value)
                | GixError::InvalidReference(value)
                | GixError::ReferenceConflict(value)
                | GixError::Other(value) => value,
            };
            panic!("unexpected FFI error: {}", message.as_str());
        }
        ffi::Result::Panic => panic!("unexpected panic marker"),
        ffi::Result::Null => panic!("unexpected null marker"),
    }
}

fn run_git(root: &Path, arguments: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("launch git")
}

fn git(root: &Path, arguments: &[&str]) -> String {
    let output = run_git(root, arguments);
    assert!(
        output.status.success(),
        "git {:?} failed:\nstdout: {}\nstderr: {}",
        arguments,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output is UTF-8")
        .trim()
        .to_owned()
}

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent directory");
    }
    std::fs::write(path, contents).expect("write fixture file");
}

fn new_repo(label: &str) -> (TempDir, Repo) {
    let root = TempDir::new(label);
    git(&root.0, &["init", "--quiet"]);
    git(&root.0, &["config", "user.name", "Status Tests"]);
    git(
        &root.0,
        &["config", "user.email", "status-tests@example.com"],
    );
    git(&root.0, &["config", "core.autocrlf", "false"]);
    git(&root.0, &["config", "core.ignorecase", "false"]);
    let root_bytes = bytes(&root.0);
    let repo = ok(Repo::open(ffi::Slice::from_slice(&root_bytes)));
    (root, repo)
}

fn commit_all(root: &Path, message: &str) {
    git(root, &["add", "--all"]);
    git(root, &["commit", "--quiet", "-m", message]);
}

fn status_entries(
    repo: &Repo,
    show: u32,
    flags: u32,
    pathspecs: &[u8],
) -> Vec<(Vec<u8>, u32)> {
    ok(repo.status(
        show,
        flags,
        ffi::Slice::from_slice(pathspecs),
    ))
    .into_vec()
    .into_iter()
    .map(|entry| (entry.path.into_vec(), entry.status))
    .collect()
}

fn status_map(
    repo: &Repo,
    show: u32,
    flags: u32,
    pathspecs: &[u8],
) -> BTreeMap<Vec<u8>, u32> {
    status_entries(repo, show, flags, pathspecs)
        .into_iter()
        .collect()
}

fn value(entries: &BTreeMap<Vec<u8>, u32>, path: &[u8]) -> u32 {
    *entries
        .get(path)
        .unwrap_or_else(|| panic!("missing status for {}", String::from_utf8_lossy(path)))
}

fn assert_absent(entries: &BTreeMap<Vec<u8>, u32>, path: &[u8]) {
    assert!(
        !entries.contains_key(path),
        "unexpected status for {}: {:?}",
        String::from_utf8_lossy(path),
        entries.get(path)
    );
}

#[test]
fn status_combined_and_show_modes_map_index_and_worktree_changes() {
    let (root, repo) = new_repo("show-modes");
    for path in [
        "staged-mod.txt",
        "worktree-mod.txt",
        "staged-delete.txt",
        "worktree-delete.txt",
        "both.txt",
        "clean.txt",
    ] {
        write(&root.0, path, b"baseline\n");
    }
    write(&root.0, ".gitignore", b"ignored-*\n");
    commit_all(&root.0, "baseline");

    write(
        &root.0,
        "staged-mod.txt",
        b"staged modification with a different size\n",
    );
    git(&root.0, &["add", "staged-mod.txt"]);
    write(
        &root.0,
        "worktree-mod.txt",
        b"worktree modification with a different size\n",
    );
    git(&root.0, &["rm", "--quiet", "staged-delete.txt"]);
    std::fs::remove_file(root.0.join("worktree-delete.txt")).expect("remove tracked file");
    write(&root.0, "both.txt", b"staged version is different\n");
    git(&root.0, &["add", "both.txt"]);
    write(
        &root.0,
        "both.txt",
        b"worktree version differs again and is longer\n",
    );
    write(&root.0, "new-index.txt", b"new in index\n");
    git(&root.0, &["add", "new-index.txt"]);
    write(&root.0, "untracked.txt", b"untracked\n");
    write(&root.0, "ignored-build.tmp", b"ignored\n");

    let flags = INCLUDE_UNTRACKED | INCLUDE_IGNORED | RECURSE_UNTRACKED_DIRECTORIES;
    let combined = status_map(&repo, SHOW_COMBINED, flags, b"");
    assert_eq!(value(&combined, b"staged-mod.txt"), INDEX_MODIFIED);
    assert_eq!(value(&combined, b"worktree-mod.txt"), WORKTREE_MODIFIED);
    assert_eq!(value(&combined, b"staged-delete.txt"), INDEX_DELETED);
    assert_eq!(
        value(&combined, b"worktree-delete.txt"),
        WORKTREE_DELETED
    );
    assert_eq!(
        value(&combined, b"both.txt"),
        INDEX_MODIFIED | WORKTREE_MODIFIED
    );
    assert_eq!(value(&combined, b"new-index.txt"), INDEX_NEW);
    assert_eq!(value(&combined, b"untracked.txt"), WORKTREE_NEW);
    assert_eq!(value(&combined, b"ignored-build.tmp"), IGNORED);
    assert_absent(&combined, b"clean.txt");

    let index_only = status_map(&repo, SHOW_INDEX_ONLY, flags, b"");
    assert_eq!(value(&index_only, b"staged-mod.txt"), INDEX_MODIFIED);
    assert_eq!(value(&index_only, b"staged-delete.txt"), INDEX_DELETED);
    assert_eq!(value(&index_only, b"both.txt"), INDEX_MODIFIED);
    assert_eq!(value(&index_only, b"new-index.txt"), INDEX_NEW);
    assert_absent(&index_only, b"worktree-mod.txt");
    assert_absent(&index_only, b"worktree-delete.txt");
    assert_absent(&index_only, b"untracked.txt");
    assert_absent(&index_only, b"ignored-build.tmp");

    let worktree_only = status_map(&repo, SHOW_WORKTREE_ONLY, flags, b"");
    assert_eq!(
        value(&worktree_only, b"worktree-mod.txt"),
        WORKTREE_MODIFIED
    );
    assert_eq!(
        value(&worktree_only, b"worktree-delete.txt"),
        WORKTREE_DELETED
    );
    assert_eq!(value(&worktree_only, b"both.txt"), WORKTREE_MODIFIED);
    assert_eq!(value(&worktree_only, b"untracked.txt"), WORKTREE_NEW);
    assert_eq!(value(&worktree_only, b"ignored-build.tmp"), IGNORED);
    assert_absent(&worktree_only, b"staged-mod.txt");
    assert_absent(&worktree_only, b"staged-delete.txt");
    assert_absent(&worktree_only, b"new-index.txt");

    let with_current = status_map(
        &repo,
        SHOW_COMBINED,
        flags | INCLUDE_UNMODIFIED,
        b"clean.txt",
    );
    assert_eq!(value(&with_current, b"clean.txt"), CURRENT);
}

#[test]
fn status_recursion_pathspec_literal_matching_and_sorting_are_deterministic() {
    let (root, repo) = new_repo("pathspec");
    write(&root.0, ".gitignore", b"ignored/\n");
    commit_all(&root.0, "ignore rules");

    write(&root.0, "untracked/nested/value.txt", b"untracked\n");
    write(&root.0, "ignored/nested/value.txt", b"ignored\n");
    write(&root.0, "wild[a].txt", b"literal wildcard\n");
    write(&root.0, "wilda.txt", b"wildcard match\n");
    write(&root.0, "Beta", b"uppercase\n");
    write(&root.0, "alpha", b"lowercase\n");

    let collapsed = status_map(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNTRACKED | INCLUDE_IGNORED,
        b"",
    );
    assert_eq!(value(&collapsed, b"untracked/"), WORKTREE_NEW);
    assert_eq!(value(&collapsed, b"ignored/"), IGNORED);
    assert_absent(&collapsed, b"untracked/nested/value.txt");
    assert_absent(&collapsed, b"ignored/nested/value.txt");

    let recursive = status_map(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNTRACKED
            | INCLUDE_IGNORED
            | RECURSE_UNTRACKED_DIRECTORIES
            | RECURSE_IGNORED_DIRECTORIES,
        b"",
    );
    assert_eq!(
        value(&recursive, b"untracked/nested/value.txt"),
        WORKTREE_NEW
    );
    assert_eq!(value(&recursive, b"ignored/nested/value.txt"), IGNORED);

    let wildcard = status_map(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNTRACKED | RECURSE_UNTRACKED_DIRECTORIES,
        b"wild[a].txt",
    );
    assert_eq!(value(&wildcard, b"wild[a].txt"), WORKTREE_NEW);
    assert_eq!(value(&wildcard, b"wilda.txt"), WORKTREE_NEW);

    let literal = status_map(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNTRACKED | RECURSE_UNTRACKED_DIRECTORIES | DISABLE_PATHSPEC_MATCH,
        b"wild[a].txt",
    );
    assert_eq!(value(&literal, b"wild[a].txt"), WORKTREE_NEW);
    assert_absent(&literal, b"wilda.txt");

    let sensitive = status_entries(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNTRACKED | SORT_CASE_SENSITIVELY,
        b"Beta\0alpha",
    );
    assert_eq!(
        sensitive
            .iter()
            .map(|entry| entry.0.as_slice())
            .collect::<Vec<_>>(),
        vec![b"Beta".as_slice(), b"alpha".as_slice()]
    );

    let insensitive = status_entries(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNTRACKED | SORT_CASE_INSENSITIVELY,
        b"Beta\0alpha",
    );
    assert_eq!(
        insensitive
            .iter()
            .map(|entry| entry.0.as_slice())
            .collect::<Vec<_>>(),
        vec![b"alpha".as_slice(), b"Beta".as_slice()]
    );
}

#[test]
fn status_rename_flags_detect_index_and_worktree_renames() {
    let (root, repo) = new_repo("renames");
    write(
        &root.0,
        "old-index.txt",
        b"exact staged rename contents with enough bytes\n",
    );
    write(
        &root.0,
        "old-worktree.txt",
        b"exact worktree rename contents with enough bytes\n",
    );
    commit_all(&root.0, "rename baseline");

    git(
        &root.0,
        &["mv", "old-index.txt", "new-index-name.txt"],
    );
    std::fs::rename(
        root.0.join("old-worktree.txt"),
        root.0.join("new-worktree-name.txt"),
    )
    .expect("rename worktree file");

    let without_renames = status_map(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNTRACKED | RECURSE_UNTRACKED_DIRECTORIES,
        b"",
    );
    assert_eq!(value(&without_renames, b"old-index.txt"), INDEX_DELETED);
    assert_eq!(value(&without_renames, b"new-index-name.txt"), INDEX_NEW);
    assert_eq!(
        value(&without_renames, b"old-worktree.txt"),
        WORKTREE_DELETED
    );
    assert_eq!(
        value(&without_renames, b"new-worktree-name.txt"),
        WORKTREE_NEW
    );

    let with_renames = status_map(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNTRACKED
            | RECURSE_UNTRACKED_DIRECTORIES
            | RENAMES_HEAD_TO_INDEX
            | RENAMES_INDEX_TO_WORKTREE,
        b"",
    );
    assert_eq!(
        value(&with_renames, b"new-index-name.txt"),
        INDEX_RENAMED
    );
    assert_eq!(
        value(&with_renames, b"new-worktree-name.txt"),
        WORKTREE_RENAMED
    );
    assert_absent(&with_renames, b"old-index.txt");
    assert_absent(&with_renames, b"old-worktree.txt");
}

#[test]
fn status_conflicts_are_reported_in_every_show_mode() {
    let (root, repo) = new_repo("conflict");
    write(&root.0, "conflict.txt", b"base\n");
    commit_all(&root.0, "base");
    let main_branch = git(&root.0, &["branch", "--show-current"]);
    git(&root.0, &["branch", "other"]);

    write(&root.0, "conflict.txt", b"main side\n");
    commit_all(&root.0, "main change");
    git(&root.0, &["checkout", "--quiet", "other"]);
    write(&root.0, "conflict.txt", b"other side\n");
    commit_all(&root.0, "other change");
    git(&root.0, &["checkout", "--quiet", &main_branch]);

    let merge = run_git(&root.0, &["merge", "--quiet", "other"]);
    assert!(
        !merge.status.success(),
        "fixture merge unexpectedly succeeded"
    );

    for show in [SHOW_COMBINED, SHOW_INDEX_ONLY, SHOW_WORKTREE_ONLY] {
        let entries = status_map(&repo, show, 0, b"");
        assert_eq!(value(&entries, b"conflict.txt"), CONFLICTED);
    }
}

#[test]
fn status_type_changes_submodule_exclusion_and_option_validation_work() {
    let (root, repo) = new_repo("types-options");
    write(&root.0, "typed", b"regular file used as a blob\n");
    write(&root.0, "typed-to-module", b"regular file becoming a gitlink\n");
    write(&root.0, "clean", b"clean\n");
    commit_all(&root.0, "baseline");

    let blob = git(&root.0, &["rev-parse", "HEAD:typed"]);
    let symlink_cache = format!("120000,{blob},typed");
    git(
        &root.0,
        &["update-index", "--cacheinfo", &symlink_cache],
    );

    let head = git(&root.0, &["rev-parse", "HEAD"]);
    let gitlink_cache = format!("160000,{head},module");
    git(
        &root.0,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &gitlink_cache,
        ],
    );
    let type_to_gitlink_cache = format!("160000,{head},typed-to-module");
    git(
        &root.0,
        &["update-index", "--cacheinfo", &type_to_gitlink_cache],
    );

    let index = status_map(&repo, SHOW_INDEX_ONLY, 0, b"");
    assert_eq!(value(&index, b"typed"), INDEX_TYPE_CHANGED);
    assert_eq!(value(&index, b"module"), INDEX_NEW);
    assert_eq!(
        value(&index, b"typed-to-module"),
        INDEX_TYPE_CHANGED
    );

    let excluded = status_map(&repo, SHOW_INDEX_ONLY, EXCLUDE_SUBMODULES, b"");
    assert_absent(&excluded, b"module");
    assert_eq!(
        value(&excluded, b"typed-to-module"),
        INDEX_TYPE_CHANGED
    );

    let combined = status_map(&repo, SHOW_COMBINED, 0, b"typed");
    assert_eq!(
        value(&combined, b"typed"),
        INDEX_TYPE_CHANGED | WORKTREE_TYPE_CHANGED
    );

    assert!(!matches!(
        repo.status(99, 0, ffi::Slice::from_slice(b"")),
        ffi::Ok(_)
    ));
    assert!(!matches!(
        repo.status(SHOW_COMBINED, 1 << 20, ffi::Slice::from_slice(b"")),
        ffi::Ok(_)
    ));
    assert!(!matches!(
        repo.status(
            SHOW_COMBINED,
            SORT_CASE_SENSITIVELY | SORT_CASE_INSENSITIVELY,
            ffi::Slice::from_slice(b"")
        ),
        ffi::Ok(_)
    ));
    assert!(!matches!(
        repo.status(
            SHOW_COMBINED,
            NO_REFRESH | UPDATE_INDEX,
            ffi::Slice::from_slice(b"")
        ),
        ffi::Ok(_)
    ));
    assert!(!matches!(
        repo.status(
            SHOW_COMBINED,
            INCLUDE_UNTRACKED,
            ffi::Slice::from_slice(b"clean\0")
        ),
        ffi::Ok(_)
    ));
}

#[test]
fn status_all_compatible_option_bits_are_accepted() {
    let (root, repo) = new_repo("all-options");
    write(&root.0, "tracked", b"tracked\n");
    commit_all(&root.0, "baseline");

    let all_compatible = INCLUDE_UNTRACKED
        | INCLUDE_IGNORED
        | INCLUDE_UNMODIFIED
        | EXCLUDE_SUBMODULES
        | RECURSE_UNTRACKED_DIRECTORIES
        | DISABLE_PATHSPEC_MATCH
        | RECURSE_IGNORED_DIRECTORIES
        | RENAMES_HEAD_TO_INDEX
        | RENAMES_INDEX_TO_WORKTREE
        | SORT_CASE_INSENSITIVELY
        | RENAMES_FROM_REWRITES
        | UPDATE_INDEX
        | INCLUDE_UNREADABLE
        | INCLUDE_UNREADABLE_AS_UNTRACKED;
    let _ = ok(repo.status(
        SHOW_COMBINED,
        all_compatible,
        ffi::Slice::from_slice(b""),
    ));
    let _ = ok(repo.status(
        SHOW_COMBINED,
        NO_REFRESH,
        ffi::Slice::from_slice(b""),
    ));
}

#[cfg(target_family = "unix")]
#[test]
fn status_raw_non_utf8_and_unreadable_paths_are_preserved() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::net::UnixListener;

    let (root, repo) = new_repo("raw-paths");
    git(&root.0, &["commit", "--allow-empty", "--quiet", "-m", "base"]);

    let raw_path = b"raw-\xff".to_vec();
    let raw_name = OsString::from_vec(raw_path.clone());
    std::fs::write(root.0.join(&raw_name), b"raw bytes\n").expect("write non-UTF-8 path");
    let _listener = UnixListener::bind(root.0.join("unreadable.socket"))
        .expect("create untrackable socket entry");

    let untracked = status_map(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNTRACKED | RECURSE_UNTRACKED_DIRECTORIES,
        b"",
    );
    assert_eq!(value(&untracked, &raw_path), WORKTREE_NEW);

    let unreadable = status_map(&repo, SHOW_COMBINED, INCLUDE_UNREADABLE, b"");
    assert_eq!(
        value(&unreadable, b"unreadable.socket"),
        WORKTREE_UNREADABLE
    );

    let unreadable_as_untracked = status_map(
        &repo,
        SHOW_COMBINED,
        INCLUDE_UNREADABLE_AS_UNTRACKED,
        b"",
    );
    assert_eq!(
        value(&unreadable_as_untracked, b"unreadable.socket"),
        WORKTREE_NEW
    );
}