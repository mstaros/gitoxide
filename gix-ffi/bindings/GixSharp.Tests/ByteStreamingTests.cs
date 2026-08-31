using System.Diagnostics;
using TUnit.Core;

namespace GixSharp.Tests;

public sealed class ByteStreamingTests
{
    private const int TestPayloadBytes = (5 * 1024 * 1024) + 123;
    private const int BufferBytes = 64 * 1024;

    [Test]
    public async Task MaterializedObject_PullsIntoReusableBuffer_AndHasStableEof()
    {
        using var fixture = new TempRepository();
        fixture.WritePatternFile("large.bin", TestPayloadBytes);
        var objectId = new GixObjectId(
            Git(fixture.Root, "hash-object", "-w", "--", "large.bin"));

        using var stream =
            fixture.Repository.OpenObjectByteStreamForPrototype(objectId);

        await Assert.That(stream.CanRead).IsTrue();
        await Assert.That(stream.CanSeek).IsFalse();
        await Assert.That(stream.CanWrite).IsFalse();
        await Assert.That(stream.HasKnownLength).IsTrue();
        await Assert.That(stream.Length).IsEqualTo(TestPayloadBytes);
        await Assert.That(stream.RetainedNativeBytes)
            .IsGreaterThanOrEqualTo((ulong)TestPayloadBytes);
        await Assert.That(stream.Read(Span<byte>.Empty)).IsEqualTo(0);
        await Assert.That(stream.EofObserved).IsFalse();

        // The native reader owns its source after construction.
        fixture.Repository.Dispose();

        var buffer = new byte[BufferBytes];
        long position = 0;
        while (true)
        {
            var read = stream.Read(buffer);
            if (read == 0)
                break;

            AssertPattern(buffer.AsSpan(0, read), position);
            position += read;
        }

        await Assert.That(position).IsEqualTo(TestPayloadBytes);
        await Assert.That(stream.Position).IsEqualTo(TestPayloadBytes);
        await Assert.That(stream.BytesDelivered).IsEqualTo((ulong)TestPayloadBytes);
        await Assert.That(stream.EofObserved).IsTrue();

        var callsAtFirstEof = stream.FfiReadCalls;
        await Assert.That(stream.Read(buffer)).IsEqualTo(0);
        await Assert.That(stream.FfiReadCalls).IsEqualTo(callsAtFirstEof + 1);
    }

    [Test]
    public async Task Read_CancellationAndEarlyDisposal_AreBoundedBetweenPulls()
    {
        using var fixture = new TempRepository();
        fixture.WritePatternFile("large.bin", TestPayloadBytes);
        var objectId = new GixObjectId(
            Git(fixture.Root, "hash-object", "-w", "--", "large.bin"));
        var stream = fixture.Repository.OpenObjectByteStreamForPrototype(objectId);
        var buffer = new byte[BufferBytes];

        using var cancellation = new CancellationTokenSource();
        cancellation.Cancel();
        var callsBeforeCancellation = stream.FfiReadCalls;
        var canceled = false;
        try
        {
            _ = await stream.ReadAsync(buffer, cancellation.Token);
        }
        catch (OperationCanceledException)
        {
            canceled = true;
        }

        await Assert.That(canceled).IsTrue();
        await Assert.That(stream.FfiReadCalls).IsEqualTo(callsBeforeCancellation);
        await Assert.That(stream.Read(buffer)).IsGreaterThan(0);

        var stopwatch = Stopwatch.StartNew();
        stream.Dispose();
        stream.Dispose();
        stopwatch.Stop();

        await Assert.That(stopwatch.Elapsed).IsLessThan(TimeSpan.FromSeconds(2));
        await Assert.That(() => stream.Read(buffer))
            .Throws<ObjectDisposedException>();
    }

    [Test]
    public async Task WorktreeSource_UsesGixReadStream_WithoutWholePayloadRetention()
    {
        using var fixture = new TempRepository();
        fixture.WritePatternFile("large.bin", TestPayloadBytes);
        fixture.CommitAllAndReopen();

        var treeId = fixture.Repository.GetCommitTreeId("HEAD");
        using var stream =
            fixture.Repository.OpenWorktreeByteStreamForPrototype(treeId);

        await Assert.That(stream.HasKnownLength).IsFalse();
        await Assert.That(() => stream.Length).Throws<NotSupportedException>();
        await Assert.That(stream.RetainedNativeBytes).IsEqualTo(0UL);

        // The pipe-backed gix stream also owns everything needed after opening.
        fixture.Repository.Dispose();

        var buffer = new byte[BufferBytes];
        long total = 0;
        while (true)
        {
            var read = stream.Read(buffer);
            if (read == 0)
                break;
            total += read;
        }

        // The transient worktree stream carries entry metadata in addition to bytes.
        await Assert.That(total).IsGreaterThan(TestPayloadBytes);
        await Assert.That(stream.BytesDelivered).IsEqualTo((ulong)total);
        await Assert.That(stream.FfiReadCalls).IsGreaterThan(1UL);
        await Assert.That(stream.EofObserved).IsTrue();
    }

    private static void AssertPattern(ReadOnlySpan<byte> actual, long offset)
    {
        for (var index = 0; index < actual.Length; index++)
        {
            var position = offset + index;
            var expected = (byte)((position * 31 + (position >> 8)) & 0xff);
            if (actual[index] != expected)
            {
                throw new InvalidDataException(
                    $"Pattern mismatch at {position}: expected {expected}, got {actual[index]}.");
            }
        }
    }

    private static string Git(string repository, params string[] arguments)
    {
        var startInfo = new ProcessStartInfo("git")
        {
            RedirectStandardError = true,
            RedirectStandardOutput = true,
            UseShellExecute = false,
        };
        startInfo.ArgumentList.Add("-C");
        startInfo.ArgumentList.Add(repository);
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

    private sealed class TempRepository : IDisposable
    {
        private readonly DirectoryInfo _parent = Directory.CreateTempSubdirectory();

        public TempRepository()
        {
            Root = Path.Combine(_parent.FullName, "repository");
            Repository = GixRepository.Init(Root);
        }

        public string Root { get; }

        public GixRepository Repository { get; private set; }

        public void WritePatternFile(string relativePath, int length)
        {
            var path = Path.Combine(Root, relativePath);
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
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

        public void CommitAllAndReopen()
        {
            Repository.Dispose();
            Git(Root, "config", "user.name", "GixSharp Tests");
            Git(Root, "config", "user.email", "tests@example.invalid");
            Git(Root, "add", "--", ".");
            Git(Root, "commit", "--quiet", "-m", "large payload");
            Repository = GixRepository.Open(Root);
        }

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
    }
}
