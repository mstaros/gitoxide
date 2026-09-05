use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use gix_ffi::{GixError, Repo};
use interoptopus::ffi;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture { parent: PathBuf, root: PathBuf, bare: bool }

impl Fixture {
    fn new(bare: bool) -> Self {
        let parent = std::env::temp_dir().join(format!(
            "gix-ffi-config-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let root = parent.join("repo");
        std::fs::create_dir_all(&parent).expect("create fixture parent");
        let bytes = path_bytes(&root);
        ok(Repo::create(ffi::Slice::from_slice(&bytes), bare));
        Self { parent, root, bare }
    }
    fn config(&self) -> PathBuf { if self.bare { self.root.join("config") } else { self.root.join(".git/config") } }
    fn repo(&self) -> Repo {
        let bytes = path_bytes(&self.root);
        ok(Repo::open(ffi::Slice::from_slice(&bytes)))
    }
    fn append(&self, bytes: &[u8]) {
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).open(self.config()).expect("open config")
            .write_all(bytes).expect("append config");
    }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.parent); } }

fn path_bytes(path: &Path) -> Vec<u8> {
    Vec::from(gix::path::into_bstr(path.to_owned()).into_owned())
}
fn ok<T>(result: ffi::Result<T, GixError>) -> T {
    match result {
        ffi::Ok(value) => value,
        ffi::Err(err) => panic!("unexpected FFI error: {err:?}"),
        _ => panic!("unexpected panic or null marker"),
    }
}
fn get(repo: &Repo, key: &[u8]) -> Option<Vec<u8>> {
    match repo.get_config_string(ffi::Slice::from_slice(key)) {
        ffi::Ok(value) => Some(value.into_vec()),
        ffi::Err(GixError::NotFound(_)) => None,
        ffi::Err(err) => panic!("unexpected read error: {err:?}"),
        _ => panic!("unexpected panic or null marker"),
    }
}
fn set(repo: &mut Repo, key: &[u8], value: &[u8]) {
    ok(repo.set_config_string(ffi::Slice::from_slice(key), ffi::Slice::from_slice(value)));
}
fn delete(repo: &mut Repo, key: &[u8]) -> bool {
    ok(repo.delete_config_value(ffi::Slice::from_slice(key)))
}
fn command(root: &Path, args: &[&str]) -> Output {
    Command::new("git").arg("-C").arg(root).args(args).output().expect("run Git")
}
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = command(root, args);
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    output.stdout
}
fn git_value(root: &Path, name: &str) -> Option<Vec<u8>> {
    let output = command(root, &["config", "--includes", "--null", "--get", name]);
    if output.status.code() == Some(1) { return None; }
    assert!(output.status.success(), "git config: {}", String::from_utf8_lossy(&output.stderr));
    Some(output.stdout.strip_suffix(&[0]).expect("NUL terminated value").to_vec())
}

#[test]
fn reads_distinguish_missing_empty_and_implicit_and_follow_git_precedence() {
    let fixture = Fixture::new(false);
    fixture.append(b"\n[wrapper]\n empty =\n implicit\n value = first\n value = second\n[wrapper \"Case\"]\n value = upper\n[wrapper \"case\"]\n value = lower\n");
    let repo = fixture.repo();
    for name in ["wrapper.missing", "wrapper.empty", "wrapper.implicit", "WRAPPER.VALUE", "wrapper.Case.value", "wrapper.case.value"] {
        assert_eq!(get(&repo, name.as_bytes()), git_value(&fixture.root, name), "{name}");
    }
    assert_eq!(get(&repo, b"wrapper.implicit"), Some(Vec::new()));
    fixture.append(b"\n[wrapper]\n value = changed\n");
    assert_eq!(get(&repo, b"wrapper.value"), Some(b"changed".to_vec()), "same handle reloads config");
}

