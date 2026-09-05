use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gix_ffi::{GixError, RemoteRecord, Repo};
use interoptopus::ffi;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new(bare: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "gix-ffi-remotes-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&root).expect("fixture");
        git(&root, if bare { &["init", "--bare", "-q"] } else { &["init", "-q"] });
        Self(root)
    }

    fn config(&self) -> PathBuf {
        gix::open(&self.0).expect("gix repository").common_dir().join("config")
    }

    fn append(&self, config: &[u8]) {
        OpenOptions::new().append(true).open(self.config()).expect("config")
            .write_all(config).expect("write config");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git").arg("-C").arg(root).args(args).output().expect("Git");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout
}

fn open(path: &Path) -> Repo {
    let bytes = Vec::<u8>::from(gix::path::into_bstr(path.to_owned()).into_owned());
    ok(Repo::open(ffi::Slice::from_slice(&bytes)))
}

fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value,
        ffi::Err(error) => panic!("FFI error: {error:?}"),
        _ => panic!("FFI panic/null"),
    }
}

fn remotes(repo: &Repo, resolve: bool) -> Vec<RemoteRecord> {
    ok(repo.remotes(resolve)).into_vec()
}

fn urls(values: ffi::Vec<ffi::Vec<u8>>) -> Vec<Vec<u8>> {
    values.into_vec().into_iter().map(ffi::Vec::into_vec).collect()
}

fn git_urls(root: &Path, name: &str, push: bool) -> Vec<Vec<u8>> {
    let args = if push {
        vec!["remote", "get-url", "--push", "--all", name]
    } else {
        vec!["remote", "get-url", "--all", name]
    };
    git(root, &args).split(|byte| *byte == b'\n').filter(|line| !line.is_empty())
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line).to_vec()).collect()
}

#[test]
fn metadata_keeps_missing_urls_duplicate_sections_and_deterministic_order() {
    for bare in [false, true] {
        let f = Fixture::new(bare);
        f.append(b"\n[remote \"z\"]\n prune = true\n[remote \"origin\"]\n url = ../first path\n url = user@host:repo\n[remote \"push-only\"]\n pushurl = ssh://push.example/repo\n[remote \"origin\"]\n url = ../third\n pushurl = ../push-one\n pushurl = ../push-two\n");
        let entries = remotes(&open(&f.0), false);
        let names: Vec<_> = entries.iter().map(|entry| entry.name.clone().into_vec()).collect();
        assert_eq!(names, vec![b"origin".to_vec(), b"push-only".to_vec(), b"z".to_vec()]);
        assert_eq!(git(&f.0, &["remote"]), b"origin\npush-only\nz\n");
        assert_eq!(urls(entries[0].fetch_urls.clone()), vec![
            b"../first path".to_vec(), b"user@host:repo".to_vec(), b"../third".to_vec(),
        ]);
        assert_eq!(urls(entries[0].push_urls.clone()), vec![b"../push-one".to_vec(), b"../push-two".to_vec()]);
        assert!(urls(entries[1].fetch_urls.clone()).is_empty());
        assert_eq!(urls(entries[1].push_urls.clone()), vec![b"ssh://push.example/repo".to_vec()]);
        assert!(urls(entries[2].fetch_urls.clone()).is_empty());
        assert!(urls(entries[2].push_urls.clone()).is_empty());
    }
}

#[test]
fn resolved_url_lists_match_git_rewrites_and_empty_resets() {
    let f = Fixture::new(false);
    f.append(b"\n[url \"https://fetch.example/\"]\n insteadOf = gh:\n[url \"https://team.example/\"]\n insteadOf = gh:team/\n[url \"ssh://push.example/\"]\n pushInsteadOf = gh:\n[remote \"fallback\"]\n url = gh:first\n url = gh:team/second\n[remote \"explicit\"]\n url = discarded\n url =\n url = gh:fetch\n pushurl = discarded-push\n pushurl =\n pushurl = gh:push-one\n pushurl = gh:push-two\n");
    let repo = open(&f.0);
    for entry in remotes(&repo, true) {
        let name = String::from_utf8(entry.name.into_vec()).expect("ASCII name");
        assert_eq!(urls(entry.fetch_urls), git_urls(&f.0, &name, false), "{name} fetch");
        assert_eq!(urls(entry.push_urls), git_urls(&f.0, &name, true), "{name} push");
    }
    let raw = remotes(&repo, false);
    assert_eq!(urls(raw[0].fetch_urls.clone()), vec![b"gh:fetch".to_vec()]);
    assert_eq!(urls(raw[0].push_urls.clone()), vec![b"gh:push-one".to_vec(), b"gh:push-two".to_vec()]);
    assert_eq!(urls(raw[1].fetch_urls.clone()), urls(raw[1].push_urls.clone()));
}

