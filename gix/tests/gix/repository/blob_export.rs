use std::{io::Write, path::Path, process::{Command, Stdio}, sync::atomic::{AtomicBool, Ordering}};
use gix::{bstr::ByteSlice, repository::blob_export::{Format, Limits, TreeOptions}};

fn git(root: &Path, args: &[&str], input: &[u8]) -> crate::Result<Vec<u8>> {
    let mut child = Command::new("git").arg("--no-replace-objects")
        .current_dir(root).args(args)
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG_COUNT")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    child.stdin.take().expect("piped stdin").write_all(input)?;
    let out = child.wait_with_output()?;
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    Ok(out.stdout)
}

fn tree(repo: &gix::Repository, mut entries: Vec<gix::objs::tree::Entry>) -> crate::Result<gix::ObjectId> {
    entries.sort();
    Ok(repo.write_object(&gix::objs::Tree { entries })?.detach())
}
fn entry(name: &[u8], mode: gix::objs::tree::EntryKind, oid: gix::ObjectId) -> gix::objs::tree::Entry {
    gix::objs::tree::Entry { filename: name.into(), mode: mode.into(), oid }
}
fn fixture(kind: gix::hash::Kind) -> crate::Result<(gix_testtools::tempfile::TempDir, gix::Repository, gix::ObjectId, gix::ObjectId)> {
    use gix::objs::tree::EntryKind::*;
    let root = gix_testtools::tempfile::tempdir()?;
    let repo = gix::ThreadSafeRepository::init_opts(
        root.path(), gix::create::Kind::Bare,
        gix::create::Options { object_hash: Some(kind), ..Default::default() },
        gix::open::Options::isolated(),
    )?.to_thread_local();
    let blob_id = repo.write_blob(b"\0raw\r\n\xffbytes\n")?.detach();
    let empty_id = repo.write_blob([])?.detach();
    let link_id = repo.write_blob(b"../outside")?.detach();
    let mut children = vec![entry(b"binary\t\n\xff", Blob, blob_id), entry(b"empty", Blob, empty_id)];
    // Store names directly in Git trees so Windows filename restrictions cannot
    // hide quoting bugs. Together these cover every legal byte in a component.
    let all_bytes: Vec<u8> = (1..=255).filter(|byte| *byte != b'/').collect();
    children.push(entry(&all_bytes, Blob, blob_id));
    children.push(entry(b"space and '%(path)%x00", Blob, blob_id));
    // Similar blobs give Git a real delta candidate during repack.
    for i in 0..12 {
        let mut bytes = vec![b'x'; 70_000];
        bytes.extend_from_slice(format!("variant-{i}\n").as_bytes());
        children.push(entry(format!("large-{i:02}").as_bytes(), Blob, repo.write_blob(&bytes)?.detach()));
    }
    let child_id = tree(&repo, children)?;
    let empty_tree_id = tree(&repo, Vec::new())?;
    let root_id = tree(&repo, vec![
        entry(b"a", Tree, child_id), entry(b"a.bin", Blob, blob_id),
        entry(b"empty-tree", Tree, empty_tree_id), entry(b"exec", BlobExecutable, blob_id),
        entry(b"link", Link, link_id), entry(b"submodule", Commit, kind.empty_blob()),
    ])?;
    let actor = gix::actor::SignatureRef {
        name: b"Export test".as_bstr(), email: b"test@example.invalid".as_bstr(), time: "0 +0000",
    };
    repo.commit_as(actor, actor, "HEAD", "export fixture", root_id, std::iter::empty::<gix::ObjectId>())?;
    Ok((root, repo, root_id, blob_id))
}

