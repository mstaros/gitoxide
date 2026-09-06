# Native Git feature coverage audit

Date: 2026-09-06

## Scope

This audit covers the native gitoxide fork at `D:\repos\gitoxide`. It evaluates
whether the Rust implementation can complete Git workflows without falling back
to `git.exe`. FFI exposure is a separate layer and is not counted here.

The findings were checked against `README.md`, `crate-status.md`,
`SHORTCOMINGS.md`, explicit unsupported branches in `gix`, and the fork's
verified blob-export and worktree handoffs. A plumbing algorithm is not counted
as a complete Git feature when state persistence, conflict recovery, hooks, or
the surrounding ref/index/worktree orchestration is absent.

Rejection is not coverage. An explicit unsupported/invalid-option branch is
listed here as a gap to remove, even when it is useful temporarily to prevent
silent data loss while a bounded implementation is incomplete. Strict
validation counts as support only when Git rejects the same input or when it
protects a documented library invariant without excluding valid Git behavior.

## Summary

Gitoxide has broad and useful plumbing for repositories, objects, references,
indexes, status, revision traversal, fetch/clone, diffs, checkout mechanics, and
merge algorithms. Its obvious gaps are concentrated in complete mutating
workflows and remote write/server behavior.

| Priority | Area | Present coverage | Material gap |
| --- | --- | --- | --- |
| P0 | Cross-domain mutation | Atomic ref updates, index locks, guarded two-tree transitions and explicit partial-failure evidence | No crash-recoverable transaction spanning refs, index and worktree; an application or filter failure can leave partially changed worktree paths |
| P0 | Checkout/reset family | Low-level checkout, bounded one-tree index replacement, and guarded two-tree `Repository::read_tree()` transitions | No complete `checkout`, `switch`, `restore`, or `reset` orchestration; three-tree read-tree, one-tree worktree/sparse semantics, and several options remain unsupported |
| P0 | Merge and sequencing | Blob, tree and commit merge algorithms | No Git-compatible persisted merge state, continuation/abort, or complete cherry-pick, revert and rebase sequencers |
| P0 | Push | Clone, fetch, negotiation and ref/object reads | No send-pack/receive-pack client, atomic push, delete-ref, push-option, or report-status workflow |
| P1 | Hooks and quarantine | Trust and process-launch foundations | No hook discovery/execution, `core.hooksPath`, `reference-transaction`, or receive-side quarantine integration |
| P1 | Partial repositories | Shallow clone/fetch | Partial clone, promisor lookup, bundle bootstrap and bundle-URI integration remain incomplete |
| P1 | Repository formats | SHA-1 and substantial SHA-256 object support | No reftable backend or complete Git 3.0 ref/protocol compatibility |
| P1 | Large-repository integration | Commit-graph, index extensions and bitmap readers exist in parts | Compressed sparse indexes are rejected by guarded transitions; split-index, fsmonitor, untracked-cache and bitmap maintenance/write paths are incomplete |
| P1 | Integrity/maintenance | Object parsing, connectivity traversal and strict verification in the fork's blob-export path | General strict object creation/hash verification and full `git fsck` parity are incomplete; pack writes lack full delta compression and bitmap generation |
| P2 | Worktree porcelain | Add, remove, move, repair, prune, lock and unlock exist; add supports force, branch create/reset, checkout, orphan, tracking, remote guessing and relative pointers | The current boolean library surface cannot spell CLI repetition or explicit `--track=inherit`/`--no-relative-paths`; list/output formatting remains caller-side |
| P2 | Submodules | Read `.gitmodules`, apply overrides, determine activity and report status | No complete add/update/sync/deinit-style CRUD workflow |
| P2 | Verified blob CLI parity | Strict tree/blob verification, raw export, custom/quoted tree listings and atomic no-replace publication | No broad tree-ish/pathspec/cwd/abbreviation selection, `cat-file --batch`, transformations, configurable `core.quotePath=false`, or constant-memory packed-delta streaming |

## Concrete unsupported surfaces

### `Repository::read_tree()`

The fork now supports bounded one-tree index replacement as well as the
existing guarded two-tree merge/reset transition. Tested one-tree modes are
plain replacement, `-m`, `--reset`, `-m -i`, `--reset -i`, and dry-run
forms of plain replacement, `-m`, and `--reset`. None of these one-tree
forms updates the worktree.

One-tree `-m` refuses an unmerged index and checks tracked paths whose index
entry would change. It rejects present modified files and intent-to-add
entries, but accepts missing tracked files, matching Git. Plain replacement
and `--reset` replace an unmerged index. For `-m` and `--reset`, unchanged
stage-zero entries retain their existing stat data and flags.

