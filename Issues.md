# Issues

## gix-ffi build should fail when the local interoptopus patch is not active

```issue
id: f7bf7635
kind: bug
severity: high
status: closed
```

- [x] Resolved on 2026-09-04 by the registry dependency change `c68796bb`: the manifest now selects uniquely named `_mps` packages from registry `local`. No local crates.io patch or additional build.rs guard is required to prevent upstream substitution. Historical failure and proposal follow.

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

## Two integrated commits were validated without gix-ffi being compiled

Severity: medium, historical. The planning gap that caused this has since been addressed in `rust-mcp-transform`; what remains is the unreliable evidence those two commits left behind.

### What happened

`gix-ffi/Cargo.toml` declares its own empty `[workspace]`, deliberately: joining the gitoxide workspace would unify `gix` feature selection with `gitoxide-core` and the `gix` CLI, which carry a `compile_error!` when `blocking-network-client` and `async-network-client` are both active. That separation is necessary and should stay.

At the time, validation planning ran a single `cargo metadata` at the repository root. A package that declares its own workspace does not appear in that answer, so `gix-ffi` was never enumerated and no step could compile it. An integrated commit logged `metadata_package_count: 12`, `selected_package_count: 11`, with the packages listed by name.

Worse, both integrated transactions that changed `gix-ffi` reported `uncovered: 0`:

- `34ce63c0` to candidate `360a8d75`: 20 files changed, `passed: 2`, `uncovered: 0`
- `5dd532a4` to candidate `0f2a4f04`: 2 files changed, `passed: 2`, `uncovered: 0`

The counter was working - an earlier failed attempt in the first of those transactions reported `uncovered: 11` across 15 files. It reported none for these. So the changes were not merely unchecked; they were counted as covered.

### Since superseded

`rust-mcp-transform` now has multi-root discovery aimed squarely at this case. `discover_cargo_roots` walks the repository for manifests and runs one `cargo metadata` per workspace, skipping members an earlier probe already claimed. `CargoUnionGraph` spans every discovered workspace with reverse dependencies resolved across workspace boundaries, and its `owner_for_path` documents the intent directly: the innermost package root wins, which is how a crate excluded from an enclosing workspace still resolves to itself. That is `gix-ffi`.

### What is still owed

- [x] Confirmed on 2026-09-05: commit `22c1da145e895e0f57768cdbffec6ed37df6161b`, operation `8ca35416ea509e2619aaa5a57668e16d`, explicitly compiled the nested `gix-ffi` workspace and ran 41/41 native tests. This supplies current evidence; it does not retroactively validate the two historical commits.
- Treat the two commits above as unvalidated for `gix-ffi` regardless of what their evidence says.
- `plan_workspace_selections` skips changes that no package owns. That is correct for `Issues.md`, `Interop.cs` and other non-Cargo inputs, and the code says the claiming decision belongs to the provider. Confirm what the provider does with them, because an unowned path silently counting as covered is the same shape as the failure above.

### Working practice meanwhile

`gix-ffi` is its own workspace root, so validate from inside it and build the C# tests separately:

```
cd gix-ffi && cargo clippy --workspace --all-targets && cargo nextest run --workspace
dotnet build gix-ffi/bindings/GixSharp.Tests/GixSharp.Tests.csproj
```
## GixError to GixErrorKind mapping is not exhaustive, so new variants degrade to Other

- [x] Replace property-pattern/catch-all translation with exhaustive generated case-type matching. Transaction `691180980e389df06166ef8f` handles all ten cases and explicit null; CS8509 is an error. Removing ReferenceLockedCase in an in-memory compiler probe reports the missing case. Managed tests cover every mapping, owned messages and null errors. The diagnosis below is historical.

Severity: medium. `GixRepository.cs` maps `GixError` to `GixErrorKind` with property patterns and a catch-all:

```csharp
{ IsReferenceConflict: true } =>
    (GixErrorKind.ReferenceConflict, error.AsReferenceConflict().String),
{ IsOther: true } =>
    (GixErrorKind.Other, error.AsOther().String),
_ =>
    (GixErrorKind.Other, error.ToString()),
```

That shape can never be exhaustive. Adding a Rust variant compiles cleanly on the C# side and degrades silently to `Other`.

It happened. Adding `GixError::ReferenceLocked` left `GixErrorKind` at nine members against ten generated variants, with no compile error and no warning. Nothing reported the drift. It surfaced only because one test asserted a specific `Kind` rather than merely that an error occurred, and the assertion then failed for a reason unrelated to what the test was about.

### Why this one is fixable now

`GixError` is union-projected. `Interop.cs` already emits real case types - `ReferenceConflictCase`, `ReferenceLockedCase`, `OtherCase` - each a `readonly record struct`, with `TryGetValue` overloads per case.

Switching on those case types instead of `Is*` properties makes exhaustiveness a compiler question. A new Rust variant then breaks the build at the mapper, which is the correct moment to notice.

This does not need the analyzer filed separately for plain enums, and it does not need closed enums. The union support is already there and already generated.

### Scope

The same catch-all shape should be looked for anywhere else the managed layer consumes a generated union. This entry covers the error mapper only because that is where it was found.

### Related

The general problem - a hand-written managed layer that does not track generated changes and has nothing enforcing it - is already filed under "Hand-written GixSharp managed layer over the generated interop". This is the first concrete instance with a demonstrated cost.
## C# enum switches are not exhaustive, so outcome enums need an analyzer until closed enums land

- [ ] Enforce the documented plain-outcome-enum coverage rule; keep this separate from union case matching.

Severity: medium. `ReferenceUpdateOutcome` is payload-free, so interoptopus projects it as a plain C# `enum` - `public enum ReferenceUpdateOutcome : byte`, crossing as `ManagedConversion::AsIs` with no marshaller. That is the cheapest possible wire shape and the right one.

The cost is that a C# `enum` switch is not exhaustive. Any integer can be cast to an enum value, so the compiler cannot require a caller to handle `Absent`. The distinction the outcome exists to express can be dropped again at the call site with no diagnostic.

This is not the same problem as the error mapper. That one switches on a union with real case types and can be made a compile error today. This one cannot, because the wire type is deliberately a plain enum.

### Deliverable

An analyzer that requires exhaustive handling of the outcome enums, reported as an error rather than a warning.

It needs a rule for which enums it polices - an attribute, a naming convention, or an explicit list. Applying it to every C# enum in the binding would be wrong; applying it only to types projected from payload-free Rust enums is the intent.

### Lifetime

This is deliberately temporary. Closed enums are expected in C# within months and make the check native. The analyzer should be written so it can be deleted rather than migrated: no configuration surface, no suppressions file, one rule.

### Why the alternative was rejected

Wrapping the enum in a hand-written C# union in the managed facade would give exhaustiveness immediately, since union switches are exhaustive without a fallback. It was rejected because the union layer is work that closed enums make redundant, and because union declarations box - they lower to a struct holding `object? Value`, so every returned outcome allocates.

If the analyzer turns out to be more work than the union wrapper, that trade is worth revisiting. It was decided on the assumption that the analyzer is small.
## add_worktree derives its identifier non-atomically; git uses mkdir/EEXIST

- [x] Atomic reservation implemented and integrated on 2026-09-05 in `0cff314e65e696a11ea37879b1919ada6ae56f15`. The diagnosis below is historical.

Severity: medium, resolved. The original `add_worktree` implementation derived the administrative identifier from the checkout's final component and disambiguated against the entries it had read a moment earlier (`unused_worktree_id`). Reading and then creating was not atomic, so two concurrent calls could derive the same unused identifier and both proceed.

### Git already solves this, and the fix is the same shape

`builtin/worktree.c:507-514`:

```c
while (mkdir(sb_repo.buf, 0777)) {
    counter++;
    if ((errno != EEXIST) || !counter /* overflow */)
        die_errno(_("could not create directory of '%s'"), sb_repo.buf);
    strbuf_setlen(&sb_repo, len);
    strbuf_addf(&sb_repo, "%d", counter);
}
```

The filesystem decides. `mkdir` either creates the directory or reports `EEXIST`, and on `EEXIST` the loser appends a counter and retries. There is no window between checking and creating because there is no check.

### What this needs here

`create_dir` rather than `create_dir_all` for the administrative directory, so an existing directory is an error rather than a silent success, and a retry loop around identifier derivation.

One wrinkle Git does not have to think about and this does: a crashed `add_worktree` leaves a registration directory that nothing owns, and under `create_dir` semantics that identifier is then permanently lost to the counter. The machinery to handle it already exists - `worktree_admin_entries()` classifies such a directory as `MissingGitdir`, and `prune_worktrees()` already removes those subject to an expiry. So on collision, consult the entries: if the winner is `MissingGitdir` and older than the caller's threshold, prune it and retry once. That reuses the existing staleness policy rather than inventing a second one.

### Related

The same read-then-act shape is what `ReferenceLockLease` suffers from, filed separately. Whatever abandonment threshold that settles on should be the one used here.
## CRLF normalisation: what was fixed, and what must not be

Resolved for the files that mattered. Recorded because the obvious next step is wrong.

### What was actually wrong

`git ls-files --eol` found thirteen tracked files whose index line endings were CRLF while upstream `v0.58.0` keeps them at LF:

- `gix-index/src/{lib,access/mod,file/init,file/mod}.rs`
- `gix/src/config/tree/sections/{core,index}.rs`
- `gix/src/repository/{index,mod,sparse_checkout/mod}.rs`
- `gix/src/worktree/mod.rs`
- `gix/tests/gix/repository/{mod,sparse_checkout}.rs`
- `gix/Cargo.toml`

Three of those - `gix/Cargo.toml`, `gix/src/repository/mod.rs`, `gix/tests/gix/repository/mod.rs` - were *mixed*, CRLF lines appended to an LF file, which only happens by editing.

All sit in the sparse-index integration this fork added. Confirmed LF upstream with `git grep -c --perl-regexp "\r" v0.58.0 -- gix/src gix-index/src gix/tests gix/Cargo.toml`, which matched only `.tar` fixtures.

### Do not normalise .gov/accounting.csv

It is upstream's own sponsor-payment ledger, CRLF in upstream and CRLF here, and this fork has never touched it. Since the point of normalising is to *reduce* divergence from upstream, rewriting a file that upstream keeps as CRLF would manufacture the very conflict being removed.

A blanket `* text=auto eol=lf` plus `git add --renormalize .` - the approach the original handoff proposed - would have rewritten it silently along with everything else. That is why `.gitattributes` now carries extension-scoped rules for `*.rs`, `*.toml` and `*.md` instead.

### Fixtures were never at risk

An earlier concern that renormalisation would corrupt deliberately-CRLF test data was unfounded, and `ls-files --eol` shows why: every such fixture already carries an explicit `-text` attribute, which overrides `text=auto`. That covers the `gix-transport` http response fixtures, all generated archives, and the fuzz corpora. The `attr/` column is the place to check before worrying.

### If it recurs

The `.gitattributes` rules stop new CRLF entering `.rs`, `.toml` or `.md`. Any other extension the fork starts editing needs its own line, or the same drift returns silently. `git ls-files --eol | findstr /V "i/lf"` is the whole diagnostic.
# Binding expansion roadmap

## Completion contract

The completion goal is **full public gix coverage for this fork**, confirmed by the user on 2026-09-05. The managed/FFI surface covers the union of the agreed native-profile capabilities, including methods added to gix in parallel. LibGit2.Native parity and CSharpMpc migration are intermediate compatibility and consumer-validation milestones.

