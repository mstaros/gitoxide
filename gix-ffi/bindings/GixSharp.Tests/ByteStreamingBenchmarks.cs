using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Text;
using TUnit.Core;

namespace GixSharp.Tests;

[Explicit]
[NotInParallel]
public sealed class ByteStreamingBenchmarks
{
    private const int PayloadBytes = (64 * 1024 * 1024) + 257;
    private const int BufferBytes = 64 * 1024;
    private const int TimingIterations = 5;

    [Test]
    public void P0a_ByteStreaming()
    {
        using var fixture = new BenchmarkRepository(PayloadBytes);
        var buffer = new byte[BufferBytes];
        var process = Process.GetCurrentProcess();
        var sources = new[]
        {
            new Source(
                "materialized-object",
                () => fixture.Repository.OpenObjectByteStreamForPrototype(fixture.ObjectId)),
            new Source(
                "worktree-read",
                () => fixture.Repository.OpenWorktreeByteStreamForPrototype(fixture.TreeId)),
        };

        var measurements = new List<SourceMeasurement>();
        foreach (var source in sources)
        {
            // Warm JIT, generated marshalling, native object lookup, and gix producer paths.
            using (var warmup = source.Open())
                _ = Drain(warmup, buffer, sample: null);

            var timed = new List<TimedRun>();
            for (var iteration = 1; iteration <= TimingIterations; iteration++)
                timed.Add(MeasureTimed(source, buffer, iteration));

            var allocation = MeasureAllocations(source, buffer);
            var memory = MeasureMemory(source, buffer, process);
            var behavior = MeasureCancellationAndDisposal(source, buffer);
            measurements.Add(new SourceMeasurement(
                source.Name,
                timed,
                allocation,
                memory,
                behavior));
        }

        var output = RenderResults(measurements);
        var resultsPath = Path.Combine(FindGixFfiRoot(), "P0A_BYTE_STREAMING_RESULTS.md");
        File.WriteAllText(resultsPath, output, new UTF8Encoding(false));
        Console.WriteLine(output);
    }

    private static TimedRun MeasureTimed(Source source, byte[] buffer, int iteration)
    {
        var open = Stopwatch.StartNew();
        using var stream = source.Open();
        open.Stop();

        var read = Stopwatch.StartNew();
        var drain = Drain(stream, buffer, sample: null);
        read.Stop();

        if (!stream.EofObserved)
            throw new InvalidOperationException($"{source.Name} did not observe EOF.");

        var callsAtFirstEof = stream.FfiReadCalls;
        var repeatedEof = Stopwatch.StartNew();
        var repeatedResult = stream.Read(buffer);
        repeatedEof.Stop();
        if (repeatedResult != 0 || stream.FfiReadCalls != callsAtFirstEof + 1)
            throw new InvalidOperationException($"{source.Name} repeated EOF was unstable.");

        return new TimedRun(
            iteration,
            drain.Bytes,
            drain.Checksum,
            open.Elapsed.TotalMilliseconds,
            read.Elapsed.TotalMilliseconds,
            ToMibPerSecond(drain.Bytes, read.Elapsed),
            callsAtFirstEof,
            repeatedEof.Elapsed.TotalMicroseconds,
            stream.RetainedNativeBytes);
    }

    private static AllocationRun MeasureAllocations(Source source, byte[] buffer)
    {
        Collect();
        var beforeOpen = GC.GetAllocatedBytesForCurrentThread();
        using var stream = source.Open();
        var openAllocated = GC.GetAllocatedBytesForCurrentThread() - beforeOpen;

        var beforeRead = GC.GetAllocatedBytesForCurrentThread();
        var drain = Drain(stream, buffer, sample: null);
        var readAllocated = GC.GetAllocatedBytesForCurrentThread() - beforeRead;

        return new AllocationRun(
            drain.Bytes,
            openAllocated,
            readAllocated,
            stream.FfiReadCalls,
            stream.RetainedNativeBytes);
    }

