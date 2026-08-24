use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gix::bstr::ByteSlice;
use gix_ffi::{GixError, Repo};
use interoptopus::ffi;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gix-ffi-index-{label}-{}-{sequence}",
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

fn bytes(path: &Path) -> Vec<u8> {
    Vec::from(gix::path::into_bstr(path.to_owned()).into_owned())
}

fn string(value: &str) -> ffi::String {
    ffi::String::from(value.to_owned())
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

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent directory");
    }
    std::fs::write(path, contents).expect("write fixture file");
}

fn new_repo(label: &str) -> (TempDir, Repo) {
    let root = TempDir::new(label);
    let path = bytes(&root.0);
    let repo = ok(Repo::create(ffi::Slice::from_slice(&path), false));
    (root, repo)
}

fn reopen(root: &Path) -> Repo {
    let path = bytes(root);
    ok(Repo::open(ffi::Slice::from_slice(&path)))
}

fn stage(repo: &Repo, pathspecs: &[u8]) {
    ok(repo.stage(ffi::Slice::from_slice(pathspecs)));
}

fn unstage(repo: &Repo, pathspecs: &[u8]) {
    ok(repo.unstage(ffi::Slice::from_slice(pathspecs)));
}

fn update(repo: &Repo, pathspecs: &[u8]) {
    ok(repo.update_index(ffi::Slice::from_slice(pathspecs)));
}

fn commit(repo: &Repo, message_text: &str, allow_empty: bool) -> String {
    ok(repo.create_commit_from_index(
        string(message_text),
        true,
        ffi::Slice::from_slice(b"Index Tests"),
        ffi::Slice::from_slice(b"index-tests@example.com"),
        allow_empty,
    ))
    .as_str()
    .to_owned()
}

fn ffi_entries(repo: &Repo) -> BTreeMap<Vec<u8>, Vec<u32>> {
    let mut entries = BTreeMap::<Vec<u8>, Vec<u32>>::new();
    for entry in ok(repo.index_entries()).into_vec() {
        entries
            .entry(entry.path.into_vec())
            .or_default()
            .push(entry.stage);
    }
    entries
}

fn direct_entry(
    root: &Path,
    path: &[u8],
) -> (gix::ObjectId, gix::index::entry::Mode, gix::index::entry::Flags) {
    let repo = gix::open(root).expect("open repository directly");
    let index = repo.open_index().expect("open index directly");
    let entry = index
        .entry_by_path(path.as_bstr())
        .unwrap_or_else(|| panic!("missing direct index entry {}", String::from_utf8_lossy(path)));
    (entry.id, entry.mode, entry.flags)
}

fn direct_blob(root: &Path, id: gix::ObjectId) -> Vec<u8> {
    gix::open(root)
        .expect("open repository directly")
        .find_blob(id)
        .expect("load blob directly")
        .data
        .to_vec()
}

#[test]
fn stage_honours_pathspecs_ignores_filters_and_recursive_untracked_files() {
    let (root, repo) = new_repo("stage");
    write(&root.0, ".gitattributes", b"*.txt text eol=lf\n");
    write(&root.0, ".gitignore", b"ignored.log\n");
    write(&root.0, "filtered.txt", b"line one\r\nline two\r\n");
    write(&root.0, "nested/value.txt", b"nested\r\n");
    write(&root.0, "other.bin", b"binary\r\n");
    write(&root.0, "ignored.log", b"ignored\n");

    stage(&repo, b"*.txt");
    let selected = ffi_entries(&repo);
    assert_eq!(
        selected.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([b"filtered.txt".to_vec(), b"nested/value.txt".to_vec()])
    );
    let (filtered_id, filtered_mode, _) = direct_entry(&root.0, b"filtered.txt");
    assert_eq!(filtered_mode, gix::index::entry::Mode::FILE);
    assert_eq!(
        direct_blob(&root.0, filtered_id),
        b"line one\nline two\n",
        "the clean filter must run before the blob is stored"
    );

    stage(&repo, b"");
    let all = ffi_entries(&repo);
    assert!(all.contains_key(b".gitattributes".as_slice()));
    assert!(all.contains_key(b".gitignore".as_slice()));
    assert!(all.contains_key(b"other.bin".as_slice()));
    assert!(!all.contains_key(b"ignored.log".as_slice()));
}

