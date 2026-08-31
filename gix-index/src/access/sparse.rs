use std::collections::{BTreeMap, btree_map};

use bstr::{BStr, BString, ByteSlice};

use crate::{
    Entry, PathStorage, State,
    entry::{Flags, Mode as EntryMode, Stage, Stat},
};

/// Configuration related to sparse indexes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// If true, certain entries in the index will be excluded / skipped for certain operations,
    /// based on the ignore patterns in the `.git/info/sparse-checkout` file. These entries will
    /// carry the [`SKIP_WORKTREE`][crate::entry::Flags::SKIP_WORKTREE] flag.
    ///
    /// This typically is the value of `core.sparseCheckout` in the git configuration.
    pub sparse_checkout: bool,

    /// Interpret the `.git/info/sparse-checkout` file using _cone mode_.
    ///
    /// If true, _cone mode_ is active and entire directories will be included in the checkout, as well as files in the root
    /// of the repository.
    /// If false, non-cone mode is active and entries to _include_ will be matched with patterns like those found in `.gitignore` files.
    ///
    /// This typically is the value of `core.sparseCheckoutCone` in the git configuration.
    pub directory_patterns_only: bool,

    /// If true, will attempt to write a sparse index file which only works in cone mode.
    ///
    /// A sparse index has [`DIR` entries][crate::entry::Mode::DIR] that represent entire directories to be skipped
    /// during checkout and other operations due to the added presence of
    /// the [`SKIP_WORKTREE`][crate::entry::Flags::SKIP_WORKTREE] flag.
    ///
    /// This is typically the value of `index.sparse` in the git configuration.
    pub write_sparse_index: bool,
}

impl Options {
    /// Derive a valid mode from all parameters that affect the 'sparseness' of the index.
    ///
    /// Some combinations of them degenerate to one particular mode.
    pub fn sparse_mode(&self) -> Mode {
        match (
            self.sparse_checkout,
            self.directory_patterns_only,
            self.write_sparse_index,
        ) {
            (true, true, true) => Mode::IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs,
            (true, true, false) => Mode::IncludeDirectoriesStoreAllEntriesSkipUnmatched,
            (true, false, _) => Mode::IncludeByIgnorePatternStoreAllEntriesSkipUnmatched,
            (false, _, _) => Mode::Disabled,
        }
    }
}

/// Describes the configuration how a sparse index should be written, or if one should be written at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// index with DIR entries for exclusion and included entries, directory-only include patterns in `.git/info/sparse-checkout` file.
    IncludeDirectoriesStoreIncludedEntriesAndExcludedDirs,
    /// index with all file entries and skip worktree flags for exclusion, directory-only include patterns in `.git/info/sparse-checkout` file.
    IncludeDirectoriesStoreAllEntriesSkipUnmatched,
    /// index with all file entries and skip-worktree flags for exclusion, `ignore` patterns to include entries in `.git/info/sparse-checkout` file.
    IncludeByIgnorePatternStoreAllEntriesSkipUnmatched,
    /// index with all entries, none is excluded, `.git/info/sparse-checkout` file is not considered, a regular index.
    Disabled,
}

/// Sparse-index expansion.
pub mod expand {
    use bstr::BString;

    /// Information about a sparse-index expansion.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Outcome {
        /// The number of entries before expansion.
        pub entries_before: usize,
        /// The number of entries after expansion.
        pub entries_after: usize,
        /// The number of sparse-directory entries replaced.
        pub sparse_directories: usize,
        /// The number of descendant entries inserted for sparse directories.
        pub entries_added: usize,
    }

    /// An error returned by sparse-index expansion.
    #[derive(Debug, thiserror::Error)]
    #[expect(missing_docs)]
    pub enum Error {
        #[error("A split index cannot be expanded as a sparse index")]
        SplitIndex,
        #[error("Sparse-directory paths must end in '/': {path:?}")]
        InvalidSparseDirectoryPath { path: BString },
        #[error("Could not expand a sparse-directory tree")]
        FromTree(#[from] crate::init::from_tree::Error),
    }
}

/// Sparse-index compression.
pub mod compress {
    use bstr::BString;

