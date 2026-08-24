# gix-ffi / GixSharp — Handoff

State as of 2026-08-25. Implemented-surface union audit against `main` at
`0978c580f8e42d6f3ea53df06df12d9be1bfe636` and the local Interoptopus
`docs/csharp-unions.md` plan.

## What this is

A .NET binding over gitoxide (`gix`) targeting **full gix coverage**.

**Status: proof of concept, both layers present.** The current surface is
small; what it proves is that the toolchain works and the core ownership and
marshalling decisions hold. It is the base for systematic expansion, not the
target surface.

Two distinct layers. Keep the names apart; calling both "the facade" caused
real confusion:

```
gix (path dep, this repo)
  -> gix-ffi              RUST FACADE, #[ffi] annotated
  -> interoptopus         generates C# from the facade inventory
  -> Interop.cs           GENERATED, never hand-edited
  -> GixSharp/Managed/    MANAGED LAYER, hand-written
  -> GixSharp.Tests       TUnit, 15 tests against a real repository
```

No cbindgen, no ClangSharp, no hand-written P/Invoke.

## Current state

Do not trust method counts in this document - the surface moves faster than
the prose. This section lists **capability areas**; for the exact surface
read `src/` and `bindings/GixSharp/Managed/`, and `git log` for history.

Both layers exist and are exercised end to end by TUnit tests running in the
**main checkout** against a loaded native DLL.

**Working capability areas:**

- repository open, discover, init
- repository info, git dir, bareness
- HEAD resolution
- commit reading, history, ancestry
- object metadata and tree ids
- **commit writing** - object creation and commit-from-index
- index - in progress at time of writing

**Rust facade** - `src/`, split across more than one module. Records plus a
single coarse error enum `GixError`.

**Managed layer** - `bindings/GixSharp/Managed/`, a partial `GixRepository`
across several files, with records `GixHead`, `GixCommitInfo`, `GixCommit`,
`GixObjectId`, `GixObjectMetadata`, `RepositoryInfo`, and `GixException` +
`GixErrorKind`.

The managed layer closes the traps the POC exposed: managed `string` in and
out, no generated resource escapes, results outlive the repository,
`Dispose` is idempotent and guards further use, native errors become typed
`GixException`. Reviewed 2026-08-24: no bugs found.

**Writes are now in scope.** This is no longer a read-only binding.
`create_commit_object` and `create_commit_from_index` fail in ways a caller
must act on - lock contention, non-fast-forward, missing parent, conflicted
index. Read paths tolerated a coarse error type; write paths do not. This is
the main reason P0b has moved ahead of cursor work.

For scale only, `gix` has 727 `pub fn` in `gix/src`. The eventual FFI
surface will not map 1:1 to those, because builders, iterators, callbacks,
transactions and borrowed views all need ABI-specific shapes. **The target
is full coverage.**

Early commits, for orientation only - use `git log` for the real history:

| Commit | What |
|---|---|
| `bbd8da04a` | facade crate, `repo_open` |
| `828a1e197` | GixSharp + Tests projects, byte-based paths |
| `a200a90e1` | `head`, `rev_walk`, `commit_info` |
| `d778a55e7` | `.gitattributes`, LF normalisation |


## Layout