Rust builders, iterators, callbacks, transactions and borrowed views need equivalent managed operations with explicit ownership; a raw `pub fn` count is not a coverage denominator. Every reachable public capability must be accounted for before the final completion checkbox is checked.

Use the checkboxes in this document to continue implementation. Check an item only after its Rust facade, generated bindings, managed API, relevant tests and transaction integration are complete. Preserve the existing issue IDs and detailed evidence below. Completed parity slices stay checked; their broader gix domains remain open until the remaining operations are implemented.

Independent new methods continue in parallel with generator and boundary-contract work. Apply a prerequisite to the APIs that depend on it: multi-field enum support gates those enum shapes; it does not block unrelated scalar methods or existing supported records. Regenerate Interop.cs after inventory changes and never edit it by hand. Commit each completed implementation transaction and update its checkboxes with validation evidence.

## Implementation checklist

### Completed foundation and compatibility slices

- [x] Rust gix-ffi facade, generated Interop.cs and hand-written GixSharp layer are connected.
- [x] Consume Interoptopus `interoptopus_mps` / `interoptopus_csharp_mps` 0.17.1 from registry `local` (`c68796bb`); upstream crates.io packages cannot silently substitute.
- [x] Consume C# 15 union generation, including Option/Result carriers, under .NET 11 preview 7 with the patched runtime.
- [x] Repository core/discovery compatibility slice — issue `2c6f1a01`.
- [x] Objects/commit-graph compatibility slice — issue `2c6f1a02`.
- [x] Status compatibility slice — issue `2c6f1a03`.
- [x] Index/conflicts compatibility slice — issue `2c6f1a04`.
- [x] References/branches compatibility slice — issue `2c6f1a05`.
- [x] Diff/patch/tree-change compatibility slice — issue `2c6f1a06`; transaction `691180980e389df06166ef8f`, 41 native and 63 managed tests passed.
- [x] Ignore/local-exclude compatibility slice — issue `2c6f1a07`; transaction `f5f6689c91e8ace3f5dd7828`, 47 native and 68 managed tests passed.
- [x] Notes compatibility slice — issue `2c6f1a0c`; transaction `ecab46028c7fc71d74eda427`, 57 native and 76 managed tests passed.
- [x] P0a byte-stream boundary prototype and measurements — `61739044966d033186908aa4ef25157ff981d7a6`; public stream coverage remains below.
- [x] Read the physical index for status/staging, including unchanged-timestamp writes; remove test reopen workarounds — `b3f28217b0b4310aa6e26782467875bbbfc31c84`, 31 native and 54 managed tests passed.
- [x] Preserve work during linked-worktree removal — integrated transaction `f4ff9277b6b18b97692a2d52`.
- [x] Reserve linked-worktree registrations atomically — `0cff314e65e696a11ea37879b1919ada6ae56f15`.

### Remaining compatibility slices

- [ ] Merge/snapshot/safe-checkout compatibility slice — issue `2c6f1a08`.
- [ ] Remote metadata compatibility slice — issue `2c6f1a09`.
- [ ] Worktree compatibility slice — issue `2c6f1a0a`.
- [ ] Configuration compatibility slice — issue `2c6f1a0b`.
- [ ] Sparse-checkout compatibility slice — issue `2c6f1a0d`.

### Remaining public gix coverage

These groups come from the current public source under `gix/src`. Split a group into method checkboxes as it is implemented; record its source APIs, managed equivalents and tests. A closed compatibility slice above does not close any of these groups.

