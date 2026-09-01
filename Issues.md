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
kind: issue
severity: high
status: closed
```

### Scope

Complete the repository lifecycle and discovery surface: `Init`, `Open`, `OpenDiscovered`, `FindWorktreeRoot`, `Discover`, `TryDiscover`, `RepositoryPath`, `WorkingDirectory`, `CommonDirectory`, `IsBare`, `IsWorktree`, `Head`, and disposal/lifetime behavior.

The existing POC already supplied `Open`, `GitDir`, `IsBare`, and `Head`; this issue closes the remaining parity and normalizes their public managed shape.

### Acceptance

Bare repositories, normal worktrees, linked worktrees, nested discovery, nonexistent paths, unborn HEAD, and detached HEAD are covered. Discovery returns owned path bytes in Rust and platform-correct strings in managed code.

### Resolution

Implemented in the repository-core/discovery transaction that closes this issue.

- Rust adds repository initialization, configurable upward discovery, owned `RepositoryInfo`, common/worktree/private paths, bare state, and linked-worktree state.
- Generated `Interop.cs` exposes `Repo.Create`, `Repo.Discover`, `Repo.Info`, and `RepositoryInfo`; it remains generated and unedited by hand.
- `GixRepository` exposes idiomatic managed lifecycle, discovery, path, and repository-state APIs without generated resources in public signatures.
- Tests cover normal and bare initialization, unborn and detached HEAD, linked worktrees, nested discovery, ceiling directories, cross-filesystem option plumbing, missing repositories, and post-disposal guards.
- `cargo test` passes the binding generator plus 3 repository-core integration tests.
- Roslyn/MSBuild reports zero diagnostics and 22/22 TUnit tests pass against the real native DLL.

## Objects and commit graph parity

```issue
id: 2c6f1a02
kind: issue
severity: high
status: closed
```

### Scope

Implement `GitObjectId`, `GitObjectType`, `GitObjectMetadata`, `GitSignature`, `GitCommit`, and `GitCommitSort` equivalents plus `LookupCommit`, both `GetCommitHistory` forms, `GetObjectMetadata`, `CreateCommit`, `GetCommitTreeId`, both `CreateCommitObject` forms, and `IsAncestorOf`.

The existing `RevWalk` and `CommitInfo` POC are inputs to this design, not sufficient parity.

### Acceptance

SHA-1 and repository-native object-format validation, missing/wrong-type objects, parent ordering, signature timestamps/offsets, sort modes, merge commits, and ancestry edge cases are tested.

### Dependencies

Repository core and discovery.

### Resolution

Implemented the complete mapped objects and commit-graph surface in the Rust FFI and idiomatic managed layer. Added object IDs/types/metadata, signatures, commits, sort flags, revision/tag peeling, history with exclusion and limits, tree lookup, index-backed and explicit commit creation, ordered parents, independent author/committer defaults, ref updates, allow-empty behavior, and ancestry (including equal IDs).

Validation evidence before integration:

- Rust: the full `gix-ffi` test suite passes, including 5 dedicated objects/commit-graph tests.
- .NET: the complete `GixSharp.Tests` suite passes, 30/30.
- Generated `Interop.cs` and the managed project build successfully.
- Public managed signatures contain no generated Interoptopus resource types.

## Status parity

```issue
id: 2c6f1a03
kind: issue
severity: high
status: closed
```

### Scope

Implement `GitStatusOptionFlags`, `GitStatusShow`, `GitStatusOptions`, `GitFileStatus`, `GitStatusEntry`, and `GetStatus`.

### Acceptance

Index-only, worktree-only, and combined views cover staged, unstaged, untracked, ignored, renamed, deleted, type-changed, conflicted, recurse-untracked, pathspec, and byte-preserving non-UTF-8 paths where the platform permits them.

### Dependencies

Repository core and discovery.

### Resolution

Implemented the complete mapped status surface in the Rust FFI and idiomatic managed layer. The native boundary returns LibGit2.Native-compatible status bits with raw repository-relative path bytes; the managed API preserves those bytes while exposing decoded paths, staged/worktree convenience properties, default CSharpMpc-compatible options, and explicit pathspecs.

All 16 status option flags have defined handling, including untracked, ignored, unmodified, submodule exclusion, both recursion modes, literal pathspec matching, both rename directions, case sorting, rewrite/copy tracking, index refresh/writeback policy, and unreadable entries. Combined, index-only, and worktree-only results aggregate or filter staged/worktree additions, modifications, deletions, renames, type changes, conflicts, ignored paths, and untracked paths.

Validation evidence before integration:

- Rust: the full `gix-ffi` suite passes, including 6 dedicated status tests and the binding generator.
- .NET: 8/8 dedicated status tests and the complete 38/38 `GixSharp.Tests` suite pass against the real native DLL.
- Roslyn/MSBuild reports zero diagnostics and generated `Interop.cs` builds without hand edits.
- Tests cover the exact option combinations used by all 11 current CSharpMpc status call sites.
- Raw path bytes round-trip through Rust/FFI; non-UTF-8 paths are covered on platforms that permit them.
- Public managed signatures contain no generated Interoptopus resource types.

Implemented and validated with OpenAI Codex assistance.

## Index and conflicts parity

```issue
id: 2c6f1a04
kind: issue
severity: high
status: closed
```

### Scope

Implement `GitIndexEntry`, `Stage`, `Unstage`, `RefreshIndex`, `UpdateIndex`, `GetIndexEntries`, `ResolveConflictAsDeleted`, and `WriteIndexTree`.

### Acceptance

Normal entries, executable/symlink modes, intent-to-add where supported, conflict stages 1/2/3, removals, path bytes, index locking, and tree writing are tested without shelling out to Git.

### Dependencies

Repository core and discovery; object identifiers from objects and commit graph.

### Resolution

Implemented the complete mapped index/conflicts surface in the Rust FFI and idiomatic managed layer.

- Staging uses gitoxide status traversal and the clean-filter pipeline, handles recursive untracked paths, tracked deletion, intent-to-add replacement, ignored paths, gitlinks, filesystem modes, pathspecs, and locked index writes.
- Unstage restores selected paths from HEAD or removes them for an unborn repository without changing the worktree. Update touches tracked paths only, refresh physically reopens the index, and every entry mutation invalidates stale tree-cache data.
- Index enumeration preserves every stage and exact raw Git path bytes while `GitIndexEntry` exposes a decoded `Path`, `Stage`, and cloned `PathBytes`.
- Conflict deletion removes every stage for one path. Tree writing rejects unresolved stages and leaves HEAD, the worktree, and the index unchanged.
- Seven dedicated Rust tests and seven dedicated managed tests cover the mapped behavior and current CSharpMpc consumer shapes without invoking the Git executable. Platform-specific executable, symlink, and non-UTF-8 path coverage runs where supported.
- The binding generator passed and reproduced the exact 203,213-byte `Interop.cs`; the full Rust suite passes (1 generator, 7 index, 5 objects/commit graph, 3 repository core, and 6 status tests).
- Roslyn/MSBuild reports zero diagnostics and the complete 45/45 TUnit suite passes against the real native DLL.
- Public managed signatures contain no generated Interoptopus resource types.

Implemented and validated with OpenAI Codex assistance.

## References and branches parity

```issue
id: 2c6f1a05
kind: issue
severity: high
status: closed
```

### Scope

Implement `GitReferenceInfo`, `GitBranchFilter`, and `GitBranch` equivalents plus `GetReferences`, `GetBranches`, `CreateBranch`, `DeleteBranch`, `SetHead`, `TryGetReferenceTarget`, `TryCreateReference`, `CompareExchangeReference`, `TryDeleteReference`, and `AcquireReferenceLocks`.

### Acceptance

Direct and symbolic refs, packed refs, local/remote branches, invalid names, expected-old-target compare/exchange, reflog messages, deletion races, detached HEAD, and atomic multi-ref lock behavior are tested.

### Dependencies

Repository core and discovery; object identifiers from objects and commit graph.

### Resolution

Closed the bounded reference/branch compatibility slice described above. This does not claim complete gitoxide reference-domain coverage; broader public `gix` coverage remains separately tracked after the boundary-contract work.

- Rust exposes deterministic direct, symbolic, packed, local, and remote-tracking reference enumeration; branch create/force/delete and HEAD changes; exact create, compare/exchange, and delete operations; and an owned multi-reference lock service.
- Reference names remain byte-preserving at the boundary, object IDs remain validated managed values, and invalid names, missing objects, and reference contention translate to distinct managed error kinds.
- Multi-reference locks validate, ordinal-sort, deduplicate, roll back partial acquisition, block cooperative edits, can outlive the repository wrapper, and release through idempotent managed disposal.
- Four focused Rust integration tests and five consumer-shaped TUnit tests cover the mapped behavior without invoking the Git executable. The complete Rust suite, reproducible binding generation, zero-diagnostic managed build, and 50/50 TUnit suite pass against the real native DLL.
- Public managed signatures expose only managed records, value types, collections, and `IDisposable`; generated Interoptopus resources do not escape.

Implemented and validated with OpenAI Codex assistance.

## Diff and tree changes parity

```issue
id: 2c6f1a06
kind: issue
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
kind: issue
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
kind: issue
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
kind: issue
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
kind: issue
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
kind: issue
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
kind: issue
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
kind: issue
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
kind: issue
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
## scripted_fixture_writable is not self-contained for worktree fixtures