    /// Information about a sparse-index compression.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Outcome {
        /// The number of entries before compression.
        pub entries_before: usize,
        /// The number of entries after compression.
        pub entries_after: usize,
        /// The number of sparse-directory entries produced.
        pub sparse_directories: usize,
    }

    /// An error returned by sparse-index compression.
    #[derive(Debug, thiserror::Error)]
    #[expect(missing_docs)]
    pub enum Error {
        #[error("A split index cannot be converted to a sparse index")]
        SplitIndex,
        #[error("The index must be expanded before compression; found sparse directory {path:?}")]
        SparseDirectoryPresent { path: BString },
        #[error("Invalid index path {path:?}: {reason}")]
        InvalidPath { path: BString, reason: &'static str },
        #[error("Index mode {mode:#o} at {path:?} cannot be represented in a tree")]
        InvalidMode { path: BString, mode: u32 },
        #[error("Could not write a tree while compressing the sparse index")]
        TreeWrite(#[from] gix_object::write::Error),
    }
}

impl State {
    /// Expand all sparse-directory entries into their recursively stored index entries.
    ///
    /// Descendants are initialized with zero stat data and `EXTENDED|SKIP_WORKTREE`,
    /// matching Git's full-index representation of paths outside the sparse cone. The
    /// sparse marker, cache-tree, fsmonitor extension, and fsmonitor-valid entry bits are
    /// removed only after all referenced trees were expanded successfully.
    pub fn expand_sparse_index<Find>(
        &mut self,
        objects: Find,
        validate: gix_validate::path::component::Options,
    ) -> Result<expand::Outcome, expand::Error>
    where
        Find: gix_object::Find,
    {
        let entries_before = self.entries.len();
        let sparse_directories = self.entries.iter().filter(|entry| entry.mode == EntryMode::DIR).count();
        if !self.is_sparse && sparse_directories == 0 {
            return Ok(expand::Outcome {
                entries_before,
                entries_after: entries_before,
                sparse_directories: 0,
                entries_added: 0,
            });
        }
        if self.link.is_some() {
            return Err(expand::Error::SplitIndex);
        }

        let mut entries = Vec::with_capacity(entries_before);
        let mut path_backing = PathStorage::with_capacity(self.path_backing.len());
        let mut entries_added = 0;

        for entry in &self.entries {
            let path = entry.path(self);
            if entry.mode != EntryMode::DIR {
                push_entry(&mut entries, &mut path_backing, entry.clone(), path);
                continue;
            }
            if !path.ends_with(b"/") {
                return Err(expand::Error::InvalidSparseDirectoryPath { path: path.to_owned() });
            }

            let expanded = State::from_tree(&entry.id, &objects, validate)?;
            entries_added += expanded.entries.len();
            for descendant in &expanded.entries {
                let mut descendant = descendant.clone();
                descendant.stat = Stat::default();
                descendant.flags = Flags::EXTENDED | Flags::SKIP_WORKTREE;

                let mut full_path = path.to_owned();
                full_path.extend_from_slice(descendant.path(&expanded));
                push_entry(
                    &mut entries,
                    &mut path_backing,
                    descendant,
                    full_path.as_bstr(),
                );
            }
        }

        clear_fsmonitor_valid(&mut entries);
        self.entries = entries;
        self.path_backing = path_backing;
        self.is_sparse = false;
        self.tree = None;
        self.fs_monitor = None;
        self.sort_entries();

        Ok(expand::Outcome {
            entries_before,
            entries_after: self.entries.len(),
            sparse_directories,
            entries_added,
        })
    }

    /// Convert a full index to Git's sparse-index representation.
    ///
    /// Every non-root directory whose descendants are all stage zero, marked
    /// `SKIP_WORKTREE`, and contain no gitlinks is replaced by one sparse-directory
    /// entry. Trees are written bottom-up through `write_tree` from the IDs and modes
    /// currently in the index, so staged changes outside the cone remain represented.
    ///
    /// The resulting state is marked sparse even when no directory can be collapsed;
    /// this causes the mandatory `sdir` extension to be written for index versions 2
    /// through 4.
    pub fn convert_to_sparse_index(
        &mut self,
        mut write_tree: impl FnMut(&gix_object::Tree) -> Result<gix_hash::ObjectId, gix_object::write::Error>,
    ) -> Result<compress::Outcome, compress::Error> {
        if self.link.is_some() {
            return Err(compress::Error::SplitIndex);
        }
        if let Some(entry) = self.entries.iter().find(|entry| entry.mode == EntryMode::DIR) {
            return Err(compress::Error::SparseDirectoryPresent {
                path: entry.path(self).to_owned(),
            });
        }

        let entries_before = self.entries.len();
        let mut root = Directory::default();
        for entry in &self.entries {
            let path = entry.path(self).to_owned();
            validate_index_path(path.as_bstr())?;
            let components = path.as_slice().split(|byte| *byte == b'/').collect::<Vec<_>>();
            insert_entry(
                &mut root,
                &components,
                StoredEntry {
                    entry: entry.clone(),
                    path: path.clone(),
                },
            )?;
        }

        let mut entries = Vec::with_capacity(entries_before);
        let mut path_backing = PathStorage::with_capacity(self.path_backing.len());
        let mut sparse_directories = 0;
        emit_directory(
            &root,
            BStr::new(b""),
            &mut write_tree,
            &mut entries,
            &mut path_backing,
            &mut sparse_directories,
        )?;

        clear_fsmonitor_valid(&mut entries);
        self.entries = entries;
        self.path_backing = path_backing;
        self.is_sparse = true;
        self.tree = None;
        self.fs_monitor = None;
        self.sort_entries();

        Ok(compress::Outcome {
            entries_before,
            entries_after: self.entries.len(),
            sparse_directories,
        })
    }
}

#[derive(Default)]
struct Directory {
    children: BTreeMap<BString, Node>,
}

enum Node {
    Directory(Directory),
    Entries(Vec<StoredEntry>),
}

struct StoredEntry {
    entry: Entry,
    path: BString,
}

impl Directory {
    fn can_collapse(&self) -> bool {
        !self.children.is_empty()
            && self.children.values().all(|node| match node {
                Node::Directory(directory) => directory.can_collapse(),
                Node::Entries(entries) => {
                    entries.len() == 1
                        && entries[0].entry.stage() == Stage::Unconflicted
                        && entries[0].entry.flags.contains(Flags::SKIP_WORKTREE)
                        && !entries[0]
                            .entry
                            .flags
                            .intersects(Flags::INTENT_TO_ADD | Flags::REMOVE)
                        && entries[0].entry.mode != EntryMode::COMMIT
                }
            })
    }
}

fn validate_index_path(path: &BStr) -> Result<(), compress::Error> {
    if path.is_empty() {
        return Err(compress::Error::InvalidPath {
            path: path.to_owned(),
            reason: "paths must not be empty",
        });
    }
    if path[0] == b'/' || path[path.len() - 1] == b'/' || path.windows(2).any(|bytes| bytes == b"//") {
        return Err(compress::Error::InvalidPath {
            path: path.to_owned(),
            reason: "paths must be relative and contain no empty components",
        });
    }
    Ok(())
}

fn insert_entry(
    directory: &mut Directory,
    components: &[&[u8]],
    stored: StoredEntry,
) -> Result<(), compress::Error> {
    let Some((name, rest)) = components.split_first() else {
        return Err(compress::Error::InvalidPath {
            path: stored.path,
            reason: "paths must contain at least one component",
        });
    };

    if rest.is_empty() {
        return match directory.children.entry(BString::from(*name)) {
            btree_map::Entry::Vacant(entry) => {
                entry.insert(Node::Entries(vec![stored]));
                Ok(())
            }
            btree_map::Entry::Occupied(mut entry) => match entry.get_mut() {
                Node::Entries(entries) => {
                    entries.push(stored);
                    Ok(())
                }
                Node::Directory(_) => Err(compress::Error::InvalidPath {
                    path: stored.path,
                    reason: "a path is both a file and a directory",
                }),
            },
        };
    }

    match directory.children.entry(BString::from(*name)) {
        btree_map::Entry::Vacant(entry) => {
            let mut child = Directory::default();
            insert_entry(&mut child, rest, stored)?;
            entry.insert(Node::Directory(child));
            Ok(())
        }
        btree_map::Entry::Occupied(mut entry) => match entry.get_mut() {
            Node::Directory(child) => insert_entry(child, rest, stored),
            Node::Entries(_) => Err(compress::Error::InvalidPath {
                path: stored.path,
                reason: "a path traverses through an indexed file",
            }),
        },
    }
}

fn emit_directory(
    directory: &Directory,
    prefix: &BStr,
    write_tree: &mut impl FnMut(&gix_object::Tree) -> Result<gix_hash::ObjectId, gix_object::write::Error>,
    entries: &mut Vec<Entry>,
    path_backing: &mut PathStorage,
    sparse_directories: &mut usize,
) -> Result<(), compress::Error> {
    for (name, node) in &directory.children {
        match node {
            Node::Entries(stored_entries) => {
                for stored in stored_entries {
                    push_entry(entries, path_backing, stored.entry.clone(), stored.path.as_bstr());
                }
            }
            Node::Directory(child) if child.can_collapse() => {
                let id = write_directory(child, write_tree)?;
                let mut path = joined_path(prefix, name.as_bstr());
                path.push(b'/');
                push_entry(
                    entries,
                    path_backing,
                    Entry {
                        stat: Stat::default(),
                        id,
                        flags: Flags::EXTENDED | Flags::SKIP_WORKTREE,
                        mode: EntryMode::DIR,
                        path: 0..0,
                    },
                    path.as_bstr(),
                );
                *sparse_directories += 1;
            }
            Node::Directory(child) => {
                let path = joined_path(prefix, name.as_bstr());
                emit_directory(
                    child,
                    path.as_bstr(),
                    write_tree,
                    entries,
                    path_backing,
                    sparse_directories,
                )?;
            }
        }
    }
    Ok(())
}

fn write_directory(
    directory: &Directory,
    write_tree: &mut impl FnMut(&gix_object::Tree) -> Result<gix_hash::ObjectId, gix_object::write::Error>,
) -> Result<gix_hash::ObjectId, compress::Error> {
    let mut entries = Vec::with_capacity(directory.children.len());
    for (name, node) in &directory.children {
        match node {
            Node::Directory(child) => entries.push(gix_object::tree::Entry {
                mode: gix_object::tree::EntryKind::Tree.into(),
                filename: name.clone(),
                oid: write_directory(child, write_tree)?,
            }),
            Node::Entries(stored_entries) => {
                let Some(stored) = stored_entries.first().filter(|_| stored_entries.len() == 1) else {
                    return Err(compress::Error::InvalidPath {
                        path: stored_entries
                            .first()
                            .map_or_else(BString::default, |entry| entry.path.clone()),
                        reason: "a collapsible tree must contain one stage-zero entry per path",
                    });
                };
                let mode = gix_object::tree::EntryMode::try_from(stored.entry.mode.bits()).map_err(|mode| {
                    compress::Error::InvalidMode {
                        path: stored.path.clone(),
                        mode,
                    }
                })?;
                entries.push(gix_object::tree::Entry {
                    mode,
                    filename: name.clone(),
                    oid: stored.entry.id,
                });
            }
        }
    }
    entries.sort();
    write_tree(&gix_object::Tree { entries }).map_err(Into::into)
}

fn joined_path(prefix: &BStr, name: &BStr) -> BString {
    let mut path = prefix.to_owned();
    if !path.is_empty() {
        path.push(b'/');
    }
    path.extend_from_slice(name);
    path
}

fn push_entry(entries: &mut Vec<Entry>, path_backing: &mut PathStorage, mut entry: Entry, path: &BStr) {
    let start = path_backing.len();
    path_backing.extend_from_slice(path);
    entry.path = start..path_backing.len();
    entries.push(entry);
}

fn clear_fsmonitor_valid(entries: &mut [Entry]) {
    for entry in entries {
        entry.flags.remove(Flags::FSMONITOR_VALID);
    }
}
