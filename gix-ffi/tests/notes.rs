use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gix_ffi::{GixError, NoteRecord, Repo};
use interoptopus::ffi;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new(bare: bool) -> Self {
        let path = std::env::temp_dir().join(format!("gix-ffi-notes-{}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).expect("create fixture");
        git(&path, if bare { &["init", "--bare", "-q"] } else { &["init", "-q"] });
        git(&path, &["config", "user.name", "Notes Fixture"]);
        git(&path, &["config", "user.email", "fixture@example.com"]);
        git(&path, &["config", "core.logAllRefUpdates", "always"]);
        Self(path)
    }
    fn repo(&self) -> Repo {
        let path = Vec::<u8>::from(gix::path::into_bstr(self.0.clone()).into_owned());
        ok(Repo::open(path.as_slice().into()))
    }
    fn blob(&self, bytes: &[u8]) -> String {
        ok(self.repo().write_blob(bytes.into())).as_str().to_owned()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn git(path: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git").arg("-C").arg(path).args(args).output().expect("run Git");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout
}
fn text(path: &Path, args: &[&str]) -> String {
    String::from_utf8(git(path, args)).expect("Git ASCII output").trim().to_owned()
}
fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value,
        ffi::Err(err) => panic!("unexpected FFI error: {err:?}"),
        _ => panic!("unexpected FFI panic/null"),
    }
}
fn write(repo: &Repo, object_id: &str, name: &[u8], data: &[u8], overwrite: bool) -> ffi::Result<ffi::String, GixError> {
    repo.write_note(object_id.to_owned().into(), name.into(), data.into(),
        b"Author".as_slice().into(), b"author@example.com".as_slice().into(), 1_700_000_000, 19800,
        b"Committer".as_slice().into(), b"committer@example.com".as_slice().into(), 1_700_000_100, -12600,
        overwrite)
}
fn read(repo: &Repo, object_id: &str, name: &[u8]) -> ffi::Result<NoteRecord, GixError> {
    repo.read_note(object_id.to_owned().into(), name.into())
}
fn remove(repo: &Repo, object_id: &str, name: &[u8]) -> ffi::Result<bool, GixError> {
    repo.remove_note(object_id.to_owned().into(), name.into(),
        b"Author".as_slice().into(), b"author@example.com".as_slice().into(), 1_700_000_200, 19800,
        b"Committer".as_slice().into(), b"committer@example.com".as_slice().into(), 1_700_000_300, -12600)
}

#[test]
fn exact_note_bytes_ids_and_distinct_signatures_match_git_in_normal_and_bare_repositories() {
    for bare in [false, true] {
        let f = Fixture::new(bare);
        let repo = f.repo();
        let annotated_object_id = f.blob(b"annotated");
        let bytes = b"raw \xff\0note\r\nwithout final LF";
        let note_blob_id = ok(write(&repo, &annotated_object_id, b"", bytes, false)).as_str().to_owned();
        assert_eq!(git(&f.0, &["cat-file", "blob", &note_blob_id]), bytes);
        assert_eq!(git(&f.0, &["notes", "show", &annotated_object_id]), bytes);
        let note = ok(read(&repo, &annotated_object_id, b"refs/notes/commits"));
        assert_eq!(note.message.into_vec(), bytes);
        assert_eq!(note.id.as_str(), note_blob_id);
        assert_eq!(note.author_name.into_vec(), b"Author");
        assert_eq!(note.author_time_seconds, 1_700_000_000);
        assert_eq!(note.author_time_offset_seconds, 19800);
        assert_eq!(note.committer_name.into_vec(), b"Committer");
        assert_eq!(note.committer_time_offset_seconds, -12600);
        assert_eq!(text(&f.0, &["show", "-s", "--format=%an|%ae|%at|%cn|%ce|%ct", "refs/notes/commits"]),
            "Author|author@example.com|1700000000|Committer|committer@example.com|1700000100");
        git(&f.0, &["notes", "--ref=refs/notes/git", "add", "-C", &note_blob_id, &annotated_object_id]);
        assert_eq!(text(&f.0, &["rev-parse", "refs/notes/git^{tree}"]),
            text(&f.0, &["rev-parse", "refs/notes/commits^{tree}"]));
    }
}

#[test]
fn overwrite_refuses_without_mutation_and_removal_keeps_an_empty_notes_commit() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let object_id = f.blob(b"annotated");
    assert!(matches!(read(&repo, &object_id, b""), ffi::Err(GixError::NotFound(_))));
    assert!(ok(repo.enumerate_notes(b"".as_slice().into())).is_empty());
    assert!(!ok(remove(&repo, &object_id, b"")));
    ok(write(&repo, &object_id, b"", b"first", false));
    let first_commit = text(&f.0, &["rev-parse", "refs/notes/commits"]);
    assert!(matches!(write(&repo, &object_id, b"", b"second", false), ffi::Err(GixError::ReferenceConflict(_))));
    assert_eq!(text(&f.0, &["rev-parse", "refs/notes/commits"]), first_commit);
    ok(write(&repo, &object_id, b"", b"", true));
    assert!(ok(read(&repo, &object_id, b"")).message.is_empty(), "empty notes remain actual blobs");
    let second_commit = text(&f.0, &["rev-parse", "refs/notes/commits"]);
    assert_eq!(text(&f.0, &["rev-parse", "refs/notes/commits^"]), first_commit);
    assert!(ok(remove(&repo, &object_id, b"")));
    assert_eq!(text(&f.0, &["rev-parse", "refs/notes/commits^"]), second_commit);
    assert!(ok(repo.enumerate_notes(b"".as_slice().into())).is_empty());
    assert!(!ok(remove(&repo, &object_id, b"")));
    assert_eq!(text(&f.0, &["rev-parse", "refs/notes/commits^{tree}"]),
        gix::ObjectId::empty_tree(gix::hash::Kind::Sha1).to_string());
}