#[test]
fn non_utf8_names_and_values_do_not_alias_through_text_conversion() {
    let f = Fixture::new(false);
    f.append(b"\n[remote \"raw\xff\"]\n url = ../raw-\xff\n[remote \"raw\xfe\"]\n url = ../raw-\xfe\n");
    let repo = open(&f.0);
    for resolve in [false, true] {
        let entries = remotes(&repo, resolve);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name.clone().into_vec(), b"raw\xfe");
        assert_eq!(entries[1].name.clone().into_vec(), b"raw\xff");
        assert_eq!(urls(entries[0].fetch_urls.clone()), vec![b"../raw-\xfe".to_vec()]);
        assert_eq!(urls(entries[1].fetch_urls.clone()), vec![b"../raw-\xff".to_vec()]);
    }
}

#[test]
fn current_includes_and_linked_worktree_configuration_are_read_without_timestamp_caches() {
    let f = Fixture::new(false);
    git(&f.0, &["-c", "user.name=Remote Tests", "-c", "user.email=remote@example.com", "commit", "--allow-empty", "-qm", "initial"]);
    git(&f.0, &["config", "extensions.worktreeConfig", "true"]);
    let included = f.0.join("remote.config");
    std::fs::write(&included, b"[remote \"included\"]\n url = ../before\n").expect("include");
    git(&f.0, &["config", "include.path", included.to_str().expect("path")]);
    let linked = f.0.join("linked");
    git(&f.0, &["worktree", "add", "-q", "-b", "linked", linked.to_str().expect("path")]);
    git(&linked, &["config", "--worktree", "remote.private.url", "../linked"]);
    let main_repo = open(&f.0);
    let linked_repo = open(&linked);
    assert_eq!(remotes(&main_repo, false).len(), 1);
    assert_eq!(remotes(&linked_repo, false).len(), 2);
    let timestamp = std::fs::metadata(&included).expect("metadata").modified().expect("timestamp");
    let previous = remotes(&linked_repo, false);
    std::fs::write(&included, b"[remote \"included\"]\n url = ../after!\n").expect("replace include");
    OpenOptions::new().write(true).open(&included).expect("include").set_modified(timestamp).expect("restore timestamp");
    for resolve in [false, true] {
        let current = remotes(&linked_repo, resolve);
        assert_eq!(urls(current[0].fetch_urls.clone()), vec![b"../after!".to_vec()]);
    }
    drop(linked_repo);
    assert_eq!(urls(previous[0].fetch_urls.clone()), vec![b"../before".to_vec()]);
}

#[test]
fn raw_metadata_survives_invalid_typed_values_but_syntax_and_resolved_errors_are_reported() {
    let f = Fixture::new(false);
    f.append(b"\n[remote \"broken-url\"]\n url = http://[\n");
    let repo = open(&f.0);
    assert_eq!(urls(remotes(&repo, false)[0].fetch_urls.clone()), vec![b"http://[".to_vec()]);
    assert!(matches!(repo.remotes(true), ffi::Err(GixError::Config(_))));
    let config = f.config();
    let original = std::fs::read(&config).expect("config");
    OpenOptions::new().append(true).open(&config).expect("config")
        .write_all(b"\n[core]\n repositoryFormatVersion = broken\n").expect("invalid typed value");
    assert_eq!(remotes(&repo, false).len(), 1);
    assert!(matches!(repo.remotes(true), ffi::Err(GixError::Config(_))));
    std::fs::write(&config, b"[broken").expect("invalid syntax");
    assert!(matches!(repo.remotes(false), ffi::Err(GixError::Config(_))));
    std::fs::write(config, original).expect("repair");
    assert_eq!(remotes(&repo, false).len(), 1);
}