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
status: closed
```

### Why

`Utf8String` arguments are moved, not borrowed. The generated marshaller calls `IntoUnmanaged()`, transferring the pointer to Rust and nulling the managed side. The generated API therefore cannot be the public consumer API.

### Resolution

Implemented and integrated in commit `dfa43c48642fd3cf5c31d0480ae2b508d5692814`.

The additive managed layer now provides:

- `GixRepository` over the generated `Repo`
- ordinary `string`, `byte[]`, and `IReadOnlyList<string>` boundaries
- value records `GixHead` and `GixCommitInfo`
- `GixException` with the originating `GixErrorCode`
- ownership of every generated disposable inside the wrapper

The generated `Interop.cs` remains generated and unedited. Generated types do not escape the managed wrapper signatures.

The Rust generation chain, Roslyn build, and 15 TUnit tests pass against the real native DLL and a real repository.

This closes only the managed-layer proof of concept. The exposed Rust facade is still six methods and is not a usable binding.

# Binding expansion roadmap

## Completion contract

The target is the complete public managed surface of `LibGit2.Native`, not every public function in `gix`. The current six-method Rust facade is approximately 1% of that target.

Each domain below is a separate issue and must be implemented in a separate transaction. A module issue is complete only when its transaction contains:

- the Rust FFI facade records, enums, and methods for that domain
- the idiomatic GixSharp managed API, with no generated disposable escaping
- generation coverage proving `Interop.cs` is reproducible and never hand-edited
- Rust tests plus TUnit tests against real repositories
- issue status and evidence updated in the same commit

The binding is complete only after every module issue is closed and the final consumer-conformance issue passes.

## Sequencing evidence

Production call-site matches in `CSharpMpc/src` were counted programmatically to rank consumer value. Dependency order can move a prerequisite ahead of a higher count.

| Domain | Call-site matches | Production files |
|---|---:|---:|
| Objects and commit graph | 14 | 3 |
| Status | 11 | 7 |
| Repository discovery/core | 10 | 8 |
| Index | 10 | 4 |
| References and branches | 7 | 4 |
| Diff and tree changes | 4 | 3 |
| Ignore/local exclude | 2 | 1 |
| Merge/checkout | 1 | 1 |
| Remotes | 1 | 1 |
| Worktrees | 0 | 0 |
| Configuration | 0 | 0 |
| Notes | 0 | 0 |
| Sparse checkout | 0 | 0 |

Zero current call sites do not remove a module from the parity target.

## Repository core and discovery parity

```issue
id: 2c6f1a01
kind: feature
severity: high
status: open
```

### Scope

Complete the repository lifecycle and discovery surface: `Init`, `Open`, `OpenDiscovered`, `FindWorktreeRoot`, `Discover`, `TryDiscover`, `RepositoryPath`, `WorkingDirectory`, `CommonDirectory`, `IsBare`, `IsWorktree`, `Head`, and disposal/lifetime behavior.

The existing POC already supplies `Open`, `GitDir`, `IsBare`, and `Head`; this issue closes the remaining parity and normalizes their public managed shape.

### Acceptance

Bare repositories, normal worktrees, linked worktrees, nested discovery, nonexistent paths, unborn HEAD, and detached HEAD are covered. Discovery returns owned path bytes in Rust and platform-correct strings in managed code.

### Order

First. It is the prerequisite for all repository-scoped modules.

## Objects and commit graph parity

```issue
id: 2c6f1a02
kind: feature
severity: high
status: open
```

### Scope

Implement `GitObjectId`, `GitObjectType`, `GitObjectMetadata`, `GitSignature`, `GitCommit`, and `GitCommitSort` equivalents plus `LookupCommit`, both `GetCommitHistory` forms, `GetObjectMetadata`, `CreateCommit`, `GetCommitTreeId`, both `CreateCommitObject` forms, and `IsAncestorOf`.

The existing `RevWalk` and `CommitInfo` POC are inputs to this design, not sufficient parity.

### Acceptance

SHA-1 and repository-native object-format validation, missing/wrong-type objects, parent ordering, signature timestamps/offsets, sort modes, merge commits, and ancestry edge cases are tested.

### Dependencies

Repository core and discovery.

## Status parity

```issue
id: 2c6f1a03
kind: feature
severity: high
status: open
```

### Scope

Implement `GitStatusOptionFlags`, `GitStatusShow`, `GitStatusOptions`, `GitFileStatus`, `GitStatusEntry`, and `GetStatus`.

### Acceptance

Index-only, worktree-only, and combined views cover staged, unstaged, untracked, ignored, renamed, deleted, type-changed, conflicted, recurse-untracked, pathspec, and byte-preserving non-UTF-8 paths where the platform permits them.

### Dependencies

Repository core and discovery.

## Index and conflicts parity

```issue
id: 2c6f1a04
kind: feature
severity: high
status: open
```

### Scope

Implement `GitIndexEntry`, `Stage`, `Unstage`, `RefreshIndex`, `UpdateIndex`, `GetIndexEntries`, `ResolveConflictAsDeleted`, and `WriteIndexTree`.

### Acceptance

Normal entries, executable/symlink modes, intent-to-add where supported, conflict stages 1/2/3, removals, path bytes, index locking, and tree writing are tested without shelling out to Git.

### Dependencies

Repository core and discovery; object identifiers from objects and commit graph.

## References and branches parity

```issue
id: 2c6f1a05
kind: feature
severity: high
status: open
```

### Scope

Implement `GitReferenceInfo`, `GitBranchFilter`, and `GitBranch` equivalents plus `GetReferences`, `GetBranches`, `CreateBranch`, `DeleteBranch`, `SetHead`, `TryGetReferenceTarget`, `TryCreateReference`, `CompareExchangeReference`, `TryDeleteReference`, and `AcquireReferenceLocks`.

### Acceptance

Direct and symbolic refs, packed refs, local/remote branches, invalid names, expected-old-target compare/exchange, reflog messages, deletion races, detached HEAD, and atomic multi-ref lock behavior are tested.

### Dependencies

Repository core and discovery; object identifiers from objects and commit graph.

## Diff and tree changes parity

```issue
id: 2c6f1a06
kind: feature
severity: high
status: open
```

### Scope

Implement `GitDiffTarget`, `GitDiffResult`, `GitTreeChangeKind`, `GitFileMode`, and `GitTreeChange` equivalents plus `GetDiff`, `GetPatch`, and `GetTreeChanges`.

### Acceptance

Index/worktree/tree/commit comparisons cover additions, deletions, modifications, renames, copies where supported, binary files, mode changes, submodules, path filters, patch text, and deterministic ordering.

### Dependencies

Repository core and discovery; objects and commit graph; index and conflicts.

## Ignore and local exclude parity

```issue
id: 2c6f1a07
kind: feature
severity: medium
status: open
```

### Scope

Implement `IsPathIgnored` and `EnsureLocalExclude`, including repository, info/exclude, global excludes, negation, directory patterns, and idempotent writes.

### Acceptance

Precedence matches Git behavior, exact path bytes are preserved, duplicate rules are not appended, and linked-worktree common-directory behavior is covered.

### Dependencies

Repository core and discovery.

## Merge, snapshot, and checkout parity

```issue
id: 2c6f1a08
kind: feature
severity: high
status: open
```

### Scope

Implement `SnapshotWorkingTree`, `MergeCommitsToTree`, `MergeTreesToTree`, `MergeCommitsIntoWorkingTree`, `MergeTreesIntoWorkingTree`, and `CheckoutTreeSafely`.

### Acceptance

Clean merges, content conflicts, rename/delete cases, merge-base selection, conflict records, dirty-worktree protection, untracked-file protection, index/worktree atomicity, and rollback-on-error are tested.

### Dependencies

Objects and commit graph; index and conflicts; references and branches; diff and tree changes.

## Remotes parity

```issue
id: 2c6f1a09
kind: feature
severity: medium
status: open
```

### Scope

Implement `GitRemote` and `GetRemotes`, including fetch and push URLs and multiple URL entries.

### Acceptance

Missing URLs, insteadOf/pushInsteadOf resolution where the managed contract requires it, non-UTF-8 configuration bytes, and deterministic remote ordering are covered.

### Dependencies

Repository core and discovery; configuration.

## Worktrees parity

```issue
id: 2c6f1a0a
kind: feature
severity: medium
status: open
```

### Scope

Implement `GitWorktreeInfo`, `GetWorktrees`, both `AddWorktree` forms, `AddDetachedWorktree`, `PruneWorktrees`, and `PruneWorktree`.

### Acceptance

Main and linked worktrees, branch-attached and detached creation, locked/prunable state, stale metadata, common-directory paths, duplicate branch protection, and safe pruning are tested.

### Dependencies

Repository core and discovery; references and branches; guarded checkout behavior.

## Configuration parity

```issue
id: 2c6f1a0b
kind: feature
severity: medium
status: open
```

### Scope

Implement `GetConfigString`, `TryGetConfigString`, `SetConfigString`, and `DeleteConfigValue` with the same repository/local configuration contract as `LibGit2.Native`.

### Acceptance

Missing keys, repeated keys, include/includeIf resolution, value encoding, write locking, deletion, and linked-worktree configuration location are tested.

### Dependencies

Repository core and discovery.

## Notes parity

```issue
id: 2c6f1a0c
kind: feature
severity: medium
status: open
```

### Scope

Implement `GitNote`, `GitNoteEntry`, `WriteNote`, `ReadNote`, `TryReadNote`, `EnumerateNotes`, and `RemoveNote`.

### Acceptance

Default and custom notes refs, overwrite behavior, author/committer signatures, missing notes, enumeration, deletion, and notes-ref updates are tested.

### Dependencies

Objects and commit graph; references and branches.

## Sparse checkout parity

```issue
id: 2c6f1a0d
kind: feature
severity: medium
status: open
```

### Scope

Implement `SetSparseCheckout`, including enable, update, disable, pattern persistence, index flags, and worktree materialization.

### Acceptance

Cone and non-cone behavior supported by the chosen gix APIs, empty patterns, ignored files, dirty-file safety, and linked worktrees are covered. Any deliberate contract difference must be documented and locked by tests.

### Dependencies

Repository core and discovery; index and conflicts; guarded checkout behavior.

## Runtime, errors, generation, and package parity

```issue
id: 2c6f1a0e
kind: feature
severity: high
status: open
```

### Scope

Complete `GitRuntime` and `GitException` parity: `GetOwnerValidation`, `SetOwnerValidation`, `Version`, structured operation/error classification, native library loading, RID assets, deterministic binding generation, NuGet packing, and public API visibility.

Generated interop remains an implementation detail; the managed API must not require generated handles or disposables.

### Acceptance

Windows, Linux, and macOS package layouts are validated for supported architectures; native-load failures are actionable; generated output is reproducible; error variants remain distinguishable; trim/AOT constraints are documented and tested where supported.

### Dependencies

Can progress incrementally, but closes after all functional modules so it captures their complete inventory.

## Full parity and CSharpMpc consumer migration

```issue
id: 2c6f1a0f
kind: issue
severity: high
status: open
```

### Scope

Create a machine-readable public-member parity inventory, replace the `LibGit2.Native` package in `CSharpMpc` with GixSharp, and run the real consumer suite.

This is the completion gate, not a place to add missing APIs. Any gap found here reopens or creates the appropriate domain issue and is fixed in its own transaction.

### Acceptance

Every public managed `LibGit2.Native` member is mapped to an equivalent GixSharp member or an explicitly approved incompatibility. All Git-related CSharpMpc code compiles without compatibility shims leaking generated interop, its full tests pass, package consumption works from a clean restore, and the end-to-end chain is documented:

`gix -> gix-ffi -> generated Interop.cs -> GixSharp -> CSharpMpc`.

### Dependencies

All module issues above.