#[test]
fn raw_tree_and_cat_file_parity_for_both_hashes_loose_and_packed() -> crate::Result {
    for kind in [gix::hash::Kind::Sha1, gix::hash::Kind::Sha256] {
        let (root, repo, tree_id, blob_id) = fixture(kind)?;
        for packed in [false, true] {
            if packed {
                git(root.path(), &["repack", "-adf", "--depth=50", "--window=50"], &[])?;
                // Prove this fixture exercises delta decoding, not just packed full objects.
                let index = std::fs::read_dir(repo.git_dir().join("objects/pack"))?
                    .filter_map(Result::ok).map(|e| e.path())
                    .find(|p| p.extension().is_some_and(|e| e == "idx")).expect("pack index");
                let output = git(root.path(), &["verify-pack", "-v", index.to_str().expect("temporary path")], &[])?;
                assert!(String::from_utf8_lossy(&output).lines().any(|line| line.split_whitespace().count() == 7));
            }
            let reader = repo.blob_export(Limits::default()).map_err(|e| e.into_error())?;
            for (recursive, show_trees, trees_only) in [(false,false,false),(true,false,false),(true,true,false),(true,false,true),(false,false,true)] {
                for (format, flag) in [
                    (Format::Default,None),(Format::Long,Some("-l")),
                    (Format::NameOnly,Some("--name-only")),(Format::ObjectOnly,Some("--object-only")),
                    (Format::Quoted,None),
                    (Format::Custom("%(objectmode) %(objecttype) %(objectname)%x09%(path)".into()),Some("--format=%(objectmode) %(objecttype) %(objectname)%x09%(path)")),
                    (Format::Custom("%(objectsize)|%(objectsize:padded)|%(path)|%%|%x00%x09%x0a%xFF".into()),Some("--format=%(objectsize)|%(objectsize:padded)|%(path)|%%|%x00%x09%x0a%xFF")),
                    (Format::Custom("%(objectmode) %(objecttype) %(objectname) %(objectsize:padded)%x09%(path)".into()),Some("--format=%(objectmode) %(objecttype) %(objectname) %(objectsize:padded)%x09%(path)")),
                    (Format::Custom("prefix:%(objectmode) %(objecttype) %(objectname)%x09%(path)".into()),Some("--format=prefix:%(objectmode) %(objecttype) %(objectname)%x09%(path)")),
                    (Format::Custom("%(objectname)".into()),Some("--format=%(objectname)")),
                    (Format::Custom("%(path)".into()),Some("--format=%(path)")),
                    (Format::Custom("literal%%end".into()),Some("--format=literal%%end")),
                    (Format::Custom("".into()),Some("--format=")),
                ] {
                    let mut args = vec!["-c", "core.quotePath=true", "ls-tree", "--full-tree"];
                    if !matches!(format, Format::Quoted) { args.push("-z"); }
                    if recursive { args.push("-r"); }
                    if show_trees { args.push("-t"); }
                    if trees_only { args.push("-d"); }
                    if let Some(flag) = flag { args.push(flag); }
                    let hex = tree_id.to_string();
                    args.push(&hex);
                    let expected = git(root.path(), &args, &[])?;
                    let mut actual = Vec::new();
                    reader.write_tree(tree_id, &TreeOptions {recursive,show_trees,trees_only,format}, &mut actual, &AtomicBool::new(false))
                        .map_err(|e| e.into_error())?;
                    assert_eq!(actual.as_bstr(), expected.as_bstr(), "{args:?}, packed={packed}");
                }
            }
            let cancel = AtomicBool::new(false);
            for id in [blob_id, tree_id, repo.head_id()?.detach(), kind.empty_blob()] {
                let object = reader.read_object(id, &cancel).map_err(|e| e.into_error())?;
                let hex = id.to_string();
                assert_eq!(git(root.path(), &["cat-file", "-t", &hex], &[])?, format!("{}\n", object.kind).as_bytes());
                assert_eq!(git(root.path(), &["cat-file", "-s", &hex], &[])?, format!("{}\n", object.data.len()).as_bytes());
                assert_eq!(git(root.path(), &["cat-file", &object.kind.to_string(), &hex], &[])?, object.data);
            }
            let mut raw = Vec::new();
            reader.write_blob(blob_id, None, &mut raw, &cancel).map_err(|e| e.into_error())?;
            assert_eq!(raw, git(root.path(), &["cat-file", "blob", &blob_id.to_string()], &[])?);
        }
    }
    Ok(())
}