    private static MemoryRun MeasureMemory(
        Source source,
        byte[] buffer,
        Process process)
    {
        Collect();
        var baselineManaged = GC.GetTotalMemory(forceFullCollection: false);
        var baselinePrivate = PrivateBytes(process);
        var peakManaged = baselineManaged;
        var peakPrivate = baselinePrivate;

        using var stream = source.Open();
        Sample();
        var drain = Drain(stream, buffer, call =>
        {
            if ((call & 15UL) == 0)
                Sample();
        });
        Sample();

        return new MemoryRun(
            drain.Bytes,
            Math.Max(0, peakManaged - baselineManaged),
            Math.Max(0, peakPrivate - baselinePrivate),
            stream.RetainedNativeBytes);

        void Sample()
        {
            peakManaged = Math.Max(
                peakManaged,
                GC.GetTotalMemory(forceFullCollection: false));
            peakPrivate = Math.Max(peakPrivate, PrivateBytes(process));
        }
    }

    private static BehaviorRun MeasureCancellationAndDisposal(
        Source source,
        byte[] buffer)
    {
        var stream = source.Open();
        var zeroLengthResult = stream.Read(Span<byte>.Empty);
        var eofAfterZeroLength = stream.EofObserved;

        using var cancellation = new CancellationTokenSource();
        cancellation.Cancel();
        var callsBeforeCancellation = stream.FfiReadCalls;
        var cancel = Stopwatch.StartNew();
        var canceled = false;
        try
        {
            _ = stream.ReadAsync(buffer, cancellation.Token)
                .AsTask()
                .GetAwaiter()
                .GetResult();
        }
        catch (OperationCanceledException)
        {
            canceled = true;
        }
        cancel.Stop();
        var cancellationFfiCalls = stream.FfiReadCalls - callsBeforeCancellation;

        var firstRead = stream.Read(buffer);
        var dispose = Stopwatch.StartNew();
        stream.Dispose();
        dispose.Stop();

        var disposedReadThrows = false;
        try
        {
            _ = stream.Read(buffer);
        }
        catch (ObjectDisposedException)
        {
            disposedReadThrows = true;
        }

        return new BehaviorRun(
            zeroLengthResult,
            eofAfterZeroLength,
            canceled,
            cancellationFfiCalls,
            cancel.Elapsed.TotalMicroseconds,
            firstRead,
            dispose.Elapsed.TotalMicroseconds,
            disposedReadThrows);
    }

    private static DrainResult Drain(
        GixRepository.PrototypeByteReaderStream stream,
        byte[] buffer,
        Action<ulong>? sample)
    {
        long total = 0;
        ulong calls = 0;
        ulong checksum = 1469598103934665603UL;

        while (true)
        {
            var read = stream.Read(buffer);
            calls++;
            sample?.Invoke(calls);
            if (read == 0)
                break;

            total += read;
            checksum ^= buffer[0];
            checksum *= 1099511628211UL;
            checksum ^= buffer[read - 1];
            checksum *= 1099511628211UL;
            checksum ^= (uint)read;
        }

        return new DrainResult(total, checksum);
    }

