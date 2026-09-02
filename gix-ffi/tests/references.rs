use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gix_ffi::{
    BranchRecord, GixError, ReferenceLockLease, ReferenceRecord, ReferenceUpdateOutcome, Repo,
};
use interoptopus::ffi;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gix-ffi-references-{label}-{}-{sequence}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("remove stale reference test directory");
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
                | GixError::ReferenceLocked(value)
                | GixError::Other(value) => value,
            };
            panic!("unexpected FFI error: {}", message.as_str());
        }
        ffi::Result::Panic => panic!("unexpected panic marker"),
        ffi::Result::Null => panic!("unexpected null marker"),
    }
}

fn reopen(root: &Path) -> Repo {
    let root = bytes(root);
    ok(Repo::open(ffi::Slice::from_slice(&root)))
}

fn append_identity(root: &Path) {
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(".git/config"))
        .expect("open repository config");
    config
        .write_all(b"\n[user]\n\tname = Reference Tests\n\temail = references@example.com\n")
        .expect("append repository identity");
}

fn create_commit(
    repo: &Repo,
    message_text: &str,
    tree: &str,
    parents: &[&str],
    update_reference: &[u8],
    timestamp: i64,
) -> String {
    let parent_ids = ffi::Vec::from(
        parents
            .iter()
            .map(|parent| string(parent))
            .collect::<Vec<_>>(),
    );
    ok(repo.create_commit_object(
        string(message_text),
        string(tree),
        parent_ids,
        ffi::Slice::from_slice(update_reference),
        true,
        ffi::Slice::from_slice(b"Reference Author"),
        ffi::Slice::from_slice(b"author@example.com"),
        timestamp,
        0,
        true,
        ffi::Slice::from_slice(b"Reference Committer"),
        ffi::Slice::from_slice(b"committer@example.com"),
        timestamp,
        0,
    ))
    .as_str()
    .to_owned()
}

fn new_repo(label: &str) -> (TempDir, Repo, String, String) {
    let root = TempDir::new(label);
    let root_bytes = bytes(&root.0);
    let created = ok(Repo::create(
        ffi::Slice::from_slice(&root_bytes),
        false,
    ));
    drop(created);
    append_identity(&root.0);

    let repo = reopen(&root.0);
    ok(repo.set_head(ffi::Slice::from_slice(b"main")));
    let direct = gix::open(&root.0).expect("open repository directly");
    let tree = gix::ObjectId::empty_tree(direct.object_hash()).to_string();
    let first = create_commit(&repo, "first", &tree, &[], b"HEAD", 100);
    let second = create_commit(&repo, "second", &tree, &[&first], b"", 200);
    (root, repo, first, second)
}

fn references(repo: &Repo, glob: &[u8]) -> BTreeMap<Vec<u8>, ReferenceRecord> {
    ok(repo.references(ffi::Slice::from_slice(glob)))
        .into_vec()
        .into_iter()
        .map(|record| (record.name.clone().into_vec(), record))
        .collect()
}

fn branches(repo: &Repo, filter: u32) -> BTreeMap<(Vec<u8>, bool), BranchRecord> {
    ok(repo.branches(filter))
        .into_vec()
        .into_iter()
        .map(|record| ((record.name.clone().into_vec(), record.is_remote), record))
        .collect()
}