#[test]
fn enumeration_reads_git_fanout_in_id_order_and_preserves_non_note_entries_on_edit() {
    let f = Fixture::new(false);
    let direct = gix::open(&f.0).expect("open gix");
    let first_id = direct.write_blob(b"one").expect("first blob").detach();
    let second_id = direct.write_blob(b"two").expect("second blob").detach();
    let note_blob_id = direct.write_blob(b"fanout bytes\xff").expect("note blob").detach();
    let extra_blob_id = direct.write_blob(b"not a note").expect("extra blob").detach();
    let mut tree = direct.edit_tree(gix::ObjectId::empty_tree(direct.object_hash())).expect("empty tree");
    let first_hex = first_id.to_string();
    let first_path = format!("{}/{}/{}", &first_hex[..2], &first_hex[2..4], &first_hex[4..]);
    tree.upsert(first_path, gix::objs::tree::EntryKind::Blob, note_blob_id).expect("fanout entry");
    tree.upsert(second_id.to_string(), gix::objs::tree::EntryKind::Blob, note_blob_id).expect("flat entry");
    tree.upsert("README", gix::objs::tree::EntryKind::Blob, extra_blob_id).expect("non-note entry");
    let tree_id = tree.write().expect("write notes tree").detach();
    direct.commit("refs/notes/fanout", "fixture", tree_id, std::iter::empty::<gix::ObjectId>()).expect("commit fixture");
    let repo = f.repo();
    let entries = ok(repo.enumerate_notes(b"refs/notes/fanout".as_slice().into())).into_vec();
    let listed = git(&f.0, &["notes", "--ref=refs/notes/fanout", "list"]);
    let expected = entries.iter().map(|entry| format!("{} {}\n",
        entry.note.id.as_str(), entry.annotated_object_id.as_str())).collect::<String>();
    assert_eq!(listed, expected.as_bytes());
    assert_eq!(entries.len(), 2);
    assert_eq!(ok(read(&repo, &first_hex, b"refs/notes/fanout")).message.into_vec(), b"fanout bytes\xff");
    ok(write(&repo, &first_hex, b"refs/notes/fanout", b"updated", true));
    assert_eq!(git(&f.0, &["show", "refs/notes/fanout:README"]), b"not a note");
    assert_eq!(ok(read(&repo, &second_id.to_string(), b"refs/notes/fanout")).message.into_vec(), b"fanout bytes\xff");
}