    private static string RenderResults(IReadOnlyList<SourceMeasurement> measurements)
    {
        var invariant = CultureInfo.InvariantCulture;
        var output = new StringBuilder();
        output.AppendLine("# P0a byte-streaming benchmark results");
        output.AppendLine();
        output.AppendLine($"- Timestamp (UTC): {DateTimeOffset.UtcNow:O}");
        output.AppendLine($"- OS: {RuntimeInformation.OSDescription}");
        output.AppendLine($"- Architecture: {RuntimeInformation.ProcessArchitecture}");
        output.AppendLine($"- .NET: {RuntimeInformation.FrameworkDescription}");
        output.AppendLine($"- Processor count: {Environment.ProcessorCount}");
        output.AppendLine($"- Git: {Git(Environment.CurrentDirectory, "--version")}");
        output.AppendLine($"- Payload: {PayloadBytes:N0} bytes deterministic blob; one committed worktree entry");
        output.AppendLine($"- Managed buffer: {BufferBytes:N0} bytes, allocated once and reused");
        output.AppendLine($"- Timing: {TimingIterations} Release iterations after one full warm-up per source");
        output.AppendLine();
        output.AppendLine("## Summary");
        output.AppendLine();
        output.AppendLine("| Source | Bytes/read | Median open ms | Median read ms | Median MiB/s | FFI calls to EOF | Derived result allocations | Read allocated bytes | Peak managed delta | Peak private delta | Exact retained native |");
        output.AppendLine("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");

        foreach (var measurement in measurements)
        {
            var medianOpen = Median(measurement.Timed.Select(static run => run.OpenMilliseconds));
            var medianRead = Median(measurement.Timed.Select(static run => run.ReadMilliseconds));
            var medianThroughput = Median(measurement.Timed.Select(static run => run.MibPerSecond));
            var calls = measurement.Timed.Select(static run => run.FfiCallsAtFirstEof).Distinct().Single();

            output.Append("| ").Append(measurement.Name)
                .Append(" | ").Append(measurement.Allocation.Bytes.ToString("N0", invariant))
                .Append(" | ").Append(medianOpen.ToString("F3", invariant))
                .Append(" | ").Append(medianRead.ToString("F3", invariant))
                .Append(" | ").Append(medianThroughput.ToString("F1", invariant))
                .Append(" | ").Append(calls.ToString("N0", invariant))
                .Append(" | ").Append(measurement.Allocation.FfiCalls.ToString("N0", invariant))
                .Append(" | ").Append(measurement.Allocation.ReadAllocatedBytes.ToString("N0", invariant))
                .Append(" | ").Append(FormatBytes(measurement.Memory.PeakManagedDelta))
                .Append(" | ").Append(FormatBytes(measurement.Memory.PeakPrivateDelta))
                .Append(" | ").Append(FormatBytes(checked((long)measurement.Memory.RetainedNativeBytes)))
                .AppendLine(" |");
        }

        output.AppendLine();
        output.AppendLine("## Raw timing runs");
        output.AppendLine();
        output.AppendLine("| Source | Iteration | Bytes | Open ms | Read ms | MiB/s | Calls at EOF | Repeat EOF us | Checksum |");
        output.AppendLine("|---|---:|---:|---:|---:|---:|---:|---:|---:|");
        foreach (var measurement in measurements)
        {
            foreach (var run in measurement.Timed)
            {
                output.Append("| ").Append(measurement.Name)
                    .Append(" | ").Append(run.Iteration)
                    .Append(" | ").Append(run.Bytes.ToString("N0", invariant))
                    .Append(" | ").Append(run.OpenMilliseconds.ToString("F3", invariant))
                    .Append(" | ").Append(run.ReadMilliseconds.ToString("F3", invariant))
                    .Append(" | ").Append(run.MibPerSecond.ToString("F1", invariant))
                    .Append(" | ").Append(run.FfiCallsAtFirstEof.ToString("N0", invariant))
                    .Append(" | ").Append(run.RepeatedEofMicroseconds.ToString("F1", invariant))
                    .Append(" | 0x").Append(run.Checksum.ToString("x16", invariant))
                    .AppendLine(" |");
            }
        }

        output.AppendLine();
        output.AppendLine("## Allocation, memory, EOF, cancellation, and disposal");
        output.AppendLine();
        output.AppendLine("| Source | Open allocated | Read allocated | Allocation calls | Peak managed delta | Peak private delta | Retained native | Zero read / EOF | Canceled / FFI calls | Cancel us | First read | Dispose us | Disposed guarded |");
        output.AppendLine("|---|---:|---:|---:|---:|---:|---:|---|---|---:|---:|---:|---|");
        foreach (var measurement in measurements)
        {
            var allocation = measurement.Allocation;
            var memory = measurement.Memory;
            var behavior = measurement.Behavior;
            output.Append("| ").Append(measurement.Name)
                .Append(" | ").Append(allocation.OpenAllocatedBytes.ToString("N0", invariant)).Append(" B")
                .Append(" | ").Append(allocation.ReadAllocatedBytes.ToString("N0", invariant)).Append(" B")
                .Append(" | ").Append(allocation.FfiCalls.ToString("N0", invariant))
                .Append(" | ").Append(FormatBytes(memory.PeakManagedDelta))
                .Append(" | ").Append(FormatBytes(memory.PeakPrivateDelta))
                .Append(" | ").Append(FormatBytes(checked((long)memory.RetainedNativeBytes)))
                .Append(" | ").Append(behavior.ZeroLengthResult).Append(" / ").Append(behavior.EofAfterZeroLength)
                .Append(" | ").Append(behavior.Canceled).Append(" / ").Append(behavior.CancellationFfiCalls)
                .Append(" | ").Append(behavior.CancellationMicroseconds.ToString("F1", invariant))
                .Append(" | ").Append(behavior.FirstRead)
                .Append(" | ").Append(behavior.DisposeMicroseconds.ToString("F1", invariant))
                .Append(" | ").Append(behavior.DisposedReadThrows)
                .AppendLine(" |");
        }

        output.AppendLine();
        output.AppendLine("## Methodology limits");
        output.AppendLine();
        output.AppendLine("- Throughput covers synchronous pull reads and a two-byte-per-chunk checksum; object/worktree open time is reported separately.");
        output.AppendLine("- Read allocation uses GC.GetAllocatedBytesForCurrentThread in a separate unsampled pass. It excludes the single reusable byte-array allocation. Generated ResultUlongGixError.IntoManaged() constructs one managed result class per FFI read, so the derived allocation count equals the call count.");
        output.AppendLine("- Peak managed delta uses sampled GC.GetTotalMemory(false). Peak private delta is a sampled process-level proxy containing native, managed, allocator, JIT, and OS effects; it is not an exact native-only counter.");
        output.AppendLine("- Exact retained native bytes covers only the materialized object's owned Vec<u8> capacity. The worktree stream reports zero because its pipe and gix scratch buffers are not guessed.");
        output.AppendLine("- Each non-empty FFI read performs one native-to-managed buffer copy. Bytes copied equal bytes delivered; gix-internal ODB/decompression and pipe copies are outside the boundary counter.");
        output.AppendLine("- Cancellation is cooperative between pulls: a pre-canceled token makes no FFI call. Disposal drops the native source; it cannot interrupt a read already executing on another thread.");

        return output.ToString();
    }

