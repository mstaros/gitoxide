use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use gix::{
    bstr::{BString, ByteSlice},
    repository::read_tree::{Error, Options},
};

fn git(root: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git")
        .current_dir(root)
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG_COUNT")
        .output()
}

fn git_ok(root: &Path, args: &[&str]) -> crate::Result {
    let out = git(root, args)?;
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    Ok(())
}

fn write(root: &Path, path: &str, bytes: &[u8]) -> crate::Result {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().expect("file parent"))?;
    std::fs::write(path, bytes)?;
    Ok(())
}

fn fixture() -> gix_testtools::Result<(
    gix_testtools::tempfile::TempDir,
    gix::Repository,
    [gix::ObjectId; 2],
)> {
    let temp = gix_testtools::tempfile::TempDir::new()?;
    let root = temp.path();
    git_ok(root, &["init", "-q", "-b", "main"])?;
    git_ok(root, &["config", "user.name", "Transition Tests"])?;
    git_ok(root, &["config", "user.email", "transition@example.com"])?;
    for path in ["tracked.txt", "kept.txt", "remove.txt", "inside/keep.txt", "outside/keep.txt"] {
        write(root, path, b"baseline\n")?;
    }
    git_ok(root, &["add", "."])?;
    git_ok(root, &["commit", "-q", "-m", "old"])?;
    let repo = gix::open_opts(root, crate::restricted())?;
    let old_tree_id = repo.head_tree_id()?.detach();
    write(root, "tracked.txt", b"target content\n")?;
    write(root, "new.txt", b"new target file\n")?;
    write(root, "inside/new.txt", b"included target file\n")?;
    write(root, "outside/new.txt", b"excluded target file\n")?;
    std::fs::remove_file(root.join("remove.txt"))?;
    git_ok(root, &["add", "-A"])?;
    git_ok(root, &["update-index", "--chmod=+x", "tracked.txt"])?;
    git_ok(root, &["commit", "-q", "-m", "new"])?;
    let new_tree_id = repo.head_tree_id()?.detach();
    git_ok(root, &["reset", "--hard", "HEAD^"])?;
    Ok((temp, repo, [old_tree_id, new_tree_id]))
}

fn scenario(root: &Path, name: &str) -> crate::Result {
    match name {
        "clean" => {}
        "disjoint" => {
            write(root, "kept.txt", b"staged local work\n")?;
            git_ok(root, &["add", "kept.txt"])?;
            write(root, "kept.txt", b"unstaged local work\n")?;
            write(root, "untracked.txt", b"caller data\n")?;
        }
        "staged overlap" => {
            write(root, "tracked.txt", b"staged overlapping work\n")?;
            git_ok(root, &["add", "tracked.txt"])?;
        }
        "unstaged overlap" => write(root, "tracked.txt", b"unstaged overlapping work\n")?,
        "untracked overlap" => write(root, "new.txt", b"untracked overlapping work\n")?,
        "ignored obstruction" => {
            std::fs::write(root.join(".git/info/exclude"), b"new.txt\n")?;
            write(root, "new.txt", b"ignored data\n")?;
        }
        "ignored directory obstruction" => {
            std::fs::write(root.join(".git/info/exclude"), b"new.txt/\n")?;
            write(root, "new.txt/child.txt", b"ignored child\n")?;
        }
        "ignored ancestor obstruction" => {
            std::fs::write(root.join(".git/info/exclude"), b"inside\n")?;
            std::fs::remove_dir_all(root.join("inside"))?;
            write(root, "inside", b"ignored ancestor\n")?;
        }
        "empty directory obstruction" => {
            std::fs::create_dir_all(root.join("new.txt/empty"))?;
        }
        "assumed-valid overlap" => {
            git_ok(root, &["update-index", "--assume-unchanged", "tracked.txt"])?;
            write(root, "tracked.txt", b"hidden overlapping work\n")?;
        }
        "already staged target" => {
            write(root, "tracked.txt", b"target content\n")?;
            git_ok(root, &["add", "tracked.txt"])?;
            git_ok(root, &["update-index", "--chmod=+x", "tracked.txt"])?;
            write(root, "tracked.txt", b"work after staging target\n")?;
        }
        "staged target deletion" => git_ok(root, &["rm", "--cached", "remove.txt"])?,
        "staged disjoint deletion" => git_ok(root, &["rm", "--cached", "kept.txt"])?,
        "staged overlapping deletion" => git_ok(root, &["rm", "--cached", "tracked.txt"])?,
        "missing tracked file" => std::fs::remove_file(root.join("tracked.txt"))?,
        "intent to add" => {
            write(root, "new.txt", b"intent to add\n")?;
            git_ok(root, &["add", "-N", "new.txt"])?;
        }
        _ => panic!("unknown fixture scenario"),
    }
    Ok(())
}