#[test]
fn includes_and_conditional_includes_are_resolved_and_cannot_be_mutated_indirectly() {
    let fixture = Fixture::new(false);
    std::fs::write(fixture.root.join(".git/included"), b"[wrapper]\n included = from-include\n repeated = included\n").expect("write include");
    std::fs::write(fixture.root.join(".git/conditional"), b"[wrapper]\n conditional = selected\n").expect("write conditional include");
    git(&fixture.root, &["symbolic-ref", "HEAD", "refs/heads/config-test"]);
    fixture.append(b"\n[include]\n path = included\n[includeIf \"onbranch:config-test\"]\n path = conditional\n[wrapper]\n repeated = direct\n");
    let mut repo = fixture.repo();
    for name in ["wrapper.included", "wrapper.conditional", "wrapper.repeated"] {
        assert_eq!(get(&repo, name.as_bytes()), git_value(&fixture.root, name), "{name}");
    }
    let original = std::fs::read(fixture.config()).expect("read original");
    for key in [b"wrapper.included".as_slice(), b"wrapper.conditional", b"wrapper.repeated"] {
        assert!(matches!(repo.set_config_string(ffi::Slice::from_slice(key), ffi::Slice::from_slice(b"replacement")), ffi::Err(GixError::Config(_))));
        assert!(matches!(repo.delete_config_value(ffi::Slice::from_slice(key)), ffi::Err(GixError::Config(_))));
    }
    assert_eq!(std::fs::read(fixture.config()).expect("read config"), original);
    std::fs::write(fixture.root.join(".git/included"), b"[wrapper]\n included = refreshed\n").expect("update include");
    assert_eq!(get(&repo, b"wrapper.included"), Some(b"refreshed".to_vec()));
    git(&fixture.root, &["symbolic-ref", "HEAD", "refs/heads/another"]);
    assert_eq!(get(&repo, b"wrapper.conditional"), None, "branch changes alter includeIf matching");
}

#[test]
fn writes_and_deletes_preserve_values_and_unrelated_bytes_in_normal_and_bare_repositories() {
    for bare in [false, true] {
        let fixture = Fixture::new(bare);
        fixture.append(b"\n# untouched \xff\r\n[unrelated \"Case\"]\r\n value = \" bytes \xfe \" # keep\r\n[wrapper]\r\n implicit\r\n");
        let mut repo = fixture.repo();
        for value in [
            b"".as_slice(), b" plain ", b"line\nnext\t\"quote\"\\slash;#comment",
            b"carriage\rreturn", b"\xff\xfe\x80", "日本語".as_bytes()
        ] {
            set(&mut repo, b"wrapper.value", value);
            assert_eq!(get(&repo, b"wrapper.value"), Some(value.to_vec()));
            assert_eq!(git_value(&fixture.root, "wrapper.value"), Some(value.to_vec()));
        }
        set(&mut repo, b"wrapper.implicit", b"now explicit");
        assert_eq!(get(&repo, b"wrapper.implicit"), Some(b"now explicit".to_vec()));
        set(&mut repo, b"wrapper.Case.sub-key", b"subsection");
        assert_eq!(git_value(&fixture.root, "wrapper.Case.sub-key"), Some(b"subsection".to_vec()));
        let written = std::fs::read(fixture.config()).expect("read written");
        assert!(written.windows(b"# untouched \xff\r\n".len()).any(|b| b == b"# untouched \xff\r\n"));
        assert!(written.windows(b" value = \" bytes \xfe \" # keep\r\n".len()).any(|b| b == b" value = \" bytes \xfe \" # keep\r\n"));
        set(&mut repo, b"wrapper.value", "日本語".as_bytes());
        assert_eq!(std::fs::read(fixture.config()).expect("read unchanged"), written, "equal writes are no-ops");
        assert!(delete(&mut repo, b"wrapper.value"));
        assert!(!delete(&mut repo, b"wrapper.value"));
        assert_eq!(git_value(&fixture.root, "wrapper.value"), None);
        assert!(delete(&mut repo, b"wrapper.implicit"));
        assert_eq!(git_value(&fixture.root, "wrapper.implicit"), None);
    }
}