#[test]
fn symbolic_notes_refs_keep_their_links_and_foreign_locks_are_not_removed() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let object_id = f.blob(b"annotated");
    ok(write(&repo, &object_id, b"refs/notes/direct", b"initial", false));
    git(&f.0, &["symbolic-ref", "refs/notes/alias", "refs/notes/direct"]);
    ok(write(&repo, &object_id, b"refs/notes/alias", b"alias update", true));
    assert_eq!(text(&f.0, &["symbolic-ref", "refs/notes/alias"]), "refs/notes/direct");
    assert_eq!(ok(read(&repo, &object_id, b"refs/notes/direct")).message.into_vec(), b"alias update");
    let before = text(&f.0, &["rev-parse", "refs/notes/direct"]);
    let lock = f.0.join(".git/refs/notes/direct.lock");
    std::fs::write(&lock, b"foreign lock").expect("hold foreign lock");
    assert!(matches!(write(&repo, &object_id, b"refs/notes/alias", b"blocked", true), ffi::Err(GixError::ReferenceLocked(_))));
    assert_eq!(std::fs::read(&lock).expect("foreign lock retained"), b"foreign lock");
    assert_eq!(text(&f.0, &["rev-parse", "refs/notes/direct"]), before);
    assert!(!f.0.join(".git/refs/notes/alias.lock").exists(), "partial acquisition releases only owned locks");
    std::fs::remove_file(lock).expect("release test lock");
    assert!(ok(remove(&repo, &object_id, b"refs/notes/alias")));
    assert_eq!(text(&f.0, &["symbolic-ref", "refs/notes/alias"]), "refs/notes/direct");
}

#[test]
fn configured_default_disabled_default_and_invalid_inputs_are_distinct() {
    let f = Fixture::new(false);
    git(&f.0, &["config", "core.notesRef", "refs/notes/configured"]);
    let repo = f.repo();
    let object_id = f.blob(b"annotated");
    ok(write(&repo, &object_id, b"", b"configured", false));
    assert_eq!(ok(read(&repo, &object_id, b"refs/notes/configured")).message.into_vec(), b"configured");
    assert!(matches!(read(&repo, &object_id, b"refs/notes/commits"), ffi::Err(GixError::NotFound(_))));
    for invalid in [b"bad ref".as_slice(), b"refs/notes/a..b", b"refs/notes/a\0b"] {
        assert!(matches!(read(&repo, &object_id, invalid), ffi::Err(GixError::InvalidReference(_))));
    }
    assert!(matches!(read(&repo, "not-an-id", b""), ffi::Err(GixError::InvalidId(_))));
    git(&f.0, &["config", "core.notesRef", ""]);
    let disabled = f.repo();
    assert!(matches!(read(&disabled, &object_id, b""), ffi::Err(GixError::NotFound(_))));
    assert!(ok(disabled.enumerate_notes(b"".as_slice().into())).is_empty());
    assert!(!ok(remove(&disabled, &object_id, b"")));
    assert!(matches!(write(&disabled, &object_id, b"", b"disabled", false), ffi::Err(GixError::Config(_))));
    assert_eq!(ok(read(&disabled, &object_id, b"refs/notes/configured")).message.into_vec(), b"configured");
}

#[test]
fn corrupt_note_objects_are_errors_instead_of_absence() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let object_id = f.blob(b"annotated");
    let note_blob_id = ok(write(&repo, &object_id, b"", b"unique corrupt payload", false)).as_str().to_owned();
    let blob_path = f.0.join(".git/objects").join(&note_blob_id[..2]).join(&note_blob_id[2..]);
    std::fs::remove_file(blob_path).expect("remove note blob");
    assert!(matches!(read(&repo, &object_id, b""), ffi::Err(GixError::Other(_))));
    assert!(matches!(repo.enumerate_notes(b"".as_slice().into()), ffi::Err(GixError::Other(_))));
}