#[test]
fn stage_records_deletions_resolves_intent_to_add_and_respects_index_locks() {
    let (root, repo) = new_repo("stage-deletion-lock");
    write(&root.0, "delete.txt", b"delete baseline\n");
    write(&root.0, "keep.txt", b"keep baseline\n");
    stage(&repo, b"");
    let (_, _, keep_flags) = direct_entry(&root.0, b"keep.txt");
    assert!(!keep_flags.contains(gix::index::entry::Flags::INTENT_TO_ADD));
    let keep_id = direct_entry(&root.0, b"keep.txt").0;
    let _ = commit(&repo, "baseline", false);

    std::fs::remove_file(root.0.join("delete.txt")).expect("remove tracked file");
    write(&root.0, "keep.txt", b"unstaged keep modification\n");
    write(&root.0, "new.txt", b"new staged file\n");
    stage(&repo, b"delete.txt\0new.txt");
    let entries = ffi_entries(&repo);
    assert!(!entries.contains_key(b"delete.txt".as_slice()));
    assert!(entries.contains_key(b"new.txt".as_slice()));
    assert_eq!(direct_entry(&root.0, b"keep.txt").0, keep_id);

    let before_lock = std::fs::read(root.0.join(".git/index")).expect("read index before lock");
    write(&root.0, "locked.txt", b"must not enter index\n");
    std::fs::write(root.0.join(".git/index.lock"), b"held").expect("create index lock");
    assert!(matches!(
        repo.stage(ffi::Slice::from_slice(b"locked.txt")),
        ffi::Err(_)
    ));
    assert_eq!(
        std::fs::read(root.0.join(".git/index")).expect("read index after lock failure"),
        before_lock
    );
    assert!(!ffi_entries(&reopen(&root.0)).contains_key(b"locked.txt".as_slice()));
    std::fs::remove_file(root.0.join(".git/index.lock")).expect("remove index lock");

    // Seed an intent-to-add entry with gix plumbing, then prove Stage replaces
    // its empty id and extended flag with an ordinary stage-zero entry.
    write(&root.0, "intent.txt", b"intent contents\n");
    let direct = gix::open(&root.0).expect("open repository directly");
    let mut index = direct.open_index().expect("open index for intent entry");
    index.dangerously_push_entry(
        Default::default(),
        gix::ObjectId::empty_blob(direct.object_hash()),
        gix::index::entry::Flags::INTENT_TO_ADD,
        gix::index::entry::Mode::FILE,
        b"intent.txt".as_bstr(),
    );
    index.sort_entries();
    index.remove_tree();
    index.write(Default::default()).expect("write intent entry");
    let refreshed = reopen(&root.0);
    stage(&refreshed, b"intent.txt");
    let (intent_id, _, intent_flags) = direct_entry(&root.0, b"intent.txt");
    assert_ne!(intent_id, gix::ObjectId::empty_blob(direct.object_hash()));
    assert!(!intent_flags.contains(gix::index::entry::Flags::INTENT_TO_ADD));
}

#[test]
fn stage_records_embedded_repositories_as_gitlinks() {
    let (root, repo) = new_repo("gitlink");
    let module_path = root.0.join("module");
    let module_bytes = bytes(&module_path);
    let module = ok(Repo::create(
        ffi::Slice::from_slice(&module_bytes),
        false,
    ));
    let module_head = commit(&module, "module root", true);

    stage(&repo, b"module");
    let (id, mode, _) = direct_entry(&root.0, b"module");
    assert_eq!(mode, gix::index::entry::Mode::COMMIT);
    assert_eq!(id.to_string(), module_head);
}

