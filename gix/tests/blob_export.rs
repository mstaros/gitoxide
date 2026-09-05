use std::sync::atomic::AtomicBool;

#[test]
fn blob_export_keeps_the_repository_location_after_cwd_changes() -> gix_testtools::Result {
    let root = gix_testtools::tempfile::tempdir()?;
    let first = root.path().join("first");
    let second = root.path().join("second");
    std::fs::create_dir(&first)?;
    std::fs::create_dir(&second)?;
    let _cwd = gix_testtools::set_current_dir(&first)?;
    gix::init_bare("repo")?;
    let repo = gix::open_opts("repo", gix::open::Options::isolated())?;
    assert!(repo.git_dir().is_relative(), "exercise an actual relative repository path");
    let blob_id = repo.write_blob(b"original repository")?.detach();
    std::env::set_current_dir(&second)?;
    gix::init_bare("repo")?;
    let export = repo.blob_export(Default::default()).map_err(|e| e.into_error())?;
    let object = export.read_object(blob_id, &AtomicBool::new(false)).map_err(|e| e.into_error())?;
    assert_eq!(object.data, b"original repository");
    Ok(())
}