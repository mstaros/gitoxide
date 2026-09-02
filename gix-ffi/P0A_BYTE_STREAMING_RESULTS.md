# P0a byte-streaming benchmark results

- Timestamp (UTC): 2026-08-25T05:00:18.4179714+00:00
- OS: Microsoft Windows 10.0.26200
- Architecture: X64
- .NET: .NET 11.0.0-preview.7.26381.103
- Processor count: 32
- Git: git version 2.55.0.windows.3
- Payload: 67,109,121 bytes deterministic blob; one committed worktree entry
- Managed buffer: 65,536 bytes, allocated once and reused
- Timing: 5 Release iterations after one full warm-up per source

## Summary

| Source | Bytes/read | Median open ms | Median read ms | Median MiB/s | FFI calls to EOF | Derived result allocations | Read allocated bytes | Peak managed delta | Peak private delta | Exact retained native |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| materialized-object | 67,109,121 | 14.404 | 3.302 | 19382.9 | 1,026 | 1,026 | 41,040 | 0.30 MiB | 65.00 MiB | 64.00 MiB |
| worktree-read | 67,109,168 | 0.673 | 104.973 | 609.7 | 1,026 | 1,026 | 41,040 | 0.31 MiB | 64.21 MiB | 0.00 MiB |

## Raw timing runs

| Source | Iteration | Bytes | Open ms | Read ms | MiB/s | Calls at EOF | Repeat EOF us | Checksum |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| materialized-object | 1 | 67,109,121 | 14.404 | 3.823 | 16742.2 | 1,026 | 1.4 | 0x9968f43ac1841649 |
| materialized-object | 2 | 67,109,121 | 13.795 | 3.205 | 19966.4 | 1,026 | 0.1 | 0x9968f43ac1841649 |
| materialized-object | 3 | 67,109,121 | 13.518 | 3.326 | 19244.7 | 1,026 | 0.0 | 0x9968f43ac1841649 |
| materialized-object | 4 | 67,109,121 | 14.678 | 3.302 | 19382.9 | 1,026 | 0.1 | 0x9968f43ac1841649 |
| materialized-object | 5 | 67,109,121 | 15.012 | 2.792 | 22926.8 | 1,026 | 0.1 | 0x9968f43ac1841649 |
| worktree-read | 1 | 67,109,168 | 0.691 | 107.466 | 595.5 | 1,026 | 0.2 | 0xdd93d23d7ac809fd |
| worktree-read | 2 | 67,109,168 | 0.428 | 95.102 | 673.0 | 1,026 | 0.1 | 0xdd93d23d7ac809fd |
| worktree-read | 3 | 67,109,168 | 0.673 | 102.861 | 622.2 | 1,026 | 0.1 | 0xdd93d23d7ac809fd |
| worktree-read | 4 | 67,109,168 | 0.589 | 104.973 | 609.7 | 1,026 | 0.1 | 0xdd93d23d7ac809fd |
| worktree-read | 5 | 67,109,168 | 0.845 | 106.141 | 603.0 | 1,026 | 0.1 | 0xdd93d23d7ac809fd |

## Allocation, memory, EOF, cancellation, and disposal

| Source | Open allocated | Read allocated | Allocation calls | Peak managed delta | Peak private delta | Retained native | Zero read / EOF | Canceled / FFI calls | Cancel us | First read | Dispose us | Disposed guarded |
|---|---:|---:|---:|---:|---:|---:|---|---|---:|---:|---:|---|
| materialized-object | 296 B | 41,040 B | 1,026 | 0.30 MiB | 65.00 MiB | 64.00 MiB | 0 / False | True / 0 | 532.7 | 65536 | 2068.0 | True |
| worktree-read | 296 B | 41,040 B | 1,026 | 0.31 MiB | 64.21 MiB | 0.00 MiB | 0 / False | True / 0 | 53.1 | 65536 | 9.0 | True |

## Methodology limits

- Throughput covers synchronous pull reads and a two-byte-per-chunk checksum; object/worktree open time is reported separately.
- Read allocation uses GC.GetAllocatedBytesForCurrentThread in a separate unsampled pass. It excludes the single reusable byte-array allocation. Generated ResultUlongGixError.IntoManaged() constructs one managed result class per FFI read, so the derived allocation count equals the call count.
- Peak managed delta uses sampled GC.GetTotalMemory(false). Peak private delta is a sampled process-level proxy containing native, managed, allocator, JIT, and OS effects; it is not an exact native-only counter.
- Exact retained native bytes covers only the materialized object's owned Vec<u8> capacity. The worktree stream reports zero because its pipe and gix scratch buffers are not guessed.
- Each non-empty FFI read performs one native-to-managed buffer copy. Bytes copied equal bytes delivered; gix-internal ODB/decompression and pipe copies are outside the boundary counter.
- Cancellation is cooperative between pulls: a pre-canceled token makes no FFI call. Disposal drops the native source; it cannot interrupt a read already executing on another thread.
