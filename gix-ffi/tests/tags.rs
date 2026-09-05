use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use gix_ffi::{FfiObjectType, GixError, Repo, TagRecord};
use interoptopus::ffi;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(bare: bool) -> Self {
        let path = std::env::temp_dir().join(format!("gix-ffi-tags-{}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).expect("create fixture");
        git(&path, if bare { &["init", "--bare", "-q"] } else { &["init", "-q"] }, None);
        git(&path, &["config", "user.name", "Tag Fixture"], None);
        git(&path, &["config", "user.email", "fixture@example.com"], None);
        Self(path)
    }
    fn repo(&self) -> Repo {
        let bytes = Vec::<u8>::from(gix::path::into_bstr(self.0.clone()).into_owned());
        ok(Repo::open(bytes.as_slice().into()))
    }
    fn blob(&self, data: &[u8]) -> String {
        ok(self.repo().write_blob(data.into())).as_str().to_owned()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn git(path: &Path, args: &[&str], input: Option<&[u8]>) -> Vec<u8> {
    let mut child = Command::new("git").arg("-C").arg(path).args(args)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("run Git");
    if let Some(input) = input {
        child.stdin.take().expect("piped stdin").write_all(input).expect("write Git input");
    }
    let output = child.wait_with_output().expect("wait for Git");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout
}
fn text(path: &Path, args: &[&str]) -> String {
    String::from_utf8(git(path, args, None)).expect("Git ASCII").trim().to_owned()
}
fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value, ffi::Err(err) => panic!("unexpected FFI error: {err:?}"),
        _ => panic!("unexpected FFI panic/null"),
    }
}
fn create(repo: &Repo, name: &[u8], target_id: &str, data: &[u8], tagger: bool, force: bool)
    -> ffi::Result<ffi::String, GixError>
{
    let tagger = tagger.then(|| gix_ffi::TagSignatureRecord {
        name: b"Tagger \xff".to_vec().into(),
        email: b"tagger-\xfe@example.com".to_vec().into(),
        time_seconds: 1_700_000_000,
        time_offset_seconds: 19800,
    }).into();
    repo.create_annotated_tag(name.into(), target_id.to_owned().into(), data.into(), tagger, force)
}
fn read(repo: &Repo, tag_id: &str) -> ffi::Result<TagRecord, GixError> {
    repo.read_tag(tag_id.to_owned().into())
}
fn tag_bytes(target_id: &str, name: &[u8], body: &[u8], tagger: bool) -> Vec<u8> {
    let mut bytes = format!("object {target_id}\ntype blob\ntag ").into_bytes();
    bytes.extend_from_slice(name);
    bytes.push(b'\n');
    if tagger { bytes.extend_from_slice(b"tagger Tagger \xff <tagger-\xfe@example.com> 1700000000 +0530\n"); }
    if !body.iter().all(|byte| *byte == b'\n') { bytes.push(b'\n'); }
    bytes.extend_from_slice(body);
    bytes
}

#[test]
fn raw_annotated_tags_match_git_objects_in_normal_and_bare_repositories() {
    for bare in [false, true] {
        let f = Fixture::new(bare);
        let repo = f.repo();
        let target_id = f.blob(b"target");
        let body = b"raw \xff\0message\r\nwithout final newline";
        let name = "releases/日本語".as_bytes();
        let tag_id = ok(create(&repo, name, &target_id, body, true, false)).as_str().to_owned();
        let expected = tag_bytes(&target_id, name, body, true);
        assert_eq!(git(&f.0, &["cat-file", "tag", &tag_id], None), expected);
        let git_id = git(&f.0, &["hash-object", "-t", "tag", "--stdin"], Some(&expected));
        assert_eq!(String::from_utf8(git_id).expect("object ID").trim(), tag_id);
        assert_eq!(text(&f.0, &["rev-parse", "refs/tags/releases/日本語"]), tag_id);
        let tag = ok(read(&repo, &tag_id));
        assert_eq!(tag.id.as_str(), tag_id);
        assert_eq!(tag.target_id.as_str(), target_id);
        assert!(matches!(tag.target_type, FfiObjectType::Blob));
        assert_eq!(tag.name.into_vec(), name);
        assert_eq!(tag.message.into_vec(), body);
        let tagger = tag.tagger.into_option().expect("tagger is present");
        assert_eq!(tagger.name.into_vec(), b"Tagger \xff");
        assert_eq!(tagger.email.into_vec(), b"tagger-\xfe@example.com");
        assert_eq!(tagger.time_seconds, 1_700_000_000);
        assert_eq!(tagger.time_offset_seconds, 19800);
        assert!(tag.signature.is_none());
    }
}

#[test]
fn optional_taggers_and_empty_or_newline_only_messages_round_trip() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let target_id = f.blob(b"target");
    for (index, body) in [b"".as_slice(), b"\n", b"\n\n", b"ordinary\n"].iter().enumerate() {
        let name = format!("empty-{index}");
        let tag_id = ok(create(&repo, name.as_bytes(), &target_id, body, false, false)).as_str().to_owned();
        let tag = ok(read(&repo, &tag_id));
        assert!(tag.tagger.is_none());
        assert_eq!(tag.message.into_vec(), *body);
        assert_eq!(git(&f.0, &["cat-file", "tag", &tag_id], None), tag_bytes(&target_id, name.as_bytes(), body, false));
    }
}

