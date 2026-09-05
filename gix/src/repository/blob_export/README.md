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
- [x] `Platform::read_object` checks hash kind, header size/kind, decoded size/kind
      and recomputed content OID. Verified kind/size/raw data match git cat-file.
- [x] `Platform::write_blob` verifies before output and writes in 64 KiB chunks.
      An optional expected size binds external lock metadata.
- [x] `Platform::export_blob` syncs a temporary sibling and uses no-clobber file
      publication. Existing files/directories/links are never replaced.
- [x] Tests cover SHA-1/SHA-256, raw binary paths/data, empty blobs/trees, modes,
      packed delta objects, corruption, missing/wrong objects, limits,
      unsupported formats, cancellation and output failure and process-directory changes.

## Deliberate boundary
This API is not a Git command-line parser. Paths are always root-relative and
NUL-delimited, IDs are full length. Pathspecs, abbreviations, custom formats,
quoted newline output, textconv, filters, mailmap and batch parsing are not
implemented. Custom/quoted formats have explicit unsupported variants and are
rejected before output. Native listing retains symlink/gitlink metadata and does
not follow them. Blob headers are inspected during listing; content verification
happens on object reads or blob export.

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