#[test]
fn ambiguous_local_keys_and_foreign_locks_fail_without_changing_the_file() {
    let fixture = Fixture::new(false);
    fixture.append(b"\n[wrapper]\n repeated = one\n[wrapper]\n repeated = two\n implicit\n implicit\n unique = original\n");
    let mut repo = fixture.repo();
    let original = std::fs::read(fixture.config()).expect("read original");
    for key in [b"wrapper.repeated".as_slice(), b"wrapper.implicit"] {
        assert!(matches!(repo.set_config_string(ffi::Slice::from_slice(key), ffi::Slice::from_slice(b"replace")), ffi::Err(GixError::Config(_))));
        assert!(matches!(repo.delete_config_value(ffi::Slice::from_slice(key)), ffi::Err(GixError::Config(_))));
    }
    let lock = fixture.config().with_extension("lock");
    std::fs::write(&lock, b"foreign owner").expect("create foreign lock");
    assert!(matches!(repo.set_config_string(ffi::Slice::from_slice(b"wrapper.unique"), ffi::Slice::from_slice(b"replace")), ffi::Err(GixError::Io(_))));
    assert!(matches!(repo.delete_config_value(ffi::Slice::from_slice(b"wrapper.unique")), ffi::Err(GixError::Io(_))));
    assert_eq!(std::fs::read(&lock).expect("foreign lock remains"), b"foreign owner");
    assert_eq!(std::fs::read(fixture.config()).expect("read unchanged"), original);
    std::fs::remove_file(&lock).expect("release fixture lock");
    assert!(delete(&mut repo, b"wrapper.unique"));
    assert!(!lock.exists(), "successful update releases its own lock");
}

#[test]
fn linked_worktrees_read_worktree_overrides_but_write_only_the_common_local_file() {
    let fixture = Fixture::new(false);
    git(&fixture.root, &["-c", "user.name=Config Tests", "-c", "user.email=config@example.com", "commit", "--allow-empty", "-qm", "fixture"]);
    let linked = fixture.parent.join("linked");
    git(&fixture.root, &["worktree", "add", "--detach", linked.to_str().expect("UTF-8 fixture path"), "HEAD"]);
    git(&fixture.root, &["config", "extensions.worktreeConfig", "true"]);
    git(&fixture.root, &["config", "wrapper.value", "common"]);
    git(&linked, &["config", "--worktree", "wrapper.value", "private"]);
    let bytes = path_bytes(&linked);
    let mut repo = ok(Repo::open(ffi::Slice::from_slice(&bytes)));
    let worktree_config = fixture.root.join(".git/worktrees/linked/config.worktree");
    let private = std::fs::read(&worktree_config).expect("read worktree config");
    assert_eq!(get(&repo, b"wrapper.value"), Some(b"private".to_vec()));
    set(&mut repo, b"wrapper.value", b"updated common");
    assert_eq!(git_value(&fixture.root, "wrapper.value"), Some(b"updated common".to_vec()));
    assert_eq!(get(&repo, b"wrapper.value"), Some(b"private".to_vec()));
    assert!(delete(&mut repo, b"wrapper.value"));
    assert!(!delete(&mut repo, b"wrapper.value"));
    assert_eq!(get(&repo, b"wrapper.value"), Some(b"private".to_vec()));
    assert_eq!(std::fs::read(&worktree_config).expect("read private"), private);
    assert!(!fixture.root.join(".git/worktrees/linked/config").exists());
}