- [ ] Repository open/discovery options, environment/trust/config overrides, operation state, locations and workdir controls — `open/options.rs`, `repository/{state,location}.rs`, `lib.rs`.
- [x] `Repository::is_shallow` → `GixRepository.IsShallow`, including same-handle observation after unshallow.
- [x] Shallow commit boundary information and shallow-file location — `GixRepository.GetShallowCommits`, `ShallowFilePath` and byte-preserving `ShallowFile()`; transaction `617a0c2ea46d9ae649f97241`. Fresh owned snapshots use the direct `gix-shallow` reader so unchanged timestamps cannot hide replacement or corrupt data. Clone/deepen/unshallow, empty/malformed/unreadable files, configured paths and linked worktrees are tested.
- [ ] Repository object/cache controls — `repository/cache.rs`.
- [x] `Repository::has_object` → `GixRepository.HasObject`; valid missing IDs return false and malformed/wrong-format IDs remain errors.
- [x] `Repository::write_blob` → `GixRepository.WriteBlob(byte[])`; exact bytes, empty blobs, normal/bare repositories and managed disposal tested.
- [ ] Blob writes from a stream — `repository/object.rs::write_blob_stream`.
- [ ] Tree entry lookup, traversal and editing — `object/tree/{mod,traverse,editor}.rs`.
- [ ] Annotated tag creation, reading and peeling — `repository/reference.rs`, `object/tag.rs`.
- [ ] General revision resolution, merge-base variants and revision-walk controls — `repository/revision.rs`, `revision/walk.rs`.
- [ ] Commit description and signature access/signing/verification — `object/commit.rs`, `commit/mod.rs`.
- [ ] Full status change details, rewrites/copies, statistics, submodules, caller-selected head/index, iteration, cancellation and writeback outcomes — `status/{platform,index_worktree}.rs`, `status/iter/types.rs`.
- [ ] Index creation/loading from trees and remaining public entry operations — `repository/index.rs`.
- [ ] Reflogs, namespaces, reference transactions, guarded symbolic edits, peeling/following and branch tracking relationships — `repository/reference.rs`, `reference/{mod,log,remote}.rs`, `head/log.rs`, `repository/config/branch.rs`.
- [ ] Full diff configuration, rewrite/copy tracking, statistics and reusable diff resources — `diff/`, `repository/diff.rs`.
- [ ] Attributes and Git/worktree content-filter pipelines — `repository/{attributes,filter}.rs`, `filter.rs`.
- [ ] Standalone pathspec matching and directory traversal/options — `repository/{pathspec,dirwalk}.rs`, `pathspec.rs`, `dirwalk/options.rs`.
- [ ] Submodule configuration, IDs, paths, state/status and repository access — `repository/submodule.rs`, `submodule/mod.rs`.
- [ ] Blame ranges and options — `repository/blame.rs`.
- [ ] Mailmap and configured author/committer identity — `repository/{mailmap,identity}.rs`.
- [ ] Remote mutation, refspecs, URL resolution and defaults — `remote/{access,build,save}.rs`, `repository/config/remote.rs`.
- [ ] Clone preparation, fetch, ref mapping and checkout with progress/cancellation/credentials — `lib.rs`, `clone/`, `remote/connect.rs`, `remote/connection/`.
- [ ] Full worktree administration, including remove/lock/unlock/repair/move — `repository/worktree_admin.rs`.
- [ ] Typed configuration snapshots, source/trust information and overrides — `config/snapshot/`.
- [ ] Remaining notes platform controls: selected/display references and glob order, multi-reference lookup, shorthand mutation names and custom commit messages — `note::Platform::{refs,with_refs,get,replace,remove,with_commit_message}`. The single-ref compatibility materializer is complete; a general note cursor remains under P0c.
- [ ] Remaining merge, checkout and sparse-checkout operations/options, including sparse pattern listing — `repository/` and their public platform types.
- [ ] Public object/worktree byte streams and archives — `repository/{object,worktree}.rs`; the existing managed byte-reader entry points are internal prototypes.
- [ ] Reconcile every remaining reachable public gix API and feature-gated capability, including subsequent parallel additions, against the managed coverage above. Record remaining gaps here before claiming completion.

### Boundary, delivery and completion