```issue
id: 138558d4
kind: bug
severity: high
status: open
```

A linked worktree records an **absolute** path in `.git/worktrees/<id>/gitdir`. `scripted_fixture_writable` copies the generated fixture into a temp directory, but the copied `gitdir` files still point at the original generated location under `tests/fixtures/generated-do-not-edit/`.

So a "writable" copy of a worktree fixture is not self-contained: anything resolved *through* `gitdir` escapes the copy and lands in the shared read-only fixture.

### How it bit

A test induced `CheckoutMissing` by calling `remove_dir_all` on the checkout path reported for `wt-a`. That deleted `wt-a` from the shared generated fixture rather than from the test's own copy, and permanently broke `from_nonbare_parent_repo`, `from_nonbare_parent_repo_set_workdir` and `custom_ref_namespace_created_in_linked_worktree_is_common` for every subsequent run — including runs that made fresh copies, since the source was already damaged.

Recovery required deleting `gix/tests/fixtures/generated-do-not-edit/make_worktree_repo` to force regeneration.

### Why it is dangerous

The failure is silent, delayed, and attributed to the wrong change. Nothing fails at the moment of damage; the next unrelated test run fails instead, in tests the author never touched.

### Workaround in use

Mutate only the administrative directory (`<repo>/.git/worktrees/<id>/...`), which really is inside the copy. Never mutate a path obtained by resolving `gitdir`. To simulate a vanished checkout, rewrite the `gitdir` file to name a path that does not exist rather than deleting the path it currently names.

