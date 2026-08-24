# Issues

## gix-ffi build should fail when the local interoptopus patch is not active

```issue
id: f7bf7635
kind: bug
severity: high
status: open
```

### Symptom

Three distinct failures today traced to one cause: `gix-ffi/.cargo/config.toml` is gitignored, so it does not follow into a new transaction worktree. Without it the build silently resolves `interoptopus_csharp` from crates.io rather than the local checkout.

The generated `Vec<T>` then lacks `AsSpan()`/`ToArray()`, and the failure surfaces as an unrelated C# error:

`error CS0411: The type arguments for method 'ImmutableArrayExtensions.ToArray<T>(ImmutableArray<T>)' cannot be inferred`

Nothing in that message mentions interoptopus, cargo, or the missing override.

### Second trap

Restoring the config is not sufficient. `Interop.cs` has by then been regenerated without the accessors, and `cargo build` does not regenerate it - only `cargo test --test generate_bindings` does. The fix needs two steps and the second is easy to miss.

### Proposed fix

A `build.rs` in `gix-ffi` that checks whether `interoptopus_csharp` resolved from a `path` source and fails otherwise with: "local interoptopus patch not active - see gix-ffi/.cargo/config.toml".

Catches it at the right layer, in every worktree and on every machine, instead of as a type-inference error in generated code.
## Hand-written GixSharp managed layer over the generated interop

```issue
id: 99208883
kind: issue
severity: high
status: open
```

### Why

`Utf8String` arguments are MOVED, not borrowed. The generated marshaller calls `IntoUnmanaged()`, transferring the pointer to Rust and nulling the managed side. So this fails:

```csharp
using var head = repo.Head();
using var ids = repo.RevWalk(head.target, 5);
var s = head.target.String;   // ArgumentNullException: Array cannot be null
```

`head.target` is single-use. No C# consumer would expect that of a property, and it was only discovered by running the tests - it compiles fine.

The same applies to every disposable the generator emits: `Utf8String`, `VecByte`, `VecUtf8String`, and each `Result*` wrapper. `repo.GitDir()` hands back a `VecByte` the caller must dispose.

### Shape

A hand-written managed layer over the generated interop, mirroring the split LibGit2.Native already uses (internal `Native/Generated`, public managed API):

- `GixRepository` wrapping `Repo`, exposing `string` / `IReadOnlyList<string>` / `byte[]`
- owns every disposable so none escapes to consumers
- converts at the boundary, so callers never see `Utf8String` or `VecByte`
- likely surfaces errors as a C# union rather than the generated `GixError` class, since `.AsOk()` throws a bare `InteropException` with no per-variant type

### Note

The generated `Interop.cs` stays as-is and internal. This layer is additive; the generator output is never hand-edited.