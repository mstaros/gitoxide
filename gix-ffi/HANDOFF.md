# gix-ffi / GixSharp — Handoff

Status updated on 2026-09-05. The earlier foundation was verified at
`b3f28217b0b4310aa6e26782467875bbbfc31c84`; the completed wrapper expansion is
integrated as `22c1da14`, followed by ignore/local-exclude at `c051d784`.
The shallow boundary/location slice is recorded in transaction
`617a0c2ea46d9ae649f97241` and integrated as `24b9b9e9`. Notes compatibility
is integrated as `ee8ffcb7`. Configuration compatibility is recorded in transaction
`d461153e562eea7ec0b754bb`. The annotated/lightweight tag slice is recorded in
transaction `b1cbb5ba3e7ab5b6b96985a2`, based on managed-boundary commit `e37f298c`.
The completion goal is full public gix coverage, confirmed by the user.
Use the checkboxes here for boundary work and the implementation checklist in
`../Issues.md` for method/domain coverage. Historical prototype counts and examples
below are design context, not current progress totals.

## What this is

A .NET binding over gitoxide (`gix`) targeting **full gix coverage**.

**Status: implementation in progress.** Both layers work end to end. Nine
initial compatibility slices are complete, with additional gix methods still to
implement. Full gix coverage includes methods being added to the fork in parallel.
Independent methods continue while their applicable boundary contracts are completed.

Two distinct layers. Keep the names apart; calling both "the facade" caused
real confusion:

```
gix (path dep, this repo)
  -> gix-ffi              RUST FACADE, #[ffi] annotated
  -> interoptopus         generates C# from the facade inventory
  -> Interop.cs           GENERATED, never hand-edited
  -> GixSharp/Managed/    MANAGED LAYER, hand-written
  -> GixSharp.Tests       TUnit against real repositories and the native DLL
```

No cbindgen, no ClangSharp, no hand-written P/Invoke.

## Current state

- [x] Repository open/discover/init, locations and lifecycle compatibility slice.
- [x] Objects/commit graph compatibility slice, including commit reads and writes.
- [x] Status compatibility slice.
- [x] Index/conflicts compatibility slice.
- [x] References/branches compatibility slice.
- [x] Diff/patch/tree-change compatibility slice, with exact-byte snapshots.
- [x] Ignore/local-exclude compatibility slice, including exact-byte/idempotent
  writes, Git precedence and linked-worktree common-directory routing.
- [x] Notes compatibility slice: WriteNote, ReadNote, TryReadNote, EnumerateNotes
  and RemoveNote with owned note/signature bytes, default/custom refs, guarded
  direct/symbolic updates and Git comparisons. Remaining platform controls and
  general note cursors stay unchecked in the full-coverage checklist.
- [x] Configuration compatibility slice: GetConfigString, TryGetConfigString,
  SetConfigString and DeleteConfigValue. Fresh raw Snapshot::reload preserves
  includes, permissions and precedence; linked git-dir conditions use the private
  directory. Raw reads, HEAD lookups, locks and writes retain the opening cwd.
  Atomic writes/deletes use common local config and reject repeated or included
  keys. Raw values remain readable and repairable after an invalid typed setting;
  successful persistence remains successful if typed reopening fails, with other
  operations retaining prior cached settings until a valid refresh. Valid identity
  writes refresh later commits on the same handle.
- [x] Configured GetAuthor/GetCommitter and committer fallback methods match gix
  absence/error and per-component precedence. Fallbacks and lazy identity state
  remain on the same handle through into_sync; configuration files are unchanged.
- [x] GixSignature owns raw name/email bytes and keeps constructor, deconstruction
  and with/init behavior. Commit/note readers and writers preserve those bytes.
  ResolveMailmap/TryResolveMailmap use gix's lenient load/resolve behavior, retain
  partial mappings and timestamps, and return owned results.
- [x] Identity/mailmap transaction `12351a13bee70d62d69cda3d`: 73/73 native tests
  (`f86b158d719d1a2f2f7f08cbb4e971a1`) and 87/87 managed tests under
  PatchedCoreRun/corerun.exe (`op_966b253955f7472e`); managed build
  `op_faca91da79504d7b` passed. These are preflight totals, not a coverage percentage.
- [ ] Strict/reusable mailmap loading, parsing, merging and entry access.
- [ ] Full Git signature time domain beyond DateTimeOffset's representable
  timestamps and offsets; the existing managed When contract is preserved.
- [x] Annotated/lightweight tags: CreateAnnotatedTag, CreateTagReference,
  ReadTag and PeelTags. GixTag owns exact name/message/tagger bytes and extracted
  signature armor; optional taggers use real Option carriers. Tests cover Git
  object equality, all target kinds, nested peeling, concurrent create, force,
  foreign locks, malformed/dangling tags and Windows path validation before
  object writes. Signature extraction does not verify authenticity.
- [x] Additional gix methods: HasObject, WriteBlob and IsShallow.
- [x] GetShallowCommits, ShallowFilePath and raw-byte ShallowFile. Owned snapshots
  read through `gix-shallow` directly, avoiding mtime-only cache staleness; empty,
  corrupt/unreadable, clone/deepen/unshallow and linked-worktree cases are covered.
- [x] Exhaustive GixError case matching; CS8509 is an error and the missing-case
  compiler probe and all ten mappings are verified.
- [x] P0a byte-stream boundary prototype and measurements.
- [x] Interoptopus C# 15 unions consumed in generated Interop.cs.
- [x] Fork dependencies select `interoptopus_mps` and `interoptopus_csharp_mps`
  0.17.1 from registry `local`.
- [x] Fresh-index status/staging correction integrated at `b3f28217`;
  retained foundation validation: 31/31 native tests and 54/54 managed tests.
- [x] Expansion validation: 75/75 native tests, operation
  `cbeca59bb68a65f7ff9bddd905aa2867`; 92/92 managed tests via
  PatchedCoreRun/corerun.exe, operation `op_349b35f8b2b941a4`; managed build
  `op_2faa924be8b04d77` succeeded. Includes all 9 native and 6 managed tag tests
  and the whole-public-surface boundary check against the regenerated bindings.
  The preceding boundary slice passed 66 native and 86 managed tests;
  configuration passed 66/82, notes 57/76, shallow 50/71, ignore/local-exclude
  47/68 and diff/scalar 41/63.
- [ ] Complete every remaining public gix capability in the `../Issues.md`
  implementation checklist, including additions made in parallel.
- [ ] Complete the remaining boundary, profile and delivery checks below.

