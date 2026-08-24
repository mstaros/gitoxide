# gix-ffi / GixSharp — Handoff

State as of 2026-08-24. Last verified commit `d778a55e7`, plus a managed
layer added after that commit (see Current state).

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
must act on - lock contention, non-fast-forward, missing parent, dirty
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
8. **Nothing generated escapes the managed layer.** Enforced by
   `ManagedRepositorySignatures_DoNotExposeGeneratedResources`, a reflection
   test. Keep that test passing as the surface grows.
9. **One managed/FFI surface, multiple native engines.** Build-profile
   differences must not add/remove FFI functions, records or enum variants.
   Every native artifact shipped in one package version must have the same
   Interoptopus inventory and API-guard hash; capability differences are
   runtime data, not a different managed API.
10. **Managed compatibility and native ABI compatibility are different
    contracts.** Public managed API follows SemVer. Generated/native ABI is an
    exact-version implementation detail shipped in lockstep and guarded by
    `guard!(ffi_inventory)`; cross-version DLL compatibility is not promised.

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

**Everything generated is `IDisposable`** - `Utf8String`, `VecByte`,
`VecUtf8String`, every `Result*` wrapper.

## Corrections to earlier assumptions

Each was believed and wrong. Recorded so they are not re-derived:

- Payload enums do **not** give free error mapping. Single-field tuple
  variants only; struct variants and multi-field tuples unsupported. gix
  error enums are overwhelmingly struct variants, hence the flattening.
- `body_exception_for_variant` does not produce a distinct exception
  *class* per variant, but the error data IS fully recoverable:
  `.AsOk()` throws `EnumException<GixError>` whose `.Value` carries the
  typed enum. Match on `IsNotARepository` / `IsIo` / ... and read the
  payload via `AsX()`. **Do not dispose the payload separately** - `AsX()`
  returns the enum's own field and the enum's `Dispose` covers it;
  disposing it again is a double-dispose. See `GixRepository.Translate`.
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
- **Replace the payload-enum error ABI with one FFI-safe error envelope/record.**
  `ffi::Result<T, E>` already supplies the discriminated `Err(E)` arm, so `E`
  can be a composite. This avoids trying to mirror gix's struct-heavy errors
  through Interoptopus payload enums, which support only single-field tuple
  variants.
- **`Kind` is a small semantic/action category, not a mirror of concrete gix
  variants.** The stable semantic buckets need to cover validation/input,
  not-found, failed precondition/state, concurrency conflict, busy/locked,
  configuration, I/O, corruption, unsupported, authentication/transport,
  cancellation and internal/unknown. Exact public enum spelling should be
  frozen once against representative mappings, but the categories themselves
  are the intended level of abstraction.
- **`Code` carries the precise machine-readable reason** as an extensible ASCII
  identifier rather than another closed enum. Adding a new specific code must
  not require changing the error record layout. Examples from the current
  surface include invalid-object-id, object-type-mismatch, index-conflicted,
  empty-commit-disallowed, reference-out-of-date and lock-unavailable.
- **Retryability is orthogonal to `Kind`.** Expose it as a flag/property rather
  than a category. This matches the direction in `gix-error`, which separately
  exposes `can_retry()` alongside validation/not-found/corruption
  classification.
- **The message remains diagnostic only.** Preserve the full display/source
  chain for humans and logs, but no managed control flow may parse message
  text.
- **Use structured detail only where recovery needs operands.** Ref-update
  conflicts need the reference name and expected/actual targets; object-kind
  mismatch needs object id and expected/actual kind. Do not create a payload
  type for every error. Generated detail types remain internal; public managed
  detail is typed and must continue to satisfy rule 8.
- **Centralise classification.** Important gix error enums get explicit,
  exhaustive matches so newly added upstream variants force a decision at
  compile time. A generic lower layer may recognise `std::io::Error` and
  `gix-error` semantic markers such as validation, not-found, corruption and
  retryability. Never classify by matching `Display` strings.

This is the one intentional taxonomy break to make before breadth. The managed
migration should preserve `GixException.Operation` and diagnostic message while
replacing the current coarse `Kind` semantics and adding `Code`, retryability
and optional typed detail. `guard!(ffi_inventory)` catches managed/native ABI
mismatch but does not substitute for this compatibility policy.

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
N-1 or N+1. `guard!(ffi_inventory)` is the enforcement mechanism: any signature,
record-layout or enum-layout change regenerates `Interop.cs` and changes the API
hash, and mismatched binaries fail immediately. Important behavioral/layout
changes not represented in signatures must also change the guard salt.

This means FFI records and enums do **not** need awkward reserved fields or a
per-struct versioning protocol merely to preserve cross-package binary
compatibility. They may evolve between package versions as long as generated
bindings and all native engines are rebuilt and shipped together. Within one
package version, however, every engine must have an identical FFI inventory and
guard hash.

**The hand-written managed API is the public compatibility contract.** Once
stable, it follows SemVer independently of native churn. During the current
pre-1.0 phase breaking changes are permitted by SemVer but must still be
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
- closed public enums are treated as closed contracts. Adding a member can
  break exhaustive consumer switches and therefore is not casually additive.
  Use extensible strings/codes for open domains (`GixException.Code` is the
  deliberate example), and use `[Flags]` only for domains explicitly
  documented to tolerate new bits;
- `GixException` remains one class. Adding diagnostic/actionable properties is
  additive; changing the meaning of an existing `Kind` is breaking. Keep the
  small `Kind` taxonomy stable and grow exact reasons through `Code`;
- generated Interoptopus resources remain internal so native ABI evolution
  never leaks directly into consumer signatures.

Release validation needs two independent gates:

1. **native profile equivalence:** build every native engine for a representative
   RID and prove the same generated inventory / guard hash, then load each with
   the same generated C# and verify its advertised capabilities;
2. **managed public-API compatibility:** compare the hand-written public surface
   against an approved baseline and separately flag changes to closed enums and
   positional record shapes. The exact checker may be ApiCompat,
   PublicApiAnalyzer or an equivalent deterministic snapshot; the required
   policy above is the contract, not a particular tool.

`guard!` therefore solves exact native pairing, not public SemVer. Conversely,
managed SemVer does not require keeping old native layouts alive. Keeping those
boundaries separate is what makes hundreds of future FFI additions tractable.
## Open architecture questions - priority order

Feature/build profiles and ABI evolution are no longer open architecture
questions; their contracts are fixed in the section above. The remaining open
questions are:

| Priority | Open question | What must be decided |
|---:|---|---|
| **P0a** | **Byte-stream data plane** | Benchmark caller-buffer pull, scoped view/callback and chunked `Vec<u8>` transfer against both a large materialised blob and a genuinely streaming gix `Read` source. Freeze the byte ABI only after throughput, allocations, copies, peak memory and cancellation/disposal are understood. Caller-buffer pull is the leading design. |
| **P0b** | **Error ABI** | Make the one intentional cross-cutting taxonomy break before breadth or cursor error semantics are frozen. Keep one public `GixException`; replace the payload enum with a composite error envelope; define small semantic `Kind`, extensible ASCII `Code`, orthogonal retryability, diagnostic-only message, selective typed detail, and centralized exhaustive classification. |
| **P0c** | **Structured cursors** | With the stream lifecycle constrained by P0a and the error envelope fixed by P0b, freeze bounded batched managed pull, direct-vs-producer/channel adaptation, item-error vs terminal-failure semantics, final outcomes, single-consumer behaviour and automated ownership-closed payload enforcement. Preserve eager managed convenience methods as materializers over streaming where compatibility warrants it. |
| **P1** | **Stateful resources / transactions** | Index editing, refs/config transactions, worktree mutation, writers/editors and reusable diff caches need explicit ownership, service-vs-method boundaries, disposal and commit/rollback rules. This can be designed per affected API family rather than changing every existing operation. |
| **P2** | **Callbacks, progress, cancellation, credentials** | Network and long-running operations need a single policy for managed callbacks, worker-thread invocation, reentrancy, cancellation and managed-exception propagation. Managed callbacks should not become the default record- or byte-stream transport merely because Interoptopus supports them. |
| **P3** | **Filesystem paths vs Git bytes** | Git names/messages remain byte-faithful. OS filesystem paths need a separate platform-faithful representation and conversion policy. |
| **P4** | **Interoptopus supportability** | No cursor/stream primitive exists today; missing `builtins_vec!` validation and the lack of a green C# snapshot baseline are maintenance risks. Prove gix-specific shapes before promoting new generic Interoptopus abstractions. |


## Next step

The managed layer is done for the POC surface, so issue `99208883` is
effectively closed. Do not resume broad surface expansion yet. Resolve the
high-coupling boundary work in this order:

1. **P0a byte streams:** prototype the three transfer shapes on a large blob
   and on a genuinely streaming gix `Read` source; measure throughput,
   allocations, copies, peak native/managed memory, call count and
   cancellation/disposal. Caller-buffer pull is the leading design.
2. **P0b error ABI:** replace the current payload-enum error with the composite
   envelope, freeze semantic `Kind` categories and the extensible `Code`
   convention, expose retryability separately, map the current read/write
   failure modes exhaustively, and preserve one public `GixException`. Include
   `Unsupported` as the defensive native representation of a missing build
   capability, while the managed layer preflights known absence as
   `NotSupportedException`. Add tests for actionable cases such as object-kind
   mismatch, conflicted index, empty-commit refusal, reference lock contention
   and reference-out-of-date; no test or managed branch should depend on
   parsing diagnostic messages.
3. **P0c structured cursors:** with both stream lifecycle and error semantics
   fixed, replace the native eager `rev_walk` precedent with a bounded-batch
   cursor and exercise the same managed contract against `dirwalk` or `status`.
   Keep the existing managed `RevWalk(...)` as a materialising convenience over
   the streaming implementation rather than breaking its public signature.
4. add automated ownership-closure enforcement for stream/cursor payloads.
5. **Install compatibility/profile infrastructure before breadth accelerates:**
   make the shared gix feature set explicit with both hash algorithms; add the
   runtime capability bootstrap; establish the managed public-API baseline;
   and add a profile-equivalence validation that requires every released native
   engine to produce the same Interoptopus inventory/API-guard hash. The second
   physical native engine and generalized RID staging must exist before the
   first profile-specific/network surface is considered complete.
6. **P1 stateful resources:** settle service-vs-method ownership plus
   commit/rollback/disposal rules before expanding index/ref/config/worktree
   mutation families.
7. then resume breadth module by module, pairing each Rust facade addition
   with its managed wrapper and tests and keeping rules 8-10 green.

The target remains **full gix coverage**: the managed/FFI surface represents
the union of the declared native-profile capabilities, not one impossible
all-features binary. Small bounded materialisation may still be selected
locally when it is demonstrably the right API shape, but it is an optimisation
decision, not the architecture for iteration or bulk data.


## Also outstanding

- Issue `f7bf7635`: `build.rs` asserting the interoptopus patch is active.
- Upstream the interoptopus `validate()` fix (`2a6da76a`) and the `Vec<T>`
  accessors. A PR also tests whether that project is responsive, which is
  the parked fork-or-revive question.
- interoptopus snapshots (`ccb105a2`) - no green baseline means template
  changes cannot be validated.
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