### Possible fixes

- Have `scripted_fixture_writable` rewrite `gitdir` and `commondir` pointers to the copy after copying, making it genuinely self-contained.
- Or generate worktree fixtures with `git worktree add --relative-paths` (Git >= 2.48), so pointers stay relative and survive copying. `make_worktree_relative_linking.sh` already exercises that form.
- Or, at minimum, document the hazard next to `scripted_fixture_writable`.

Relevant for the upcoming `add` / `remove` / `prune` / `move` / `repair` work, which is mutation testing against exactly this fixture shape.
## Gitignored gix-ffi/.cargo/config.toml makes transaction worktrees silently generate wrong bindings

```issue
id: 0be88a6a
kind: bug
severity: high
status: open
```

`gix-ffi/.cargo/config.toml` carries a `[patch.crates-io]` pointing `interoptopus` and `interoptopus_csharp` at the local checkout. That file is **gitignored**, so `git worktree add` never creates it and every transaction worktree starts without it.

Cargo then resolves `interoptopus` from crates.io, whose generated `Vec<T>` lacks `AsSpan()` / `ToArray()`. Regenerating `Interop.cs` in that state silently reverts the checked-in bindings to a shape the hand-written managed code cannot compile against.

### Why it is dangerous

The Rust side compiles and **all 26 `gix-ffi` tests pass against the bad bindings**. `lib.rs` already warns about this class of problem: the omission surfaces only as a C# compile error. A green Rust suite is not evidence that generated bindings are correct.

Observed symptom when it happened: 22 errors in `GixSharp.Tests` — 18x CS0411 (`ImmutableArrayExtensions.ToArray<T>` cannot infer type arguments) across five `GixRepository.*.cs` files, plus 4x CS9135 (`A constant value of type 'FfiObjectType' is expected`), none of them in lines that had been edited.

