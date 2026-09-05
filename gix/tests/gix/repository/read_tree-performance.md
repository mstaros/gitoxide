# Two-tree index transition measurement

Measured on Windows on 2026-09-05, using the unoptimized Cargo test profile and default gix features. The baseline includes native ref fixes through `33f21eb0e534b510e1572fff0d3c80bfae54e1dc`.

Run from the repository root:

```text
cargo nextest run -p gix --run-ignored only --test-threads 1 measure_index_only_transitions
```

The ignored test writes `gix/target/read-tree-measurement.csv`. It creates fresh SHA-1 repositories with sorted, flat trees of 1,000, 10,000 and 100,000 files. Exactly one blob changes. Initialization and two warmup transitions precede 20 timed transitions alternating both directions. Each sample includes opening and publishing the index; it does not materialize working-tree files.

The comparison runs sequentially, after the native correctness gate finished. An earlier run overlapped the gate and is excluded because its host load was different.

| Entries | Before median (ms) | After median (ms) | Before range (ms) | After range (ms) |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 | 85.307 | 62.895 | 71.718–109.762 | 57.741–64.579 |
| 10,000 | 1048.896 | 909.815 | 807.772–1179.611 | 636.843–1086.317 |
| 100,000 | 8865.918 | 8847.165 | 6942.835–11900.880 | 7656.850–21590.603 |

Smaller cases improved in these runs; the 100k median was effectively unchanged. Its wide timing spread does not establish a general throughput gain. The allocation reduction is the reason to retain this small change.

Baseline operation: `40b14664716135300b9aba004d4c0767`. Borrowed-entry operation: `2cb8d82dd4ce02205dc5e6b648f76913`.

The change borrows paths and entries for the old-tree, new-tree and current-index lookup maps. At 100,000 entries it eliminates 300,000 path copies/allocations and 300,000 entry clones per transition. The input indexes still own their backing storage; selected replacements and the result index retain their existing ownership. Ordering and conflict rules are unchanged.

These are local debug-build measurements, not release-build throughput claims. The algorithm still visits the complete input indexes and uses ordered maps and sets. Working-tree I/O, sparse/filter workloads and checkout parallelism require separate measurements.