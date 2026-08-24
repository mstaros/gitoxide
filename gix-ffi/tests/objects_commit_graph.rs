use gix_ffi::{CommitRecord, FfiObjectType, GixError, Repo};
use interoptopus::ffi;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "gix-ffi-objects-{label}-{}-{sequence}",
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
                | GixError::Other(value) => value,
            };
            panic!("unexpected FFI error: {}", message.as_str());
        }
        ffi::Result::Panic => panic!("unexpected panic marker"),
        ffi::Result::Null => panic!("unexpected null marker"),
    }
}

fn new_repo(label: &str) -> (TempDir, Repo, String) {
    let root = TempDir::new(label);
    let root_path = bytes(&root.0);
    let repo = ok(Repo::create(ffi::Slice::from_slice(&root_path), false));
    let direct = gix::open(&root.0).expect("open repository directly");
    let tree = gix::ObjectId::empty_tree(direct.object_hash()).to_string();
    (root, repo, tree)
}

#[allow(clippy::too_many_arguments)]
fn create_commit(
    repo: &Repo,
    message: &str,
    tree: &str,
    parents: &[&str],
    update_reference: &str,
    author_name: &str,
    author_email: &str,
    author_seconds: i64,
    author_offset: i32,
    committer_name: &str,
    committer_email: &str,
    committer_seconds: i64,
    committer_offset: i32,
) -> String {
    let parent_ids = ffi::Vec::from(
        parents
            .iter()
            .map(|parent| string(parent))
            .collect::<Vec<_>>(),
    );
    let id = ok(repo.create_commit_object(
        string(message),
        string(tree),
        parent_ids,
        ffi::Slice::from_slice(update_reference.as_bytes()),
        true,
        ffi::Slice::from_slice(author_name.as_bytes()),
        ffi::Slice::from_slice(author_email.as_bytes()),
        author_seconds,
        author_offset,
        true,
        ffi::Slice::from_slice(committer_name.as_bytes()),
        ffi::Slice::from_slice(committer_email.as_bytes()),
        committer_seconds,
        committer_offset,
    ));
    id.as_str().to_owned()
}

fn history(
    repo: &Repo,
    revision: &str,
    excluded_revision: &str,
    max_count: u64,
    sort_flags: u32,
) -> Vec<String> {
    ok(repo.commit_history(
        string(revision),
        string(excluded_revision),
        max_count,
        sort_flags,
    ))
    .into_vec()
    .into_iter()
    .map(|id| id.as_str().to_owned())
    .collect()
}