It also silently re-resolved `gix-ffi/Cargo.lock`, bumping unrelated transitive dependencies, because the missing patch invalidated the recorded resolution.

### Contributing factor

The file's own comment documents this exact failure mode, and even explains that absolute paths are required *because transaction worktrees live outside the directory tree*. The hazard was understood; nothing makes the file present.

### Possible fixes

- Add `<Error Condition="!Exists('$(GixFfiCrateDir)/.cargo/config.toml')">` to the `BuildGixFfi` target in `GixSharp.csproj`, gated on `SkipCargoBuild` like the `cargo build` invocation beside it. `StageGixFfi` already uses this idiom for a missing native library. Converts an invisible failure into a loud one.
- Or check in a template and copy it on first build.
- Or have the worktree tooling copy machine-local gitignored config.

### Related

`gix-ffi` currently cannot be built correctly by anyone without a local `interoptopus` checkout: crates.io 0.16.4 is genuinely insufficient, not merely stale. That reproducibility constraint deserves recording independently of the worktree problem.
## CRLF committed in fork-touched core files guarantees whole-file conflicts on upstream merges

```issue
id: af7e3693
kind: bug
severity: medium
status: open
```

`git ls-files --eol` reports `i/crlf` for fork-touched core files, including `gix/Cargo.toml`, `gix/tests/gix/repository/mod.rs` and `gix/src/worktree/mod.rs`. Upstream keeps these as LF, so every line differs and Git cannot produce hunks.

### How it bit

Merging 348 upstream commits produced exactly two conflicts, and both were whole-file: each side spanned the entire file and both sides ended with identical content. Re-running the merge with `-Xignore-cr-at-eol` reduced it to a single five-line hunk of stale dependency versions.

### Why it will get worse

Only two files conflicted because only two had been touched by both sides. As the fork edits more core files, each becomes a guaranteed whole-file conflict against every future upstream merge. The cost grows with the fork's footprint.

### Fix

Repo-level `.gitattributes` with `* text=auto eol=lf`, `core.autocrlf=false` locally, then `git add --renormalize .`. The pattern already exists in `gix-ffi/.gitattributes`, and commit `d778a55e7` normalised that crate to LF, so the approach is established — it was simply never applied repo-wide.

Note a root `.gitattributes` already exists; check what it covers before adding rules.

### Caveat

Renormalisation rewrites many files in one commit, so it should be its own change with nothing else in it, landed when no other work is in flight.
## No guarded symbolic-ref write: compare_exchange_reference is object-only and set_head is unconditional

```issue
id: b660e063
kind: issue
severity: medium
status: open
```

Two gaps found while surveying the ref layer for worktree administration. Neither is a defect in what exists; both are missing capability that `worktree add` will need.

### No compare-and-swap for symbolic references

`gix-ffi`'s `compare_exchange_reference` builds `PreviousValue::MustExistAndMatch(Target::Object(expected))`, so it can only guard an object target. There is no way to say "point `HEAD` at `refs/heads/x`, but only if it currently points at `refs/heads/y`".

`gix-ref` itself models symbolic targets in `PreviousValue`, so the limit is in the FFI surface rather than the underlying store — worth confirming before designing around it.

### `set_head` is unconditional

`gix-ffi`'s `set_head` writes with `PreviousValue::Any`, so it always wins. That is correct for `git symbolic-ref HEAD <ref>` semantics, but it means a caller cannot detach or re-attach a worktree's `HEAD` safely against concurrent modification.

### Why it matters here

`git worktree add` writes a `HEAD` into the new administrative directory, and the resumable-materialisation path re-attaches an existing worktree's `HEAD` to a specific branch at a specific tip. Both want a guarded symbolic-ref write: unconditional writes make "finish what was interrupted" indistinguishable from "clobber what someone else did".

Not blocking the read model, but decide before implementing `add`.
## ReferenceLockLease has no stale-lock recovery: a crashed holder wedges its transaction forever

```issue
id: b66f8f9c
kind: issue
severity: high
status: open
```

`gix-ffi/src/references.rs` implements the cooperative lock as a real Git lock file: `ReferenceLockLease { _markers: Vec<gix::lock::Marker> }`, acquired over a set of ref names and held for the lease's lifetime.