A checked compatibility slice records its implemented scope; it does not imply
that the corresponding gix domain is fully wrapped. LibGit2.Native parity and
CSharpMpc migration are intermediate validation milestones. The final target is
full gix coverage.

For exact methods, read `src/` and `bindings/GixSharp/Managed/`. Keep generated
resources out of managed signatures, preserve Git bytes and managed result
lifetimes, and pair each native addition with its wrapper and validation.

## Layout

```
gix-ffi/
  Cargo.toml            own [workspace] - NOT a gitoxide workspace member
  .cargo/config.toml    legacy machine-local overrides; no fork path patch required
  .gitattributes        forces LF; generator emits LF, autocrlf rewrites it
  .gitignore            *.sln ignored, GixSharp.slnx deliberately NOT
  src/                  the Rust facade, split across modules
  tests/generate_bindings.rs   generates Interop.cs (a test, not build.rs)
  bindings/
    GixSharp.slnx       solution, tests in a /tests/ folder
    Interop.cs          GENERATED, committed, namespace GixSharp.Native
    GixSharp/
      GixSharp.csproj   packable class library, builds the cdylib
      Managed/          HAND-WRITTEN managed layer, partial GixRepository
    GixSharp.Tests/     TUnit - interop-level and managed-level tests
```

File names are deliberately omitted below the directory level: both `src/`
and `Managed/` gain modules as coverage grows, and enumerating them here
only creates drift. Read the directories.

`gix-ffi` is a **nested independent cargo workspace** on purpose. As a
gitoxide workspace member, `cargo test --workspace` would unify `gix`
feature selection with `gitoxide-core`, which carries a `compile_error!`
for `blocking-client` + `async-client` together.


## Design rules - decided, validated by execution

1. **No borrowed data crosses the boundary.** `Commit<'repo>`, `Tree<'repo>`
   etc. are lifetime-bound and cannot be represented in the ABI. For streams,
   the producer may borrow internally but every payload crossing the ABI or
   entering a producer/consumer buffer must own or detach all of its data.
   This is a required invariant and needs automated enforcement as coverage
   grows; it is not merely a convention. The exact test mechanism is still
   to be chosen.
2. **`ThreadSafeRepository` is the stored form.** `gix::Repository` holds
   `Option<RefCell<...>>` so is never `Sync`. `to_thread_local()` per call.
   The `parallel` feature is **non-negotiable** - without it `Repository`
   is not even `Send`.
3. **Git paths, ref names and messages cross as BYTES** (`ffi::Vec<u8>` /
   `ffi::Slice<u8>`), and stay `byte[]` in the managed layer. Git stores
   them as `BString`; `ffi::String` would UTF-8-validate and reject
   repositories git itself handles. Filesystem paths are a separate open
   design question; do not silently apply the Git-byte rule to OS paths.
4. **Object ids cross as lowercase HEX strings**, and are `string` in the
   managed layer. ASCII by construction, debuggable.
5. **Small read-once things are records; services only for types with many
   operations, state, or lazy sub-access.**
6. **Streaming is not eager-by-default.** The current eager `rev_walk`
   implementation is POC/legacy and must not be copied as guidance for new
   iterator APIs. Eager materialisation remains acceptable only when a result
   is deliberately known to be small and bounded. General iteration uses the
   boundary architecture below.
7. **One public `GixException`; the current error taxonomy is POC-only.**
   Do not replace it with exception subclasses. Settle the stable error
   envelope before cursor/error semantics or breadth: small semantic `Kind`,
   extensible machine-readable `Code`, orthogonal retryability, diagnostic
   message, and structured detail only where callers need recovery operands.
8. **Nothing generated escapes the hand-written managed API.**
   `ManagedApiBoundaryTests` inspects the entire public/protected managed
   surface, including externally accessible nested declarations, methods,
   properties/indexers, constructors/records, fields, events, bases, interfaces,
   delegates and generic constraints. Signature traversal follows nested generic,
   array, pointer and by-ref types. Generated types and nested cases are identified
   structurally through the reserved `GixSharp.Native` namespace; no fixed list
   of resources or case names is maintained. Regression probes demonstrate that
   leaks are rejected. `ReferenceUpdateOutcome` is now a hand-written managed
   enum with its existing public name, byte representation and values preserved;
   native outcomes are explicitly translated and unknown values are rejected.
   Generated C# 15 union cases remain implementation details; deliberate public
   sum types belong to the hand-written managed contract.
9. **One managed/FFI surface, multiple native engines.** Build-profile
   differences must not add/remove FFI functions, records or enum variants.
   Every native artifact shipped in one package version must have the same
   Interoptopus inventory and API-guard hash; capability differences are
   runtime data, not a different managed API.
10. **Managed compatibility and native ABI compatibility are different
    contracts.** Public managed API follows SemVer. Generated/native ABI is an
    exact-version implementation detail shipped in lockstep and guarded by
    `guard!(ffi_inventory)`; cross-version DLL compatibility is not promised.
11. **Closed semantic alternatives are Rust enums; Interoptopus owns their
    projection.** Do not permanently flatten a natural Rust data enum into a
    tag-plus-nullable-field record or facade-only wrapper cases to compensate
    for the current generator's single-payload limitation. Interoptopus is the
    generic owner of discriminant modeling, ABI lowering and C# projection. Its
    current union plan fixes discriminant correctness and projects the existing
    plain `DataEnum` model; full named/multi-field Rust variant support is a
    separate generic-tool evolution. GixSharp will wait rather than freeze a
    temporary competing sum-type architecture into its ABI. Open domains such
    as `GixException.Code` and capability identifiers remain deliberately
    non-exhaustive and must not become closed unions.

The **current POC build** still uses gix default features plus explicit
`parallel`, `attributes`, `revision`, `blame`, `merge`, `status`, `dirwalk`,
`index`, `worktree-mutation` and `blocking-network-client`. That is current
state, not the target build architecture. The target profiles below make gix
features explicit (`default-features = false`), enable both `sha1` and `sha256`
in every released engine, and select exactly one native networking engine per
artifact.


## Traps - each of these cost real time

**The fork is selected by distinct registry package names.** Since `c68796bb`,
`gix-ffi/Cargo.toml` requests `interoptopus_mps` and
`interoptopus_csharp_mps` 0.17.1 from registry `local`. Configure that existing
registry under CARGO_HOME on the build machine. A transaction worktree does not
need a copied crates.io path-patch config; the old silent-substitution issues
`f7bf7635` and `0be88a6a` are resolved by the manifest change.

**Building does not regenerate Interop.cs.** Run the `generate_bindings` test,
then build and execute the managed consumer. A generated diff can be an intentional
inventory/generator change; review it rather than interpreting every diff as an
upstream downgrade.

