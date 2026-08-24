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

15 TUnit tests pass in the **main checkout** against a loaded native DLL.

**Rust facade** - the current POC includes `Repo.open`, `git_dir`, `is_bare`,
`head`, `rev_walk`, and `commit_info`, plus records `HeadInfo`, `CommitInfo`
and the error enum `GixError`.

**Managed layer** - `bindings/GixSharp/Managed/`:

- `GixRepository : IDisposable` - `Open`, `GitDir`, `IsBare`, `Head`,
  `RevWalk`, `CommitInfo`
- records `GixHead`, `GixCommitInfo`
- `GixException` + `GixErrorKind`

The managed layer closes the traps the POC exposed: managed `string` in and
out, no generated resource escapes, results outlive the repository,
`Dispose` is idempotent and guards further use, native errors become typed
`GixException`. Reviewed 2026-08-24: no bugs found.

For scale only, `gix` has 727 `pub fn` in `gix/src`. The eventual FFI surface
will not map 1:1 to those functions because builders, iterators, callbacks,
transactions and borrowed views need ABI-specific shapes. The target is full
coverage.

Commits (managed layer landed after `d778a55e7`; check `git log`):

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
  src/lib.rs            the Rust facade
  tests/generate_bindings.rs   generates Interop.cs (a test, not build.rs)
  bindings/
    GixSharp.slnx       solution, tests in a /tests/ folder
    Interop.cs          GENERATED, committed, namespace GixSharp
    GixSharp/
      GixSharp.csproj   packable class library, builds the cdylib
      Managed/          HAND-WRITTEN managed layer
    GixSharp.Tests/     TUnit - RepoTests (interop), ManagedRepositoryTests
```

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
   streaming architecture below.
7. **One coarse error type for now** (`GixError` -> `GixErrorKind`).
   Splitting into per-domain types is a breaking change, so defer until
   there are enough operations to design a real taxonomy.
8. **Nothing generated escapes the managed layer.** Enforced by
   `ManagedRepositorySignatures_DoNotExposeGeneratedResources`, a reflection
   test. Keep that test passing as the surface grows.

Pinned gix features (matches what `cargo rustdoc --output-format json` was
validated with): `parallel`, `attributes`, `revision`, `blame`, `merge`,
`status`, `dirwalk`, `index`, `worktree-mutation`, `blocking-network-client`.
Default features stay ON. Full coverage does **not** imply that every mutually
incompatible Cargo feature can live in one binary; feature/build profiles are
an open architecture question below.

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

## P0 - streaming architecture

Full coverage requires one coherent streaming **lifecycle** with two distinct
data-plane shapes. Cursor design and large-byte streaming must be settled
together before either ABI is treated as frozen.

**Structured record streams** include rev-walk, full-repo dirwalk, status,
references/reflogs, tree changes, object enumeration, parser tokens, pack
indexes and similar APIs. The accepted direction is:

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

This one consumer contract does **not** require one Rust implementation.
`rev_walk` and reference iterators borrow repositories; `dirwalk` and status
already contain producer/interrupt patterns; tree diff is callback-driven but
`Change::detach()` produces an owned change. They can all adapt to the same
managed consumption model.

**Bulk byte streams** include blobs, large patches, archives, pack/network
content and similar data. Do not model these as `Cursor<byte>` by default.
The expected data-plane shape is caller-buffer pull (for example a
`Read(Span<byte>)`-style managed surface) or an equivalent chunked primitive,
with the same ownership, disposal, cancellation, backpressure and error
semantics as structured cursors. The exact byte-stream ABI remains open until
a representative large blob/patch case is prototyped.

The detached-payload rule above must gain an automated test/check analogous
to the existing managed reflection invariant. The handoff intentionally does
not prescribe the mechanism yet; the requirement is that violations become a
build/test failure rather than a review convention.

Do **not** add a generic cursor abstraction to Interoptopus yet. Prove the
contract first in `gix-ffi` with at least one repo-borrowing structured stream
(`rev_walk`) and one naturally streaming workload (`dirwalk` or `status`),
plus one bulk-byte reader. Once those share a coherent lifecycle, extract the
reusable Interoptopus pattern from observed requirements.

## Open architecture questions - priority order

| Priority | Open question | What must be decided |
|---:|---|---|
| **P0** | **Streaming: structured cursors + bulk bytes** | Shared ownership/lifecycle/error/cancellation/backpressure contract; batched structured pull; caller-buffer/chunked byte transfer; final outcomes; enforcement of ownership-closed payloads. |
| **P1** | **Stateful resources / transactions** | Index editing, refs/config transactions, worktree mutation, writers/editors and reusable diff caches need explicit ownership, disposal and commit/rollback rules. |
| **P2** | **Callbacks, progress, cancellation, credentials** | Network and long-running operations need a single policy for managed callbacks, worker-thread invocation, reentrancy, cancellation and managed-exception propagation. Managed callbacks should not become the default record-stream transport merely because Interoptopus supports them. |
| **P3** | **Error ABI** | The current coarse `GixErrorKind` is insufficient for full coverage, but mirroring every Rust error type is impractical. Define a stable domain/context/source taxonomy before the surface makes it expensive to change. |
| **P4** | **Filesystem paths vs Git bytes** | Git names/messages remain byte-faithful. OS filesystem paths need a separate platform-faithful representation and conversion policy. |
| **P5** | **Feature/build profiles** | Full coverage meets mutually awkward blocking/async and transport feature sets. Decide whether coverage is delivered by profiles/artifacts rather than pretending all features can coexist in one binary. |
| **P6** | **ABI evolution and API guards** | `gix-ffi::ffi_inventory()` already registers `guard!(ffi_inventory)`, and the C# backend checks the baked API hash against the loaded DLL. Keep that guard non-optional. The open work is the compatibility policy for additions and changes to enums, records and functions; use the guard salt for important behavioural/layout changes not reflected in signatures. |
| **P7** | **Interoptopus supportability** | No cursor primitive exists today; missing `builtins_vec!` validation and the lack of a green C# snapshot baseline are maintenance risks. Prove gix-specific shapes before promoting new generic Interoptopus abstractions. |

## Next step

The managed layer is done for the POC surface, so issue `99208883` is
effectively closed. The immediate work is **P0 streaming architecture**, not
breadth by eager materialisation:

1. replace the eager `rev_walk` precedent with a structured cursor prototype;
2. exercise the same cursor lifecycle against `dirwalk` or `status`;
3. prototype a caller-buffer bulk-byte reader on a large blob or patch;
4. add automated ownership-closure enforcement for stream payloads;
5. then resume breadth module by module, pairing each Rust facade addition
   with its managed wrapper and tests and keeping rule 8 green.

The target remains **full gix coverage**. Small bounded materialisation may
still be selected locally when it is demonstrably the right API shape, but it
is an optimisation decision, not the architecture for iteration.

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
