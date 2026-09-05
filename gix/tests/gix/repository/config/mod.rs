mod config_snapshot;
#[cfg(feature = "command")]
mod editor;
mod identity;
mod remote;

#[test]
fn linked_worktree_gitdir_includes_use_the_private_git_directory() -> crate::Result {
    use std::io::Write;

    let temp = gix_testtools::tempfile::TempDir::new()?;
    let main_path = temp.path().join("main");
    let main = gix::init(&main_path)?;
    let linked_path = temp.path().join("linked");
    let private = main.git_dir().join("worktrees").join("linked");
    std::fs::create_dir_all(&linked_path)?;
    std::fs::create_dir_all(&private)?;
    // An unborn linked worktree still has its own git-dir. Its common config is
    // shared, while includeIf.gitdir conditions must match this private path.
    std::fs::write(private.join("HEAD"), b"ref: refs/heads/main\n")?;
    std::fs::write(private.join("commondir"), b"../..\n")?;
    std::fs::write(
        private.join("gitdir"),
        format!("{}\n", linked_path.join(".git").display()),
    )?;
    std::fs::write(linked_path.join(".git"), format!("gitdir: {}\n", private.display()))?;
    std::fs::write(main.git_dir().join("private-include"), b"[testing]\n private = selected\n")?;
    let normalized = private.to_string_lossy().replace('\\', "/");
    let mut config = std::fs::OpenOptions::new()
        .append(true)
        .open(main.git_dir().join("config"))?;
    write!(
        config,
        "\n[includeIf \"gitdir:{normalized}\"]\n path = private-include\n"
    )?;
    drop(config);

    let mut options = gix::open::Options::isolated();
    options.permissions.config.includes = true;
    let linked = gix::open_opts(&linked_path, options.clone())?;
    assert_eq!(
        linked.config_snapshot().string("testing.private"),
        Some("selected".into()),
        "linked worktree conditions use its private git-dir"
    );
    let main = gix::open_opts(&main_path, options)?;
    assert_eq!(
        main.config_snapshot().string("testing.private"),
        None,
        "the shared config does not make a private git-dir condition match the main worktree"
    );
    assert_eq!(
        std::fs::canonicalize(linked.common_dir())?,
        std::fs::canonicalize(main.common_dir())?,
        "configuration storage remains shared"
    );
    Ok(())
}

#[test]
fn big_file_threshold() -> crate::Result {
    let repo = repo("with-hasconfig");
    assert_eq!(
        repo.big_file_threshold()?,
        512 * 1024 * 1024,
        "Git really handles huge files, and this is the default"
    );

    let repo = crate::repository::config::repo("big-file-threshold");
    assert_eq!(repo.big_file_threshold()?, 42, "It picks up configured values as well");
    Ok(())
}

#[cfg(feature = "blocking-network-client")]
mod ssh_options {
    use std::ffi::OsStr;

    use crate::repository::config::repo;

    #[test]
    fn with_command_and_variant() -> crate::Result {
        let repo = repo("ssh-all-options");
        let opts = repo.ssh_connect_options()?;
        assert_eq!(opts.command.as_deref(), Some(OsStr::new("ssh -VVV")));
        assert_eq!(
            opts.kind,
            Some(gix::protocol::transport::client::blocking_io::ssh::ProgramKind::Ssh)
        );
        assert!(!opts.disallow_shell, "we can use the shell by default");
        Ok(())
    }

    #[test]
    fn with_command_fallback_which_disallows_shell() -> crate::Result {
        let repo = repo("ssh-command-fallback");
        let opts = repo.ssh_connect_options()?;
        assert_eq!(opts.command.as_deref(), Some(OsStr::new("ssh --fallback")));
        assert_eq!(
            opts.kind,
            Some(gix::protocol::transport::client::blocking_io::ssh::ProgramKind::Putty)
        );
        assert!(
            opts.disallow_shell,
            "fallbacks won't allow shells, so must be a program or program name"
        );
        Ok(())
    }
}

#[cfg(any(feature = "blocking-network-client", feature = "async-network-client"))]
mod transport_options;

pub fn repo(name: &str) -> gix::Repository {
    repo_opts(name, |opts| opts.strict_config(true))
}

pub fn repo_opts(name: &str, modify: impl FnOnce(gix::open::Options) -> gix::open::Options) -> gix::Repository {
    let dir = gix_testtools::scripted_fixture_read_only("make_config_repos.sh").unwrap();
    gix::open_opts(dir.join(name), modify(gix::open::Options::isolated())).unwrap()
}