The implementation explicitly rejects one-tree `-u`, one-tree sparse-checkout
semantics, plain one-tree `-i`, and merge-plus-reset. Three-tree operation,
compressed sparse indexes, `--prefix`, `--index-output`, `--aggressive`,
`--trivial`, recursive submodule updates, and replacement of a submodule
working directory also remain unsupported.

The two-tree path deliberately publishes the index only after worktree
application. On an I/O or filter failure the old index remains, but worktree
paths may already have changed and are returned in `Error::UpdateFailed`.
That is useful recovery evidence, not crash recovery or atomicity across the
worktree and index.

### `Repository::add_worktree()`

Administrative creation and cleanup are guarded, including the fixed Windows
contention race. Valid add modes are implemented rather than rejected:

- force permits reuse of stale registrations and deliberate sharing of an
  existing branch, but never takes over a non-empty directory;
- `-b` reserves and creates a branch only after registration succeeds;
- `-B` uses an expected-value reference transaction and still refuses to
  reset a branch held by any worktree, even with force;
- checkout materialises the index/worktree when the crate has the
  `worktree-mutation` capability; without that compile-time feature the
  operation reports a capability error before mutation;
- orphan mode creates an unborn branch and an empty index/worktree;
- direct tracking, no-track, `branch.autoSetupMerge` modes, configured
  remote refspec mapping, `worktree.guessRemote`, and
  `checkout.defaultRemote` are honored;
- relative mode writes both directional pointers relatively and is also
  selected by `worktree.useRelativePaths`; repair and move preserve relative
  registrations.

The library's existing `Option<bool>` tracking field represents direct,
disabled, or configured automatic behavior. Explicit CLI
`--track=inherit` is available through `branch.autoSetupMerge=inherit`, but
does not yet have a distinct per-call value. Likewise, a false
`relative_paths` value consults configuration and cannot explicitly override
a configured true value. These are option-model gaps, not runtime feature
rejections. CLI repetition and progress/output formatting belong at the caller
boundary.

### Protocol and transport

Fetch is implemented. Push and receive-pack plumbing are not. `file://` uses
an external service program and SSH uses an external SSH process; there is no
self-contained native SSH transport/server path.

## Recommended order

1. Define and implement crash-recovery semantics for ref/index/worktree
   transitions. This requires an explicit design before introducing any durable
   journal or public transaction contract.
2. Complete checkout/reset composition using the existing guarded transition
   code, then connect ref movement only after its recovery contract is defined.
3. Persist merge state and implement continue/abort, then build cherry-pick,
   revert and rebase on the same sequencer.
4. Implement hook discovery/execution and `reference-transaction` before
   claiming full Git-compatible mutating workflows.
5. Add push, followed by promisor/partial-clone and bundle bootstrapping.
6. Complete reftable and write-side large-repository accelerators.
7. Fill the remaining worktree option-model, submodule and blob-command
   parity gaps without treating explicit rejection as completion.

## Implementation status

The first bounded slice, one-tree `Repository::read_tree()` index replacement,
is implemented on the existing method, option and error surface. It is
foundational for reset/restore, but does not add a durable transaction format
or claim worktree/ref atomicity.

Validation on 2026-09-06 includes:

- an 88-case Git oracle matrix: 11 index/worktree states across eight valid
  one-tree option combinations;
- explicit conflicted-index comparisons for plain, `-m`, and `--reset`;
- strict rejection tests for unsupported sparse, worktree-update, three-tree,
  option-conflict, and lock-contention cases;
- checks that one-tree operations never modify worktree bytes, and that dry-run,
  rejection, and lock contention never modify index bytes or leave a stale
  lock; and
- a passing `nextest` run for the full `repository::read_tree` test filter.

This closes only one-tree index replacement. Full checkout/reset orchestration,
one-tree worktree and sparse-checkout behavior, three-tree read-tree, and
cross-ref/index/worktree crash recovery remain open. The rejection tests above
are safety rails for that incomplete slice, not delivered Git-feature coverage.

The linked-worktree add slice is now implemented on the existing
`Repository::add_worktree()` method. Validation includes 36 focused core
worktree tests, Git-readable attached/detached/orphan registrations, force and
stale-registration handling, guarded branch reset, tracking and remote guessing,
bidirectional relative pointers, checkout materialisation, and the five
registration-race/cleanup tests. Native FFI and managed validation are recorded
in the transaction handoff once the final gates complete.