#[test]
fn reads_keep_the_entire_signed_message_and_extract_git_supported_armor() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let target_id = f.blob(b"target");
    for (index, signature) in [
        b"-----BEGIN PGP SIGNATURE-----\nraw \xff\n-----END PGP SIGNATURE-----\n".as_slice(),
        b"-----BEGIN SSH SIGNATURE-----\nraw\n-----END SSH SIGNATURE-----\n",
        b"-----BEGIN SIGNED MESSAGE-----\nraw\n",
    ].iter().enumerate() {
        let mut body = b"message \xfe\n\n".to_vec();
        body.extend_from_slice(signature);
        let raw = tag_bytes(&target_id, b"raw-\xff", &body, true);
        let tag_id = String::from_utf8(git(&f.0, &["hash-object", "-t", "tag", "-w", "--stdin"], Some(&raw)))
            .expect("object ID").trim().to_owned();
        let tag = ok(read(&repo, &tag_id));
        assert_eq!(tag.name.into_vec(), b"raw-\xff");
        assert_eq!(tag.message.into_vec(), body, "signed message {index} must retain all original bytes");
        assert_eq!(tag.signature.into_option().expect("detected armor").into_vec(), *signature);
        assert_eq!(ok(repo.peel_tags(tag_id.into())).as_str(), target_id);
    }
}

#[test]
fn actual_target_kinds_and_nested_tag_peeling_match_git() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let blob_id = f.blob(b"blob");
    let direct = gix::open(&f.0).expect("open gix");
    let tree_id = direct.write_object(&gix::objs::Tree::default()).expect("empty tree").detach();
    let commit_id = direct.commit("refs/heads/main", "commit", tree_id, std::iter::empty::<gix::ObjectId>())
        .expect("write commit").detach();
    for (name, target_id, kind) in [("blob", blob_id.clone(), FfiObjectType::Blob),
        ("tree", tree_id.to_string(), FfiObjectType::Tree), ("commit", commit_id.to_string(), FfiObjectType::Commit)] {
        let tag_id = ok(create(&repo, name.as_bytes(), &target_id, b"tag", false, false)).as_str().to_owned();
        let tag = ok(read(&repo, &tag_id));
        assert_eq!(format!("{:?}", tag.target_type), format!("{kind:?}"));
        let nested_id = ok(create(&repo, format!("{name}-nested").as_bytes(), &tag_id, b"nested", false, false))
            .as_str().to_owned();
        let nested = ok(read(&repo, &nested_id));
        assert!(matches!(nested.target_type, FfiObjectType::Tag));
        assert_eq!(nested.target_id.as_str(), tag_id);
        assert_eq!(ok(repo.peel_tags(nested_id.clone().into())).as_str(),
            text(&f.0, &["rev-parse", &format!("{nested_id}^{{}}")]));
        assert_eq!(ok(repo.peel_tags(target_id.clone().into())).as_str(), target_id);
        ok(repo.create_tag_reference(format!("{name}-lightweight").as_bytes().into(), nested_id.clone().into(), false));
        assert_eq!(text(&f.0, &["rev-parse", &format!("refs/tags/{name}-lightweight")]), nested_id);
    }
}

#[test]
fn absent_guards_refuse_identical_tags_and_force_replaces_only_the_named_reference() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let first_id = f.blob(b"first");
    let second_id = f.blob(b"second");
    let tag_id = ok(create(&repo, b"release", &first_id, b"message", false, false)).as_str().to_owned();
    assert!(matches!(create(&repo, b"release", &first_id, b"message", false, false),
        ffi::Err(GixError::ReferenceConflict(_))));
    assert!(matches!(create(&repo, b"release", &second_id, b"changed", false, false),
        ffi::Err(GixError::ReferenceConflict(_))));
    assert_eq!(text(&f.0, &["rev-parse", "refs/tags/release"]), tag_id);
    ok(repo.create_tag_reference(b"light".as_slice().into(), first_id.clone().into(), false));
    assert!(matches!(repo.create_tag_reference(b"light".as_slice().into(), first_id.clone().into(), false),
        ffi::Err(GixError::ReferenceConflict(_))));
    git(&f.0, &["symbolic-ref", "refs/tags/alias", "refs/tags/light"], None);
    assert!(matches!(repo.create_tag_reference(b"alias".as_slice().into(), second_id.clone().into(), false),
        ffi::Err(GixError::ReferenceConflict(_))));
    ok(repo.create_tag_reference(b"alias".as_slice().into(), second_id.clone().into(), true));
    assert_eq!(text(&f.0, &["rev-parse", "refs/tags/alias"]), second_id);
    assert_eq!(text(&f.0, &["rev-parse", "refs/tags/light"]), first_id);
    let replaced = ok(create(&repo, b"release", &second_id, b"changed", false, true)).as_str().to_owned();
    assert_eq!(text(&f.0, &["rev-parse", "refs/tags/release"]), replaced);
}

