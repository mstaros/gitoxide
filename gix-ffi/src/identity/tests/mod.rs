use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gix::bstr::ByteSlice;
use interoptopus::ffi;

use crate::{GixError, Repo, SignatureRecord};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture { root: PathBuf, bare: bool }

impl Fixture {
    fn new(bare: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "gix-ffi-identity-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let bytes = Vec::from(gix::path::into_bstr(root.clone()).into_owned());
        drop(ok(Repo::create(slice(&bytes), bare)));
        Self { root, bare }
    }

    fn config(&self) -> PathBuf {
        if self.bare { self.root.join("config") } else { self.root.join(".git/config") }
    }

    fn open(&self, overrides: &[&str]) -> Repo {
        let options = gix::open::Options::isolated().config_overrides(
            ["gitoxide.commit.authorDate=1700000000 +0530",
             "gitoxide.commit.committerDate=1700000000 +0530"].into_iter().chain(overrides.iter().copied()));
        Repo { inner: gix::ThreadSafeRepository::open_opts(&self.root, options).expect("open isolated fixture") }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); }
}

fn slice(bytes: &[u8]) -> ffi::Slice<'_, u8> { ffi::Slice::from_slice(bytes) }

fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value,
        ffi::Err(error) => panic!("unexpected FFI error: {error:?}"),
        _ => panic!("unexpected panic or null marker"),
    }
}

fn assert_signature(record: &SignatureRecord, name: &[u8], email: &[u8]) {
    assert!(record.is_present, "a complete signature is present");
    assert_eq!(record.name.clone().into_vec(), name, "name bytes remain exact");
    assert_eq!(record.email.clone().into_vec(), email, "email bytes remain exact");
    assert_eq!((record.time_seconds, record.time_offset_seconds), (1_700_000_000, 19_800));
}

#[test]
fn fallback_is_owned_retained_in_the_session_and_never_written_to_disk() {
    for bare in [false, true] {
        let fixture = Fixture::new(bare);
        let original = std::fs::read(fixture.config()).expect("read original config");
        let mut repo = fixture.open(&[]);
        assert!(!ok(repo.get_author()).is_present);
        assert!(!ok(repo.get_committer()).is_present);
        let name = b"raw-\xff-name";
        let email = b"raw-\xfe@example.com";
        let fallback = ok(repo.get_committer_or_set_fallback(slice(name), slice(email)));
        assert_signature(&fallback, name, email);
        assert_signature(&ok(repo.get_committer()), name, email);
        assert_signature(&ok(repo.get_committer_or_set_generic_fallback()), name, email);
        assert!(!ok(repo.get_author()).is_present, "committer fallback does not configure author");
        assert_eq!(std::fs::read(fixture.config()).expect("read config"), original);
        assert!(!ok(fixture.open(&[]).get_committer()).is_present, "a new handle has no session fallback");
        drop(repo);
        assert_signature(&fallback, name, email);
    }
}

#[test]
fn partial_identities_keep_each_configured_component_and_generic_fallback_matches_core() {
    let fixture = Fixture::new(false);
    let mut named = fixture.open(&["user.name=Configured Name"]);
    assert!(!ok(named.get_committer()).is_present);
    assert_signature(&ok(named.get_committer_or_set_fallback(slice(b"Fallback"), slice(b"fallback@example.com"))),
        b"Configured Name", b"fallback@example.com");
    assert_signature(&ok(named.get_committer()), b"Configured Name", b"fallback@example.com");
    let mut emailed = fixture.open(&["committer.email=configured@example.com"]);
    assert_signature(&ok(emailed.get_committer_or_set_generic_fallback()),
        b"no name configured", b"configured@example.com");
    let mut empty = fixture.open(&[]);
    assert_signature(&ok(empty.get_committer_or_set_generic_fallback()),
        b"no name configured", b"noEmailAvailable@example.com");
}

#[test]
fn configured_roles_win_and_are_cached_without_installing_unused_fallbacks() {
    let fixture = Fixture::new(false);
    let mut repo = fixture.open(&[
        "user.name=User", "user.email=user@example.com",
        "author.name=Author", "committer.email=committer@example.com",
    ]);
    assert_signature(&ok(repo.get_author()), b"Author", b"user@example.com");
    assert_signature(&ok(repo.get_committer()), b"User", b"committer@example.com");
    assert_signature(&ok(repo.get_committer_or_set_fallback(slice(b"unused"), slice(b"unused@example.com"))),
        b"User", b"committer@example.com");
    assert!(repo.inner.to_thread_local().config_snapshot().string("gitoxide.committer.nameFallback").is_none(),
        "a resolved committer must not install fallback configuration");
    let mut present_empty = fixture.open(&["user.name=", "user.email="]);
    let identity = ok(present_empty.get_committer());
    assert_signature(&identity, b"", b"");
    assert_signature(&ok(present_empty.get_committer_or_set_generic_fallback()), b"", b"");
}

#[test]
fn configured_result_errors_remain_errors_instead_of_missing_identities() {
    let error = gix::config::time::Error {
        key: "gitoxide.commit.committerDate".into(),
        value: Some("invalid date".into()),
        environment_override: Some("GIT_COMMITTER_DATE"),
        source: None,
    };
    assert!(matches!(super::configured(Some(Err(error))), Err(GixError::Config(_))),
        "Some(Err) is never mapped to absent identity or fallback");
    assert!(!super::configured(None).expect("absence is not an error").is_present);
}

