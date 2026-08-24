# gix-ffi / GixSharp — Handoff

State as of `mstaros/gitoxide` @ `d778a55e7`, 2026-08-24.

## What this is

A .NET binding over gitoxide (`gix`), intended to replace `LibGit2.Native` -
libgit2 upstream is effectively dead and its features lag.

**Status: proof of concept.** Six methods. Roughly 1% of a realistic
binding, and useless to a consumer as it stands. What it proves is that the
toolchain works and the design decisions hold - nothing more.

Two distinct layers, both easy to call "the facade". Keep them separate:

```
gix (path dep, this repo)
  -> gix-ffi          RUST FACADE, #[ffi] annotated      <-- POC, 6 methods
  -> interoptopus     generates C# from the facade inventory
  -> Interop.cs       GENERATED, internal, never hand-edited
  -> GixSharp         MANAGED LAYER - does not exist yet
  -> GixSharp.Tests   TUnit, calls the real DLL against a real repository
```

Today `GixSharp` is only the csproj that compiles `Interop.cs`; there is no
hand-written managed code in it. That gap is the next step.

No cbindgen, no ClangSharp, no hand-written P/Invoke.

## Current state - POC only

7 TUnit tests pass against a loaded native DLL and a real repository.

The **entire** exposed surface is six methods on `Repo`: `Open`, `GitDir`,
`IsBare`, `Head`, `RevWalk`, `CommitInfo` - plus records `HeadInfo`,
`CommitInfo` and the error enum `GixError`.

For scale: `gix` has 727 `pub fn` in `gix/src`; full coverage was estimated
at 450-650 exported functions. `LibGit2.Native`'s demonstrated-sufficient
surface is 60-90 managed methods, which is the realistic target. Either way
this is about 1%.

**What the POC establishes** (each validated by running, not argument):

- the chain works: `cargo test` -> `Interop.cs` -> `dotnet build` -> tests
- borrowed gix handles can become owned records (`HeadInfo`)
- iterators can cross as eager ids with lazy bodies (`RevWalk` + `CommitInfo`)
- bytes for paths and ref names round-trip; hex for object ids works
- `#[ffi]` structs with non-`Copy` fields and `ffi::Vec<ffi::String>` work

**What it does not establish:** anything about breadth. No status, diff,
index, refs, branches, remotes, config, worktrees.

Commits, in order:

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
  .gitignore            note: *.sln ignored, GixSharp.slnx deliberately NOT
  src/lib.rs            the Rust facade
  tests/generate_bindings.rs   generates Interop.cs (a test, not build.rs)
  bindings/
    GixSharp.slnx       solution, tests in a /tests/ folder
    Interop.cs          GENERATED, committed, namespace GixSharp
    GixSharp/           packable class library, builds the cdylib
    GixSharp.Tests/     TUnit
```

`gix-ffi` is a **nested independent cargo workspace** on purpose. As a
gitoxide workspace member, `cargo test --workspace` would unify `gix`
feature selection with `gitoxide-core`, which carries a `compile_error!`
for `blocking-client` + `async-client` together.

## Design rules - decided, validated by execution

1. **No borrowed data crosses the boundary.** `Commit<'repo>`, `Tree<'repo>`
   etc. are lifetime-bound and cannot be represented in the ABI.
2. **`ThreadSafeRepository` is the stored form.** `gix::Repository` holds
   `Option<RefCell<...>>` so is never `Sync`. `to_thread_local()` per call.
   The `parallel` feature is **non-negotiable** - without it `Repository`
   is not even `Send`.
3. **Paths, ref names, messages cross as BYTES** (`ffi::Vec<u8>` /
   `ffi::Slice<u8>`). Git stores them as `BString`; `ffi::String` would
   UTF-8-validate and reject repositories git itself handles.
4. **Object ids cross as lowercase HEX strings.** ASCII by construction,
   debuggable, 2x size irrelevant next to an FFI crossing.
5. **Small read-once things are records; services only for types with many
   operations or lazy sub-access.** `HeadInfo` and `CommitInfo` are records.
6. **Iterators: materialise ids eagerly, load bodies lazily.** The gix
   iterator borrows the repo and cannot cross. `RevWalk` returns bounded hex
   ids; `CommitInfo` loads one commit per id on demand.
7. **One coarse error type for now** (`GixError`, single-field tuple
   variants). Splitting into per-domain types is a breaking change, so defer
   until there are enough operations to design a real taxonomy.

Pinned gix features (matches what `cargo rustdoc --output-format json` was
validated with): `parallel`, `attributes`, `revision`, `blame`, `merge`,
`status`, `dirwalk`, `index`, `worktree-mutation`, `blocking-network-client`.
Default features stay ON.

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

**`Utf8String` arguments are MOVED, not borrowed.** The marshaller calls
`IntoUnmanaged()`, transferring the pointer and nulling the managed side.
`head.target` is single-use: pass it once and reading `.String` after throws
`ArgumentNullException`. Read values out first, build fresh instances per
call. Main driver for issue `99208883`.

**Everything generated is `IDisposable`** - `Utf8String`, `VecByte`,
`VecUtf8String`, every `Result*` wrapper. `repo.GitDir()` returns a
`VecByte` the caller must dispose.

## Corrections to earlier assumptions

Each was believed and wrong. Recorded so they are not re-derived:

- Payload enums do **not** give free error mapping. Single-field tuple
  variants only; struct variants and multi-field tuples unsupported. gix
  error enums are overwhelmingly struct variants.
- `body_exception_for_variant` does **not** produce per-variant C# exception
  subclasses. `GixError` is a disposable class; `.AsOk()` throws a bare
  `InteropException`.
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

## Next step - pick before expanding

The POC surfaced a blocker for breadth: the generated API cannot be called
directly by consumers, because `Utf8String` arguments are moved on use.

**(a) Managed layer first** - issue `99208883`. `GixRepository` wrapping
`Repo`, exposing `string` / `IReadOnlyList<string>` / `byte[]`, owning every
disposable so none escapes. Mirrors the split `LibGit2.Native` already uses
(internal `Native/Generated`, public managed API). Proves the two-layer
shape on six methods, where it is cheap to get wrong. **Recommended.**

**(b) Facade breadth first** - status, refs, index, diff toward a usable
surface. Reaches something consumers can try sooner, but every method
compounds the move-semantics problem and the managed layer gets rewritten
later against a larger surface.

Either way, expansion is the phase **after** the POC, and the target is
`LibGit2.Native` scope (60-90 managed methods), not full `gix` coverage.

## After that

- Issue `f7bf7635`: `build.rs` asserting the interoptopus patch is active.
- Upstream the interoptopus `validate()` fix (`2a6da76a`) and the `Vec<T>`
  accessors. A PR also tests whether the project is responsive, which is the
  parked fork-or-revive question.
- interoptopus snapshots (`ccb105a2`) - no green baseline means template
  changes cannot be validated.
- Parked, unrelated to the bridge: `gix-ref` reflog creates directories and
  an empty file BEFORE validating the committer, leaving debris on a pure
  validation failure. `gix-ref/src/store/file/loose/reflog.rs` ~119-153.

## Environment

- Windows, `core.autocrlf` on; `gix-ffi/.gitattributes` forces LF for this
  tree.
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
