use std::{
    path::{Path, PathBuf},
    process::Command,
};

use gix::bstr::ByteSlice;

#[test]
fn cone_mode_matches_git_and_disable_restores_everything() -> crate::Result {
    let temp = gix_testtools::tempfile::TempDir::new()?;
    let git_root = temp.path().join("git-baseline");
    let gix_root = temp.path().join("gix-subject");
    make_repository(&git_root)?;
    make_repository(&gix_root)?;

    git(&git_root, &["sparse-checkout", "set", "--cone", "src/deep"])?;
    let git_repo = open(&git_root)?;
    let mut repo = open(&gix_root)?;
    repo.set_sparse_checkout(
        gix::index::sparse::Mode::IncludeDirectoriesStoreAllEntriesSkipUnmatched,
        ["src/deep"],
    )?;

    assert_eq!(skip_paths(&repo)?, skip_paths(&git_repo)?);
    assert_eq!(worktree_files(&gix_root)?, worktree_files(&git_root)?);
    assert_eq!(repo.list_sparse_checkout()?, git_repo.list_sparse_checkout()?);
    assert_eq!(
        repo.list_sparse_checkout()?,
        Some((
            gix::index::sparse::Mode::IncludeDirectoriesStoreAllEntriesSkipUnmatched,
            vec!["src/deep".into()]
        ))
    );

    let definition = repo.git_dir().join("info").join("sparse-checkout");
    assert!(definition.is_file());

    let mut index = repo.open_index()?;
    let root_idx = index
        .entries()
        .iter()
        .position(|entry| entry.path_in(index.path_backing()) == "root.txt")
        .expect("fixture path");
    index.entries_mut()[root_idx].flags.insert(
        gix::index::entry::Flags::EXTENDED | gix::index::entry::Flags::SKIP_WORKTREE,
    );
    index.write(Default::default())?;

    repo.disable_sparse_checkout()?;
    assert!(skip_paths(&repo)?.is_empty());
    assert_eq!(worktree_files(&gix_root)?, tracked_paths(&repo)?);
    assert!(definition.is_file(), "disable retains the definition");
    assert_eq!(repo.list_sparse_checkout()?, None);
    assert_eq!(
        repo.config_snapshot().boolean("core.sparseCheckout"),
        Some(false)
    );
    Ok(())
}

#[test]
fn sparse_index_matches_git_and_transitions_back_to_full_indexes() -> crate::Result {
    let temp = gix_testtools::tempfile::TempDir::new()?;
    let git_root = temp.path().join("git-baseline");
    let gix_root = temp.path().join("gix-subject");
    make_repository(&git_root)?;
    make_repository(&gix_root)?;

    git(
        &git_root,
        &["sparse-checkout", "set", "--cone", "--sparse-index", "src/deep"],
    )?;
    let mut repo = open(&gix_root)?;
    repo.set_sparse_checkout(
        gix::index::sparse::Mode::IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs,
        ["src/deep"],
    )?;

    let git_repo = open(&git_root)?;
    assert!(repo.open_index()?.is_sparse());
    assert_eq!(sparse_directory_entries(&repo)?, sparse_directory_entries(&git_repo)?);
    assert_eq!(worktree_files(&gix_root)?, worktree_files(&git_root)?);
    assert_eq!(repo.list_sparse_checkout()?, git_repo.list_sparse_checkout()?);
    let status = repo
        .status(gix::progress::Discard)?
        .into_iter(Vec::new())?
        .collect::<Result<Vec<_>, _>>()?;
    assert!(status.is_empty());
    assert!(!repo.is_dirty()?);

    let patterns = ["/other/"];
    git(
        &git_root,
        &["sparse-checkout", "set", "--no-cone", patterns[0]],
    )?;
    repo.set_sparse_checkout(
        gix::index::sparse::Mode::IncludeByIgnorePatternStoreAllEntriesSkipUnmatched,
        patterns,
    )?;
    let git_repo = open(&git_root)?;
    assert!(!repo.open_index()?.is_sparse());
    assert_eq!(skip_paths(&repo)?, skip_paths(&git_repo)?);
    assert_eq!(worktree_files(&gix_root)?, worktree_files(&git_root)?);

    git(
        &git_root,
        &["sparse-checkout", "set", "--cone", "--sparse-index", "src/deep"],
    )?;
    repo.set_sparse_checkout(
        gix::index::sparse::Mode::IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs,
        ["src/deep"],
    )?;
    assert!(repo.open_index()?.is_sparse());

    git(&git_root, &["sparse-checkout", "disable"])?;
    repo.disable_sparse_checkout()?;
    let git_repo = open(&git_root)?;
    assert!(!repo.open_index()?.is_sparse());
    assert_eq!(tracked_paths(&repo)?, tracked_paths(&git_repo)?);
    assert_eq!(worktree_files(&gix_root)?, worktree_files(&git_root)?);
    assert_eq!(repo.config_snapshot().boolean("index.sparse"), Some(false));
    Ok(())
}

