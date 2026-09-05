use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gix_ffi::{DiffRecord, GixError, Repo};
use interoptopus::ffi;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("gix-ffi-diff-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).expect("create fixture");
        git(&path, &["init", "-q"]);
        git(&path, &["config", "user.name", "Diff Tests"]);
        git(&path, &["config", "user.email", "diff@example.com"]);
        git(&path, &["config", "core.autocrlf", "false"]);
        Self(path)
    }
    fn repo(&self) -> Repo {
        let bytes = Vec::<u8>::from(gix::path::into_bstr(self.0.clone()).into_owned());
        ok(Repo::open(ffi::Slice::from(bytes.as_slice())))
    }
    fn write(&self, path: &str, data: &[u8]) {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(path, data).expect("write fixture");
    }
    fn commit(&self) -> String {
        git(&self.0, &["add", "."]);
        git(&self.0, &["commit", "-qm", "fixture"]);
        git(&self.0, &["rev-parse", "HEAD"])
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value,
        ffi::Err(err) => panic!("unexpected FFI error: {err:?}"),
        _ => panic!("unexpected FFI panic/null"),
    }
}
fn git(root: &Path, arguments: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(root).args(arguments).output().expect("run git");
    assert!(out.status.success(), "git {arguments:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("git ASCII output").trim().to_owned()
}
fn diff(repo: &Repo, target: u32, patterns: &[u8]) -> DiffRecord {
    ok(repo.diff(target, patterns.into()))
}
fn patch(diff: DiffRecord) -> String {
    String::from_utf8(diff.patch.into_vec()).expect("text patch")
}

#[test]
fn targets_stats_filters_and_patches_match_real_git_without_mutating_the_index() {
    let f = Fixture::new();
    f.write("file with spaces.txt", b"before\nkeep\n");
    let head = f.commit();
    f.write("file with spaces.txt", b"staged\nkeep\n");
    f.write("staged.txt", b"new\n");
    git(&f.0, &["add", "."]);
    f.write("file with spaces.txt", b"working\nkeep\n");
    f.write("nested/new.txt", b"untracked\n");
    let index_before = std::fs::read(f.0.join(".git/index")).expect("index");
    let objects_before = git(&f.0, &["count-objects", "-v"]);
    let repo = f.repo();

    let staged = diff(&repo, 0, b"");
    assert_eq!((staged.file_count, staged.lines_added, staged.lines_deleted), (2, 2, 1));
    let staged_patch = patch(staged);
    assert!(staged_patch.contains("+staged\n"));
    f.write(".git/staged.patch", staged_patch.as_bytes());
    git(&f.0, &["apply", "--cached", "--check", "--reverse", ".git/staged.patch"]);

    let unstaged = diff(&repo, 1, b"");
    assert_eq!((unstaged.file_count, unstaged.lines_added, unstaged.lines_deleted), (1, 1, 1));
    let unstaged_patch = patch(unstaged);
    f.write(".git/unstaged.patch", unstaged_patch.as_bytes());
    git(&f.0, &["apply", "--check", "--reverse", ".git/unstaged.patch"]);

    let all = diff(&repo, 2, b"");
    assert_eq!((all.file_count, all.lines_added, all.lines_deleted), (3, 3, 1));
    let all_patch = patch(all);
    f.write(".git/all.patch", all_patch.as_bytes());
    git(&f.0, &["apply", "--check", "--reverse", ".git/all.patch"]);
    assert_eq!(diff(&repo, 2, b"nested/**").file_count, 1);
    assert_eq!(diff(&repo, 2, b":(exclude)nested/**").file_count, 2);
    assert_eq!(std::fs::read(f.0.join(".git/index")).expect("index"), index_before, "diff must not refresh/write the index");
    assert_eq!(git(&f.0, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&f.0, &["count-objects", "-v"]), objects_before, "diff must not write tree or blob objects");
}

#[test]
fn unborn_empty_binary_and_missing_final_newline_are_preserved() {
    let f = Fixture::new();
    f.write("no-newline.txt", b"one");
    f.write("empty.txt", b"");
    f.write(".gitignore", b"ignored\n");
    f.write("ignored", b"hidden\n");
    let repo = f.repo();
    assert_eq!(diff(&repo, 0, b"").file_count, 0);
    assert_eq!(diff(&repo, 1, b"").file_count, 0);
    let data = diff(&repo, 2, b"*.txt");
    assert_eq!((data.file_count, data.lines_added, data.lines_deleted), (2, 1, 0));
    let text = patch(data);
    assert!(text.contains("@@ -0,0 +1,1 @@"));
    assert!(text.contains("\\ No newline at end of file"));
    f.write(".git/no-newline.patch", text.as_bytes());
    git(&f.0, &["apply", "--check", "--reverse", ".git/no-newline.patch"]);
    f.commit();
    f.write("binary.dat", b"\0one\0two");
    let binary = diff(&repo, 2, b"binary.dat");
    assert_eq!((binary.file_count, binary.lines_added, binary.lines_deleted), (1, 0, 0));
    assert!(patch(binary).contains("Binary files /dev/null and b/binary.dat differ"));
    git(&f.0, &["add", "binary.dat"]);
    assert_eq!(diff(&repo, 0, b"binary.dat").file_count, 1);
}

#[test]
fn tree_changes_keep_ids_modes_paths_renames_and_gitlinks() {
    let f = Fixture::new();
    f.write("rename.txt", b"retained content that is unique\n");
    f.write("delete.txt", b"delete only\n");
    f.write("modify.txt", b"old\n");
    let first = f.commit();
    git(&f.0, &["mv", "rename.txt", "renamed.txt"]);
    git(&f.0, &["rm", "-q", "delete.txt"]);
    f.write("modify.txt", b"changed\n");
    f.write("binary.dat", b"\0binary");
    git(&f.0, &["add", "."]);
    git(&f.0, &["update-index", "--chmod=+x", "modify.txt"]);
    let cache_info = format!("160000,{first},submodule");
    git(&f.0, &["update-index", "--add", "--cacheinfo", &cache_info]);
    git(&f.0, &["commit", "-qm", "second"]);
    let second = git(&f.0, &["rev-parse", "HEAD"]);
    git(&f.0, &["tag", "-am", "annotated", "second"]);
    let repo = f.repo();
    let changes = ok(repo.tree_changes(first.clone().into(), "second".to_owned().into(), b"".as_slice().into())).into_vec();
    let names: Vec<_> = changes.iter().map(|entry| entry.path.clone().into_vec()).collect();
    assert_eq!(names, vec![b"binary.dat".to_vec(), b"delete.txt".to_vec(), b"modify.txt".to_vec(), b"renamed.txt".to_vec(), b"submodule".to_vec()]);
    let rename = &changes[3];
    assert_eq!(rename.kind, 4);
    assert_eq!(rename.old_path.as_ref().expect("rename source").clone().into_vec(), b"rename.txt");
    assert_eq!(rename.object_id.as_ref().expect("new id").as_str(), rename.old_object_id.as_ref().expect("old id").as_str());
    assert_eq!(changes[0].kind, 1);
    assert!(changes[0].old_object_id.is_none());
    assert_eq!(changes[1].kind, 2);
    assert!(changes[1].object_id.is_none());
    assert_eq!((changes[2].old_mode, changes[2].mode), (0o100644, 0o100755));
    assert_eq!(changes[4].mode, 0o160000);
    assert_eq!(changes[4].object_id.as_ref().expect("gitlink").as_str(), first);
    let tree_id = git(&f.0, &["rev-parse", "HEAD^{tree}"]);
    assert_eq!(ok(repo.tree_changes(first.clone().into(), tree_id.into(), b"rename.txt".as_slice().into())).into_vec().len(), 1);
    assert_eq!(ok(repo.tree_changes("".to_owned().into(), second.into(), b"".as_slice().into())).into_vec().len(), 4);
}

#[test]
fn configured_copy_detection_and_deletions_are_reported() {
    let f = Fixture::new();
    let contents = b"line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\n";
    f.write("source.txt", contents);
    let first = f.commit();
    f.write("copy.txt", contents);
    f.write("source.txt", b"line1\nline2\nline3\nline4\nline5\nline6\nline7\nedited\n");
    let second = f.commit();
    git(&f.0, &["config", "diff.renames", "copies"]);
    let repo = f.repo();
    let changes = ok(repo.tree_changes(first.into(), second.into(), b"".as_slice().into())).into_vec();
    assert!(changes.iter().any(|entry| entry.kind == 5), "configured copy detection should expose gix rewrite copies");
    std::fs::remove_file(f.0.join("copy.txt")).expect("delete file");
    let deletion = diff(&repo, 1, b"copy.txt");
    assert_eq!((deletion.file_count, deletion.lines_added, deletion.lines_deleted), (1, 0, 8));
    let text = patch(deletion);
    f.write(".git/delete.patch", text.as_bytes());
    git(&f.0, &["apply", "--check", "--reverse", ".git/delete.patch"]);
}

#[test]
fn invalid_options_and_unmerged_indices_fail_without_mutation() {
    let f = Fixture::new();
    f.write("file.txt", b"base\n");
    f.commit();
    let repo = f.repo();
    assert!(matches!(repo.diff(99, b"".as_slice().into()), ffi::Err(_)));
    assert!(matches!(repo.diff(0, b"file.txt\0".as_slice().into()), ffi::Err(_)));
    assert!(matches!(repo.tree_changes("HEAD".to_owned().into(), "does-not-exist".to_owned().into(), b"".as_slice().into()), ffi::Err(_)));
    git(&f.0, &["checkout", "-qb", "side"]);
    f.write("file.txt", b"side\n");
    f.commit();
    git(&f.0, &["checkout", "-q", "-"]);
    f.write("file.txt", b"main\n");
    f.commit();
    let merge = Command::new("git").arg("-C").arg(&f.0).args(["merge", "side"]).output().expect("merge conflict");
    assert!(!merge.status.success());
    let index = std::fs::read(f.0.join(".git/index")).expect("index");
    assert!(matches!(repo.diff(0, b"".as_slice().into()), ffi::Err(_)));
    assert_eq!(std::fs::read(f.0.join(".git/index")).expect("index"), index);
}

#[test]
fn working_tree_reversions_and_raw_patch_bytes_are_faithful() {
    let f = Fixture::new();
    f.write("binary.dat", b"\0original");
    f.write("ümlaut file.txt", b"before \xff\n");
    f.write(".gitignore", b"ignored.txt\n");
    f.commit();
    f.write("binary.dat", b"\0staged");
    git(&f.0, &["add", "binary.dat"]);
    f.write("binary.dat", b"\0original");
    let repo = f.repo();
    assert_eq!(diff(&repo, 2, b"binary.dat").file_count, 0, "staged changes reverted in the worktree must cancel, including binary content");
    assert_eq!(diff(&repo, 0, b"binary.dat").file_count, 1);
    assert_eq!(diff(&repo, 1, b"binary.dat").file_count, 1);

    f.write("ignored.txt", b"explicitly tracked\n");
    git(&f.0, &["add", "-f", "ignored.txt"]);
    assert_eq!(diff(&repo, 2, b"ignored.txt").file_count, 1, "staged additions are tracked even when ignored");

    f.write("ümlaut file.txt", b"after \xfe\n");
    let raw = diff(&repo, 1, "ümlaut file.txt".as_bytes()).patch.into_vec();
    assert!(raw.contains(&0xfe) && raw.contains(&0xff), "patch text must preserve arbitrary non-NUL bytes");
    f.write(".git/raw.patch", &raw);
    git(&f.0, &["apply", "--check", "--reverse", ".git/raw.patch"]);

    git(&f.0, &["update-index", "--chmod=+x", "ümlaut file.txt"]);
    let mode = diff(&repo, 0, "ümlaut file.txt".as_bytes()).patch.into_vec();
    assert!(mode.windows(b"old mode 100644\nnew mode 100755\n".len()).any(|part| part == b"old mode 100644\nnew mode 100755\n"));
    f.write(".git/mode.patch", &mode);
    git(&f.0, &["apply", "--cached", "--check", "--reverse", ".git/mode.patch"]);
}


#[test]
fn trailing_space_index_paths_keep_patch_filename_boundaries() {
    let f = Fixture::new();
    f.write("blob.txt", b"content\n");
    f.commit();
    git(&f.0, &["config", "core.protectNTFS", "false"]);
    let blob = git(&f.0, &["rev-parse", "HEAD:blob.txt"]);
    git(&f.0, &["update-index", "--add", "--cacheinfo", &format!("100644,{blob},trailing ")]);
    let text = patch(diff(&f.repo(), 0, b"trailing "));
    assert!(text.contains("\"a/trailing \" \"b/trailing \""));
    f.write(".git/trailing.patch", text.as_bytes());
    git(&f.0, &["apply", "--cached", "--check", "--reverse", ".git/trailing.patch"]);
}
