# gix-ffi / GixSharp — Handoff

Written 2026-08-24. State as of `mstaros/gitoxide` @ `d778a55e7`.

## What this is

A .NET binding over gitoxide (`gix`), replacing a libgit2-based binding
(`LibGit2.Native`) that is being abandoned because libgit2 upstream is
effectively dead and its features lag.

Toolchain, now proven end to end:

```
gix (path dep, this repo)
  -> gix-ffi          Rust facade, #[ffi] annotated
  -> interoptopus     generates C# from the facade inventory
  -> GixSharp         .NET class library, packable
  -> GixSharp.Tests   TUnit, calls the real DLL against a real repository
```

No cbindgen, no ClangSharp, no hand-written P/Invoke.

## Current state — working

7 TUnit tests pass, exercising a loaded native DLL against this repository.

Exposed on `Repo`: `Open`, `GitDir`, `IsBare`, `Head`, `RevWalk`, `CommitInfo`.

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
  src/lib.rs            the facade
  tests/generate_bindings.rs   generates Interop.cs (a test, not build.rs)
  bindings/
    Interop.cs          GENERATED, committed, namespace GixSharp
    GixSharp/           packable class library, builds the cdylib
    GixSharp.Tests/     TUnit
```

`gix-ffi` is a **nested independent cargo workspace** on purpose. As a
gitoxide workspace member, `cargo test --workspace` would unify `gix`
feature selection with `gitoxide-core`, which carries a `compile_error!`
for `blocking-client` + `async-client` together.

## Design rules — decided, validated by execution

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

Pinned gix features (matches what `cargo rustdoc --output-format json`
was validated with): `parallel`, `attributes`, `revision`, `blame`,
`merge`, `status`, `dirwalk`, `index`, `worktree-mutation`,
`blocking-network-client`. Default features stay ON.

## Traps — all of these cost real time

**`.cargo/config.toml` is gitignored and does NOT follow into a new
worktree.** Without it the build silently resolves `interoptopus_csharp`
from crates.io instead of the local checkout, the generated `Vec<T>` loses
`AsSpan()`/`ToArray()`, and the failure appears as
`CS0411 ImmutableArrayExtensions.ToArray<T> cannot be inferred` - naming
nothing relevant. **Every new worktree needs a copy.** Tracked as
gitoxide issue `f7bf7635`.

**Fixing the config is not sufficient.** `Interop.cs` will already have
been regenerated wrong, and `cargo build` does NOT regenerate it - only
`cargo test --test generate_bindings` does.

**Every `ffi::Vec<T>` needs its own `builtins_vec!(T)`.** Without it the
backend emits *references* to `VecByte` / `VecUtf8String` but never the
type. Rust compiles clean; only `csc` catches it. Bit twice. Note the
macro takes the element type, unlike `builtins_string!()`. Tracked as
interoptopus issue `2a6da76a`.

**`Utf8String` arguments are MOVED, not borrowed.** The marshaller calls
`IntoUnmanaged()`, transferring the pointer and nulling the managed side.
So `head.target` is single-use: pass it once and reading `.String` after
throws `ArgumentNullException`. Read values out first, build fresh
instances per call. This is the main driver for issue `99208883`.

**Everything generated is `IDisposable`** - `Utf8String`, `VecByte`,
`VecUtf8String`, every `Result*` wrapper. `repo.GitDir()` returns a
`VecByte` the caller must dispose.

## Corrections to earlier assumptions

Recorded because each was believed and wrong:

- Payload enums do **not** give free error mapping. Single-field tuple
  variants only; struct variants and multi-field tuples are unsupported.
  gix error enums are overwhelmingly struct variants.
- `body_exception_for_variant` does **not** produce per-variant C#
  exception subclasses. `GixError` is a disposable class; `.AsOk()` throws
  a bare `InteropException`.
- Binding generation is a **test**, not `build.rs` - a build script cannot
  call into the crate it is building.
- `interoptopus_csharp::Interop` does not exist; it is `RustLibrary`.
  CLAUDE.md in that repo is stale on this.
- gix handles linked worktrees correctly - `git_dir()` resolves to
  `<main>/.git/worktrees/<name>`, which does NOT end in `.git`.
- The 138 LICENSE files showing as modified in transaction worktrees are
  `core.autocrlf`, not symlinks and not LFS.

## Next step

Issue `99208883`: hand-written managed layer over the generated interop.

`GixRepository` wrapping `Repo`, exposing `string` / `IReadOnlyList<string>`
/ `byte[]`, owning every disposable so none escapes. Mirrors the split
`LibGit2.Native` already uses (internal `Native/Generated`, public managed
API). The `Utf8String` move semantics make this necessary rather than
merely nice - the generated API cannot be consumed directly.

Generated `Interop.cs` stays internal and is never hand-edited.

## After that

- Issue `f7bf7635`: `build.rs` asserting the interoptopus patch is active.
- Upstream the interoptopus `validate()` fix (`2a6da76a`) and the
  `Vec<T>` accessors. Divergence from upstream is only 2 commits, so
  reviving upstream is realistic; a PR also tests whether the project is
  responsive.
- interoptopus snapshots (`ccb105a2`) - no green baseline means template
  changes cannot be validated.
- Parked, unrelated to the bridge: `gix-ref` reflog writes an empty file
  and creates directories BEFORE validating the committer, leaving debris
  on a pure validation failure. `gix-ref/src/store/file/loose/reflog.rs`
  around lines 119-153.

## Environment

- Windows, `core.autocrlf` on; `gix-ffi/.gitattributes` forces LF for the
  generated tree.
- `core.longpaths true` is set; gitoxide's own fixtures still hit MAX_PATH
  in long transaction worktrees.
- interoptopus 0.16.4, local checkout at
  `C:/Users/mstar/source/repos/interoptopus`, fork `mstaros/interoptopus`.
- .NET 11 preview, `LangVersion=preview`, `EnablePreviewFeatures`,
  `runtime-async=on`, TUnit 1.36.0 - matching `CSharpMpc.Server`.

## Commands

```powershell
# regenerate bindings (also the ONLY way to refresh Interop.cs)
cd C:\Users\mstar\source\repos\gitoxide\gix-ffi
cargo test --test generate_bindings

# build + run the C# tests (dotnet build drives cargo build)
cd bindings\GixSharp.Tests
dotnet run
```

A clean `git diff --stat bindings/Interop.cs` after regenerating is the
check that the local interoptopus patch is active.