#[test]
fn sparse_index_preserves_staged_changes_outside_the_cone() -> crate::Result {
    let temp = gix_testtools::tempfile::TempDir::new()?;
    let git_root = temp.path().join("git-baseline");
    let gix_root = temp.path().join("gix-subject");
    make_repository(&git_root)?;
    make_repository(&gix_root)?;

    for root in [&git_root, &gix_root] {
        std::fs::write(root.join("other/drop.txt"), "staged outside cone\n")?;
        git(root, &["add", "other/drop.txt"])?;
    }
    let staged_id = git(&gix_root, &["rev-parse", ":other/drop.txt"])?
        .trim()
        .to_owned();

    git(
        &git_root,
        &["sparse-checkout", "set", "--cone", "--sparse-index", "src/deep"],
    )?;
    let mut repo = open(&gix_root)?;
    repo.set_sparse_checkout(
        gix::index::sparse::Mode::IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs,
        ["src/deep"],
    )?;

    let git_repo = open(&git_root)?;
    assert_eq!(sparse_directory_entries(&repo)?, sparse_directory_entries(&git_repo)?);
    assert!(repo.is_dirty()?);
    let index = repo.open_index()?;
    let other = index
        .entries()
        .iter()
        .find(|entry| entry.path_in(index.path_backing()) == b"other/".as_bstr())
        .expect("the excluded directory is compressed");
    assert_eq!(other.id.to_hex().to_string(), sparse_entry_id(&git_repo, "other/")?);

    repo.disable_sparse_checkout()?;
    let index = repo.open_index()?;
    let staged = index
        .entries()
        .iter()
        .find(|entry| entry.path_in(index.path_backing()) == b"other/drop.txt".as_bstr())
        .expect("disable expands the sparse directory");
    assert_eq!(staged.id.to_hex().to_string(), staged_id);
    assert_eq!(
        std::fs::read_to_string(gix_root.join("other/drop.txt"))?,
        "staged outside cone\n"
    );
    Ok(())
}