**Each ffi::Vec<T> still needs builtins_vec!(T).** The C# backend now explicitly
rejects missing Vec and Utf8String helpers; Interoptopus issue `2a6da76a` is closed.
This is backend generation validation, not a new rule in core
`RustInventory::validate()`.

**`Utf8String` arguments are MOVED, not borrowed.** The marshaller calls
`IntoUnmanaged()`, transferring the pointer and nulling the managed side, so
a generated `Utf8String` is single-use. The managed layer handles this by
building a fresh one per call from a managed `string` - do not reintroduce
generated types into public signatures.

**Generated ownership is per type, not universal.** Owned generated resources such
as `Utf8String`, `VecByte`, `VecUtf8String`, `GixError` and the current
`Result*` wrappers are `IDisposable`; scalar generated types such as
`FfiObjectType` are not. Under the implemented union projection, nested `*Case`
record structs are case views/values, not separately-owned copies of an enum's
payload. Dispose the owning generated enum/result according to its generated
contract and do not infer that a matched case should be disposed independently.

## Corrections to earlier assumptions

Each was believed and wrong. Recorded so they are not re-derived:

- The **current** Interoptopus enum model is limited to unit variants and
  single-payload tuple variants; named/struct variants and multi-field tuples
  are not represented. That is a real generic-tool limitation, not a GixSharp
  architecture to preserve. `docs/csharp-unions.md` in the Interoptopus checkout
  plans to correct discriminant modeling and project the existing plain
  `DataEnum` model as opt-in C# 15 custom unions without changing the native ABI.
  Full named/multi-field Rust variant modeling is explicitly separate from that
  first union implementation. The plan is not implemented yet and still marks
  Step 0 as blocking.
- The current C# generator's `body_exception_for_variant` does not produce a
  distinct exception *class* per variant, but the error data is recoverable:
  `.AsOk()` throws `EnumException<GixError>` whose `.Value` carries the typed
  enum. Match on `IsNotARepository` / `IsIo` / ... and read payloads through
  `AsX()`. **Do not dispose the payload separately** - `AsX()` returns the
  enum's own field and the enum's `Dispose` covers it; disposing it again is a
  double-dispose. This describes the current generated bindings only, not the
  target C# 15 union-facing consumption style.
- Interoptopus currently has a real discriminant-model defect for payload
  variants: its C# model uses the positional index for tuple-variant tags while
  unit variants retain their declared tag. The planned Step 0 fixes this in the
  generic model before union projection. Do not build GixSharp logic that
  assumes today's tuple-variant tag behavior is authoritative.
- Binding generation is a **test**, not `build.rs` - a build script cannot
  call into the crate it is building.
- `interoptopus_csharp::Interop` does not exist; it is `RustLibrary`. That
  repo's CLAUDE.md is stale on this.
- gix handles linked worktrees correctly - `git_dir()` resolves to
  `<main>/.git/worktrees/<name>`, which does NOT end in `.git`.
- `gix-hash` does **not** require exactly one of `sha1` / `sha256`. It only
  rejects builds with neither enabled; gitoxide's own development dependencies
  enable both. GixSharp should therefore compile both hash algorithms into
  every released native engine instead of creating hash-specific profiles.
- The blocking/async networking split **is** real, but the explicit
  `compile_error!` is in `gitoxide-core`, not `gix-ffi` itself. `gix` documents
  its async and blocking network-client feature families as mutually exclusive,
  so released GixSharp artifacts must still select exactly one engine rather
  than depending on accidental feature unification.


## Unverified claims

Stated during the work but never confirmed. Do not rely on them:

- The 138 LICENSE files showing as modified inside transaction worktrees are
  attributed to `core.autocrlf`. Symlinks and git-lfs were each asserted
  first and were wrong, so treat this third guess with suspicion.
- "Divergence from upstream interoptopus is only 2 commits" was measured
  before `b42399a4` and has not been re-checked.

## Known trade-off, not a bug

