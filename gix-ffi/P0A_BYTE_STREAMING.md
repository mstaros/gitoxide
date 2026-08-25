# P0a byte-streaming prototype

## Recommendation

Keep the stateful native reader plus caller-owned reusable managed buffer as the leading boundary design. It provides familiar pull/backpressure semantics, deterministic disposal, stable EOF, a single copy across the FFI boundary, and no whole-payload managed allocation. Keep it experimental and gix-specific for now; do not turn it into an Interoptopus-wide streaming abstraction until more payload shapes have exercised the ownership and error policy.

The prototype also establishes two important limits:

1. A normal gix object lookup still owns the complete decompressed object in a native `Vec<u8>`. Streaming the boundary removes a second whole-payload managed allocation, but it does not reduce that native peak.
2. `gix_worktree_stream::Stream` is a real pipe-backed `Read` source, but its current producer calls `Find::find(entry.oid, &mut Vec<u8>)` before writing each blob to the pipe. It is streaming at the consumer boundary and across a multi-entry tree, but a single large entry is still materialized upstream.

## Prototype shape

- Native `ByteReader` owns either `Cursor<Vec<u8>>` for object data or `gix::worktree::stream::Stream` for the pipe-backed source.
- `read(SliceMut<u8>)` fills a caller buffer and returns the byte count. A non-empty read returning zero is EOF.
- Managed `PrototypeByteReaderStream : Stream` is internal and exposed only through internal experimental repository factories. The existing public managed surface is unchanged.
- `Read(Span<byte>)` fixes the span only for the synchronous FFI call. A single unowned generated `SliceMutByte` view is reused per stream and invalidated immediately after the call.
- All existing `GixError` / generated result handling remains in place.

## Results

The reproducible Release harness uses a deterministic 67,109,121-byte blob, one committed worktree entry, a single reusable 65,536-byte managed buffer, one warm-up, and five timed iterations per source. Full raw measurements and environment details are in [P0A_BYTE_STREAMING_RESULTS.md](P0A_BYTE_STREAMING_RESULTS.md).

| Source | Median open | Median pull | Pull throughput | Calls to first EOF | Managed pull allocation | Exact owned native payload |
|---|---:|---:|---:|---:|---:|---:|
| Materialized object | 14.404 ms | 3.302 ms | 19,382.9 MiB/s | 1,026 | 41,040 B | 64.00 MiB |
| Worktree pipe `Read` | 0.673 ms | 104.973 ms | 609.7 MiB/s | 1,026 | 41,040 B | 0 B |

The materialized-object pull number is a hot in-memory copy after gix has already opened/decompressed the object; its separate open time must be included when judging end-to-end object access.

Each source made 1,025 non-empty reads plus one EOF read. Boundary bytes copied exactly matched bytes delivered: 67,109,121 bytes for the object and 67,109,168 bytes for the transient worktree representation (47 bytes of entry framing). There was no whole-payload managed copy.

The 41,040 managed bytes are 1,026 generated `ResultUlongGixError` objects at 40 bytes per FFI read. The reusable buffer and reusable `SliceMutByte` view do not allocate per pull. Removing the result allocation would require a deliberate Interoptopus/error-boundary decision and was intentionally left outside this task.

Sampled peak managed growth was about 0.3 MiB for both sources. Sampled process-private growth was about 65 MiB for both. For the object, the reader reports 64 MiB of exact retained `Vec` capacity. For the worktree source, the reader retains no whole output, but source inspection and the process peak show that the producer currently materializes the single 64 MiB blob before pipe output. Process-private memory is only a proxy and includes allocator/JIT/OS effects.

## EOF, disposal, and cancellation

- A zero-length read returns zero, counts as one FFI call, and does not mark EOF.
- The first non-empty zero read marks EOF; repeated EOF reads are stable and took about 0.1 microseconds once warm.
- A pre-canceled `ReadAsync(Memory<byte>)` throws before entering native code and makes zero FFI calls.
- Cancellation is cooperative between pulls only. There is no token inside the synchronous native `Read`; an in-flight call cannot be interrupted.
- Early disposal after one chunk was deterministic: about 2.068 ms for the materialized object (including release of its 64 MiB allocation) and 9 microseconds for the pipe-backed source in this run.
- Managed locking serializes reads and disposal. Disposal waits for an active read, drops the native source once, and later reads throw `ObjectDisposedException`.

## Ownership and lifetime constraints

- The managed caller owns the destination memory. Native code may write only during the call and must never store the pointer or slice.
- The temporary pin is valid only inside `Read(Span<byte>)`; no borrowed managed address survives the fixed block.
- The native reader owns its source and is independent of the `GixRepository` handle after construction. Tests dispose the repository before draining both source types.
- A reader is stateful and single-consumer. Concurrent reads are serialized in managed code; native mutation is never concurrent.
- Length is known for the materialized object and intentionally unsupported for the worktree stream.
- A generated service handle must be disposed. The managed stream owns exactly one handle and makes disposal idempotent.
- Native I/O failures continue through the existing result/union architecture; no new error family or cancellation union was introduced.

## Alternatives

| Design | Boundary copies and allocation | Lifetime / API fit | Assessment |
|---|---|---|---|
| Stateful reader + caller buffer | One boundary copy per non-empty pull; reusable payload buffer; currently one small generated result object per call | Natural `Stream.Read(Span<byte>)`; bounded pin; deterministic ownership | Recommended leading design |
| Scoped callback / borrowed view | Can avoid the boundary copy when managed consumes synchronously in the callback | Borrow is valid only during callback; awkward for `Stream`, reentrancy and exception propagation need policy, and data cannot escape | Useful for specialized synchronous parsers, not the default stream surface |
| Chunked `Vec<u8>` cursor | Allocates/owns a native vector per chunk and normally adds a copy into the consumer buffer; generated resource disposal per chunk | Simple ownership but high churn and easy retention mistakes | Acceptable fallback for coarse paging, inferior for sustained streaming |

## Reproduction

From `gix-ffi`:

```text
cargo test generate_csharp_bindings -- --test-threads=1
cargo test -- --test-threads=1
dotnet build bindings/GixSharp.slnx -c Release
dotnet run --project bindings/GixSharp.Tests/GixSharp.Tests.csproj -c Release -- --treenode-filter "/*/*/ByteStreamingTests/*" --maximum-parallel-tests 1
dotnet run --project bindings/GixSharp.Tests/GixSharp.Tests.csproj -c Release -- --treenode-filter "/*/*/ByteStreamingBenchmarks/*" --maximum-parallel-tests 1
```

The benchmark class is marked `[Explicit]`, so it is excluded from an unfiltered normal test run.