#[test]
fn enumeration_preserves_direct_symbolic_packed_and_filtered_references() {
    let (root, repo, first, second) = new_repo("enumeration");

    assert!(ok(repo.try_create_reference(
        ffi::Slice::from_slice(b"refs/remotes/origin/main"),
        string(&first),
    )));
    std::fs::write(
        root.0.join(".git/refs/heads/symbolic"),
        b"ref: refs/heads/main\n",
    )
    .expect("write symbolic branch");
    std::fs::write(
        root.0.join(".git/packed-refs"),
        format!(
            "# pack-refs with: sorted\n{second} refs/heads/packed\n{first} refs/tags/v1\n"
        ),
    )
    .expect("write packed references");
    drop(repo);

    let repo = reopen(&root.0);
    let all = references(&repo, b"");
    let names = all.keys().cloned().collect::<Vec<_>>();
    let mut sorted_names = names.clone();
    sorted_names.sort();
    assert_eq!(names, sorted_names, "reference records must be deterministic");

    let main = all
        .get(b"refs/heads/main".as_slice())
        .expect("main reference");
    assert!(main.has_target);
    assert_eq!(main.target.as_str(), first);
    assert!(!main.has_symbolic_target);

    let symbolic = all
        .get(b"refs/heads/symbolic".as_slice())
        .expect("symbolic reference");
    assert!(!symbolic.has_target);
    assert!(symbolic.has_symbolic_target);
    assert_eq!(
        symbolic.symbolic_target.clone().into_vec(),
        b"refs/heads/main".to_vec()
    );

    let packed = all
        .get(b"refs/heads/packed".as_slice())
        .expect("packed branch");
    assert!(packed.has_target);
    assert_eq!(packed.target.as_str(), second);

    let tag = all.get(b"refs/tags/v1".as_slice()).expect("packed tag");
    assert_eq!(tag.target.as_str(), first);

    let remote = all
        .get(b"refs/remotes/origin/main".as_slice())
        .expect("remote-tracking reference");
    assert_eq!(remote.target.as_str(), first);

    let heads = references(&repo, b"refs/heads/*");
    assert!(heads.keys().all(|name| name.starts_with(b"refs/heads/")));
    assert!(heads.contains_key(b"refs/heads/main".as_slice()));
    assert!(heads.contains_key(b"refs/heads/packed".as_slice()));
    assert!(heads.contains_key(b"refs/heads/symbolic".as_slice()));
    assert!(!heads.contains_key(b"refs/remotes/origin/main".as_slice()));

    let local = branches(&repo, 1);
    assert!(local.contains_key(&(b"main".to_vec(), false)));
    assert!(local.contains_key(&(b"packed".to_vec(), false)));
    let symbolic_branch = local
        .get(&(b"symbolic".to_vec(), false))
        .expect("symbolic local branch");
    assert!(!symbolic_branch.has_target);

    let remote = branches(&repo, 2);
    assert_eq!(remote.len(), 1);
    assert!(remote.contains_key(&(b"origin/main".to_vec(), true)));

    let all_branches = branches(&repo, 3);
    assert_eq!(all_branches.len(), local.len() + remote.len());
    assert!(matches!(
        repo.branches(0),
        ffi::Err(GixError::Other(_))
    ));
}