#[test]
fn pattern_mode_matches_git_with_negation_anchoring_and_last_match_wins() -> crate::Result {
    let temp = gix_testtools::tempfile::TempDir::new()?;
    let git_root = temp.path().join("git-baseline");
    let gix_root = temp.path().join("gix-subject");
    make_repository(&git_root)?;
    make_repository(&gix_root)?;

    let patterns = ["/src/", "!/src/drop.txt", "/src/drop.txt", "!nested.txt"];
    git(
        &git_root,
        &[
            "sparse-checkout",
            "set",
            "--no-cone",
            patterns[0],
            patterns[1],
            patterns[2],
            patterns[3],
        ],
    )?;

    let git_repo = open(&git_root)?;
    let mut repo = open(&gix_root)?;
    repo.set_sparse_checkout(
        gix::index::sparse::Mode::IncludeByIgnorePatternStoreAllEntriesSkipUnmatched,
        patterns,
    )?;

    assert_eq!(skip_paths(&repo)?, skip_paths(&git_repo)?);
    assert_eq!(worktree_files(&gix_root)?, worktree_files(&git_root)?);
    assert_eq!(repo.list_sparse_checkout()?, git_repo.list_sparse_checkout()?);
    assert_eq!(
        repo.list_sparse_checkout()?,
        Some((
            gix::index::sparse::Mode::IncludeByIgnorePatternStoreAllEntriesSkipUnmatched,
            patterns.into_iter().map(Into::into).collect()
        ))
    );

    let skipped = skip_paths(&repo)?;
    assert!(
        !skipped.iter().any(|path| path == "src/drop.txt"),
        "the final positive pattern wins"
    );
    assert!(
        skipped.iter().any(|path| path == "src/deep/nested.txt"),
        "a bare filename matches at every depth"
    );
    assert!(
        skipped.iter().any(|path| path == "other/src/drop.txt"),
        "a leading slash anchors the pattern at the repository root"
    );
    Ok(())
}

#[test]
fn linked_worktree_configuration_and_definition_are_isolated() -> crate::Result {
    let temp = gix_testtools::tempfile::TempDir::new()?;
    let main_root = temp.path().join("main");
    let linked_root = temp.path().join("linked");
    make_repository(&main_root)?;

    let linked_root_arg = linked_root
        .to_str()
        .expect("test fixture paths are valid UTF-8");
    git(
        &main_root,
        &["worktree", "add", "-q", "--detach", linked_root_arg, "HEAD"],
    )?;

    let mut linked_repo = open(&linked_root)?;
    assert_ne!(
        linked_repo.git_dir(),
        linked_repo.common_dir(),
        "the fixture must be a linked worktree"
    );

    linked_repo.set_sparse_checkout(
        gix::index::sparse::Mode::IncludeDirectoriesStoreAllEntriesSkipUnmatched,
        ["src/deep"],
    )?;

    let private_definition = linked_repo.git_dir().join("info").join("sparse-checkout");
    assert!(
        private_definition.is_file(),
        "the definition belongs to the linked worktree"
    );
    assert!(
        !linked_repo
            .common_dir()
            .join("info")
            .join("sparse-checkout")
            .exists(),
        "the common git directory must not gain a definition"
    );
    let main_repo = open(&main_root)?;
    assert_eq!(main_repo.list_sparse_checkout()?, None);
    assert_eq!(
        linked_repo.list_sparse_checkout()?,
        Some((
            gix::index::sparse::Mode::IncludeDirectoriesStoreAllEntriesSkipUnmatched,
            vec!["src/deep".into()]
        ))
    );
    assert!(linked_root.join("src/deep/keep.txt").is_file());
    assert!(
        !linked_root.join("other/drop.txt").exists(),
        "linked skip paths: {:?}",
        skip_paths(&linked_repo)?
    );
    assert!(main_root.join("other/drop.txt").is_file());
    Ok(())
}

#[test]
fn modified_and_obstructed_exclusions_are_preserved() -> crate::Result {
    let temp = gix_testtools::tempfile::TempDir::new()?;
    let root = temp.path().join("subject");
    make_repository(&root)?;

    std::fs::write(root.join("other/drop.txt"), "locally modified\n")?;
    std::fs::remove_file(root.join("other/nested.txt"))?;
    std::fs::create_dir(root.join("other/nested.txt"))?;
    std::fs::write(root.join("other/nested.txt/untracked"), "obstruction\n")?;

    let mut repo = open(&root)?;
    repo.set_sparse_checkout(
        gix::index::sparse::Mode::IncludeByIgnorePatternStoreAllEntriesSkipUnmatched,
        ["/src/"],
    )?;

    assert_eq!(
        std::fs::read_to_string(root.join("other/drop.txt"))?,
        "locally modified\n"
    );
    assert!(root.join("other/nested.txt/untracked").is_file());

    let skipped = skip_paths(&repo)?;
    assert!(
        !skipped.iter().any(|path| path == "other/drop.txt"),
        "modified exclusions remain visible to status"
    );
    assert!(
        !skipped.iter().any(|path| path == "other/nested.txt"),
        "obstructed exclusions remain visible to status"
    );
    assert!(
        skipped.iter().any(|path| path == "other/src/drop.txt"),
        "clean exclusions are removed and marked skip-worktree"
    );
    assert!(!root.join("other/src/drop.txt").exists());
    Ok(())
}