`GixRepository.Invoke` holds a `lock (_sync)` for the whole native call, so
all access to one repository is serialised. The current eager
`RevWalk(tip, 0)` can therefore hold it for a long history; that implementation
is legacy under rule 6 and should disappear when rev-walk moves to a cursor.
The Rust side is `Sync` by construction (rule 2), so the lock is more
conservative than required and is really protecting the dispose race. If
contention remains after streaming is in place, an interlocked call-counter
(the pattern uniffi's generated wrapper uses) keeps dispose safety without
serialising unrelated calls.

## P0 - boundary contracts for affected API families

Full coverage has three high-coupling boundary decisions. **P0a byte streaming
is now experimentally resolved**; its selected pull/lifetime contract constrains
the shared stream lifecycle. P0b error semantics and P0c structured cursors remain
to be frozen before broad surface expansion.

### P0a - byte streams first - experimentally resolved

P0a is resolved at the **boundary-contract level** by commit
`61739044966d033186908aa4ef25157ff981d7a6`. The implementation, methodology and
raw evidence are committed in `P0A_BYTE_STREAMING.md` and
`P0A_BYTE_STREAMING_RESULTS.md`. Keep the implementation experimental/gix-specific
for now, but use this contract for future bulk-byte APIs unless a qualitatively
different source disproves it.

The selected contract is:

- **stateful native reader + caller-owned reusable managed buffer**;
- `ffi::SliceMut<u8>` for the synchronous pull, exposed managed-side with
  `Stream.Read(Span<byte>)` semantics; the managed pointer is valid only for that
  call and native code never retains it;
- one native-to-managed boundary copy per non-empty pull and no whole-payload
  managed copy;
- a non-empty read returning zero is EOF; a zero-length read returning zero is
  **not** an EOF probe; repeated reads after real EOF are stable;
- stateful/single-consumer ownership with managed serialization of read/dispose;
  disposal is deterministic and idempotent, and the reader owns its source
  independently of the repository after construction;
- cancellation is cooperative **between** pulls: pre-cancel makes zero FFI calls,
  while an already-running synchronous native read is not interruptible;
- length is optional metadata when cheaply known, not part of the pull protocol.

The reproducible Release benchmark used a deterministic ~64 MiB payload and one
reused 64 KiB managed buffer:

| Source | Median open | Median pull | Pull throughput | FFI calls to EOF | Pull allocations | Exact reader-retained native payload |
|---|---:|---:|---:|---:|---:|---:|
| Materialized object | 14.404 ms | 3.302 ms | 19,382.9 MiB/s | 1,026 | 41,040 B | 64.00 MiB |
| Worktree pipe `Read` | 0.673 ms | 104.973 ms | 609.7 MiB/s | 1,026 | 41,040 B | 0 B |

Interpret those numbers precisely. The materialized-object pull rate is a hot
in-memory copy after gix has already opened/decompressed the object, so open time
matters for end-to-end access. The worktree reader retains no whole output, but
process-private memory still rose by about 64 MiB and source inspection shows
`gix_worktree_stream` currently materializes each large entry into a `Vec<u8>`
upstream before pipe output. Thus the ABI preserves streaming/backpressure and
removes the second whole-payload managed allocation, but it is **not an
end-to-end zero-copy or constant-memory claim about current gix internals**.

The 41,040 B pull allocation is 1,026 generated `ResultUlongGixError` objects at
40 B per FFI call. Do not distort the stream contract to remove that allocation;
revisit it with the Interoptopus error/`Result` union work.

Alternatives were evaluated and are not the default:

- scoped callback/borrowed view can avoid a copy for specialized synchronous
  consumers, but its lifetime, reentrancy and managed-exception constraints make
  it a poor general `Stream` surface;
- chunked `Vec<u8>` cursors add per-chunk ownership/allocation/copying and are a
  fallback for coarse paging, not sustained byte streaming.

Do **not** generalize this into an Interoptopus-wide stream primitive yet. The
gix-specific prototype is sufficient to freeze P0a and to inform P0c lifecycle
semantics.

### P0b - error ABI before cursors

The coarse seven-value `GixError` / `GixErrorKind` model is POC-only and is now
the most urgent compatibility decision after byte streaming. Deferring it is
uniquely expensive because every existing and future operation shares the same
public `GixException` contract. Stateful resources and callbacks can mostly be
added when their APIs arrive; changing error semantics later breaks filters and
switches across the whole managed surface.

The current surface is already sufficient to design against. In particular,
write paths expose distinctions that callers must act on rather than recover
from prose. Current `Other` mappings include wrong object type, invalid explicit
signature, conflicted index, empty-commit refusal, reference lock contention,
reference compare-and-swap races and object/write failures. Two corrections to
keep precise:

- a missing parent object is already classified as `NotFound`;
- `create_commit_from_index` currently rejects an index containing conflicts,
  not a merely dirty worktree/index.

`gix-ref` already preserves the important ref-update distinctions internally:
`LockAcquire`, `MustExist`, `MustNotExist` and `ReferenceOutOfDate`, including
reference name and expected/actual targets. The facade must stop flattening
these into `Other`.

The accepted error direction is:

- **Keep one public `GixException` class.** Do not create an exception subclass
  hierarchy. Existing `catch (GixException)` code should remain valid.
- **Keep one semantic FFI error envelope.** `ffi::Result<T, E>` still uses one
  composite `E` carrying stable GixSharp semantics: `Kind`, `Code`, retryability,
  diagnostic message and selective structured detail. The envelope is retained
  because these fields cut across concrete gix error variants and protect the
  managed contract from upstream taxonomy churn; it is **not** a workaround for
  Interoptopus's current payload-enum limitation.
- **Structured recovery detail is a closed semantic sum type.** Once the planned
  Interoptopus C# 15 union projection is available, model actionable detail as
  an internal Rust data enum and consume the generated union through pattern
  matching. Examples include reference-conflict detail and object-type-mismatch
  detail. Use a payload record when that record is itself the natural domain
  object, not merely to compensate for a generator limitation. Do not mirror
  every upstream gix error enum one-for-one.
- **`Kind` is a small semantic/action category, not a mirror of concrete gix
  variants.** The stable semantic buckets need to cover validation/input,
  not-found, failed precondition/state, concurrency conflict, busy/locked,
  configuration, I/O, corruption, unsupported, authentication/transport,
  cancellation and internal/unknown. Exact public enum spelling should be
  frozen once against representative mappings, but the categories themselves
  are the intended level of abstraction.
- **`Code` carries the precise machine-readable reason** as an extensible ASCII
  identifier rather than another closed enum/union. Adding a new specific code
  must not become an exhaustive-match break for consumers. Examples from the
  current surface include invalid-object-id, object-type-mismatch,
  index-conflicted, empty-commit-disallowed, reference-out-of-date and
  lock-unavailable.
- **Retryability is orthogonal to `Kind`.** Expose it as a flag/property rather
  than a category. This matches the direction in `gix-error`, which separately
  exposes `can_retry()` alongside validation/not-found/corruption
  classification.
- **The message remains diagnostic only.** Preserve the full display/source
  chain for humans and logs, but no managed control flow may parse message
  text.
- **Use structured detail only where recovery needs operands.** Ref-update
  conflicts need the reference name and expected/actual targets; object-kind
  mismatch needs object id and expected/actual kind. Do not create a detail case
  for every error just because generated unions make it convenient. Generated
  union/case types remain internal under rule 8; the public managed detail shape
  must be deliberately designed and compatibility-reviewed.
- **Centralise classification.** Important gix error enums get explicit,
  exhaustive matches so newly added upstream variants force a decision at
  compile time. A generic lower layer may recognise `std::io::Error` and
  `gix-error` semantic markers such as validation, not-found, corruption and
  retryability. Never classify by matching `Display` strings.

This is the one intentional taxonomy break to make before breadth. Freeze the
semantic envelope now, but do not lock in temporary generated-detail ergonomics
from today's Interoptopus backend. The final structured-detail ABI should be
implemented against the planned discriminant fix and C# 15 union projection so
it does not immediately require a second representation migration. Preserve
`GixException.Operation` and diagnostic message while replacing the current
coarse `Kind` semantics and adding `Code`, retryability and optional typed
detail. `guard!(ffi_inventory)` catches managed/native ABI mismatch but does not
substitute for this compatibility policy.

### P0c - structured cursors third

Structured record streams include rev-walk, full-repo dirwalk, status,
references/reflogs, tree changes, object enumeration, parser tokens, pack
indexes and similar APIs. After P0a establishes the shared stream lifecycle and
P0b establishes the error envelope used for yielded and terminal errors, freeze
the record-stream contract around:

- managed pull enumeration (`IEnumerable<T>` / `IEnumerator<T>` semantics)
- bounded batches across the FFI boundary to amortise native-call overhead
- Rust-owned producer state; C# owns a disposable cursor handle
- direct pull from an owned/detached Rust iterator where possible
- producer + bounded channel adapter where the gix iterator borrows a repo,
  where a callback traversal must be adapted, or where producer work should
  run independently
- payloads are owned/detached before they enter the channel or cross the ABI
- early dispose means cancellation/consumer disconnect plus native cleanup;
  natural exhaustion is distinct from disposal
- APIs such as dirwalk/status may expose a final `Outcome` only after natural
  exhaustion; early disposal must not manufacture one
- yielded item errors and terminal producer failure are separate concepts;
  some gix iterators can yield recoverable/intermediate `Err` items
- a cursor is single-consumer unless an API explicitly says otherwise

This one managed consumer contract does **not** require one Rust
implementation. `rev_walk` and reference iterators borrow repositories;
`dirwalk` and status already contain producer/interrupt patterns; tree diff is
callback-driven but `Change::detach()` produces an owned change. They can all
adapt to the same managed consumption model.

Cursor introduction does not need to break the existing managed
`RevWalk(string, int) -> IReadOnlyList<string>` convenience API. Keep that
method as a materialising wrapper over the new streaming surface while the
native eager implementation is retired. New lazy enumeration is therefore
additive even though eager native iteration stops being the architectural
precedent.

The detached-payload rule above must gain an automated test/check analogous to
the existing managed reflection invariant. The handoff intentionally does not
prescribe the mechanism yet; the requirement is that violations become a
build/test failure rather than a review convention.

Do **not** add a generic stream/cursor abstraction to Interoptopus yet. Prove
the byte reader first, settle the error envelope, then prove the cursor contract
in `gix-ffi` with at least one repo-borrowing stream (`rev_walk`) and one
naturally streaming workload (`dirwalk` or `status`). Extract a reusable
Interoptopus pattern only from requirements demonstrated by those prototypes.

## Resolved cross-cutting contracts - native profiles and ABI evolution

### Native build/profile matrix

"Full gix coverage" does **not** mean one `cargo --all-features` cdylib. It
means one managed/FFI contract whose implementation coverage is the union of a
small declared native-profile matrix.

The released matrix has two networking-engine families:

- **blocking engine** - the default engine and the broad transport engine;
- **async engine** - built from gix's async network-client family. Its current
  built-in transport capability is narrower: `gix-transport` documents the
  async client as supporting the TCP `git` transport while custom async
  transports provide other I/O.

Both engines compile **both SHA-1 and SHA-256**. Hash algorithms are not a
profile axis. Common gix components (`parallel`, revision, status, diff/merge,
worktree/index functionality, etc.) are selected identically across engines;
only genuinely incompatible implementation features differ. The target
manifest should use explicit features with `default-features = false` so the
profile definition, not upstream defaults, is the coverage specification.

HTTP implementation/TLS selection is an implementation choice inside the
blocking engine, not a public GixSharp profile unless it produces a genuinely
different semantic capability. Do not multiply managed APIs or package
identities merely to expose curl-vs-reqwest or TLS-backend choices.

**Profile invariance is mandatory:** every engine for one GixSharp package
version exports the exact same Interoptopus inventory. Do not `#[cfg]` an FFI
function/type out of one engine. A logically unavailable operation remains in
the ABI and is reported as unavailable at runtime. CI must generate/validate
both engine builds and require identical generated bindings / API-guard hash.

The managed layer keeps one logical native library name. Engine selection is a
hand-written managed concern performed before the first native call; a .NET
native-library resolver can map the generated logical import to the selected
physical engine artifact. Selection becomes immutable once native resolution
has occurred. The exact public configuration type/name is intentionally not
specified here.

The current package glob `runtimes/**/native/*` can already carry multiple
native files per RID. The build/staging target currently assumes one output and
must be generalized when the second engine lands; the NuGet directory shape
itself does not need redesigning.

Every engine exposes the same small runtime-information/capability bootstrap.
Capabilities describe stable **GixSharp semantics** (for example supported hash
algorithms and transport families), not Cargo feature names. Prefer a
fixed-layout capability bitset with reserved bits so adding a capability does
not itself change the FFI record layout. Capability values may differ by
engine; the ABI may not.

Capability absence is not an operational failure:

- the managed wrapper preflights a known-missing capability and throws
  `NotSupportedException`;
- the native FFI still defensively returns the normal error envelope with
  `Kind=Unsupported` / an appropriate machine-readable code when called
  directly or when capability state races/mismatches;
- stream/cursor creation fails before yielding items when the whole operation
  is unavailable. Do not turn profile absence into a terminal stream error.

The public networking API must likewise remain engine-independent. Do not make
managed method shape (sync vs async, parameters, result types) vary according
to which native engine was loaded. The later callback/cancellation work decides
how a blocking native implementation and an async native implementation satisfy
the same managed operation contract.

### ABI evolution policy

There are two compatibility boundaries and they intentionally have different
rules.

**Generated/native FFI ABI is exact-version lockstep.** It is a private
implementation detail shipped with the managed package. GixSharp does not
promise that managed bindings from package version N can load a native DLL from
N-1 or N+1. `guard!(ffi_inventory)` is the enforcement mechanism: changes to the
Rust inventory such as signatures, FFI record layouts or enum/discriminant shape
regenerate bindings and change the API hash, and mismatched binaries fail
immediately. Important native behavioral/layout changes not represented in the
inventory must also change the guard salt.

This means FFI records and enums do **not** need awkward reserved fields or a
per-struct versioning protocol merely to preserve cross-package binary
compatibility. They may evolve between package versions as long as generated
bindings and all native engines are rebuilt and shipped together. Within one
package version, however, every engine must have an identical FFI inventory and
guard hash.

A **generator projection change is different from a native ABI change**. The
planned Interoptopus C# 15 union mode deliberately changes generated managed
source while keeping the same `Unmanaged` layout and Rust inventory. That may
leave the API-guard hash unchanged. Therefore the pinned Interoptopus revision,
generated-source baseline and exact-toolchain compile gate are part of release
validation; `guard!` alone cannot prove that the desired C# projection was used.

**The hand-written managed API is the public compatibility contract.** Once
stable, it follows SemVer independently of native/generated churn. During the
current pre-1.0 phase breaking changes are permitted by SemVer but must still be
intentional and documented rather than accidental. Apply these rules now:

- adding a new top-level method/type is normally additive;
- never change/remove an existing public method signature to add an option;
  preserve it and add a non-ambiguous overload or a new operation shape;
- return-type changes, renames and removals are breaking;
- avoid growable public interfaces; adding an abstract interface member is
  breaking;
- public positional records have frozen constructor/deconstruction shape once
  exposed. Do not append positional fields later; use a new result type or a
  separate accessor when materially new data is needed;
- closed public enums **and public union/sum types** are closed contracts. Adding
  a member/case can break exhaustive consumer switches and is therefore not
  casually additive. Use extensible strings/codes for open domains
  (`GixException.Code` is the deliberate example), and use `[Flags]` only for
  domains explicitly documented to tolerate new bits;
- `GixException` remains one class. Adding diagnostic/actionable properties is
  additive; changing the meaning of an existing `Kind` is breaking. Keep the
  small `Kind` taxonomy stable and grow exact reasons through `Code`;
- generated Interoptopus resources, including generated union `*Case` types,
  remain internal so native/generator evolution never leaks directly into
  consumer signatures.

Release validation needs three independent gates:

1. **native profile equivalence:** build every native engine for a representative
   RID and prove the same Interoptopus inventory / guard hash, then load each with
   the same generated C# and verify its advertised capabilities;
2. **generated projection/toolchain:** pin the Interoptopus revision and generator
   mode, regenerate deterministically, and compile the generated bindings with
   the exact .NET 11 / C# 15 preview toolchain used by GixSharp;
3. **managed public-API compatibility:** compare the hand-written public surface
   against an approved baseline and separately flag changes to closed enums,
   public union case sets and positional record shapes. The exact checker may be
   ApiCompat, PublicApiAnalyzer or an equivalent deterministic snapshot; the
   required policy above is the contract, not a particular tool.

`guard!` therefore solves exact native pairing, not public SemVer or generator
projection correctness. Conversely, managed SemVer does not require keeping old
native/generated layouts alive. Keeping those boundaries separate is what makes
hundreds of future FFI additions tractable.
### Interoptopus enum / C# 15 union dependency

The union projection in `interoptopus/docs/csharp-unions.md` is complete and is
already consumed by GixSharp. The remaining rich-variant work has its own plan.

- [x] Correct discriminants and collision-safe generated names.
- [x] net11 / LangVersion=preview targeting.
- [x] Public nested case types and single-case constructors, [Union] and IUnion.
- [x] Value, HasValue and TryGetValue with consistent empty-state behavior.
- [x] Reject default struct unions and null class unions when marshalled out;
  validate native tags during conversion.
- [x] Project Option and Result carriers, including their accessor/factory
  integration and executed generated-consumer tests.
- [x] Snapshot/builtin-helper/naming issues `ccb105a2`, `2a6da76a`,
  `09b82d44`, `4e9a17c3` and `7c8cb22e` are closed.
- [x] Interoptopus named/multi-field Step 1: derived Payload/payloads accessors.
- [x] Interoptopus named/multi-field Step 2: output/model consumers use payload
  views; byte-identical reference output and C# suite passed (`dbb6c0d`).
- [x] Interoptopus named/multi-field Steps 3–4: centralized per-field names and
  payload iteration in WireIO/wire (`fcff19e78`); reference output stayed identical,
  with 200 Rust tests and 223 reference C# tests passing the exact commit gate.
- [ ] Verify an explicitly patched runtime launch for the generator reference
  suite; its current harness uses `dotnet run`. GixSharp is verified with corerun.
- [ ] Complete named/multi-field Step 5 in
  `interoptopus/docs/csharp-multi-field-variants.md` before exposing API shapes
  that require those variants.
- [ ] Complete the remaining GixSharp public-surface and semantic-error adoption
  checks below.

The generated custom [Union] representation retains the native tag/payload ABI.
Its explicit managed empty-state policy distinguishes unconstructed structs from
actual Rust variants. Case values remain views of their owning generated resource;
dispose the owner according to its contract. Unit-only enums may remain plain C#
enums. Generated case types are implementation details of the GixSharp boundary.

Use the user's .NET 11 preview 7 / C# 15 patched runtime and
`D:/repos/Unions/src/Unions/UnionSpecification.md` as the current environment.
The native API guard does not validate C# projection semantics; verify both
generation and the real managed consumer.

Ordinary supported methods continue in parallel. A rich-enum requirement gates its
own final ABI shape; do not introduce a temporary competing sum-type design.

### Existing implementation migration checklist and design

The existing POC has been audited against the planned union projection so this
is not only guidance for future APIs. The migration is intentionally split by
the Interoptopus rollout boundary and by whether the affected shape is generated
implementation detail or already-public GixSharp API.

Generated unions are already adopted. The detailed migration rationale below
retains the earlier POC examples; use current source for the inventory and the
checkboxes for unfinished work.

- [x] Regenerate GixSharp against the fork's completed union projection.
- [x] Replace the non-exhaustive GixError property-pattern mapper with generated
  case-type matching; validate missing-case compiler errors and managed ownership.
- [x] Extend the managed public-signature invariant to all public/protected shapes
  and nested generated case types — transaction `475a8ce11774fbdc801a1e09`.
  Combined validation against target `b49657f6` passed 66/66 native tests including
  generation (`ec2873a5b6426ac8ab9f658c137972ea`) and 86/86 patched-required
  managed tests (`op_39fb50b624104519`), including deliberate leak probes and the
  existing byte-stream behavior. Regenerated output differs from that target
  only in its two namespace lines; the native inventory guard is unchanged.
- [ ] Implement the semantic error envelope and actionable recovery detail.
- [ ] Redesign HEAD state after the required named/multi-field support is ready.
- [ ] Complete remaining internal Option/Result cleanup while preserving the
  public managed nullable/exception contracts.

#### Historical union-adoption recipe

The generation dependency in this earlier recipe is complete. Its inventory counts
are historical, and the checkbox list above supersedes its migration sequencing;
in particular, exhaustive GixError case matching is now implemented and validated.


1. **Pin the revision and regenerate. There is no switch to enable.**
   `tests/generate_bindings.rs` builds `RustLibrary`; unions are the default
   `DataEnum` projection in the pinned fork, so adoption is pinning that
   revision and regenerating committed `bindings/Interop.cs`. Earlier revisions
   of this list said to enable a builder option - `RustLibraryBuilder::unions(bool)`
   does not exist and was deliberately abandoned. `GixSharp.csproj` and
   `GixSharp.Tests.csproj` are already `net11.0` with `LangVersion=preview` and
   preview features enabled; no target-framework migration is required.
2. **Expect a large `Interop.cs` diff, and replace the diff-based health check.**
   The documented sanity test - a clean `git diff --stat bindings/Interop.cs`
   after regenerating proves the local interoptopus patch is active - stops
   working the moment projection lands, because the projection legitimately
   rewrites generated managed source. Replace it with a check that survives:
   the API-guard hash plus a compile of the regenerated bindings. See Commands.
3. **Audit the generated `Result*` paths in the same transaction.** `Result`
   carriers are projected in this pass, not a later one. The surface is twelve
   `Result*GixError` wrapper classes and 22 generated `Repo` methods reaching
   native results through `.AsOk()`. Confirm that a native `Err` still arrives at
   the one hand-written `GixException` translation boundary with owned error data
   intact, that payloads are disposed exactly once, that generated `Panic` / `Null`
   states remain distinguishable from ordinary `Err`, and that the new empty
   `default(ResultX)` is never mistaken for `Ok`. There are no hand-written
   managed `.AsOk()` / `.AsErr()` calls today, so this is verification, not rewriting.
4. **Migrate the `FfiObjectType` consumer - as a smoke test, not for its own sake.**
   `GixRepository.Objects.cs::ReadObjectType` tests `IsCommit` / `IsTree` /
   `IsBlob` / `IsTag`. Switching it to case matching buys nothing semantically on a
   payload-free type, and public `GixObjectType` stays a plain `enum` either way.
   Its value is that it exercises the new generated shape against a real consumer.
   Do it for that reason or skip it deliberately; do not present it as an
   ergonomic improvement.
5. **Do not spend a standalone migration on the coarse `GixError`.** The generator
   preserves legacy factories, checks and accessors, so enabling unions does not
   require rewriting the existing `GixRepository.Translate` `IsX` / `AsX` switch.
   P0b replaces this coarse seven-case ABI anyway; fold the managed translation
   rewrite into that work rather than modernising a type that will disappear. If
   an intermediate branch does use the generated `GixError` cases, dispose the
   owning generated error exactly once and do not separately dispose a payload
   obtained through a case view.
6. **Strengthen rule 8's reflection test - before adoption, not after.**
   `ManagedRepositorySignatures_DoNotExposeGeneratedResources` enumerates a fixed
   set of generated top-level types and inspects only public `GixRepository`
   method parameters and returns. Nested generated case types are a new leak
   surface, and the final invariant also covers hand-written public
   records/properties/constructors plus generated types nested inside generic,
   array and by-ref signatures. Replace the fixed-set repository-only check with a
   generic whole-public-surface check, and make generated-type detection recognise
   nested case types structurally rather than by hand-enumerating them.
7. **Keep existing public behavior tests stable.** Union projection by itself must
   not change `GixException`, `GixErrorKind`, `GixObjectType` or any public
   repository signature. Existing tests for native-error translation and object
   metadata are therefore the regression suite for this migration. Rust-side tests
   that pattern-match `GixError` / `FfiObjectType` do not change merely because the
   C# projection changes; they change later when P0b changes the Rust error ABI.


#### Existing public shape to redesign before 1.0 - `GixHead`

`HeadInfo` / `GixHead` is already a discriminated union encoded manually as a
record with sentinels:

- detached HEAD: target present, empty referent, `IsDetached = true`;
- symbolic resolved HEAD: target + referent present;
- symbolic unborn HEAD: empty target, referent present, `IsUnborn = true`.

The current public positional `GixHead(string Target, byte[] Referent,
bool IsDetached, bool IsUnborn)` can represent impossible combinations and
requires consumers to coordinate flags with empty values. This is exactly the
kind of state that should become a deliberately closed public sum type once the
union infrastructure is available. The semantic cases are detached, symbolic
resolved, and symbolic unborn; **do not freeze the final public case/type names
in this handoff**.

This is different from `FfiObjectType`: it is an intentional public pre-1.0
breaking change. Keep generated Interoptopus union/case types internal under
rule 8 and translate them to a hand-written public `GixHead` sum type. The Rust
FFI should ultimately model the same closed states as a data enum rather than
retain booleans and empty-value sentinels. Because the current Interoptopus plan
for plain `DataEnum` still lacks natural named/multi-field Rust variants, do not
rush `HeadInfo` into artificial generator-driven shapes solely to land with the
first union projection; consume the generic richer-enum support when it is
available, or use payload records only where they are semantically meaningful in
the domain.

Existing tests already cover all three useful HEAD states: ordinary symbolic
resolved HEAD, newly initialized/unborn HEAD, and detached HEAD. Convert those
tests from `Target`/`Referent`/`IsDetached`/`IsUnborn` sentinel assertions to
case-based assertions in the same intentional API migration. Existing calls
that immediately consume `Head().Target` will also need a deliberate choice of
which HEAD cases provide a target; do not recreate the old empty-string sentinel
as a convenience property on the new union.

The hand-written public enums were also audited and should **not** be
mass-converted to unions. `GixCommitSort`, `GitStatusOptionFlags` and
`GitFileStatus` are true `[Flags]` sets. `GixObjectType` and `GitStatusShow` are
simple scalar domains, and P0b's `GixErrorKind` remains the small semantic
category even when its POC taxonomy is replaced. Changing any of these existing
public types to a union would be a breaking API change without a case-specific
payload benefit. Reserve public unions for deliberately closed alternatives
whose cases actually carry different shapes/state; C# 15 unions are not a
replacement for ordinary scalar enums or flags.

The remaining hand-written public records were audited as well and stay
records: `GixObjectMetadata`, `GixSignature`, `GixCommit`, `GixCommitInfo`,
`GitIndexEntry`, `GitStatusOptions` and `GitStatusEntry` are ordinary product,
configuration or flag-bearing data. They do not encode mutually exclusive case
shapes and should not be changed merely because union syntax becomes available.

#### Remaining `ffi::Option` cleanup - internal FFI debt, public API stays stable

Several current FFI shapes manually encode optional values even though the
hand-written managed surface already expresses them idiomatically:

- `RepositoryInfo.working_directory` + `has_working_directory` becomes public
  `string? WorkingDirectory`;
- `commit_history` uses an empty `excluded_revision` string for no exclusion,
  while the managed implementation already uses a nullable internal value / an
  overload without exclusion;
- `create_commit_object` uses empty `update_reference` bytes for no ref update,
  plus `author_is_explicit` / `committer_is_explicit` booleans alongside payload
  fields, while the public API already uses an overload and nullable
  `GixSignature` values;
- `create_commit_from_index` uses `has_explicit_identity` plus name/email payload
  fields, while the public API already treats the supplied identity as optional.

`ffi::Option` already has the generated union projection. Tag creation and reads
use real optional tagger records, and tag reads use optional signature bytes.
Their generated cases stay inside the boundary; public `GixSignature?` and
`byte[]?` contracts remain idiomatic managed values. The sentinel shapes above
are remaining cleanup candidates, with no deferred generator prerequisite.
Preserve their existing public nullable and overload contracts.

#### Existing `ffi::Result` verification checkpoint

`ffi::Result` already has the generated union projection. The prototype counts
of twelve result wrappers and 22 service methods are historical; the inventory
now grows with the implemented methods. Generated service methods obtain native
results through `.AsOk()`, whose `EnumException<GixError>` failures reach the
hand-written `Invoke` / `InvokeStatic` boundary and become `GixException`.

Re-audit this path when the inventory or generator changes. Preserve these
invariants:

- a native `Err` still reaches the one hand-written `GixException` translation
  boundary with its owned error data intact;
- result/error payloads remain disposed exactly once;
- generated service methods continue to distinguish ordinary `Err` from their
  current generated `Panic` / `Null` states (or an explicitly approved
  replacement contract);
- the new managed empty/default Result state is never mistaken for `Ok`;
- generated Result case types remain internal under rule 8 and do not alter any
  public GixSharp signature.

The tag input/output Option paths now exercise this projection through the
patched managed runtime. Remaining sentinel migrations are tracked separately
above; their unchecked state does not imply that Option support is missing.

## Open architecture questions - priority order

Feature/build profiles and ABI evolution are no longer open architecture
questions; their contracts are fixed above. The Interoptopus enum/union work is
an external implementation prerequisite with a documented direction, not a new
GixSharp sum-type design question. The remaining open questions are:

| Priority | Open question | What must be decided |
|---:|---|---|
| **P0b** | **Error ABI** | Freeze the semantic envelope now: one public `GixException`, small semantic `Kind`, extensible ASCII `Code`, orthogonal retryability, diagnostic-only message and selective typed recovery detail. Keep the envelope independent of generated `Result` ergonomics; implement the final closed structured-detail ABI against the planned Interoptopus discriminant fix/C# 15 `DataEnum` union projection rather than today's `IsX`/`AsX` representation. |
| **P0c** | **Structured cursors** | With the stream lifecycle constrained by P0a and the error envelope fixed by P0b, freeze bounded batched managed pull, direct-vs-producer/channel adaptation, item-error vs terminal-failure semantics, final outcomes, single-consumer behaviour and automated ownership-closed payload enforcement. Preserve eager managed convenience methods as materializers over streaming where compatibility warrants it. Closed item/outcome states should use Rust data enums once the generator support is available, not nullable-field bags. |
| **P1** | **Stateful resources / transactions** | Index editing, refs/config transactions, worktree mutation, writers/editors and reusable diff caches need explicit ownership, service-vs-method boundaries, disposal and commit/rollback rules. This can be designed per affected API family rather than changing every existing operation. |
| **P2** | **Callbacks, progress, cancellation, credentials** | Network and long-running operations need a single policy for managed callbacks, worker-thread invocation, reentrancy, cancellation and managed-exception propagation. Managed callbacks should not become the default record- or byte-stream transport merely because Interoptopus supports them. |
| **P3** | **Filesystem paths vs Git bytes** | Git names/messages remain byte-faithful. OS filesystem paths need a separate platform-faithful representation and conversion policy. |
| **P4** | **Interoptopus supportability** | Union projection, snapshot and helper-validation fixes are complete. Finish the named/multi-field variant plan; prove gix-specific cursor shapes before promoting any additional generic abstraction. |


## Remaining work

Continue independent method implementation in parallel with the boundary work.
Use `../Issues.md` as the method/domain checklist; update and commit each completed
slice with its validation evidence.

- [x] Consume the completed Interoptopus union projection.
- [x] Complete P0a byte-stream boundary experiments; do not repeat them as a prerequisite.
- [ ] Finish the named/multi-field variant migration where required by closed
  recovery-detail and HEAD-state shapes.
- [ ] Implement P0b: one GixException, semantic Kind, extensible ASCII Code,
  independent retryability and actionable typed detail. Cover object-kind
  mismatch, conflicted index, empty-commit refusal, reference lock contention
  and out-of-date references without parsing diagnostic messages.
- [ ] Implement P0c: bounded batched cursors, final/error outcomes, single-consumer
  behavior and ownership-closure enforcement. Preserve existing materializing
  conveniences over streaming where needed.
- [ ] Establish a managed public-API compatibility baseline. The separate
  public-signature leakage invariant is complete above.
- [ ] Implement explicit native profiles with both hash algorithms, runtime
  capabilities and inventory/API-guard equivalence across released engines.
- [ ] Generalize RID staging and verify clean package consumption on supported
  platforms before marking profile/network delivery complete.
- [ ] Replace the documented HEAD sentinel shape with the closed model once the
  required rich variants are available; validate born/unborn/detached behavior.
- [ ] Complete per-family stateful ownership, disposal and commit/rollback rules.
- [ ] Complete callback, progress, cancellation, credential and filesystem-path
  contracts for APIs that need them.
- [ ] Promote the internal byte-reader prototype to the intended public
  object/worktree stream and archive coverage.
- [ ] Finish all public gix coverage and validate the CSharpMpc consumer migration.

## Other tracked work

- [x] Resolve local-patch substitution by selecting distinct 0.17.1 fork packages
  from registry `local`; no build.rs path-patch check is needed.
- [x] Close the Interoptopus snapshot/helper/naming and Git LFS transaction issues;
  their historical descriptions are retained in that fork's Issues.md.
- [ ] Add the documented plain-outcome-enum coverage check; unions and plain enums
  require different checks.
- [ ] Resolve the remaining reference-lock recovery, guarded symbolic-ref and
  fixture/platform issues in `../Issues.md`.
- [ ] Reassess the separately parked reflog pre-validation filesystem side effect
  before treating that core issue as fixed.

## Environment

- Windows, `core.autocrlf` on; `gix-ffi/.gitattributes` forces LF here.
- `core.longpaths true` is set; gitoxide's own fixtures still hit MAX_PATH
  in long transaction worktrees.
- interoptopus fork at `D:/repos/interoptopus`, consumed as `_mps` packages
  version 0.17.1 from the existing machine-configured registry `local`.
- .NET 11 preview 7 with the patched runtime, C# 15, `LangVersion=preview`, `EnablePreviewFeatures`,
  `runtime-async=on`, TUnit 1.36.0 - matching `CSharpMpc.Server`.
- `CSharpEditor:build_diagnostics` does not accept `.slnx`; point it at a
  `.csproj`.

## Commands

```powershell
# regenerate bindings (the ONLY way to refresh Interop.cs)
cd D:\repos\gitoxide\gix-ffi
cargo test --test generate_bindings

# build + run the C# tests (dotnet build drives cargo build)
cd bindings\GixSharp.Tests
dotnet run
```

After regeneration, review the Interop.cs diff and validate the native and managed
suites. For unchanged source and generator inputs, generation must reproduce the
committed output; intentional inventory or generator changes require a reviewed update.