`gix::lock::Marker` releases on `Drop`. A process killed while holding the lease leaves `<ref>.lock` on disk with no owner, no timestamp and no expiry, and nothing will ever remove it. The affected transaction is then permanently unable to acquire its own lock.

### Why the design is otherwise sound

The lock is taken on the transaction's `_created` marker ref, which the lifecycle publishes once and never modifies. No ordinary Git operation contends for it, so holding a physical lock across validation and mutation blocks nothing but other cooperating clients — which is the intent. The objection that a physical ref lock is the wrong mechanism does not hold for *this* ref. It would hold for a branch.

### What is missing

A stale-lock policy. Git's own lock files have the same property, and Git's answer is that operations are short. This lease is deliberately long, so it needs something Git does not provide:

- an age-based break, requiring a timestamp the lock file does not currently carry; or
- an owner record (pid plus host or boot id) so a dead holder can be distinguished from a live one; or
- an explicit break-lock operation and a documented human procedure.

### Undecided

Whether this belongs in the consumer, as an age-based break over the file it already knows the path of, or in `gix-lock` as a supported break operation. Recorded here so the choice is made deliberately rather than discovered after a crash.

### Related

The lock path is now computed via `gix-ref`'s `reference_path_with_base`, so it correctly follows commondir-versus-worktree routing including namespaces.
## Stale State::is_sparse doc comment, and tree-extension ordering blocks sparse-index byte parity

```issue
id: b3fc59d9
kind: bug
severity: low
status: open
```

Two smaller findings from the same survey, both in sparse-index territory.

### `State::is_sparse` doc comment is stale

The doc says an index is sparse if it *contains at least one `Mode::DIR` entry*. That stopped being the whole truth once `decode/mod.rs` began ORing in the `sdir` extension marker:

```rust
is_sparse |= is_sparse_from_ext; // a marker is needed in case there are no directories
```

A sparse-marked index with no collapsed directories is now sparse, and correctly round-trips its `sdir` marker. The comment describes the old entry-derived rule only.

This is not cosmetic: reading it led directly to a wrong diagnosis of the `v2_sparse_index_no_dirs` fixture, on the assumption the marker was being lost. It was not — the fixture's `TODO` had simply outlived its fix, and re-enabling it passed unchanged.

### `roundtrips_sparse_index` cannot compare raw bytes

`gix-index/tests/index/file/write.rs` keeps `compare_raw_bytes` commented out for the sparse round-trip, because Git orders tree-extension entries differently from gitoxide. State comparison passes; byte comparison is untested.

Distinct from the `sdir` question, and still open. Byte-level parity is the stronger guarantee, and worth having for anything that writes an index Git will later read.
## Transaction worktrees materialise 137 LICENSE symlinks as plain text files

```issue
id: a1e4c9be
kind: bug
severity: low
status: open
```

Every `LICENSE-APACHE` and `LICENSE-MIT` in the repository — 137 files — shows as `typechange` in a transaction worktree: `deleted file mode 120000` / `new file mode 100644`, with an **unchanged blob hash**.

Gitoxide stores per-crate licence files as symlinks to the two real files at the repository root. In a transaction worktree they are materialised as ordinary one-line text files whose content is the relative path, e.g. `../LICENSE-MIT`.

### Cause

`core.symlinks` is `true` in both the main checkout and the worktree, so this is not configuration. The process that ran `git worktree add` lacked the Windows privilege to create symlinks (`SeCreateSymbolicLink`, granted by Developer Mode), and Git silently fell back to writing plain files.

### Impact

Two effects, one benign and one not.

Benign: the transaction tooling stages only known-modified paths, so the typechanges never reach a commit. Verified — the checkpoint and the full branch diff both show only the intended files. If a future tool staged everything, it would convert 137 symlinks to text files in the fork's history, and checkouts on Linux or macOS would then carry no licence text at all.

Not benign: anything that *reads* licence text from inside a transaction worktree sees the string `../LICENSE-MIT` rather than a licence. Licence scanners, packaging steps and `cargo package` would all be misled.

### Possible fixes

- Enable Developer Mode, or grant the privilege to whatever launches the worktree tooling.
- Or have the worktree tooling detect the fallback and warn, since the failure is silent today.
- Or verify whether `cargo package` / `cargo publish` from a transaction worktree would embed the wrong content, and if so refuse to publish from one.