fn open(path: &Path) -> Result<gix::Repository, gix::open::Error> {
    gix::open_opts(path, gix::open::Options::isolated())
}

fn make_repository(root: &Path) -> crate::Result {
    std::fs::create_dir_all(root)?;
    git(root, &["init", "-q"])?;
    git(root, &["config", "user.name", "Gitoxide Test"])?;
    git(root, &["config", "user.email", "gitoxide@example.com"])?;

    for (path, content) in [
        ("root.txt", "root\n"),
        ("src/ancestor.txt", "ancestor\n"),
        ("src/deep/keep.txt", "keep\n"),
        ("src/deep/nested.txt", "nested\n"),
        ("src/drop.txt", "drop\n"),
        ("src/other/drop.txt", "drop\n"),
        ("other/drop.txt", "drop\n"),
        ("other/nested.txt", "nested\n"),
        ("other/src/drop.txt", "anchoring\n"),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("all fixture files have a parent"))?;
        std::fs::write(path, content)?;
    }
    git(root, &["add", "."])?;
    git(root, &["commit", "-q", "-m", "fixture"])?;
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> crate::Result<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["-c", "core.autocrlf=false"])
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_CONFIG_COUNT")
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn skip_paths(repo: &gix::Repository) -> crate::Result<Vec<String>> {
    let index = repo.open_index()?;
    let mut paths: Vec<_> = index
        .entries()
        .iter()
        .filter(|entry| entry.flags.contains(gix::index::entry::Flags::SKIP_WORKTREE))
        .map(|entry| {
            entry
                .path_in(index.path_backing())
                .to_str_lossy()
                .into_owned()
        })
        .collect();
    paths.sort();
    Ok(paths)
}

fn tracked_paths(repo: &gix::Repository) -> crate::Result<Vec<String>> {
    let index = repo.open_index()?;
    let mut paths: Vec<_> = index
        .entries()
        .iter()
        .map(|entry| {
            entry
                .path_in(index.path_backing())
                .to_str_lossy()
                .into_owned()
        })
        .collect();
    paths.sort();
    Ok(paths)
}

fn sparse_directory_entries(repo: &gix::Repository) -> crate::Result<Vec<(String, String)>> {
    let index = repo.open_index()?;
    let mut entries = index
        .entries()
        .iter()
        .filter(|entry| entry.mode == gix::index::entry::Mode::DIR)
        .map(|entry| {
            (
                entry
                    .path_in(index.path_backing())
                    .to_str_lossy()
                    .into_owned(),
                entry.id.to_hex().to_string(),
            )
        })
        .collect::<Vec<_>>();
    entries.sort();
    Ok(entries)
}

fn sparse_entry_id(repo: &gix::Repository, path: &str) -> crate::Result<String> {
    let index = repo.open_index()?;
    Ok(index
        .entries()
        .iter()
        .find(|entry| entry.path_in(index.path_backing()) == path.as_bytes().as_bstr())
        .ok_or_else(|| format!("missing sparse entry {path:?}"))?
        .id
        .to_hex()
        .to_string())
}

fn worktree_files(root: &Path) -> std::io::Result<Vec<String>> {
    fn recurse(root: &Path, directory: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if directory == root && entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                recurse(root, &path, out)?;
            } else {
                let relative: PathBuf = path
                    .strip_prefix(root)
                    .expect("entries are below root")
                    .into();
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    recurse(root, root, &mut out)?;
    out.sort();
    Ok(out)
}