#[test]
fn malformed_trees_fail_before_output_and_semantic_defects_are_rejected() -> crate::Result {
    let (_root, repo, _tree_id, blob_id) = fixture(gix::hash::Kind::Sha256)?;
    let reader = repo.blob_export(Limits::default()).map_err(|e| e.into_error())?;
    let record = |mode: &str, name: &[u8]| {
        let mut data = format!("{mode} ").into_bytes();
        data.extend_from_slice(name); data.push(0); data.extend_from_slice(blob_id.as_bytes()); data
    };
    let good = record("100644", b"target");
    let mut trailing = good.clone(); trailing.extend_from_slice(b"100644 broken\0");
    let mut duplicate = good.clone(); duplicate.extend_from_slice(&good);
    let mut unsorted = record("100644", b"z"); unsorted.extend_from_slice(&good);
    for data in [trailing, duplicate, unsorted, record("100600", b"target"), record("100644", b"../escape"), record("100644", b""), record("100644", b".")] {
        let id = gix::objs::Write::write_buf(&repo.objects, gix::objs::Kind::Tree, &data)?;
        let mut output = Vec::new();
        assert!(reader.write_tree(id, &TreeOptions::default(), &mut output, &AtomicBool::new(false)).is_err());
        assert!(output.is_empty());
    }
    Ok(())
}

#[test]
fn export_rejects_bad_identity_kind_size_limits_and_replacements() -> crate::Result {
    let (root, repo, tree_id, blob_id) = fixture(gix::hash::Kind::Sha1)?;
    let reader = repo.blob_export(Limits::default()).map_err(|e| e.into_error())?;
    let cancel = AtomicBool::new(false);
    let mut output = Vec::new();
    assert!(reader.write_blob(tree_id, None, &mut output, &cancel).is_err());
    assert!(reader.write_blob(blob_id, Some(1), &mut output, &cancel).is_err());
    assert!(reader.read_object(repo.object_hash().null(), &cancel).is_err());
    assert!(reader.read_object(gix::hash::Kind::Sha256.empty_blob(), &cancel).is_err());
    assert!(output.is_empty());
    for limits in [
        Limits { object_bytes: 1, ..Default::default() },
        Limits { entries: 1, ..Default::default() },
        Limits { path_bytes: 1, ..Default::default() },
    ] {
        let limited = repo.blob_export(limits).map_err(|e| e.into_error())?;
        assert!(limited.list_tree(tree_id, &TreeOptions { recursive: true, ..Default::default() }, &cancel).is_err());
    }
    let replacement_id = repo.write_blob(b"replacement")?.detach();
    git(root.path(), &["replace", &blob_id.to_string(), &replacement_id.to_string()], &[])?;
    let fresh = repo.blob_export(Limits::default()).map_err(|e| e.into_error())?;
    assert_eq!(fresh.read_object(blob_id, &cancel).map_err(|e| e.into_error())?.data, b"\0raw\r\n\xffbytes\n");
    // Valid compressed bytes under the wrong loose-object name must fail rehashing.
    let object_path = |id: gix::ObjectId| {
        let hex = id.to_string(); repo.git_dir().join("objects").join(&hex[..2]).join(&hex[2..])
    };
    std::fs::copy(object_path(replacement_id), object_path(blob_id))?;
    let corrupt = repo.blob_export(Limits::default()).map_err(|e| e.into_error())?;
    assert!(corrupt.write_blob(blob_id, None, &mut output, &cancel).is_err());
    assert!(output.is_empty());
    Ok(())
}