fn git(root: &std::path::Path, arguments: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("launch git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        arguments,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn rich_commit_lookup_peels_tags_and_preserves_signatures_and_parents() {
    let (root, repo, tree) = new_repo("lookup");
    let first = create_commit(
        &repo,
        "first message",
        &tree,
        &[],
        "HEAD",
        "Author One",
        "author@example.com",
        1_700_000_000,
        5 * 3_600 + 30 * 60,
        "Committer One",
        "committer@example.com",
        1_700_000_100,
        -(7 * 3_600),
    );
    let second = create_commit(
        &repo,
        "second message",
        &tree,
        &[&first],
        "HEAD",
        "Author Two",
        "two@example.com",
        1_700_000_200,
        3_600,
        "Committer Two",
        "commit-two@example.com",
        1_700_000_300,
        -90 * 60,
    );

    let CommitRecord {
        id,
        message,
        author_name,
        author_email,
        author_time_seconds,
        author_time_offset_seconds,
        committer_name,
        committer_email,
        committer_time_seconds,
        committer_time_offset_seconds,
        parent_ids,
    } = ok(repo.lookup_commit(string("HEAD")));

    assert_eq!(id.as_str(), second);
    assert_eq!(message.into_vec(), b"second message");
    assert_eq!(author_name.into_vec(), b"Author Two");
    assert_eq!(author_email.into_vec(), b"two@example.com");
    assert_eq!(author_time_seconds, 1_700_000_200);
    assert_eq!(author_time_offset_seconds, 3_600);
    assert_eq!(committer_name.into_vec(), b"Committer Two");
    assert_eq!(committer_email.into_vec(), b"commit-two@example.com");
    assert_eq!(committer_time_seconds, 1_700_000_300);
    assert_eq!(committer_time_offset_seconds, -90 * 60);
    let parents = parent_ids.into_vec();
    assert_eq!(parents.len(), 1);
    assert_eq!(parents[0].as_str(), first);

    git(&root.0, &["config", "user.name", "Tagger"]);
    git(&root.0, &["config", "user.email", "tagger@example.com"]);
    git(&root.0, &["tag", "-a", "annotated", "-m", "annotated tag"]);
    let peeled = ok(repo.lookup_commit(string("annotated")));
    assert_eq!(peeled.id.as_str(), second);
}

#[test]
fn history_supports_sorting_exclusions_reverse_limits_and_merges() {
    const TOPOLOGICAL: u32 = 1;
    const TIME: u32 = 2;
    const REVERSE: u32 = 4;

    let (_root, repo, tree) = new_repo("history");
    let root = create_commit(
        &repo,
        "root",
        &tree,
        &[],
        "HEAD",
        "A",
        "a@example.com",
        100,
        0,
        "C",
        "c@example.com",
        100,
        0,
    );
    let left = create_commit(
        &repo,
        "left",
        &tree,
        &[&root],
        "HEAD",
        "A",
        "a@example.com",
        300,
        0,
        "C",
        "c@example.com",
        300,
        0,
    );
    let right = create_commit(
        &repo,
        "right",
        &tree,
        &[&root],
        "",
        "A",
        "a@example.com",
        200,
        0,
        "C",
        "c@example.com",
        200,
        0,
    );
    let merge = create_commit(
        &repo,
        "merge",
        &tree,
        &[&left, &right],
        "HEAD",
        "A",
        "a@example.com",
        400,
        0,
        "C",
        "c@example.com",
        400,
        0,
    );

    assert_eq!(
        history(&repo, &merge, "", u64::MAX, TOPOLOGICAL | TIME),
        vec![merge.clone(), left.clone(), right.clone(), root.clone()]
    );
    assert_eq!(
        history(&repo, &merge, &left, u64::MAX, TOPOLOGICAL | TIME),
        vec![merge.clone(), right.clone()]
    );
    assert_eq!(
        history(
            &repo,
            &merge,
            "",
            2,
            TOPOLOGICAL | TIME | REVERSE,
        ),
        vec![root.clone(), right.clone()]
    );
    assert!(history(&repo, &merge, "", 0, 0).is_empty());
    assert!(matches!(
        repo.commit_history(string(&merge), string(""), 10, 8),
        ffi::Err(GixError::Other(_))
    ));

    let merge_commit = ok(repo.lookup_commit(string(&merge)));
    let parents = merge_commit.parent_ids.into_vec();
    assert_eq!(parents[0].as_str(), left);
    assert_eq!(parents[1].as_str(), right);
}

#[test]
fn metadata_and_identifier_failures_are_distinct() {
    let (_root, repo, tree) = new_repo("metadata");
    let commit = create_commit(
        &repo,
        "metadata",
        &tree,
        &[],
        "HEAD",
        "A",
        "a@example.com",
        100,
        0,
        "C",
        "c@example.com",
        100,
        0,
    );

    let commit_metadata = ok(repo.object_metadata(string(&commit)));
    assert!(matches!(commit_metadata.object_type, FfiObjectType::Commit));
    assert!(commit_metadata.size > 0);

    let tree_metadata = ok(repo.object_metadata(string(&tree)));
    assert!(matches!(tree_metadata.object_type, FfiObjectType::Tree));
    assert_eq!(tree_metadata.size, 0);

    assert!(matches!(
        repo.object_metadata(string("not-an-id")),
        ffi::Err(GixError::InvalidId(_))
    ));
    assert!(matches!(
        repo.object_metadata(string(
            "0000000000000000000000000000000000000000000000000000000000000000"
        )),
        ffi::Err(GixError::InvalidId(_))
    ));
    assert!(matches!(
        repo.object_metadata(string("1111111111111111111111111111111111111111")),
        ffi::Err(GixError::NotFound(_))
    ));
    assert!(matches!(
        repo.lookup_commit(string(&tree)),
        ffi::Err(GixError::Other(_))
    ));
}

#[test]
fn ancestry_handles_equal_false_missing_and_merge_histories() {
    let (_root, repo, tree) = new_repo("ancestry");
    let root = create_commit(
        &repo,
        "root",
        &tree,
        &[],
        "HEAD",
        "A",
        "a@example.com",
        100,
        0,
        "C",
        "c@example.com",
        100,
        0,
    );
    let left = create_commit(
        &repo,
        "left",
        &tree,
        &[&root],
        "HEAD",
        "A",
        "a@example.com",
        200,
        0,
        "C",
        "c@example.com",
        200,
        0,
    );
    let right = create_commit(
        &repo,
        "right",
        &tree,
        &[&root],
        "",
        "A",
        "a@example.com",
        300,
        0,
        "C",
        "c@example.com",
        300,
        0,
    );
    let merge = create_commit(
        &repo,
        "merge",
        &tree,
        &[&left, &right],
        "",
        "A",
        "a@example.com",
        400,
        0,
        "C",
        "c@example.com",
        400,
        0,
    );

    assert!(ok(repo.is_ancestor_of(string(&root), string(&merge))));
    assert!(ok(repo.is_ancestor_of(string(&left), string(&merge))));
    assert!(ok(repo.is_ancestor_of(string(&right), string(&merge))));
    assert!(ok(repo.is_ancestor_of(string(&merge), string(&merge))));
    assert!(!ok(repo.is_ancestor_of(string(&right), string(&left))));
    assert!(matches!(
        repo.is_ancestor_of(
            string("1111111111111111111111111111111111111111"),
            string(&merge),
        ),
        ffi::Err(GixError::NotFound(_))
    ));
}

#[test]
fn ref_updates_tree_lookup_and_index_commits_are_real() {
    let (root, repo, tree) = new_repo("writes");
    let first = create_commit(
        &repo,
        "first",
        &tree,
        &[],
        "HEAD",
        "A",
        "a@example.com",
        100,
        0,
        "C",
        "c@example.com",
        100,
        0,
    );
    assert_eq!(
        ok(repo.commit_tree_id(string("HEAD"))).as_str(),
        tree
    );

    let topic = create_commit(
        &repo,
        "topic",
        &tree,
        &[&first],
        "refs/heads/topic",
        "A",
        "a@example.com",
        200,
        0,
        "C",
        "c@example.com",
        200,
        0,
    );
    assert_eq!(
        ok(repo.lookup_commit(string("refs/heads/topic"))).id.as_str(),
        topic
    );

    std::fs::write(root.0.join("tracked.txt"), b"tracked contents\n")
        .expect("write tracked file");
    git(&root.0, &["add", "tracked.txt"]);

    let indexed = ok(repo.create_commit_from_index(
        string("index commit"),
        true,
        ffi::Slice::from_slice(b"Index Author"),
        ffi::Slice::from_slice(b"index@example.com"),
        false,
    ));
    let indexed_id = indexed.as_str().to_owned();
    let indexed_commit = ok(repo.lookup_commit(string("HEAD")));
    assert_eq!(indexed_commit.id.as_str(), indexed_id);
    assert_eq!(
        indexed_commit.parent_ids.into_vec()[0].as_str(),
        first
    );
    assert_ne!(
        ok(repo.commit_tree_id(string("HEAD"))).as_str(),
        tree
    );

    assert!(matches!(
        repo.create_commit_from_index(
            string("refuse empty"),
            true,
            ffi::Slice::from_slice(b"Index Author"),
            ffi::Slice::from_slice(b"index@example.com"),
            false,
        ),
        ffi::Err(GixError::Other(_))
    ));
    let empty = ok(repo.create_commit_from_index(
        string("allow empty"),
        true,
        ffi::Slice::from_slice(b"Index Author"),
        ffi::Slice::from_slice(b"index@example.com"),
        true,
    ));
    let empty_commit = ok(repo.lookup_commit(string(empty.as_str())));
    assert_eq!(
        empty_commit.parent_ids.into_vec()[0].as_str(),
        indexed_id
    );
}