#[test]
fn update_index_changes_only_tracked_paths_and_refresh_reads_the_physical_file() {
    let (root, repo) = new_repo("update-refresh");
    write(&root.0, "tracked.txt", b"tracked baseline\n");
    stage(&repo, b"");
    let baseline_id = direct_entry(&root.0, b"tracked.txt").0;
    let _ = commit(&repo, "baseline", false);

    write(&root.0, "tracked.txt", b"tracked update with a new size\n");
    write(&root.0, "untracked.txt", b"must remain untracked\n");
    update(&repo, b"");
    let entries = ffi_entries(&repo);
    assert!(entries.contains_key(b"tracked.txt".as_slice()));
    assert!(!entries.contains_key(b"untracked.txt".as_slice()));
    assert_ne!(direct_entry(&root.0, b"tracked.txt").0, baseline_id);
    ok(repo.refresh_index(false));

    let index_path = root.0.join(".git/index");
    let valid_index = std::fs::read(&index_path).expect("read valid index");
    std::fs::write(&index_path, b"not an index").expect("corrupt temporary index");
    assert!(matches!(repo.refresh_index(true), ffi::Err(_)));
    std::fs::write(&index_path, valid_index).expect("restore temporary index");
    ok(repo.refresh_index(true));
}

#[test]
fn unstage_restores_head_or_removes_entries_when_head_is_unborn() {
    let (root, repo) = new_repo("unstage");
    write(&root.0, "tracked.txt", b"baseline\n");
    stage(&repo, b"");
    assert!(ffi_entries(&repo).contains_key(b"tracked.txt".as_slice()));

    unstage(&repo, b"");
    assert!(ffi_entries(&repo).is_empty());
    assert_eq!(
        std::fs::read(root.0.join("tracked.txt")).expect("read unborn worktree file"),
        b"baseline\n"
    );

    stage(&repo, b"");
    let baseline_id = direct_entry(&root.0, b"tracked.txt").0;
    let _ = commit(&repo, "baseline", false);
    write(&root.0, "tracked.txt", b"working tree stays modified\n");
    write(&root.0, "new.txt", b"new file stays on disk\n");
    stage(&repo, b"");
    assert_ne!(direct_entry(&root.0, b"tracked.txt").0, baseline_id);

    unstage(&repo, b"tracked.txt");
    assert_eq!(direct_entry(&root.0, b"tracked.txt").0, baseline_id);
    assert!(ffi_entries(&repo).contains_key(b"new.txt".as_slice()));
    assert_eq!(
        std::fs::read(root.0.join("tracked.txt")).expect("read modified worktree file"),
        b"working tree stays modified\n"
    );

    unstage(&repo, b"new.txt");
    assert!(!ffi_entries(&repo).contains_key(b"new.txt".as_slice()));
    assert_eq!(
        std::fs::read(root.0.join("new.txt")).expect("read untracked worktree file"),
        b"new file stays on disk\n"
    );
}

fn inject_conflict(root: &Path, path: &[u8]) {
    let repo = gix::open(root).expect("open repository for conflict");
    let mut index = repo.open_index().expect("open index for conflict");
    index.remove_entries(|_, entry_path, _| entry_path == path.as_bstr());

    for (stage, contents) in [
        (gix::index::entry::Stage::Base, b"base\n".as_slice()),
        (gix::index::entry::Stage::Ours, b"ours\n".as_slice()),
        (gix::index::entry::Stage::Theirs, b"theirs\n".as_slice()),
    ] {
        let id = repo.write_blob(contents).expect("write conflict blob").detach();
        index.dangerously_push_entry(
            Default::default(),
            id,
            gix::index::entry::Flags::from_stage(stage),
            gix::index::entry::Mode::FILE,
            path.as_bstr(),
        );
    }
    index.sort_entries();
    index.remove_tree();
    index.write(Default::default()).expect("write conflict index");
}