#[test]
fn malformed_configured_dates_keep_the_current_gix_fallback_behavior() {
    let fixture = Fixture::new(false);
    let mut repo = fixture.open(&[
        "user.name=User", "user.email=user@example.com", "gitoxide.commit.committerDate=invalid",
    ]);
    let identity = ok(repo.get_committer());
    assert!(identity.is_present, "the current gix date parser falls back to now");
    let fallback = ok(repo.get_committer_or_set_generic_fallback());
    assert_eq!(fallback.name.clone().into_vec(), b"User");
    assert_eq!(fallback.time_seconds, identity.time_seconds, "lazy persona state survives the FFI call");
}

fn resolve(repo: &Repo, name: &[u8], email: &[u8], only_mapped: bool) -> SignatureRecord {
    if only_mapped {
        repo.try_resolve_mailmap(slice(name), slice(email), 1_700_000_000, 19_800)
    } else {
        repo.resolve_mailmap(slice(name), slice(email), 1_700_000_000, 19_800)
    }
}

fn git_check_mailmap(root: &Path, signature: &str) -> Vec<u8> {
    let output = std::process::Command::new("git").arg("-C").arg(root)
        .args(["check-mailmap", signature]).output().expect("run git check-mailmap");
    assert!(output.status.success(), "git check-mailmap: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout.trim_end().to_vec()
}

#[test]
fn mailmap_preserves_raw_bytes_time_and_lenient_partial_results() {
    let fixture = Fixture::new(false);
    std::fs::write(fixture.root.join(".mailmap"),
        b"malformed line\nMapped-\xff <mapped-\xfe@example.com> Original-\xfd <old@example.com>\nPlain <plain@example.com> <plain-old@example.com>\n")
        .expect("write mailmap");
    let mut repo = fixture.open(&["mailmap.blob=does-not-exist"]);
    let mapped = resolve(&repo, b"Original-\xfd", b"old@example.com", true);
    assert_signature(&mapped, b"Mapped-\xff", b"mapped-\xfe@example.com");
    assert_signature(&resolve(&repo, b"Original-\xfd", b"old@example.com", false),
        b"Mapped-\xff", b"mapped-\xfe@example.com");
    assert!(!resolve(&repo, b"Unknown", b"unknown@example.com", true).is_present);
    assert_signature(&resolve(&repo, b"Unknown", b"unknown@example.com", false), b"Unknown", b"unknown@example.com");
    assert_eq!(git_check_mailmap(&fixture.root, "Old <plain-old@example.com>"), b"Plain <plain@example.com>");
    // Mailmap reads must not discard a separately installed in-memory identity fallback.
    ok(repo.get_committer_or_set_fallback(slice(b"Fallback"), slice(b"fallback@example.com")));
    let _ = resolve(&repo, b"Unknown", b"unknown@example.com", false);
    assert_signature(&ok(repo.get_committer()), b"Fallback", b"fallback@example.com");
    drop(repo);
    assert_signature(&mapped, b"Mapped-\xff", b"mapped-\xfe@example.com");
}

#[test]
fn mailmap_source_precedence_bare_head_and_email_case_follow_gix() {
    let fixture = Fixture::new(false);
    std::fs::write(fixture.root.join(".mailmap"), b"Worktree <worktree@example.com> <old@example.com>\n")
        .expect("write worktree mailmap");
    let repo = fixture.open(&[]);
    let blob_id = ok(repo.write_blob(slice(b"Blob <blob@example.com> <old@example.com>\n")));
    let configured_path = fixture.root.join("configured.mailmap");
    std::fs::write(&configured_path, b"File <file@example.com> <old@example.com>\n<Case@Example.com> <Case@Example.com>\n")
        .expect("write configured mailmap");
    let blob_option = format!("mailmap.blob={}", blob_id.as_str());
    let file_option = format!("mailmap.file={}", configured_path.to_string_lossy().replace('\\', "/"));
    let repo = fixture.open(&[&blob_option, &file_option]);
    assert_signature(&resolve(&repo, b"Old", b"old@example.com", true), b"File", b"file@example.com");
    assert_signature(&resolve(&repo, b"Name", b"case@example.com", true), b"Name", b"Case@Example.com");

    let bare = Fixture::new(true);
    let repo = bare.open(&[]);
    let core = repo.inner.to_thread_local();
    let blob_id = core.write_blob(b"Bare <bare@example.com> <old@example.com>\n").expect("write bare mailmap blob");
    let mut tree = core.edit_tree(gix::ObjectId::empty_tree(core.object_hash())).expect("edit tree");
    tree.upsert(".mailmap", gix::objs::tree::EntryKind::Blob, blob_id).expect("insert mailmap");
    let tree_id = tree.write().expect("write tree");
    let sig = gix::actor::SignatureRef { name: b"Fixture".as_bstr(), email: b"fixture@example.com".as_bstr(), time: "1700000000 +0000" };
    core.commit_as(sig, sig, "HEAD", "mailmap", tree_id, std::iter::empty::<gix::ObjectId>())
        .expect("create bare HEAD");
    assert_signature(&resolve(&repo, b"Old", b"old@example.com", true), b"Bare", b"bare@example.com");
}