#[test]
fn invalid_missing_wrong_kind_and_foreign_lock_failures_remain_distinct() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let target_id = f.blob(b"target");
    for name in [b"".as_slice(), b"-bad", b"bad name", b"bad..name", b"bad\0name", b"/bad"] {
        assert!(matches!(create(&repo, name, &target_id, b"valid", true, false),
            ffi::Err(GixError::InvalidReference(_))), "{name:?}");
    }
    let missing_id = "1111111111111111111111111111111111111111";
    assert!(matches!(create(&repo, b"missing", missing_id, b"valid", false, false), ffi::Err(GixError::NotFound(_))));
    assert!(matches!(read(&repo, missing_id), ffi::Err(GixError::NotFound(_))));
    assert!(matches!(read(&repo, &target_id), ffi::Err(GixError::Other(_))));
    for invalid in ["invalid", "1111111111111111111111111111111111111111111111111111111111111111"] {
        assert!(matches!(read(&repo, invalid), ffi::Err(GixError::InvalidId(_))));
    }
    ok(repo.create_tag_reference(b"locked".as_slice().into(), target_id.clone().into(), false));
    let lock_path = f.0.join(".git/refs/tags/locked.lock");
    std::fs::write(&lock_path, b"foreign lock").expect("hold lock");
    assert!(matches!(create(&repo, b"locked", &target_id, b"new", true, true), ffi::Err(GixError::ReferenceLocked(_))));
    assert_eq!(std::fs::read(&lock_path).expect("retained foreign lock"), b"foreign lock");
    assert_eq!(text(&f.0, &["rev-parse", "refs/tags/locked"]), target_id);
    std::fs::remove_file(lock_path).expect("release lock");
}

#[test]
fn malformed_and_dangling_tags_are_errors_without_hiding_corruption_as_absence() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let missing_id = "1111111111111111111111111111111111111111";
    let raw = tag_bytes(missing_id, b"dangling", b"message", true);
    let tag_id = String::from_utf8(git(&f.0, &["hash-object", "-t", "tag", "-w", "--stdin"], Some(&raw)))
        .expect("object ID").trim().to_owned();
    assert_eq!(ok(read(&repo, &tag_id)).target_id.as_str(), missing_id);
    assert!(matches!(repo.peel_tags(tag_id.into()), ffi::Err(GixError::Other(_))));
    let malformed_id = String::from_utf8(git(&f.0, &["hash-object", "--literally", "-t", "tag", "-w", "--stdin"], Some(b"broken tag\n")))
        .expect("object ID").trim().to_owned();
    assert!(matches!(read(&repo, &malformed_id), ffi::Err(GixError::Other(_))));
}


#[test]
fn raw_reference_names_match_the_host_platform_before_any_object_is_written() {
    let f = Fixture::new(false);
    let repo = f.repo();
    let target_id = f.blob(b"target");
    let name = b"raw-\xff";
    let raw = tag_bytes(&target_id, name, b"message", true);
    let expected_id = String::from_utf8(git(&f.0, &["hash-object", "-t", "tag", "--stdin"], Some(&raw)))
        .expect("object ID").trim().to_owned();
    let result = create(&repo, name, &target_id, b"message", true, false);
    #[cfg(not(unix))]
    {
        assert!(matches!(result, ffi::Err(GixError::InvalidPath(_))));
        assert!(!ok(repo.has_object(expected_id.into())), "path validation must precede object creation");
        assert!(ok(repo.references(b"".as_slice().into())).is_empty());
    }
    #[cfg(unix)]
    {
        assert_eq!(ok(result).as_str(), expected_id);
        assert_eq!(ok(read(&repo, &expected_id)).name.into_vec(), name);
    }
}

#[test]
fn concurrent_absent_tag_creation_has_one_winner() {
    let f = Fixture::new(false);
    let target_id = f.blob(b"target");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2).map(|index| {
        let barrier = barrier.clone();
        let repo = f.repo();
        let target_id = target_id.clone();
        std::thread::spawn(move || {
            barrier.wait();
            create(&repo, b"concurrent", &target_id, format!("writer {index}").as_bytes(), false, false)
        })
    }).collect::<Vec<_>>();
    let mut wins = 0;
    for thread in threads {
        match thread.join().expect("writer thread") {
            ffi::Ok(_) => wins += 1,
            ffi::Err(GixError::ReferenceConflict(_)) => {},
            result => panic!("unexpected writer outcome: {result:?}"),
        }
    }
    assert_eq!(wins, 1);
    assert_eq!(text(&f.0, &["rev-parse", "refs/tags/concurrent^{}"]), target_id);
}