type IndexEntry = (BString, u32, u32, gix::ObjectId, bool);

fn index_state(repo: &gix::Repository) -> gix_testtools::Result<(Vec<IndexEntry>, bool)> {
    let index = repo.open_index()?;
    Ok((
        index.entries().iter().map(|entry| (
            entry.path(&index).to_owned(),
            entry.mode.bits(),
            entry.stage_raw(),
            entry.id,
            entry.flags.contains(gix::index::entry::Flags::SKIP_WORKTREE),
        )).collect(),
        index.is_sparse(),
    ))
}

fn files(root: &Path) -> gix_testtools::Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) -> crate::Result {
        for item in std::fs::read_dir(dir)? {
            let item = item?;
            if dir == root && item.file_name() == ".git" {
                continue;
            }
            let path = item.path();
            let kind = item.file_type()?;
            if kind.is_dir() {
                visit(root, &path, out)?;
            } else {
                let data = if kind.is_symlink() {
                    gix::path::into_bstr(std::fs::read_link(&path)?).into_owned().to_vec()
                } else {
                    std::fs::read(&path)?
                };
                out.insert(path.strip_prefix(root)?.to_owned(), data);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out)?;
    Ok(out)
}

fn compare_with_git(name: &str, options: Options, sparse: bool) -> crate::Result {
    let (actual_root, mut actual, actual_trees) = fixture()?;
    let (baseline_root, baseline, baseline_trees) = fixture()?;
    if sparse {
        use gix::repository::sparse_checkout::set;
        actual.set_sparse_checkout(
            set::Options { cone: Some(true), sparse_index: Some(false), ..Default::default() },
            ["inside"],
        )?;
        git_ok(baseline_root.path(), &["sparse-checkout", "set", "--cone", "--no-sparse-index", "inside"])?;
    }
    scenario(actual_root.path(), name)?;
    scenario(baseline_root.path(), name)?;
    let before_files = files(actual_root.path())?;
    let before_index = std::fs::read(actual.index_path())?;
    let old = baseline_trees[0].to_string();
    let new = baseline_trees[1].to_string();
    let mut args = vec!["read-tree"];
    args.push(if options.reset { "--reset" } else { "-m" });
    if options.update_worktree {
        args.push("-u");
    }
    if options.index_only {
        args.push("-i");
    }
    if options.dry_run {
        args.push("--dry-run");
    }
    args.extend([old.as_str(), new.as_str()]);
    let expected = git(baseline_root.path(), &args)?;
    let result = actual.read_tree(&actual_trees, options.clone());
    assert_eq!(
        result.is_ok(), expected.status.success(),
        "{name}, sparse={sparse}: gix={result:?}; Git={}",
        String::from_utf8_lossy(&expected.stderr)
    );
    if expected.status.success() {
        assert_eq!(index_state(&actual)?, index_state(&baseline)?, "{name}: complete index semantics");
        assert_eq!(files(actual_root.path())?, files(baseline_root.path())?, "{name}: worktree bytes");
    } else {
        let error = result.expect_err("Git refused the same transition").into_inner();
        assert!(
            matches!(error, Error::IndexConflict { .. } | Error::DirtyWorktree { .. } | Error::Obstructed { .. }),
            "{name}: refusal must identify local work: {error:?}"
        );
    }
    if options.dry_run || !expected.status.success() {
        assert_eq!(std::fs::read(actual.index_path())?, before_index, "{name}: untouched index bytes");
        assert_eq!(files(actual_root.path())?, before_files, "{name}: untouched worktree bytes");
    }
    assert!(!actual.index_path().with_extension("lock").exists());
    Ok(())
}

#[test]
fn two_tree_carry_forward_matches_git() -> crate::Result {
    for name in [
        "clean", "disjoint", "staged overlap", "unstaged overlap", "untracked overlap",
        "already staged target", "staged disjoint deletion", "staged overlapping deletion",
        "missing tracked file", "intent to add",
    ] {
        compare_with_git(name, Options { update_worktree: true, ..Default::default() }, false)?;
    }
    Ok(())
}

#[test]
fn index_only_reset_and_dry_run_match_git() -> crate::Result {
    for name in ["clean", "disjoint", "staged overlap", "unstaged overlap", "untracked overlap", "staged target deletion"] {
        for options in [
            Options::default(),
            Options { index_only: true, ..Default::default() },
            Options { merge: false, reset: true, update_worktree: true, ..Default::default() },
            Options { update_worktree: true, dry_run: true, ..Default::default() },
        ] {
            compare_with_git(name, options, false)?;
        }
    }
    Ok(())
}

#[test]
fn cone_checkout_keeps_an_ordinary_index() -> crate::Result {
    compare_with_git("disjoint", Options { update_worktree: true, ..Default::default() }, true)
}

#[test]
fn unsupported_options_and_index_locks_leave_no_changes() -> crate::Result {
    let (root, repo, trees) = fixture()?;
    let before_index = std::fs::read(repo.index_path())?;
    let before_files = files(root.path())?;
    for options in [
        Options { aggressive: true, ..Default::default() },
        Options { trivial: true, ..Default::default() },
        Options { prefix: Some("prefix".into()), ..Default::default() },
        Options { index_output: Some("elsewhere".into()), ..Default::default() },
    ] {
        assert!(matches!(repo.read_tree(&trees, options).expect_err("unsupported option").into_inner(), Error::Unsupported { .. }));
    }
    assert!(matches!(repo.read_tree(&trees[..1], Options::default()).expect_err("one tree").into_inner(), Error::Unsupported { .. }));
    assert!(matches!(repo.read_tree(&[trees[0], trees[0], trees[1]], Options::default()).expect_err("three trees").into_inner(), Error::Unsupported { .. }));
    let lock_path = repo.git_dir().join("index.lock");
    std::fs::write(&lock_path, b"another writer")?;
    assert!(matches!(
        repo.read_tree(&trees, Options { update_worktree: true, ..Default::default() })
            .expect_err("respect the existing index lock").into_inner(),
        Error::IndexLock(_)
    ));
    assert_eq!(std::fs::read(&lock_path)?, b"another writer");
    assert_eq!(std::fs::read(repo.index_path())?, before_index);
    assert_eq!(files(root.path())?, before_files);
    Ok(())
}

#[test]
fn ignored_and_assumed_valid_paths_match_git() -> crate::Result {
    for name in [
        "ignored obstruction",
        "ignored directory obstruction",
        "ignored ancestor obstruction",
        "empty directory obstruction",
        "assumed-valid overlap",
    ] {
        for options in [
            Options { update_worktree: true, ..Default::default() },
            Options { merge: false, reset: true, update_worktree: true, ..Default::default() },
        ] {
            compare_with_git(name, options, false)?;
        }
    }
    Ok(())
}

#[test]
fn pattern_checkout_keeps_an_ordinary_index() -> crate::Result {
    let (root, mut repo, trees) = fixture()?;
    let (baseline_root, baseline, baseline_trees) = fixture()?;
    repo.set_sparse_checkout(
        gix::repository::sparse_checkout::set::Options {
            cone: Some(false), sparse_index: Some(false), ..Default::default()
        },
        ["/*", "!/outside/"],
    )?;
    git_ok(baseline_root.path(), &["sparse-checkout", "set", "--no-cone", "/*", "!/outside/"])?;
    scenario(root.path(), "disjoint")?;
    scenario(baseline_root.path(), "disjoint")?;
    git_ok(baseline_root.path(), &["read-tree", "-m", "-u", &baseline_trees[0].to_string(), &baseline_trees[1].to_string()])?;
    repo.read_tree(&trees, Options { update_worktree: true, ..Default::default() })?;
    assert_eq!(index_state(&repo)?, index_state(&baseline)?);
    assert_eq!(files(root.path())?, files(baseline_root.path())?);
    Ok(())
}

#[test]
fn initial_checkout_and_case_only_renames_match_git() -> crate::Result {
    for initial in [false, true] {
        let mut fixtures = Vec::new();
        for _ in 0..2 {
            let (root, repo, mut trees) = fixture()?;
            if initial {
                for path in files(root.path())?.keys() {
                    std::fs::remove_file(root.path().join(path))?;
                }
                std::fs::remove_file(repo.index_path())?;
            } else {
                trees[0] = repo.head_tree_id()?.detach();
                git_ok(root.path(), &["mv", "tracked.txt", "TRACKED.txt"])?;
                git_ok(root.path(), &["commit", "-q", "-m", "case rename"])?;
                trees[1] = repo.head_tree_id()?.detach();
                git_ok(root.path(), &["reset", "--hard", "HEAD^"])?;
            }
            fixtures.push((root, repo, trees));
        }
        let (baseline_root, baseline, baseline_trees) = fixtures.pop().expect("baseline");
        let (root, repo, trees) = fixtures.pop().expect("actual");
        git_ok(baseline_root.path(), &["read-tree", "-m", "-u", &baseline_trees[0].to_string(), &baseline_trees[1].to_string()])?;
        repo.read_tree(&trees, Options { update_worktree: true, ..Default::default() })?;
        assert_eq!(index_state(&repo)?, index_state(&baseline)?);
        assert_eq!(files(root.path())?, files(baseline_root.path())?);
    }
    Ok(())
}

#[test]
fn directory_file_transitions_protect_untracked_children() -> crate::Result {
    for reverse in [false, true] {
        for obstructed in [false, true] {
            let mut fixtures = Vec::new();
            for _ in 0..2 {
                let (root, repo, _) = fixture()?;
                let old_tree_id = repo.head_tree_id()?.detach();
                std::fs::remove_file(root.path().join("tracked.txt"))?;
                write(root.path(), "tracked.txt/child.txt", b"replacement child\n")?;
                git_ok(root.path(), &["add", "-A"])?;
                git_ok(root.path(), &["commit", "-q", "-m", "directory"])?;
                let new_tree_id = repo.head_tree_id()?.detach();
                if !reverse {
                    git_ok(root.path(), &["reset", "--hard", "HEAD^"])?;
                }
                if obstructed {
                    if reverse {
                        write(root.path(), "tracked.txt/untracked.txt", b"retained child\n")?;
                    } else {
                        write(root.path(), "tracked.txt", b"retained local edit\n")?;
                    }
                }
                let trees = if reverse { [new_tree_id, old_tree_id] } else { [old_tree_id, new_tree_id] };
                fixtures.push((root, repo, trees));
            }
            let (baseline_root, baseline, baseline_trees) = fixtures.pop().expect("baseline");
            let (root, repo, trees) = fixtures.pop().expect("actual");
            let before_files = files(root.path())?;
            let before_index = std::fs::read(repo.index_path())?;
            let expected = git(baseline_root.path(), &["read-tree", "-m", "-u", &baseline_trees[0].to_string(), &baseline_trees[1].to_string()])?;
            let actual = repo.read_tree(&trees, Options { update_worktree: true, ..Default::default() });
            assert_eq!(actual.is_ok(), expected.status.success(), "reverse={reverse}, obstructed={obstructed}: {actual:?}; {}", String::from_utf8_lossy(&expected.stderr));
            assert_eq!(index_state(&repo)?, index_state(&baseline)?);
            assert_eq!(files(root.path())?, files(baseline_root.path())?);
            if obstructed {
                assert!(matches!(actual.expect_err("local work").into_inner(), Error::DirtyWorktree { .. } | Error::Obstructed { .. }));
                assert_eq!(std::fs::read(repo.index_path())?, before_index);
                assert_eq!(files(root.path())?, before_files);
            }
        }
    }
    Ok(())
}

fn conflict(repo: &gix::Repository) -> crate::Result {
    let mut index = repo.open_index()?;
    let old = index.entries().iter().find(|entry| entry.path(&index) == b"tracked.txt".as_bstr())
        .expect("tracked path").clone();
    index.remove_entries(|_, path, _| path == b"tracked.txt".as_bstr());
    for stage in [gix::index::entry::Stage::Base, gix::index::entry::Stage::Ours, gix::index::entry::Stage::Theirs] {
        index.dangerously_push_entry(old.stat, old.id, gix::index::entry::Flags::from_stage(stage), old.mode, b"tracked.txt".as_bstr());
    }
    index.sort_entries();
    index.remove_tree();
    index.write(Default::default())?;
    Ok(())
}

#[test]
fn unmerged_indexes_are_refused_or_reset_like_git() -> crate::Result {
    for reset in [false, true] {
        let (root, repo, trees) = fixture()?;
        let (baseline_root, baseline, baseline_trees) = fixture()?;
        conflict(&repo)?;
        conflict(&baseline)?;
        let before_index = std::fs::read(repo.index_path())?;
        let before_files = files(root.path())?;
        let expected = git(baseline_root.path(), &["read-tree", if reset { "--reset" } else { "-m" }, "-u", &baseline_trees[0].to_string(), &baseline_trees[1].to_string()])?;
        let actual = repo.read_tree(&trees, Options { merge: !reset, reset, update_worktree: true, ..Default::default() });
        assert_eq!(actual.is_ok(), expected.status.success(), "{actual:?}; {}", String::from_utf8_lossy(&expected.stderr));
        assert_eq!(index_state(&repo)?, index_state(&baseline)?);
        assert_eq!(files(root.path())?, files(baseline_root.path())?);
        if !reset {
            assert!(matches!(actual.expect_err("unmerged index").into_inner(), Error::UnmergedIndex));
            assert_eq!(std::fs::read(repo.index_path())?, before_index);
            assert_eq!(files(root.path())?, before_files);
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn replacing_a_tracked_symlink_never_removes_its_target_contents() -> crate::Result {
    let (root, repo, _) = fixture()?;
    let outside = gix_testtools::tempfile::TempDir::new()?;
    write(outside.path(), "child.txt", b"outside data\n")?;
    std::fs::remove_file(root.path().join("tracked.txt"))?;
    std::os::unix::fs::symlink(outside.path(), root.path().join("tracked.txt"))?;
    git_ok(root.path(), &["add", "-A"])?;
    git_ok(root.path(), &["commit", "-q", "-m", "symlink"])?;
    let old_tree_id = repo.head_tree_id()?.detach();
    std::fs::remove_file(root.path().join("tracked.txt"))?;
    write(root.path(), "tracked.txt/child.txt", b"new tracked child\n")?;
    git_ok(root.path(), &["add", "-A"])?;
    git_ok(root.path(), &["commit", "-q", "-m", "directory"])?;
    let new_tree_id = repo.head_tree_id()?.detach();
    git_ok(root.path(), &["reset", "--hard", "HEAD^"])?;
    repo.read_tree(&[old_tree_id, new_tree_id], Options { update_worktree: true, ..Default::default() })?;
    assert_eq!(std::fs::read(outside.path().join("child.txt"))?, b"outside data\n");
    assert_eq!(std::fs::read(root.path().join("tracked.txt/child.txt"))?, b"new tracked child\n");
    Ok(())
}

#[test]
fn an_apply_failure_reports_recovery_and_preserves_the_old_index() -> crate::Result {
    let (root, repo, trees) = fixture()?;
    let mut tree = repo.find_tree(trees[1])?.decode()?.to_owned();
    let missing_blob_id = gix::ObjectId::from_hex(b"1111111111111111111111111111111111111111")?;
    tree.entries.push(gix::objs::tree::Entry {
        mode: gix::objs::tree::EntryKind::Blob.into(),
        filename: "z-missing.txt".into(),
        oid: missing_blob_id,
    });
    let new_tree_id = repo.write_object(&tree)?.detach();
    let before_index = std::fs::read(repo.index_path())?;
    let error = repo.read_tree(&[trees[0], new_tree_id], Options { update_worktree: true, ..Default::default() })
        .expect_err("the target contains a missing blob").into_inner();
    match error {
        Error::UpdateFailed { old_tree_id, new_tree_id: actual_new, index_path, possibly_changed_paths, .. } => {
            assert_eq!(old_tree_id, trees[0]);
            assert_eq!(actual_new, new_tree_id);
            assert_eq!(index_path, repo.index_path());
            assert!(possibly_changed_paths.iter().any(|path| path == b"z-missing.txt".as_bstr()));
        }
        other => panic!("expected explicit recovery evidence, got {other:?}"),
    }
    assert_eq!(std::fs::read(repo.index_path())?, before_index, "index publication follows successful checkout");
    assert!(root.path().join("new.txt").is_file(), "failure happened after an earlier file was applied");
    Ok(())
}

#[test]
#[ignore = "opt-in timing measurement; writes gix/target/read-tree-measurement.csv"]
fn measure_index_only_transitions() -> crate::Result {
    use std::{fmt::Write, time::Instant};

    let mut report = String::from("entries,samples,min_ms,median_ms,max_ms\n");
    for count in [1_000, 10_000, 100_000] {
        let root = gix_testtools::tempfile::TempDir::new()?;
        let repo = gix::init(root.path())?;
        let old_blob = repo.write_blob(b"baseline\n")?.detach();
        let new_blob = repo.write_blob(b"changed\n")?.detach();
        let mut tree = gix::objs::Tree {
            entries: (0..count)
                .map(|idx| gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Blob.into(),
                    filename: format!("file-{idx:06}.txt").into(),
                    oid: old_blob,
                })
                .collect(),
        };
        let old = repo.write_object(&tree)?.detach();
        tree.entries[count / 2].oid = new_blob;
        let new = repo.write_object(&tree)?.detach();
        let options = Options { index_only: true, ..Default::default() };
        repo.read_tree(&[old, old], options.clone())?;
        // Warm both directions; fixture and tree construction are outside the measurements.
        repo.read_tree(&[old, new], options.clone())?;
        repo.read_tree(&[new, old], options.clone())?;
        let mut elapsed = Vec::with_capacity(20);
        for _ in 0..10 {
            for trees in [[old, new], [new, old]] {
                let start = Instant::now();
                let outcome = repo.read_tree(&trees, options.clone())?;
                elapsed.push(start.elapsed());
                assert_eq!(outcome.index_paths.len(), 1);
                assert!(outcome.worktree_paths.is_empty());
            }
        }
        elapsed.sort_unstable();
        writeln!(
            report, "{count},{},{:.3},{:.3},{:.3}",
            elapsed.len(),
            elapsed[0].as_secs_f64() * 1_000.0,
            (elapsed[9].as_secs_f64() + elapsed[10].as_secs_f64()) * 500.0,
            elapsed[19].as_secs_f64() * 1_000.0,
        )?;
    }
    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/read-tree-measurement.csv");
    std::fs::create_dir_all(output.parent().expect("measurement has a parent"))?;
    std::fs::write(&output, &report)?;
    eprintln!("{}\n{report}", output.display());
    Ok(())
}