#[test]
fn conflicts_keep_all_stages_block_write_tree_and_can_resolve_as_deleted() {
    let (root, repo) = new_repo("conflict");
    write(&root.0, "conflict.txt", b"baseline\n");
    stage(&repo, b"");
    let _ = commit(&repo, "baseline", false);
    inject_conflict(&root.0, b"conflict.txt");
    let repo = reopen(&root.0);

    assert_eq!(
        ffi_entries(&repo).get(b"conflict.txt".as_slice()),
        Some(&vec![1, 2, 3])
    );
    assert!(matches!(repo.write_index_tree(), ffi::Err(GixError::Other(_))));

    std::fs::write(root.0.join(".git/index.lock"), b"held").expect("create index lock");
    assert!(matches!(
        repo.resolve_conflict_as_deleted(ffi::Slice::from_slice(b"conflict.txt")),
        ffi::Err(_)
    ));
    assert_eq!(
        ffi_entries(&reopen(&root.0)).get(b"conflict.txt".as_slice()),
        Some(&vec![1, 2, 3])
    );
    std::fs::remove_file(root.0.join(".git/index.lock")).expect("remove index lock");

    ok(repo.resolve_conflict_as_deleted(ffi::Slice::from_slice(
        b"conflict.txt",
    )));
    assert!(!ffi_entries(&repo).contains_key(b"conflict.txt".as_slice()));
    assert!(matches!(
        repo.resolve_conflict_as_deleted(ffi::Slice::from_slice(b"conflict.txt")),
        ffi::Err(GixError::NotFound(_))
    ));

    let tree = ok(repo.write_index_tree());
    let tree_id = gix::ObjectId::from_hex(tree.as_str().as_bytes()).expect("parse tree id");
    assert_eq!(
        gix::open(&root.0)
            .expect("open repository")
            .find_header(tree_id)
            .expect("find written tree")
            .kind(),
        gix::objs::Kind::Tree
    );
}

#[test]
fn write_index_tree_does_not_change_head_or_the_index_file() {
    let (root, repo) = new_repo("write-tree");
    write(&root.0, "tree.txt", b"tree contents\n");
    stage(&repo, b"");
    let head_before = ok(repo.head()).target.as_str().to_owned();
    let index_before = std::fs::read(root.0.join(".git/index")).expect("read index");

    let tree = ok(repo.write_index_tree());
    assert!(!tree.as_str().is_empty());
    assert_eq!(ok(repo.head()).target.as_str(), head_before);
    assert_eq!(
        std::fs::read(root.0.join(".git/index")).expect("read index after write-tree"),
        index_before
    );
}

#[cfg(unix)]
#[test]
fn stage_preserves_executable_symlink_and_non_utf8_paths() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::{PermissionsExt, symlink};

    let (root, repo) = new_repo("unix-paths");
    write(&root.0, "script", b"#!/bin/sh\n");
    let mut permissions = std::fs::metadata(root.0.join("script"))
        .expect("stat script")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(root.0.join("script"), permissions).expect("make executable");
    symlink("script", root.0.join("link")).expect("create symlink");

    let raw_path = b"raw-\xff".to_vec();
    std::fs::write(
        root.0.join(OsString::from_vec(raw_path.clone())),
        b"raw path\n",
    )
    .expect("write non-UTF8 path");

    stage(&repo, b"");
    assert_eq!(
        direct_entry(&root.0, b"script").1,
        gix::index::entry::Mode::FILE_EXECUTABLE
    );
    let (link_id, link_mode, _) = direct_entry(&root.0, b"link");
    assert_eq!(link_mode, gix::index::entry::Mode::SYMLINK);
    assert_eq!(direct_blob(&root.0, link_id), b"script");
    assert!(ffi_entries(&repo).contains_key(&raw_path));
}