- [ ] Complete named/multi-field Rust enum support in Interoptopus — `docs/csharp-multi-field-variants.md`; Steps 1–4 are done (`fcff19e78` completes Steps 3–4); Step 5 and an explicitly patched generator-suite launch remain.
- [x] Make the GixError mapper exhaustive with generated case types and promote missing-case diagnostic CS8509 to an error; compiler probe and managed behavior verified.
- [ ] P0b stable error envelope: Kind, extensible Code, retryability and actionable typed recovery detail.
- [ ] P0c bounded structured cursors with terminal/error semantics and ownership-closure checks.
- [ ] Close the managed public-surface leakage invariant across methods, properties, records and nested/generic/array/by-ref types.
- [ ] Replace sentinel-based HEAD state with the documented closed model once rich variants are supported.
- [ ] Complete resource/transaction ownership and callback/progress/cancellation/credential contracts for their affected families.
- [ ] Complete native feature profiles, runtime capability bootstrap, shared inventory/API guard and exact managed/native version checks.
- [ ] Complete RID packaging, clean package consumption and supported-platform validation — issue `2c6f1a0e`.
- [ ] Complete CSharpMpc consumer migration and its full tests — issue `2c6f1a0f`.
- [ ] Full gix coverage: every public capability accounted for, implemented through the managed boundary, validated, integrated and checked above.

Latest validated expansion on 2026-09-05: notes transaction `ecab46028c7fc71d74eda427`, gix-ffi 57/57 native tests and GixSharp 76/76 managed tests. Managed build `op_6165c83baf3047e2` and patched-runtime run `op_a6e1ec1ac720430d` succeeded. The preceding shallow boundary/location slice integrated as `24b9b9e977fc0ec67f541ab4690bb8372ba963da` with 50 native and 71 managed tests; its native run `a988982311282be8300531e7bda388cf`, build `op_1b5738b5e9fe431e` and patched run `op_88bad4eb45094c28` remain evidence. Ignore/local-exclude `c051d784` passed 47 native and 68 managed tests, and diff/scalar `22c1da14` passed 41 native and 63 managed tests. These totals are validation evidence, not a coverage percentage.

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
status: closed
```

### Scope

Implement `GitDiffTarget`, `GitDiffResult`, `GitTreeChangeKind`, `GitFileMode`, and `GitTreeChange` equivalents plus `GetDiff`, `GetPatch`, and `GetTreeChanges`.

### Acceptance

Index/worktree/tree/commit comparisons cover additions, deletions, modifications, renames, copies where supported, binary files, mode changes, submodules, path filters, patch text, and deterministic ordering.

### Dependencies

Repository core and discovery; objects and commit graph; index and conflicts.

### Resolution

- [x] Implemented in transaction `691180980e389df06166ef8f`: `GetDiff`, `GetPatch` and `GetTreeChanges`, with generated inventory updates and managed records.
- [x] Staged (HEAD/index), unstaged (index/worktree) and complete worktree (HEAD/worktree) comparisons; unborn HEAD, pathspecs, binary summaries, no-final-newline data, additions/deletions, mode changes and read-only behavior.
- [x] Tree/commit/tag comparisons preserve raw path bytes, object IDs, modes, deterministic ordering, configured renames/copies and gitlinks.
- [x] Git-apply checks cover ordinary patches, raw non-UTF8 text, Unicode and ASCII spaced paths, and trailing-space index paths. Managed records retain owned bytes and keep text/byte values consistent after record `with` edits.
- [x] Regenerated bindings; 41 native tests and 63 managed tests passed under the patched runtime. Missing-case compiler probe reports CS8509 as an error.

This closes the documented compatibility slice. Configurable advanced diff resources, cursors, the remaining options and sparse-index support stay in the full-gix checklist above; unmerged and sparse-directory index inputs are rejected by this bounded patch API.

## Ignore and local exclude parity

```issue
id: 2c6f1a07
kind: issue
severity: medium
status: closed
```

### Scope

Implement `IsPathIgnored` and `EnsureLocalExclude`, including repository, info/exclude, global excludes, negation, directory patterns, and idempotent writes.

### Acceptance

Precedence matches Git behavior, exact path bytes are preserved, duplicate rules are not appended, and linked-worktree common-directory behavior is covered.

### Dependencies

Repository core and discovery.


### Resolution

- [x] `IsPathIgnored` and `EnsureLocalExclude` have native, generated and managed string/byte overloads; paths use repository-relative '/' separators and an explicit directory flag.
- [x] Ignore matching follows Git precedence, including global/local/repository rules, nested negation, tracked paths and fresh rule reads on the same handle.
- [x] Local additions are literal root-anchored rules: escape metacharacters, preserve existing bytes and line endings, avoid duplicate lines and write through the owned `gix::lock::File` lock. Failed acquisition preserves a foreign lock.
- [x] Normal, bare and linked worktrees are covered; linked worktrees use common `info/exclude`. Higher-priority negation remains authoritative: the added rule is retained and the failed verification is reported.
- [x] Six native ignore tests compare behavior with Git; five managed tests exercise marshaling, disposal, errors, exact bytes and shared paths. Full suites passed 47/47 native and 68/68 managed under the patched runtime in transaction `f5f6689c91e8ace3f5dd7828`.

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
status: closed
```