    private static double Median(IEnumerable<double> values)
    {
        var ordered = values.Order().ToArray();
        return ordered[ordered.Length / 2];
    }

    private static double ToMibPerSecond(long bytes, TimeSpan elapsed) =>
        bytes / 1048576d / elapsed.TotalSeconds;

    private static long PrivateBytes(Process process)
    {
        process.Refresh();
        return process.PrivateMemorySize64;
    }

    private static void Collect()
    {
        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();
    }

    private static string FormatBytes(long bytes)
    {
        const double mib = 1024d * 1024d;
        return $"{bytes / mib:F2} MiB";
    }

    private static string FindGixFfiRoot()
    {
        for (var directory = new DirectoryInfo(AppContext.BaseDirectory);
             directory is not null;
             directory = directory.Parent)
        {
            if (File.Exists(Path.Combine(directory.FullName, "Cargo.toml")) &&
                Directory.Exists(Path.Combine(directory.FullName, "bindings")))
            {
                return directory.FullName;
            }
        }

        throw new DirectoryNotFoundException("Could not locate the gix-ffi root.");
    }

    private static string Git(string repository, params string[] arguments)
    {
        var startInfo = new ProcessStartInfo("git")
        {
            RedirectStandardError = true,
            RedirectStandardOutput = true,
            UseShellExecute = false,
        };
        if (Directory.Exists(repository))
        {
            startInfo.ArgumentList.Add("-C");
            startInfo.ArgumentList.Add(repository);
        }

        foreach (var argument in arguments)
            startInfo.ArgumentList.Add(argument);

        using var process = Process.Start(startInfo)
            ?? throw new InvalidOperationException("Could not start git.");
        var standardOutput = process.StandardOutput.ReadToEnd();
        var standardError = process.StandardError.ReadToEnd();
        process.WaitForExit();

        if (process.ExitCode != 0)
        {
            throw new InvalidOperationException(
                $"git {string.Join(' ', arguments)} failed: {standardError}{standardOutput}");
        }

        return standardOutput.Trim();
    }

