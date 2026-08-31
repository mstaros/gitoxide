use bstr::{BString, ByteSlice};
use gix_index::{
    State,
    entry::{Flags, Mode, Stage, Stat},
};
use gix_object::Write;

use crate::{fixture_index_path, odb_at};

fn sparse_fixture() -> crate::Result<(
    State,
    gix_odb::memory::Proxy<gix_odb::Handle>,
)> {
    let index_path = fixture_index_path("v3_sparse_index");
    let objects = gix_odb::memory::Proxy::new(
        odb_at(index_path.parent().expect("the index is inside .git").join("objects"))?,
        gix_testtools::object_hash(),
    );
    let state = gix_index::File::at(
        index_path,
        gix_testtools::object_hash(),
        false,
        Default::default(),
    )?
    .into();
    Ok((state, objects))
}

#[test]
fn sparse_directories_expand_to_full_index_entries() -> crate::Result {
    let (mut state, objects) = sparse_fixture()?;
    assert!(state.is_sparse());

    let outcome = state.expand_sparse_index(&objects, Default::default())?;

    assert_eq!(outcome.sparse_directories, 2);
    assert_eq!(outcome.entries_added, 7);
    assert!(!state.is_sparse());
    assert!(state.tree().is_none());
    assert!(state.fs_monitor().is_none());

    let paths = state
        .entries()
        .iter()
        .map(|entry| entry.path(&state).to_owned())
        .collect::<Vec<_>>();
    let expected = [
        "a",
        "b",
        "c1/a",
        "c1/b",
        "c1/c2/a",
        "c1/c2/b",
        "c1/c3/a",
        "c1/c3/b",
        "d/a",
        "d/b",
        "d/c4/a",
        "d/c4/b",
        "d/c4/c5",
    ]
    .map(BString::from)
    .to_vec();
    assert_eq!(paths, expected);

    for entry in state.entries() {
        let path = entry.path(&state);
        if path.starts_with(b"c1/c3/") || path.starts_with(b"d/") {
            assert_eq!(entry.flags, Flags::EXTENDED | Flags::SKIP_WORKTREE);
            assert_eq!(entry.stat, Stat::default());
        } else {
            assert!(entry.flags.is_empty());
        }
        assert_ne!(entry.mode, Mode::DIR);
    }
    Ok(())
}

#[test]
fn compression_roundtrip_preserves_staged_entries_outside_the_cone() -> crate::Result {
    let (mut state, objects) = sparse_fixture()?;
    state.expand_sparse_index(&objects, Default::default())?;

    let staged = objects.write_buf(gix_object::Kind::Blob, b"staged outside cone")?;
    let path_backing = state.path_backing().to_owned();
    state
        .entries_mut_with_paths_in(&path_backing)
        .find(|(_, path)| *path == b"d/a".as_bstr())
        .expect("d/a exists")
        .0
        .id = staged;

    let outcome = state.convert_to_sparse_index(|tree| objects.write(tree))?;

    assert_eq!(outcome.sparse_directories, 2);
    assert_eq!(outcome.entries_before, 13);
    assert_eq!(outcome.entries_after, 8);
    assert!(state.is_sparse());
    assert_eq!(
        state
            .entries()
            .iter()
            .filter(|entry| entry.mode == Mode::DIR)
            .map(|entry| entry.path(&state).to_owned())
            .collect::<Vec<_>>(),
        ["c1/c3/", "d/"].map(BString::from).to_vec()
    );

    state.expand_sparse_index(&objects, Default::default())?;
    let staged_entry = state
        .entries()
        .iter()
        .find(|entry| entry.path(&state) == b"d/a".as_bstr())
        .expect("d/a expands again");
    assert_eq!(staged_entry.id, staged);
    Ok(())
}

#[test]
fn compression_leaves_ineligible_directories_expanded() -> crate::Result {
    let (mut state, objects) = sparse_fixture()?;
    state.expand_sparse_index(&objects, Default::default())?;

    let path_backing = state.path_backing().to_owned();
    for (entry, path) in state.entries_mut_with_paths_in(&path_backing) {
        if path == b"c1/c3/a".as_bstr() {
            entry.flags = Flags::from_stage(Stage::Ours) | Flags::EXTENDED | Flags::SKIP_WORKTREE;
        } else if path == b"d/a".as_bstr() {
            entry.mode = Mode::COMMIT;
        }
    }

    let outcome = state.convert_to_sparse_index(|tree| objects.write(tree))?;

    assert_eq!(outcome.sparse_directories, 1);
    let sparse_paths = state
        .entries()
        .iter()
        .filter(|entry| entry.mode == Mode::DIR)
        .map(|entry| entry.path(&state).to_owned())
        .collect::<Vec<_>>();
    assert_eq!(sparse_paths, [BString::from("d/c4/")]);
    assert!(state
        .entries()
        .iter()
        .any(|entry| entry.path(&state) == b"c1/c3/a".as_bstr()));
    assert!(state
        .entries()
        .iter()
        .any(|entry| entry.path(&state) == b"d/a".as_bstr()));
    Ok(())
}

#[test]
fn sparse_marker_roundtrips_even_without_directory_entries() -> crate::Result {
    let index_path = fixture_index_path("v2_sparse_index_no_dirs");
    let objects = gix_odb::memory::Proxy::new(
        odb_at(index_path.parent().expect("the index is inside .git").join("objects"))?,
        gix_testtools::object_hash(),
    );
    let mut state: State = gix_index::File::at(
        index_path,
        gix_testtools::object_hash(),
        false,
        Default::default(),
    )?
    .into();
    assert!(state.is_sparse());

    let expanded = state.expand_sparse_index(&objects, Default::default())?;
    assert_eq!(expanded.sparse_directories, 0);
    assert!(!state.is_sparse());

    let compressed = state.convert_to_sparse_index(|tree| objects.write(tree))?;
    assert_eq!(compressed.sparse_directories, 0);
    assert!(state.is_sparse());

    let mut bytes = Vec::new();
    let file = gix_index::File::from_state(state, "unused-index-path");
    file.write_to(&mut bytes, Default::default())?;
    let (roundtrip, _) = State::from_bytes(
        &bytes,
        filetime::FileTime::now(),
        gix_testtools::object_hash(),
        Default::default(),
    )?;
    assert!(roundtrip.is_sparse());
    assert!(roundtrip.entries().iter().all(|entry| entry.mode != Mode::DIR));
    Ok(())
}

#[test]
fn split_indexes_are_not_silently_converted() -> crate::Result {
    let bytes = std::fs::read(fixture_index_path("v2_split_index"))?;
    let (mut state, _) = State::from_bytes(
        &bytes,
        filetime::FileTime::now(),
        gix_testtools::object_hash(),
        Default::default(),
    )?;

    let err = state
        .convert_to_sparse_index(|_| unreachable!("split indexes fail before tree writes"))
        .expect_err("split indexes must remain split");
    assert!(matches!(
        err,
        gix_index::sparse::compress::Error::SplitIndex
    ));
    Ok(())
}