#[test]
fn linked_worktree_gitdir_condition_matches_git_private_directory() {
    let fixture = Fixture::new(false);
    git(&fixture.root, &["-c", "user.name=Config Tests", "-c", "user.email=config@example.com", "commit", "--allow-empty", "-qm", "fixture"]);
    let linked = fixture.parent.join("conditional-linked");
    git(&fixture.root, &["worktree", "add", "--detach", linked.to_str().expect("UTF-8 fixture path"), "HEAD"]);
    let private = fixture.root.join(".git/worktrees/conditional-linked");
    std::fs::write(fixture.root.join(".git/private-include"), b"[wrapper]\n private = selected\n").expect("write include");
    let gitdir = private.to_str().expect("UTF-8 fixture path").replace('\\', "/");
    fixture.append(format!("\n[includeIf \"gitdir:{gitdir}/\"]\n path = private-include\n").as_bytes());
    let bytes = path_bytes(&linked);
    let repo = ok(Repo::open(ffi::Slice::from_slice(&bytes)));
    let base = std::fs::read(fixture.config()).expect("read condition base");
    for (suffix, expected) in [("", Some(b"selected".to_vec())), ("/", None), ("/**", None), ("**", Some(b"selected".to_vec()))] {
        let updated = String::from_utf8(base.clone()).expect("UTF-8 fixture").replace(
            &format!("gitdir:{gitdir}/"), &format!("gitdir:{gitdir}{suffix}"));
        std::fs::write(fixture.config(), updated).expect("write condition");
        assert_eq!(git_value(&linked, "wrapper.private"), expected, "Git premise for suffix {suffix:?}");
        assert_eq!(get(&repo, b"wrapper.private"), expected, "gix comparison for suffix {suffix:?}");
    }
    assert_eq!(get(&fixture.repo(), b"wrapper.private"), None);
}

