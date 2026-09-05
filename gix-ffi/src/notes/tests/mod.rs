use super::*;

#[test]
fn notes_publish_rejects_competing_creation_update_and_symbolic_retarget() {
    let path = std::env::temp_dir().join(format!("gix-notes-guard-{}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create fixture");
    let repo = gix::init(&path).expect("init fixture");
    let signature = signature(b"Guard", b"guard@example.com", 1_700_000_000, 0).expect("signature");
    let annotated_object_id = repo.write_blob(b"annotated").expect("write object").to_string();
    let annotated = ffi::String::from(annotated_object_id);
    let name = FullName::try_from("refs/notes/commits").expect("reference name");
    for exists in [false, true] {
        let pending = root(&repo, name.clone()).expect("capture root");
        write(&repo, &annotated, b"refs/notes/commits", if exists { b"second" } else { b"first" },
            signature.clone(), signature.clone(), true).expect("competing writer");
        let before = repo.find_reference(name.as_ref()).expect("current ref").target().into_owned();
        let old_tree_id = pending.tree_id;
        assert!(matches!(publish(&repo, pending, old_tree_id, &signature, &signature, "stale"),
            Err(GixError::ReferenceConflict(_))));
        assert_eq!(repo.find_reference(name.as_ref()).expect("preserved ref").target().into_owned(), before);
    }

    let alias = FullName::try_from("refs/notes/alias").expect("alias name");
    let other_name = FullName::try_from("refs/notes/other").expect("other name");
    let mut time = gix::date::parse::TimeBuf::default();
    repo.edit_references_as([RefEdit {
        name: alias.clone(), deref: false,
        change: Change::Update { log: LogChange::default(), expected: PreviousValue::MustNotExist,
            new: Target::Symbolic(name.clone()) },
    }], Some(signature.to_ref(&mut time))).expect("create symbolic alias");
    let pending = root(&repo, alias.clone()).expect("capture alias chain");
    repo.edit_references_as([RefEdit {
        name: alias.clone(), deref: false,
        change: Change::Update { log: LogChange::default(),
            expected: PreviousValue::MustExistAndMatch(Target::Symbolic(name.clone())),
            new: Target::Symbolic(other_name.clone()) },
    }], Some(signature.to_ref(&mut time))).expect("competing symbolic retarget");
    let direct_before = repo.find_reference(name.as_ref()).expect("current direct ref").target().into_owned();
    let old_tree_id = pending.tree_id;
    assert!(matches!(publish(&repo, pending, old_tree_id, &signature, &signature, "stale alias"),
        Err(GixError::ReferenceConflict(_))));
    assert_eq!(repo.find_reference(alias.as_ref()).expect("preserved alias").target().into_owned(), Target::Symbolic(other_name));
    assert_eq!(repo.find_reference(name.as_ref()).expect("preserved direct ref").target().into_owned(), direct_before);
    drop(repo);
    std::fs::remove_dir_all(path).expect("clean fixture");
}