```
gix-ffi/
  Cargo.toml            own [workspace] - NOT a gitoxide workspace member
  .cargo/config.toml    GITIGNORED, machine-local, required (see Traps)
  .gitattributes        forces LF; generator emits LF, autocrlf rewrites it
  .gitignore            *.sln ignored, GixSharp.slnx deliberately NOT
  src/                  the Rust facade, split across modules
  tests/generate_bindings.rs   generates Interop.cs (a test, not build.rs)
  bindings/
    GixSharp.slnx       solution, tests in a /tests/ folder
    Interop.cs          GENERATED, committed, namespace GixSharp
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
8. **Nothing generated escapes the hand-written managed API.** The current
   `ManagedRepositorySignatures_DoNotExposeGeneratedResources` reflection test
   proves this only for public `GixRepository` method signatures and a fixed set
   of generated top-level types. That is sufficient for the current POC but not
   the final invariant. Before generated union cases are enabled, extend the
   check across the complete hand-written public surface (methods, properties,
   constructors/record shapes and nested generic/array/by-ref signature types)
   and make generated-type detection include nested `*Case` types without
   hand-enumerating every case. Generated C# 15 union cases remain internal
   implementation detail unless the hand-written API deliberately defines its
   own public sum type.
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

**`.cargo/config.toml` is gitignored and does NOT follow into a new
worktree.** Without it the build silently resolves `interoptopus_csharp`
from crates.io instead of the local checkout, the generated `Vec<T>` loses
`AsSpan()`/`ToArray()`, and the failure appears as
`CS0411 ImmutableArrayExtensions.ToArray<T> cannot be inferred` - naming
nothing relevant. **Every new worktree needs a copy.** Issue `f7bf7635`.

**Fixing the config is not sufficient.** `Interop.cs` will already have been
regenerated wrong, and `cargo build` does NOT regenerate it - only
`cargo test --test generate_bindings` does.

**Every `ffi::Vec<T>` needs its own `builtins_vec!(T)`.** Without it the
backend emits *references* to `VecByte` / `VecUtf8String` but never the
type. Rust compiles clean; only `csc` catches it. Bit twice. The macro takes
the element type, unlike `builtins_string!()`. Issue `2a6da76a`.

This becomes infrastructure-level correctness once batched structured
streaming starts using `ffi::Vec<T>` pervasively. The missing-registration
validation should be fixed rather than relying on C# compilation to catch it.

**`Utf8String` arguments are MOVED, not borrowed.** The marshaller calls
`IntoUnmanaged()`, transferring the pointer and nulling the managed side, so
a generated `Utf8String` is single-use. The managed layer handles this by
building a fresh one per call from a managed `string` - do not reintroduce
generated types into public signatures.

**Generated ownership is per type, not universal.** Owned generated resources such
as `Utf8String`, `VecByte`, `VecUtf8String`, `GixError` and the current
`Result*` wrappers are `IDisposable`; scalar generated types such as
`FfiObjectType` are not. Under the planned union projection, nested `*Case`
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

## P0 - boundary contracts before breadth

Full coverage now has three high-coupling boundary decisions that must be
settled before broad surface expansion: bulk-byte transfer, the error contract,
and structured cursors. Resolve them in that order. Byte streaming constrains
the shared stream lifecycle; the error contract then has to exist before cursor
item/terminal error semantics are frozen.

### P0a - byte streams first

Bulk byte streams include blobs, large patches, archives, pack/network content,
filter output and similar data. `ffi::Vec<u8>` is not the general solution:
it materialises the whole payload in Rust-owned memory and `ToArray()` copies
it again into managed memory. That is acceptable for small Git byte strings,
not for large content.

Interoptopus already supports `ffi::SliceMut<T>` as a borrowed mutable slice.
The generated C# pins a managed array and passes pointer + length without an
FFI copy, so caller-buffer pull is viable at the ABI boundary.

There is an important gix constraint: ordinary object lookup is currently
`Find::try_find(id, &mut Vec<u8>)`. Loose-object lookup resizes that `Vec` to
the full decompressed size, and packed/delta decoding is also built around
`Vec<u8>` result/scratch buffers. Therefore caller-buffer blob reads are not
end-to-end zero-copy today: gix still materialises the decoded object in Rust,
but caller-buffer pull removes the second full Rust-to-managed allocation and
copy and keeps the managed API compatible with genuinely streaming sources.

The leading byte-stream contract is therefore a stateful native reader with a
caller-owned reusable buffer, exposed managed-side as `Stream.Read(Span<byte>)`
semantics or equivalent. EOF is learned by reading to exhaustion; known length
may be exposed as metadata when cheap, but the protocol does not require a
size-query/read two-call dance.

Three transfer shapes should still be measured before the byte ABI is frozen:

- **Caller-supplied buffer** - preferred direction. Works with existing gix
  materialised blobs and can become direct streaming when the underlying gix
  source implements `Read`.
- **Scoped callback/view** - can expose an already-materialised Rust buffer to
  managed code without another bulk copy, but introduces reverse P/Invoke,
  strict scoped lifetime, reentrancy and managed-exception concerns. Keep as a
  possible specialised fast path, not the default stream model.
- **Chunked `Vec<u8>` cursor** - unifies the `next()` shape mechanically but
  adds chunk allocation/copying and prevents direct use of caller-owned
  reusable buffers. Do not adopt it as the default merely for API uniformity.

Benchmark in two distinct scenarios so gix materialisation is not confused
with FFI cost:

1. a large blob: compare whole `ffi::Vec<u8>` + managed copy, caller-buffer
   pull from a materialised native blob, scoped view/callback, and chunked
   `Vec<u8>` transfer;
2. a genuinely streaming gix source such as `gix-worktree-stream::Entry` (or
   another `Read`-based path): compare caller-buffer pull against callback and
   chunked transfer to verify that the ABI preserves real native streaming.

The decision criterion is not only throughput. Measure peak native memory,
peak managed memory, allocations, copies, call count and cancellation/disposal
behaviour. The managed contract must not encode today's ODB materialisation if
that implementation can improve later.

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
### Interoptopus enum / C# 15 union dependency - planned, not landed

The GixSharp architecture now assumes the direction documented in the local
Interoptopus `docs/csharp-unions.md`, but that document currently says **plan,
not approved** and identifies its discriminant Step 0 as blocking. Treat this
as an external prerequisite/direction, not as code that already exists.

The parts GixSharp relies on are:

- **Correct discriminants first.** Interoptopus currently stores an explicit tag
  only on unit variants and reconstructs tuple-variant tags positionally in the
  C# model. The plan requires one authoritative native discriminant for every
  variant; its current recommendation is a `tag` field on `Variant`, but that
  Step 0 shape is still marked open. Do not finalize new discriminated GixSharp
  ABI around today's tuple-tag behavior.
- **Custom `[Union]`, not the C# `union` keyword.** The planned C# backend keeps
  the existing managed tag/payload storage and native `Unmanaged` layout, then
  adds C# 15 union behavior through nested `*Case` types, `IUnion`, `Value`,
  `HasValue` and `TryGetValue`. The native ABI stays unchanged.
- **Default/invalid state is managed-only.** Struct-backed generated enums gain
  a managed `_hasValue` bit; `default(E)` is empty and cannot be marshalled to
  Rust. Class-backed enums have no empty non-null instance. Unknown native tags
  remain interop corruption and must fail during conversion.
- **Projection is opt-in and target-sensitive.** Flag-off output remains
  byte-identical for existing/net10 consumers. GixSharp already targets .NET 11
  preview, so once the feature lands and its generator tests are green,
  GixSharp should enable the union projection for its generated bindings rather
  than design new managed code around the legacy generated `IsX` / `AsX`
  ergonomics.
- **Initial scope is plain `DataEnum`.** `ffi::Option` / `ffi::Result` are
  explicitly deferred in the Interoptopus plan. GixSharp must therefore keep
  its hand-written public result/error translation independent of generated
  `Result` ergonomics. Plain internal detail/state enums may consume the first
  union projection; `Option` / `Result` can converge later without changing the
  public managed contract.
- **Generated cases are output-only.** Nested case types stay with the parent
  generated enum, have no FFI `TypeId`, and remain internal under rule 8. Do not
  expose them from public GixSharp signatures.

GixSharp will **wait rather than create a temporary competing sum-type layer**.
If a planned API naturally wants a closed data enum but would currently require
representation boilerplate solely because of Interoptopus limitations, defer
that final ABI shape until the relevant generic support is available. This does
not block independent work such as P0a byte streaming or semantic classification
of the P0b error envelope.

`guard!(ffi_inventory)` does not validate C# projection quality when the Rust
inventory is unchanged. Union projection deliberately changes generated managed
source while preserving the native ABI/hash. Therefore two independent checks
are required once GixSharp enables it: the normal native API guard, plus a pinned
Interoptopus/generated-source build gate that compiles the generated bindings
under the exact .NET 11 / C# 15 preview toolchain. The Interoptopus plan owns
compiler-facing union tests; GixSharp only needs to verify its generated bindings
and hand-written translation compile and behave against the pinned fork.
### Existing implementation migration when unions land

The existing POC has been audited against the planned union projection so this
is not only guidance for future APIs. The migration is intentionally split by
the Interoptopus rollout boundary and by whether the affected shape is generated
implementation detail or already-public GixSharp API.

**Current plain `DataEnum` inventory is only two types:**

- `FfiObjectType` is a unit-only, struct-backed data enum used inside
  `ObjectMetadata`;
- `GixError` is the class-backed seven-case payload enum used as the error side
  of every current `ffi::Result<_, GixError>`.

There is currently **no `ffi::Option<T>` in the gix-ffi surface**. `ffi::Result`
is already pervasive, but its generated union projection is explicitly a later
Interoptopus phase and must not be conflated with the first plain-`DataEnum`
rollout.

The hand-written managed layer does not construct `GixError`, `FfiObjectType`
or any generated `Result*` wrapper directly. Today `default(FfiObjectType)` is
indistinguishable from its tag-zero `Commit` case in generated checks, but no
hand-written GixSharp code relies on that behavior; `ReadObjectType` receives it
from native `ObjectMetadata`. Likewise, generated Result construction and
`.AsOk()` calls live in `Interop.cs`, not `Managed/`. The planned stricter
empty/default and class-construction semantics are therefore generator migration
concerns for the current surface, not public GixSharp source breaks.

#### Plain `DataEnum` adoption - immediate internal migration

When plain `DataEnum` union projection becomes consumable:

1. **Enable it in binding generation, not in the public API.**
   `tests/generate_bindings.rs` currently builds `RustLibrary` without a union
   option. Once the pinned Interoptopus revision exposes the planned builder
   switch, enable it there and regenerate committed `bindings/Interop.cs`.
   `GixSharp.csproj` and `GixSharp.Tests.csproj` are already `net11.0` with
   `LangVersion=preview` and preview features enabled; no target-framework
   migration is required.
2. **Migrate the durable `FfiObjectType` consumer to union cases.**
   `GixRepository.Objects.cs::ReadObjectType` currently tests `IsCommit`,
   `IsTree`, `IsBlob` and `IsTag`. After union projection, consume
   `CommitCase` / `TreeCase` / `BlobCase` / `TagCase` through C# pattern
   matching and keep the hand-written public `GixObjectType` unchanged. This is
   an internal generated-shape migration, not a public API change. The generator
   itself owns the new empty/default struct state and native-tag validation.
3. **Do not spend a standalone migration on the coarse `GixError`.** The planned
   generator preserves legacy factories/checks/accessors, so enabling plain
   unions does not require immediately rewriting the existing
   `GixRepository.Translate` `IsX` / `AsX` switch. P0b is already scheduled to
   replace this coarse seven-case ABI with the semantic error envelope; fold the
   managed translation rewrite into that work instead of first modernising a
   type that will immediately disappear. If an intermediate branch does use the
   generated `GixError` cases, dispose the owning generated error exactly once;
   do not separately dispose a payload obtained through a case view.
4. **Strengthen rule 8's reflection test.** The existing
   `ManagedRepositorySignatures_DoNotExposeGeneratedResources` test enumerates a
   fixed set of generated top-level types and inspects only public
   `GixRepository` method parameters/returns. Nested generated `*Case` types are
   a new leak surface, and the final invariant also covers hand-written public
   records/properties/constructors plus generated types nested inside generic,
   array and by-ref signatures. Replace the fixed-set repository-only check with
   a generic whole-public-surface check before union projection is enabled.
5. **Keep existing public behavior tests stable.** Union projection by itself
   must not change `GixException`, `GixErrorKind`, `GixObjectType` or any public
   repository signature. Existing tests for native-error translation and object
   metadata are therefore regression tests for the migration. Rust-side tests
   that pattern-match `GixError` / `FfiObjectType` do not change merely because
   the C# projection changes; they change later when P0b changes the Rust error
   ABI itself.

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

#### Later `ffi::Option` cleanup - internal FFI debt, public API stays stable

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

When Interoptopus's deferred `ffi::Option` union phase is available, these are
candidates to become real optional FFI values instead of `has_* + payload` or
empty-value sentinels. That cleanup should **not** change the existing public
nullable/overload contracts merely to expose generated Option cases. Generated
Option cases remain internal; preserve public `string?`, nullable records and
operation overloads where those are already the correct managed shape.

#### Later `ffi::Result` migration checkpoint

The current generated surface contains twelve `Result*GixError` wrapper classes,
and 22 generated `Repo` methods obtain native results through `.AsOk()`. Failed
`.AsOk()` calls surface `EnumException<GixError>`, which the hand-written
`Invoke` / `InvokeStatic` methods catch and translate to `GixException`. There
are no hand-written managed `.AsOk()` / `.AsErr()` calls today.

When Interoptopus later projects `ffi::Result` as a union, re-audit the generated
service/result path rather than pre-emptively rewriting GixSharp. Preserve these
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

There is no current `ffi::Option` wrapper to migrate immediately; the optional
sentinel shapes above are the inventory to revisit when that later phase lands.
## Open architecture questions - priority order

Feature/build profiles and ABI evolution are no longer open architecture
questions; their contracts are fixed above. The Interoptopus enum/union work is
an external implementation prerequisite with a documented direction, not a new
GixSharp sum-type design question. The remaining open questions are:

| Priority | Open question | What must be decided |
|---:|---|---|
| **P0a** | **Byte-stream data plane** | Benchmark caller-buffer pull, scoped view/callback and chunked `Vec<u8>` transfer against both a large materialised blob and a genuinely streaming gix `Read` source. Freeze the byte ABI only after throughput, allocations, copies, peak memory and cancellation/disposal are understood. Caller-buffer pull is the leading design. |
| **P0b** | **Error ABI** | Freeze the semantic envelope now: one public `GixException`, small semantic `Kind`, extensible ASCII `Code`, orthogonal retryability, diagnostic-only message and selective typed recovery detail. Keep the envelope independent of generated `Result` ergonomics; implement the final closed structured-detail ABI against the planned Interoptopus discriminant fix/C# 15 `DataEnum` union projection rather than today's `IsX`/`AsX` representation. |
| **P0c** | **Structured cursors** | With the stream lifecycle constrained by P0a and the error envelope fixed by P0b, freeze bounded batched managed pull, direct-vs-producer/channel adaptation, item-error vs terminal-failure semantics, final outcomes, single-consumer behaviour and automated ownership-closed payload enforcement. Preserve eager managed convenience methods as materializers over streaming where compatibility warrants it. Closed item/outcome states should use Rust data enums once the generator support is available, not nullable-field bags. |
| **P1** | **Stateful resources / transactions** | Index editing, refs/config transactions, worktree mutation, writers/editors and reusable diff caches need explicit ownership, service-vs-method boundaries, disposal and commit/rollback rules. This can be designed per affected API family rather than changing every existing operation. |
| **P2** | **Callbacks, progress, cancellation, credentials** | Network and long-running operations need a single policy for managed callbacks, worker-thread invocation, reentrancy, cancellation and managed-exception propagation. Managed callbacks should not become the default record- or byte-stream transport merely because Interoptopus supports them. |
| **P3** | **Filesystem paths vs Git bytes** | Git names/messages remain byte-faithful. OS filesystem paths need a separate platform-faithful representation and conversion policy. |
| **P4** | **Interoptopus supportability** | The current fork still has no cursor/stream primitive, snapshot baseline issue `ccb105a2` gates template review, and `docs/csharp-unions.md` still marks the discriminant fix as blocking/not approved. Consume those generic fixes rather than duplicating them in GixSharp, then prove gix-specific cursor shapes before promoting any additional generic Interoptopus abstraction. |


## Next step

The managed layer is done for the POC surface, so issue `99208883` is
effectively closed. Do not resume broad surface expansion yet. Resolve the
high-coupling boundary work in this order:

1. **P0a byte streams:** prototype the three transfer shapes on a large blob
   and on a genuinely streaming gix `Read` source; measure throughput,
   allocations, copies, peak native/managed memory, call count and
   cancellation/disposal. Caller-buffer pull is the leading design. This work
   is independent of the Interoptopus enum/union plan and can proceed now.
2. **Consume the Interoptopus enum prerequisite instead of building a GixSharp
   workaround:** wait for the discriminant Step 0 and opt-in plain-`DataEnum`
   C# 15 custom-union projection described by `docs/csharp-unions.md`, with the
   Interoptopus snapshot baseline repaired and a flag-on generated-C# compile
   fixture green. Pin that fork revision, enable its union builder option in
   `tests/generate_bindings.rs`, and regenerate `bindings/Interop.cs`. As part of
   that adoption, migrate `ReadObjectType(FfiObjectType)` to generated union-case
   matching and strengthen the managed-signature reflection invariant to catch
   nested generated `*Case` types. Do **not** do a standalone rewrite of the
   coarse `GixError` translation or the generated `Result*`/`.AsOk()` path at
   this stage: fold the former into P0b and re-audit the latter only when
   Interoptopus's deferred Result-union phase lands.
3. **P0b error ABI:** with the generator prerequisite available, implement the
   semantic envelope, freeze `Kind` categories and the extensible `Code`
   convention, expose retryability separately, and model only actionable
   recovery detail as an internal closed data enum. Preserve one public
   `GixException`. Include `Unsupported` as the defensive native representation
   of a missing build capability, while the managed layer preflights known
   absence as `NotSupportedException`. Replace the current coarse
   `EnumException<GixError>` translation here rather than modernising it twice.
   Add tests for object-kind mismatch, conflicted index, empty-commit refusal,
   reference lock contention and reference-out-of-date; no test or managed
   branch should parse diagnostics.
4. **P0c structured cursors:** with stream lifecycle and error semantics fixed,
   replace the native eager `rev_walk` precedent with a bounded-batch cursor and
   exercise the same managed contract against `dirwalk` or `status`. Keep the
   existing managed `RevWalk(...)` as a materialising convenience over the
   streaming implementation. Use Rust data enums for genuinely closed
   item/outcome states where appropriate; do not invent nullable-field bags.
5. add automated ownership-closure enforcement for stream/cursor payloads.
6. **Install compatibility/profile infrastructure before breadth accelerates:**
   make the shared gix feature set explicit with both hash algorithms; add the
   runtime capability bootstrap; establish the managed public-API baseline and
   record the `GixHead` sum-type redesign as a known approved pre-1.0 break;
   add the pinned-generator/exact-toolchain compile gate; and add a
   profile-equivalence validation requiring every released native engine to
   produce the same Interoptopus inventory/API-guard hash. The second physical
   native engine and generalized RID staging must exist before the first
   profile-specific/network surface is considered complete.
7. **Remove the known public sentinel sum type before API stabilization:** once
   the needed generic rich-enum support is available, replace `HeadInfo`'s
   target/referent/boolean encoding and the public positional `GixHead` record
   with the closed HEAD-state model described above. Treat the baseline change
   as intentional, update the existing born/unborn/detached tests in the same
   transaction, then approve the new public baseline. Do not use this as a
   reason to convert ordinary scalar enums or flags to unions. The later
   `ffi::Option` and `ffi::Result` migrations remain internal cleanup checkpoints
   and preserve their already-correct public nullable/exception contracts.
8. **P1 stateful resources:** settle service-vs-method ownership plus
   commit/rollback/disposal rules before expanding index/ref/config/worktree
   mutation families.
9. then resume breadth module by module, pairing each Rust facade addition with
   its managed wrapper and tests and keeping rules 8-11 green.

The target remains **full gix coverage**: the managed/FFI surface represents
the union of the declared native-profile capabilities, not one impossible
all-features binary. Small bounded materialisation may still be selected
locally when it is demonstrably the right API shape, but it is an optimisation
decision, not the architecture for iteration or bulk data.


## Also outstanding

- Issue `f7bf7635`: `build.rs` asserting the pinned local Interoptopus patch is
  active.
- Interoptopus `docs/csharp-unions.md`: current status is **plan, not approved**.
  Its Step 0 discriminant-model fix is blocking the planned opt-in plain
  `DataEnum` C# 15 custom-union projection. GixSharp intentionally waits for
  this generic work rather than creating a parallel wrapper-enum architecture.
- Interoptopus snapshot baseline issue `ccb105a2`: template changes cannot be
  reviewed reliably until the failing snapshot drift is repaired. The union
  plan also requires a flag-on net11 / `LangVersion=preview` generated-C#
  compile fixture; native API guard coverage is not sufficient because the
  union projection preserves the native ABI/hash.
- The existing Interoptopus `validate()` fix (`2a6da76a`) and `Vec<T>` accessors
  remain local-fork dependencies until their upstream/fork disposition is
  settled.
- `ffi::Option` / `ffi::Result` union projection is explicitly deferred by the
  current Interoptopus plan. Keep GixSharp's public managed result/error
  translation independent of that generated ergonomics so it can converge
  later without a public API break.
- Parked, unrelated to the bridge: `gix-ref` reflog creates directories and
  an empty file BEFORE validating the committer, leaving debris on a pure
  validation failure. `gix-ref/src/store/file/loose/reflog.rs` ~119-153.

## Environment

- Windows, `core.autocrlf` on; `gix-ffi/.gitattributes` forces LF here.
- `core.longpaths true` is set; gitoxide's own fixtures still hit MAX_PATH
  in long transaction worktrees.
- interoptopus 0.16.4, local checkout at
  `C:/Users/mstar/source/repos/interoptopus`, fork `mstaros/interoptopus`.
  The `Vec<T>` accessors exist only there, not on crates.io.
- .NET 11 preview, `LangVersion=preview`, `EnablePreviewFeatures`,
  `runtime-async=on`, TUnit 1.36.0 - matching `CSharpMpc.Server`.
- `CSharpEditor:build_diagnostics` does not accept `.slnx`; point it at a
  `.csproj`.

## Commands

```powershell
# regenerate bindings (the ONLY way to refresh Interop.cs)
cd C:\Users\mstar\source\repos\gitoxide\gix-ffi
cargo test --test generate_bindings

# build + run the C# tests (dotnet build drives cargo build)
cd bindings\GixSharp.Tests
dotnet run
```

A clean `git diff --stat bindings/Interop.cs` after regenerating confirms
the local interoptopus patch is active. A diff means it is not, and the
bindings have just been silently downgraded.