### Scope

Implement `GitNote`, `GitNoteEntry`, `WriteNote`, `ReadNote`, `TryReadNote`, `EnumerateNotes`, and `RemoveNote`.

### Acceptance

Default and custom notes refs, overwrite behavior, author/committer signatures, missing notes, enumeration, deletion, and notes-ref updates are tested.

### Dependencies

Objects and commit graph; references and branches.

### Resolution

- [x] `GitNote`, `GitNoteEntry` and all five methods are implemented with the existing consumer signatures, plus configured-default and byte-oriented overloads. `ReadNote` throws `KeyNotFoundException` for absence; `TryReadNote` and `RemoveNote` return false. Corrupt notes remain errors.
- [x] Exact note/ref bytes cross the ABI; owned note and signature byte snapshots survive native/repository disposal. Record init/with assignments keep decoded text and stored bytes consistent. Returned signatures describe the current notes commit, matching the existing consumer.
- [x] The facade reuses `gix::note::plumbing::{get,replace,remove}`, `new_commit_as` and guarded reference edits. Notes fanout and non-note tree entries are preserved. Creation, replacement, deletion and every symbolic link are guarded against competing ref edits; foreign locks remain owned by their writer.
- [x] Default/custom/disabled refs, overwrite refusal, empty notes, signatures, missing notes, deterministic enumeration, deletion retaining an empty notes commit, symbolic refs, lock contention and corruption are tested. Native comparisons use Git as the oracle.
- [x] Transaction `ecab46028c7fc71d74eda427` includes regenerated bindings and passes 57/57 nested native tests (seven dedicated notes tests) and 76/76 managed tests (five dedicated TUnit tests). Build `op_6165c83baf3047e2` and patched-required run `op_a6e1ec1ac720430d` succeeded after merging shallow metadata commit `24b9b9e9`.

This closes the documented compatibility slice. Display-reference selection/globs, multi-reference lookup, custom mutation messages and general cursor delivery remain in the full-gix checklist.

Implemented and validated with OpenAI Codex assistance.


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

## Full gix coverage and CSharpMpc consumer migration

```issue
id: 2c6f1a0f
kind: issue
severity: high
status: open
```

### Scope

Complete the public gix capability-to-managed-member mapping in the checklist, including feature-gated operations and parallel additions. Replace the `LibGit2.Native` package in `CSharpMpc` with GixSharp and run the real consumer suite.

This is the completion gate, not a place to add missing APIs. Any gap found here reopens or creates the appropriate domain issue and is fixed in its own transaction.

### Acceptance