#[test]
fn relative_configuration_paths_stay_with_the_opened_repository_after_cwd_change() {
    const CHILD: &str = "GIX_FFI_CONFIG_CWD_CHILD";
    if let Some(parent) = std::env::var_os(CHILD) {
        let parent = PathBuf::from(parent);
        std::env::set_current_dir(&parent).expect("enter fixture parent");
        let core = gix::open("repo").expect("open relative core repository");
        let mut repo = ok(Repo::open(ffi::Slice::from_slice(b"repo")));
        let trap_config = parent.join("other/repo/.git/config");
        let trap_before = std::fs::read(&trap_config).expect("read trap config");
        std::env::set_current_dir(parent.join("other")).expect("move current directory");
        let fresh = core.config_snapshot().reload().expect("reload original core config");
        assert_eq!(fresh.string("wrapper.value"), Some("original".into()));
        assert_eq!(fresh.string("wrapper.conditional"), Some("original branch".into()), "HEAD is read from the opened repository");
        assert!(fresh.meta().path.as_ref().expect("local path").is_absolute());
        assert_eq!(get(&repo, b"wrapper.value"), Some(b"original".to_vec()));
        set(&mut repo, b"core.repositoryFormatVersion", b"invalid");
        assert_eq!(get(&repo, b"core.repositoryFormatVersion"), Some(b"invalid".to_vec()));
        set(&mut repo, b"wrapper.value", b"updated original");
        assert!(delete(&mut repo, b"wrapper.remove"));
        set(&mut repo, b"core.repositoryFormatVersion", b"0");
        assert_eq!(get(&repo, b"wrapper.value"), Some(b"updated original".to_vec()));
        assert_eq!(git_value(&parent.join("repo"), "wrapper.value"), Some(b"updated original".to_vec()));
        assert_eq!(git_value(&parent.join("repo"), "wrapper.remove"), None);
        assert_eq!(std::fs::read(&trap_config).expect("read untouched trap"), trap_before);
        assert!(!trap_config.with_extension("lock").exists());
        return;
    }

    let fixture = Fixture::new(false);
    fixture.append(b"\n[wrapper]\n value = original\n remove = original\n[includeIf \"onbranch:original\"]\n path = conditional\n");
    git(&fixture.root, &["symbolic-ref", "HEAD", "refs/heads/original"]);
    std::fs::write(fixture.root.join(".git/conditional"), b"[wrapper]\n conditional = original branch\n").expect("write original include");
    let trap = fixture.parent.join("other/repo");
    std::fs::create_dir_all(trap.parent().expect("trap parent")).expect("create trap parent");
    gix::init(&trap).expect("initialize same-named trap repository");
    git(&trap, &["config", "wrapper.value", "trap"]);
    git(&trap, &["symbolic-ref", "HEAD", "refs/heads/trap"]);
    // Changing cwd is process-global. Isolate it from parallel tests in a child.
    let output = Command::new(std::env::current_exe().expect("test executable"))
        .args(["--exact", "relative_configuration_paths_stay_with_the_opened_repository_after_cwd_change", "--nocapture"])
        .env(CHILD, &fixture.parent)
        .output()
        .expect("run isolated cwd regression");
    assert!(output.status.success(), "child stdout: {}\nchild stderr: {}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
fn generic_values_remain_readable_and_repairable_when_typed_repository_reload_fails() {
    let fixture = Fixture::new(false);
    let mut repo = fixture.repo();
    set(&mut repo, b"core.repositoryFormatVersion", b"not-an-integer");
    assert_eq!(get(&repo, b"core.repositoryFormatVersion"), Some(b"not-an-integer".to_vec()));
    assert!(gix::open(&fixture.root).is_err(), "the value is invalid for typed repository opening");
    set(&mut repo, b"wrapper.repair", b"still writable");
    assert_eq!(get(&repo, b"wrapper.repair"), Some(b"still writable".to_vec()));
    set(&mut repo, b"core.repositoryFormatVersion", b"0");
    assert_eq!(get(&repo, b"core.repositoryFormatVersion"), Some(b"0".to_vec()));
    assert!(gix::open(&fixture.root).is_ok(), "the same handle repairs the value");
    set(&mut repo, b"core.repositoryFormatVersion", b"not-an-integer");
    assert!(delete(&mut repo, b"core.repositoryFormatVersion"));
    assert_eq!(get(&repo, b"core.repositoryFormatVersion"), None);
    assert!(gix::open(&fixture.root).is_ok(), "the same handle can also delete an invalid value");
}

#[test]
fn invalid_names_and_nul_values_fail_and_successful_writes_refresh_later_repository_operations() {
    let fixture = Fixture::new(false);
    let mut repo = fixture.repo();
    let original = std::fs::read(fixture.config()).expect("read original");
    for key in [b"".as_slice(), b"without-dot", b".key", b"section.", b"section.1key", b"-section.key", b"sec tion.key", b"a.b\nc.key", b"a.b\0c.key"] {
        assert!(matches!(repo.get_config_string(ffi::Slice::from_slice(key)), ffi::Err(GixError::Config(_))), "get {key:?}");
        assert!(matches!(repo.set_config_string(ffi::Slice::from_slice(key), ffi::Slice::from_slice(b"x")), ffi::Err(GixError::Config(_))), "set {key:?}");
        assert!(matches!(repo.delete_config_value(ffi::Slice::from_slice(key)), ffi::Err(GixError::Config(_))), "delete {key:?}");
    }
    assert!(matches!(repo.set_config_string(ffi::Slice::from_slice(b"wrapper.value"), ffi::Slice::from_slice(b"nul\0tail")), ffi::Err(GixError::Config(_))));
    assert_eq!(std::fs::read(fixture.config()).expect("read unchanged"), original);
    set(&mut repo, b"user.name", b"Configured Native");
    set(&mut repo, b"user.email", b"configured@example.com");
    let commit = ok(repo.create_commit_from_index("configured".to_owned().into(), false, ffi::Slice::empty(), ffi::Slice::empty(), true));
    let info = ok(repo.commit_info(commit));
    assert_eq!(info.author_name.into_vec(), b"Configured Native");
    assert_eq!(info.author_email.into_vec(), b"configured@example.com");
}