#[test]
fn branch_mutation_head_states_and_validation_match_the_managed_contract() {
    let (root, repo, first, second) = new_repo("branches");

    let created = ok(repo.create_branch(
        ffi::Slice::from_slice(b"topic"),
        string(&second),
        false,
    ));
    assert_eq!(created.name.clone().into_vec(), b"topic".to_vec());
    assert_eq!(created.target.as_str(), second);

    assert!(matches!(
        repo.create_branch(
            ffi::Slice::from_slice(b"topic"),
            string(&first),
            false,
        ),
        ffi::Err(GixError::ReferenceConflict(_))
    ));

    let replaced = ok(repo.create_branch(
        ffi::Slice::from_slice(b"topic"),
        string(&first),
        true,
    ));
    assert_eq!(replaced.target.as_str(), first);

    ok(repo.set_head(ffi::Slice::from_slice(b"refs/heads/topic")));
    let topic_head = ok(repo.head());
    assert!(!topic_head.is_detached);
    assert!(!topic_head.is_unborn);
    assert_eq!(topic_head.referent.clone().into_vec(), b"refs/heads/topic".to_vec());
    assert!(matches!(
        repo.delete_branch(ffi::Slice::from_slice(b"topic"), false),
        ffi::Err(GixError::ReferenceConflict(_))
    ));

    ok(repo.set_head(ffi::Slice::from_slice(b"unborn")));
    let unborn = ok(repo.head());
    assert!(unborn.is_unborn);
    assert!(!unborn.is_detached);
    assert_eq!(unborn.referent.clone().into_vec(), b"refs/heads/unborn".to_vec());

    assert!(matches!(
        repo.set_head(ffi::Slice::from_slice(b"refs/tags/v1")),
        ffi::Err(GixError::InvalidReference(_))
    ));
    assert!(matches!(
        repo.set_head(ffi::Slice::from_slice(b"bad..name")),
        ffi::Err(GixError::InvalidReference(_))
    ));
    assert!(matches!(
        repo.create_branch(
            ffi::Slice::from_slice(b"bad..name"),
            string(&first),
            false,
        ),
        ffi::Err(GixError::InvalidReference(_))
    ));

    ok(repo.delete_branch(
        ffi::Slice::from_slice(b"topic"),
        false,
    ));
    assert!(matches!(
        repo.delete_branch(ffi::Slice::from_slice(b"topic"), false),
        ffi::Err(GixError::NotFound(_))
    ));

    std::fs::write(root.0.join(".git/HEAD"), format!("{first}\n"))
        .expect("detach HEAD");
    drop(repo);
    let detached = reopen(&root.0);
    let head = ok(detached.head());
    assert!(head.is_detached);
    assert!(!head.is_unborn);
    assert_eq!(head.target.as_str(), first);
    ok(detached.delete_branch(
        ffi::Slice::from_slice(b"main"),
        false,
    ));
}

#[test]
fn exact_reference_operations_preserve_expected_old_semantics_and_empty_reflog_messages() {
    let (root, repo, first, second) = new_repo("compare-exchange");
    let name = b"refs/heads/cas";

    let missing = ok(repo.try_get_reference_target(ffi::Slice::from_slice(name)));
    assert!(!missing.found);
    assert!(missing.id.as_str().is_empty());

    assert!(ok(repo.try_create_reference(
        ffi::Slice::from_slice(name),
        string(&first),
    )));
    assert!(!ok(repo.try_create_reference(
        ffi::Slice::from_slice(name),
        string(&first),
    )));

    let found = ok(repo.try_get_reference_target(ffi::Slice::from_slice(name)));
    assert!(found.found);
    assert_eq!(found.id.as_str(), first);

    assert_eq!(
        ok(repo.compare_exchange_reference(
            ffi::Slice::from_slice(name),
            string(&second),
            string(&second),
        )),
        ReferenceUpdateOutcome::Mismatch,
        "the reference exists but holds `first`, so the swap is refused"
    );
    assert_eq!(
        ok(repo.try_get_reference_target(ffi::Slice::from_slice(name)))
            .id
            .as_str(),
        first
    );

    assert_eq!(
        ok(repo.compare_exchange_reference(
            ffi::Slice::from_slice(name),
            string(&second),
            string(&first),
        )),
        ReferenceUpdateOutcome::Applied
    );
    assert_eq!(
        ok(repo.try_get_reference_target(ffi::Slice::from_slice(name)))
            .id
            .as_str(),
        second
    );

    let reflog = std::fs::read(root.0.join(".git/logs/refs/heads/cas"))
        .expect("read reference reflog");
    let lines = reflog
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert!(
        lines.iter().all(|line| !line.contains(&b'\t')),
        "null compatibility messages map to gix's message-free reflog form"
    );

    assert_eq!(
        ok(repo.delete_reference(
            ffi::Slice::from_slice(name),
            string(&first),
        )),
        ReferenceUpdateOutcome::Mismatch,
        "the reference holds `second`, so it is left alone rather than deleted"
    );
    assert_eq!(
        ok(repo.delete_reference(
            ffi::Slice::from_slice(name),
            string(&second),
        )),
        ReferenceUpdateOutcome::Applied
    );

    // The distinction the previous `bool` return could not express: both of
    // these used to answer `false`, identically to the two refusals above.
    assert_eq!(
        ok(repo.delete_reference(
            ffi::Slice::from_slice(name),
            string(&second),
        )),
        ReferenceUpdateOutcome::Absent,
        "deleting an already-deleted reference is not the same as refusing to \
         delete one that moved"
    );
    assert_eq!(
        ok(repo.compare_exchange_reference(
            ffi::Slice::from_slice(name),
            string(&first),
            string(&second),
        )),
        ReferenceUpdateOutcome::Absent,
        "swapping an absent reference is not the same as a mismatch"
    );
    assert!(
        !ok(repo.try_get_reference_target(ffi::Slice::from_slice(name))).found
    );

    assert!(matches!(
        repo.try_get_reference_target(ffi::Slice::from_slice(b"refs/heads/bad..name")),
        ffi::Err(GixError::InvalidReference(_))
    ));
}