    private sealed class BenchmarkRepository : IDisposable
    {
        private readonly DirectoryInfo _parent = Directory.CreateTempSubdirectory();

        public BenchmarkRepository(int payloadBytes)
        {
            Root = Path.Combine(_parent.FullName, "repository");
            using (GixRepository.Init(Root))
            {
            }

            WritePatternFile(Path.Combine(Root, "large.bin"), payloadBytes);
            Git(Root, "config", "user.name", "GixSharp Benchmark");
            Git(Root, "config", "user.email", "benchmark@example.invalid");
            Git(Root, "add", "--", ".");
            Git(Root, "commit", "--quiet", "-m", "benchmark payload");

            Repository = GixRepository.Open(Root);
            ObjectId = new GixObjectId(Git(Root, "rev-parse", "HEAD:large.bin"));
            TreeId = Repository.GetCommitTreeId("HEAD");
        }

        public string Root { get; }

        public GixRepository Repository { get; }

        public GixObjectId ObjectId { get; }

        public GixObjectId TreeId { get; }

        public void Dispose()
        {
            Repository.Dispose();
            foreach (var file in Directory.EnumerateFiles(
                         _parent.FullName,
                         "*",
                         SearchOption.AllDirectories))
            {
                File.SetAttributes(file, FileAttributes.Normal);
            }

            _parent.Delete(recursive: true);
        }

        private static void WritePatternFile(string path, int length)
        {
            using var output = new FileStream(path, FileMode.Create, FileAccess.Write);
            var buffer = new byte[BufferBytes];
            long position = 0;

            while (position < length)
            {
                var count = (int)Math.Min(buffer.Length, length - position);
                for (var index = 0; index < count; index++)
                {
                    var absolute = position + index;
                    buffer[index] = (byte)((absolute * 31 + (absolute >> 8)) & 0xff);
                }

                output.Write(buffer, 0, count);
                position += count;
            }
        }
    }

    private sealed record Source(
        string Name,
        Func<GixRepository.PrototypeByteReaderStream> Open);

    private sealed record SourceMeasurement(
        string Name,
        IReadOnlyList<TimedRun> Timed,
        AllocationRun Allocation,
        MemoryRun Memory,
        BehaviorRun Behavior);

    private sealed record TimedRun(
        int Iteration,
        long Bytes,
        ulong Checksum,
        double OpenMilliseconds,
        double ReadMilliseconds,
        double MibPerSecond,
        ulong FfiCallsAtFirstEof,
        double RepeatedEofMicroseconds,
        ulong RetainedNativeBytes);

    private sealed record AllocationRun(
        long Bytes,
        long OpenAllocatedBytes,
        long ReadAllocatedBytes,
        ulong FfiCalls,
        ulong RetainedNativeBytes);

    private sealed record MemoryRun(
        long Bytes,
        long PeakManagedDelta,
        long PeakPrivateDelta,
        ulong RetainedNativeBytes);

    private sealed record BehaviorRun(
        int ZeroLengthResult,
        bool EofAfterZeroLength,
        bool Canceled,
        ulong CancellationFfiCalls,
        double CancellationMicroseconds,
        int FirstRead,
        double DisposeMicroseconds,
        bool DisposedReadThrows);

    private readonly record struct DrainResult(long Bytes, ulong Checksum);
}