#[test]
fn output_failure_cancellation_and_no_clobber_leave_no_partial_destination() -> crate::Result {
    let (root, repo, tree_id, blob_id) = fixture(gix::hash::Kind::Sha256)?;
    let reader = repo.blob_export(Limits::default()).map_err(|e| e.into_error())?;
    let cancel = AtomicBool::new(false);
    let destination = root.path().join("export");
    reader.export_blob(blob_id, None, &destination, &cancel).map_err(|e| e.into_error())?;
    let original = std::fs::read(&destination)?;
    assert!(reader.export_blob(blob_id, None, &destination, &cancel).is_err());
    assert_eq!(std::fs::read(&destination)?, original);
    let directory = root.path().join("directory"); std::fs::create_dir(&directory)?;
    assert!(reader.export_blob(blob_id, None, &directory, &cancel).is_err());
    assert!(directory.is_dir());
    let absent = root.path().join("absent");
    assert!(reader.export_blob(blob_id, Some(999), &absent, &cancel).is_err());
    assert!(!absent.exists());
    cancel.store(true, Ordering::Relaxed);
    assert!(reader.export_blob(blob_id, None, &absent, &cancel).is_err());
    assert!(!absent.exists());
    struct Fail;
    impl Write for Fail {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> { Err(std::io::Error::other("injected output failure")) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    cancel.store(false, Ordering::Relaxed);
    assert!(reader.write_blob(blob_id, None, &mut Fail, &cancel).is_err());
    for format in [Format::Quoted, Format::Custom("%(path)!".into())] {
        let options = TreeOptions { format, ..Default::default() };
        assert!(reader.write_tree(tree_id, &options, &mut Fail, &cancel).is_err());
    }
    Ok(())
}

#[test]
fn depth_limit_and_mid_write_cancellation_are_enforced() -> crate::Result {
    let (_root, repo, tree_id, _blob_id) = fixture(gix::hash::Kind::Sha256)?;
    let wrapper = tree(&repo, vec![entry(b"wrapped", gix::objs::tree::EntryKind::Tree, tree_id)])?;
    let reader = repo.blob_export(Limits { depth: 1, ..Default::default() }).map_err(|e| e.into_error())?;
    assert!(reader.list_tree(wrapper, &TreeOptions { recursive: true, ..Default::default() }, &AtomicBool::new(false)).is_err());
    let blob_id = repo.write_blob(vec![b'a'; 200_000])?.detach();
    let cancel = AtomicBool::new(false);
    struct CancelWriter<'a> { cancel: &'a AtomicBool, written: usize }
    impl Write for CancelWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.written += bytes.len();
            self.cancel.store(true, Ordering::Relaxed);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut out = CancelWriter { cancel: &cancel, written: 0 };
    assert!(reader.write_blob(blob_id, None, &mut out, &cancel).is_err());
    assert_eq!(out.written, 64 * 1024);
    Ok(())
}

#[test]
fn custom_formats_validate_completely_even_for_empty_trees() -> crate::Result {
    let (_root, repo, tree_id, _blob_id) = fixture(gix::hash::Kind::Sha256)?;
    let empty_tree_id = tree(&repo, Vec::new())?;
    let reader = repo.blob_export(Limits::default()).map_err(|e| e.into_error())?;
    let cancel = AtomicBool::new(false);
    for input in [
        "%", "prefix%", "%x", "%x0", "%xGG", "%X00", "%q",
        "%(", "%(path", "%()", "%(unknown)", "%(path:quoted)",
        "%(objectsize:bad)", "%(objectname:short)", "%(path)valid%x0",
    ] {
        for tree_id in [tree_id, empty_tree_id] {
            let options = TreeOptions { format: Format::Custom(input.into()), ..Default::default() };
            let mut output = Vec::new();
            assert!(reader.write_tree(tree_id, &options, &mut output, &cancel).is_err(), "{input:?}");
            assert!(output.is_empty(), "invalid formats fail before output: {input:?}");
            assert!(reader.list_tree(tree_id, &options, &cancel).is_err(), "enumeration also validates options");
        }
    }
    Ok(())
}

#[test]
fn custom_output_checks_cancellation_between_literal_chunks() -> crate::Result {
    let (_root, repo, tree_id, _blob_id) = fixture(gix::hash::Kind::Sha1)?;
    let reader = repo.blob_export(Limits::default()).map_err(|e| e.into_error())?;
    let cancel = AtomicBool::new(false);
    struct CancelWriter<'a> { cancel: &'a AtomicBool, written: usize }
    impl Write for CancelWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.written += bytes.len();
            self.cancel.store(true, Ordering::Relaxed);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let options = TreeOptions { format: Format::Custom(vec![b'x'; 200_000].into()), ..Default::default() };
    let mut output = CancelWriter { cancel: &cancel, written: 0 };
    assert!(reader.write_tree(tree_id, &options, &mut output, &cancel).is_err());
    assert_eq!(output.written, 64 * 1024, "custom literals use bounded cancellable writes");
    Ok(())
}
