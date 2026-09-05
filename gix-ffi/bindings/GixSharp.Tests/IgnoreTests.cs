using System.Diagnostics;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class IgnoreTests
{
    [Test]
    public async Task MatchingUsesExactPathsDirectoryFlagsAndCurrentRulesEvenForTrackedFiles()
    {
        using var fixture = new Fixture();
        fixture.Write("tracked.log", "tracked\n");
        fixture.Repository.Stage("tracked.log");
        fixture.Write(".gitignore", "*.log\n!keep.log\n/cache/\n/日本語.txt\n");

        await Assert.That(fixture.Repository.IsPathIgnored("tracked.log", false)).IsTrue();
        await Assert.That(fixture.Repository.IsPathIgnored("keep.log"u8.ToArray(), false)).IsFalse();
        await Assert.That(fixture.Repository.IsPathIgnored("cache", true)).IsTrue();
        await Assert.That(fixture.Repository.IsPathIgnored("cache", false)).IsFalse();
        await Assert.That(fixture.Repository.IsPathIgnored("日本語.txt", false)).IsTrue();
        await Assert.That(fixture.Repository.IsPathIgnored("日本語.txt"u8.ToArray(), false)).IsTrue();

        var ignorePath = Path.Combine(fixture.Root, ".gitignore");
        var timestamp = File.GetLastWriteTimeUtc(ignorePath);
        fixture.Write(".gitignore", "!tracked.log\n");
        File.SetLastWriteTimeUtc(ignorePath, timestamp);
        await Assert.That(fixture.Repository.IsPathIgnored("tracked.log", false)).IsFalse();
        await Assert.That(fixture.Repository.GetIndexEntries().Single().Path).IsEqualTo("tracked.log");
    }

    [Test]
    public async Task LocalExcludesPreserveBytesAndLiteralPathsAndAreIdempotentInBareRepositoriesToo()
    {
        foreach (var bare in new[] { false, true })
        {
            using var fixture = new Fixture(bare);
            var exclude = Path.Combine(fixture.Repository.CommonDirectory, "info", "exclude");
            Directory.CreateDirectory(Path.GetDirectoryName(exclude)!);
            byte[] original = [(byte)'#', (byte)' ', 0xff, 13, 10, (byte)'#', (byte)'x'];
            File.WriteAllBytes(exclude, original);
            const string name = "日本語 [ab].txt ";

            fixture.Repository.EnsureLocalExclude(name, false);
            var expected = original.Concat(Encoding.UTF8.GetBytes("\n/日本語\\ \\[ab\\].txt\\ \n")).ToArray();
            await Assert.That(File.ReadAllBytes(exclude).SequenceEqual(expected)).IsTrue();
            fixture.Repository.EnsureLocalExclude(Encoding.UTF8.GetBytes(name), false);
            await Assert.That(File.ReadAllBytes(exclude).SequenceEqual(expected)).IsTrue();
            await Assert.That(fixture.Repository.IsPathIgnored(name, false)).IsTrue();
            await Assert.That(fixture.Repository.IsPathIgnored("日本語 a.txt ", false)).IsFalse();
            await Assert.That(fixture.Repository.IsPathIgnored("nested/" + name, false)).IsFalse();

            fixture.Repository.EnsureLocalExclude("cache"u8.ToArray(), true);
            await Assert.That(fixture.Repository.IsPathIgnored("cache", true)).IsTrue();
            await Assert.That(fixture.Repository.IsPathIgnored("cache", false)).IsFalse();
            await Assert.That(File.Exists(exclude + ".lock")).IsFalse();
        }
    }

    [Test]
    public async Task LinkedWorktreesWriteAndReadTheCommonExcludeFile()
    {
        using var fixture = new Fixture();
        fixture.Git("-c", "user.name=Ignore Tests", "-c", "user.email=ignore@example.com",
            "commit", "--allow-empty", "-qm", "fixture");
        var linkedPath = Path.Combine(fixture.Parent, "linked");
        fixture.Git("worktree", "add", "--detach", linkedPath, "HEAD");
        using var linked = GixRepository.Open(linkedPath);

        linked.EnsureLocalExclude("shared.cache", false);

        await Assert.That(Path.GetFullPath(linked.CommonDirectory))
            .IsEqualTo(Path.GetFullPath(fixture.Repository.CommonDirectory));
        await Assert.That(linked.IsPathIgnored("shared.cache", false)).IsTrue();
        await Assert.That(fixture.Repository.IsPathIgnored("shared.cache", false)).IsTrue();
        await Assert.That(File.ReadAllText(Path.Combine(linked.CommonDirectory, "info", "exclude"))
            .Contains("/shared.cache\n")).IsTrue();
        await Assert.That(File.Exists(Path.Combine(linked.RepositoryPath, "info", "exclude"))).IsFalse();
    }

    [Test]
    public async Task WriteFailuresRetainForeignLocksAndTranslateTheNativeOperation()
    {
        using var fixture = new Fixture();
        var exclude = Path.Combine(fixture.Repository.CommonDirectory, "info", "exclude");
        Directory.CreateDirectory(Path.GetDirectoryName(exclude)!);
        File.WriteAllText(exclude, "# original\n");
        File.WriteAllText(exclude + ".lock", "foreign owner");
        try
        {
            var locked = CaptureGix(() => fixture.Repository.EnsureLocalExclude("locked", false));
            await Assert.That(locked.Kind).IsEqualTo(GixErrorKind.Io);
            await Assert.That(locked.Operation).IsEqualTo("EnsureLocalExclude");
            await Assert.That(File.ReadAllText(exclude + ".lock")).IsEqualTo("foreign owner");
            await Assert.That(File.ReadAllText(exclude)).IsEqualTo("# original\n");
        }
        finally
        {
            File.Delete(exclude + ".lock");
        }

        fixture.Write(".gitignore", "!included\n");
        var overridden = CaptureGix(() => fixture.Repository.EnsureLocalExclude("included", false));
        await Assert.That(overridden.Kind).IsEqualTo(GixErrorKind.Other);
        await Assert.That(overridden.Operation).IsEqualTo("EnsureLocalExclude");
        await Assert.That(File.ReadAllText(exclude).Contains("/included\n")).IsTrue();
        await Assert.That(fixture.Repository.IsPathIgnored("included", false)).IsFalse();
        await Assert.That(File.Exists(exclude + ".lock")).IsFalse();
    }

    [Test]
    public async Task InvalidArgumentsAndDisposedRepositoriesFailAtTheManagedBoundary()
    {
        using var fixture = new Fixture();
        await Assert.That(() => fixture.Repository.IsPathIgnored((string)null!, false))
            .Throws<ArgumentNullException>();
        await Assert.That(() => fixture.Repository.EnsureLocalExclude((byte[])null!, false))
            .Throws<ArgumentNullException>();
        foreach (var path in new[] { "", "/root", "trailing/", "a//b", ".", "a/../b", "a/./b", "a\\b", "a\0b", "a\nb", "a\rb" })
        {
            await Assert.That(() => fixture.Repository.IsPathIgnored(path, false)).Throws<ArgumentException>();
            await Assert.That(() => fixture.Repository.EnsureLocalExclude(Encoding.UTF8.GetBytes(path), false))
                .Throws<ArgumentException>();
        }
        await Assert.That(() => fixture.Repository.EnsureLocalExclude("\ud800", false))
            .Throws<ArgumentException>();
        fixture.Repository.Dispose();
        await Assert.That(() => fixture.Repository.IsPathIgnored("valid", false)).Throws<ObjectDisposedException>();
        await Assert.That(() => fixture.Repository.EnsureLocalExclude("valid"u8.ToArray(), false))
            .Throws<ObjectDisposedException>();
    }

    private static GixException CaptureGix(Action action)
    {
        try
        {
            action();
        }
        catch (GixException exception)
        {
            return exception;
        }
        throw new InvalidOperationException("Expected a GixException.");
    }

    private sealed class Fixture : IDisposable
    {
        private readonly DirectoryInfo _parent = Directory.CreateTempSubdirectory("gixsharp-ignore-");
        public Fixture(bool bare = false)
        {
            Root = Path.Combine(_parent.FullName, "repository");
            Repository = GixRepository.Init(Root, bare);
        }
        public string Parent => _parent.FullName;
        public string Root { get; }
        public GixRepository Repository { get; }
        public void Write(string path, string contents)
        {
            var fullPath = Path.Combine(Root, path.Replace('/', Path.DirectorySeparatorChar));
            Directory.CreateDirectory(Path.GetDirectoryName(fullPath)!);
            File.WriteAllText(fullPath, contents, new UTF8Encoding(false));
        }
        public void Git(params string[] arguments)
        {
            var start = new ProcessStartInfo("git")
            {
                WorkingDirectory = Root,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true,
            };
            foreach (var argument in arguments) start.ArgumentList.Add(argument);
            using var process = Process.Start(start) ?? throw new InvalidOperationException("Could not start git.");
            _ = process.StandardOutput.ReadToEnd();
            var error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            if (process.ExitCode != 0) throw new InvalidOperationException($"git {string.Join(' ', arguments)}: {error}");
        }
        public void Dispose()
        {
            Repository.Dispose();
            foreach (var path in Directory.EnumerateFiles(Parent, "*", SearchOption.AllDirectories))
                File.SetAttributes(path, FileAttributes.Normal);
            _parent.Delete(recursive: true);
        }
    }
}