Every reachable public gix capability is mapped to an implemented and tested GixSharp equivalent across the agreed native profiles. Every public managed `LibGit2.Native` member is also mapped for consumer migration. All Git-related CSharpMpc code compiles without compatibility shims leaking generated interop, its full tests pass, package consumption works from a clean restore, and the end-to-end chain is documented:

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
status: closed
```

- [x] Resolved on 2026-09-04 by `c68796bb`: gix-ffi selects `interoptopus_mps` and `interoptopus_csharp_mps` 0.17.1 from registry `local`; the machine registry configuration is in CARGO_HOME. Transaction worktrees no longer need a copied path-patch config. Historical failure and alternatives follow.

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
## gix-ffi exposes no guarded symbolic-ref write, although gix-ref supports one

```issue
id: b660e063
kind: issue
severity: medium
status: open
```

The original entry suspected that the limit was in the FFI surface rather than the store, and asked for that to be confirmed. It has been. The store supports guarded symbolic writes, so this is an exposure task in `gix-ffi`, not a change to `gix-ref`.

### Confirmed: gix-ref already guards symbolic targets

`PreviousValue::MustExistAndMatch` and `ExistingMustMatch` both take a `Target`, and `Target` is `Object(ObjectId) | Symbolic(FullName)`. The transaction layer in `gix-ref/src/store/file/transaction/prepare.rs` destructures the expectation and compares it against the existing target without caring which variant it holds, so symbolic expectations work by construction rather than by special case.

It is exercised in both directions:

- `gix-ref/tests/refs/file/transaction/prepare_and_commit/delete.rs` guards deletes with `MustExistAndMatch(Target::Symbolic("refs/heads/main"))`
- `gix/src/remote/connection/fetch/update_refs/mod.rs` matches on `MustExistAndMatch(Target::Symbolic(_))` in production fetch code

So nothing needs adding to `gix-ref`, and the acceptance bar for this work is `gix-ref`'s own transaction tests rather than a differential against `git`. Note that `git symbolic-ref` has no expected-value argument, so there is no porcelain equivalent to compare against; `git update-ref --stdin` gained `symref-verify` and `symref-update` in recent versions and is the closest git-shaped signature if the installed git is new enough.

### The actual gap, in gix-ffi

`compare_exchange_reference` takes `expected` and `target` as hex `ffi::String` values and builds `Target::Object` from both. Its signature cannot express a symbolic expectation, so this is a new entry point rather than a change to the existing one. libgit2 solved the same problem the same way, with `git_reference_symbolic_create_matching` alongside `git_reference_create_matching`.

`set_head` writes `Target::Symbolic(target)` with `PreviousValue::Any`. That is correct for `git symbolic-ref HEAD <ref>` semantics; what is missing is a guarded variant beside it.

### Correction to the earlier framing

The original entry cited `git worktree add` writing a `HEAD` into the new administrative directory as a motivating case. That is not one. `add_worktree` wrote `HEAD` into a directory it had created moments earlier, so there was no prior value to clobber and no race of that shape.

`add_worktree` no longer writes `HEAD` unguarded in any case. It now goes through a reference transaction on a store scoped to the new administrative directory: `MustNotExist` when detaching, and `ExistingMustMatch(Object(tip))` when attaching to a branch, the latter because that is the only form for which `gix-ref` emits a reflog on a symbolic update. So the `set_head` gap below is the remaining one, and it is entirely in `gix-ffi`.

The real hazard left in `add_worktree` is that identifier derivation is not atomic. Git avoids it by looping on `mkdir` until it no longer reports `EEXIST`, appending a counter (`builtin/worktree.c:507-514`), which is the same `create_new` shape proposed for the fix here.

### Why it still matters

Resumable materialisation re-attaches an existing worktree's `HEAD` to a specific branch at a specific tip. There, an unconditional write does make "finish what was interrupted" indistinguishable from "clobber what someone else did", and that is the case this issue exists for.

`repair` does not need it: `repair_worktrees` writes only `gitdir` and `.git` pointer files and touches no refs.
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