# Native verified blob export

Entry point: Repository::blob_export(Limits) -> Platform.
The reader reopens the repository with explicit object allocation limits and
replacement refs disabled. Reopening anchors the Git directory to the original
captured working directory and retains the caller's repository trust level.

## Completed implementation checklist
- [x] `Tree::try_find_entry` consumes the full encoding, including trailing errors.
      Existing best-effort lookup APIs retain their signatures.
- [x] `Platform::list_tree` validates every visited tree's bytes and OID, complete
      decoding, legal mode encodings, ordering, duplicate names and components.
- [x] `Platform::write_tree` preserves byte paths, full OIDs and depth-first order;
      tested against git ls-tree -z --full-tree with -r, -t, -d (including gitlinks),
      -l, --name-only and --object-only.
- [x] Existing `Format::Custom` supports all documented fields, padded sizes,
      literal percent and hexadecimal byte escapes with Git's exact-format dispatch.
      `Format::Quoted` emits default fields with C-style paths and newline records.
- [x] Malformed custom formats fail before output, including empty trees.
      Long literal output checks cancellation between 64 KiB chunks.
- [x] `Platform::read_object` checks hash kind, header size/kind, decoded size/kind
      and recomputed content OID. Verified kind/size/raw data match git cat-file.
- [x] `Platform::write_blob` verifies before output and writes in 64 KiB chunks.
      An optional expected size binds external lock metadata.
- [x] `Platform::export_blob` syncs a temporary sibling and uses no-clobber file
      publication. Existing files/directories/links are never replaced.
- [x] Tests cover SHA-1/SHA-256, raw binary paths/data, empty blobs/trees, modes,
      packed delta objects, corruption, missing/wrong objects, limits,
      invalid formats, cancellation, output failure and process-directory changes.

## Deliberate boundary
Paths are repository-root-relative and IDs are full length. Built-in formats use
raw paths and NUL records. `Format::Quoted` uses the default columns with newline
records and fixed `core.quotePath=true` behavior, independent of repository config.

`Format::Custom` implements `objectmode`, `objecttype`, `objectname`,
`objectsize`, `objectsize:padded` and `path` fields, plus `%%` and `%xNN`.
Each record ends in NUL; a format can also embed NUL via `%x00`. Generic
`%(path)` fields use C-style quoting even with NUL termination, matching Git.
The four exact formats corresponding to default, long, name-only and object-only
use their built-in behavior, including raw paths. Quoting always follows
`core.quotePath=true`. Format literals and expanded fields are written in
bounded chunks; expanded records and a token list are not buffered.

This remains a typed native API. Command-line parsing, tree-ish/pathspec
selection, cwd-relative names, abbreviations, arbitrary combinations of quoting
and columns, `core.quotePath=false`, textconv, filters, mailmap and cat-file batch
parsing are not implemented. Native listing retains symlink/gitlink metadata
without following them. Blob headers are inspected during listing; content
verification happens on object reads or blob export.

The companion RmcpLib exporter owns the versioned manifest/lock schema and
directory publication. Its TODO and contract live in Docs/BlobExport.md.

## Resource and publication guarantees
Object and pack-delta allocations have a caller-specified bound. Tree entry
count, path length and depth are bounded. This buffers one bounded object and
tree metadata; it is not constant-memory streaming for arbitrarily large pack
objects or a total RSS budget. Cancellation is checked around object reads and
between output chunks; it cannot interrupt a synchronous decoder or blocked OS
write. Caller-owned streams can contain partial output after a write error;
export_blob publishes only a completed file.

Destination parents must be existing, stable directories controlled by the
caller. Atomic visibility does not imply parent-directory power-loss durability.
Ordinary failures remove this operation's temporary file; process termination
may leave an unpublished temporary file. No automatic scavenging is performed.

References: [Git ls-tree](https://git-scm.com/docs/git-ls-tree) and
[Git cat-file](https://git-scm.com/docs/git-cat-file).