#[test]
fn multi_reference_locks_are_ordered_atomic_and_release_on_drop() {
    let (root, repo, first, _second) = new_repo("locks");
    let repository_path = repo.info().repository_path.into_vec();
    let names = b"refs/heads/two\0refs/heads/one\0refs/heads/two";

    let lease = ok(ReferenceLockLease::acquire(
        ffi::Slice::from_slice(&repository_path),
        ffi::Slice::from_slice(names),
    ));
    let one_lock = root.0.join(".git/refs/heads/one.lock");
    let two_lock = root.0.join(".git/refs/heads/two.lock");
    assert!(one_lock.is_file());
    assert!(two_lock.is_file());
    assert!(matches!(
        repo.try_create_reference(
            ffi::Slice::from_slice(b"refs/heads/one"),
            string(&first),
        ),
        ffi::Err(GixError::ReferenceLocked(_))
    ));

    drop(lease);
    assert!(!one_lock.exists());
    assert!(!two_lock.exists());
    assert!(ok(repo.try_create_reference(
        ffi::Slice::from_slice(b"refs/heads/one"),
        string(&first),
    )));

    let external_lock = root.0.join(".git/refs/heads/z.lock");
    std::fs::write(&external_lock, b"held").expect("create external reference lock");
    assert!(matches!(
        ReferenceLockLease::acquire(
            ffi::Slice::from_slice(&repository_path),
            ffi::Slice::from_slice(b"refs/heads/z\0refs/heads/a"),
        ),
        ffi::Err(GixError::ReferenceLocked(_))
    ));
    assert!(
        !root.0.join(".git/refs/heads/a.lock").exists(),
        "partial acquisition must roll back earlier markers"
    );
    assert!(external_lock.is_file());
    std::fs::remove_file(external_lock).expect("remove external reference lock");

    assert!(matches!(
        ReferenceLockLease::acquire(
            ffi::Slice::from_slice(&repository_path),
            ffi::Slice::empty(),
        ),
        ffi::Err(GixError::InvalidReference(_))
    ));
    assert!(matches!(
        ReferenceLockLease::acquire(
            ffi::Slice::from_slice(&repository_path),
            ffi::Slice::from_slice(b"refs/heads/a\0"),
        ),
        ffi::Err(GixError::InvalidReference(_))
    ));
}

#[cfg(unix)]
#[test]
fn enumeration_preserves_non_utf8_reference_name_bytes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let (root, repo, first, _second) = new_repo("raw-name");
    let shorthand = b"raw-\xff".to_vec();
    let mut full_name = b"refs/heads/".to_vec();
    full_name.extend_from_slice(&shorthand);

    let path = root
        .0
        .join(".git/refs/heads")
        .join(OsString::from_vec(shorthand.clone()));
    std::fs::write(path, format!("{first}\n")).expect("write non-UTF8 reference");

    let all = references(&repo, b"");
    assert!(all.contains_key(&full_name));
    let local = branches(&repo, 1);
    assert!(local.contains_key(&(shorthand, false)));
}