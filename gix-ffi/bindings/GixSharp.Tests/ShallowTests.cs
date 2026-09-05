using System.Diagnostics;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class ShallowTests
{
    [Test]
    public async Task EmptyAndMalformedBoundariesAreDistinctAndOwnedValuesSurviveDisposal()
    {
        foreach (var bare in new[] { false, true })
        {
            using var fixture = new Fixture(bare);
            var repo = fixture.Repository;
            await Assert.That(repo.GetShallowCommits()).IsEmpty();
            await Assert.That(File.Exists(repo.ShallowFilePath)).IsFalse();
            var path = repo.ShallowFilePath;
            File.WriteAllText(path, "");
            await Assert.That(repo.GetShallowCommits()).IsEmpty();
            const string id = "1111111111111111111111111111111111111111";
            File.WriteAllText(path, id + "\n");
            var snapshot = repo.GetShallowCommits();
            var pathBytes = repo.ShallowFile();
            await Assert.That(Path.GetFullPath(Encoding.UTF8.GetString(pathBytes)))
                .IsEqualTo(Path.GetFullPath(path));
            pathBytes[0] = 0;
            await Assert.That(repo.ShallowFile()[0]).IsNotEqualTo((byte)0);

            var timestamp = File.GetLastWriteTimeUtc(path);
            File.WriteAllText(path, "not-an-object-id\n");
            File.SetLastWriteTimeUtc(path, timestamp);
            var error = CaptureGix(() => repo.GetShallowCommits());
            await Assert.That(error.Kind).IsEqualTo(GixErrorKind.InvalidId);
            await Assert.That(error.Operation).IsEqualTo("GetShallowCommits");
            repo.Dispose();
            await Assert.That(snapshot.Single().Value).IsEqualTo(id);
            await Assert.That(() => repo.GetShallowCommits()).Throws<ObjectDisposedException>();
            await Assert.That(() => repo.ShallowFile()).Throws<ObjectDisposedException>();
            await Assert.That(() => repo.ShallowFilePath).Throws<ObjectDisposedException>();
        }
    }

    [Test]
    public async Task RealCloneAndLinkedWorktreesExposeTheCurrentCommonBoundary()
    {
        using var fixture = new Fixture();
        fixture.Git("config", "user.name", "Shallow Tests");
        fixture.Git("config", "user.email", "shallow@example.com");
        fixture.Git("commit", "--allow-empty", "-qm", "first");
        var first = fixture.Git("rev-parse", "HEAD");
        fixture.Git("commit", "--allow-empty", "-qm", "second");
        var second = fixture.Git("rev-parse", "HEAD");
        var clonePath = Path.Combine(fixture.Parent, "clone");
        fixture.Git("clone", "--no-local", "--depth", "1", fixture.Root, clonePath);
        using var clone = GixRepository.Open(clonePath);
        await Assert.That(clone.GetShallowCommits().Single().Value).IsEqualTo(second);
        fixture.Git("-C", clonePath, "fetch", "--deepen", "1");
        await Assert.That(clone.GetShallowCommits().Single().Value).IsEqualTo(first);
        fixture.Git("-C", clonePath, "fetch", "--unshallow");
        await Assert.That(clone.GetShallowCommits()).IsEmpty();

        var linkedPath = Path.Combine(fixture.Parent, "linked");
        fixture.Git("worktree", "add", "--detach", linkedPath, "HEAD");
        using var linked = GixRepository.Open(linkedPath);
        File.WriteAllText(fixture.Repository.ShallowFilePath, second + "\n");
        await Assert.That(Path.GetFullPath(linked.ShallowFilePath))
            .IsEqualTo(Path.GetFullPath(fixture.Repository.ShallowFilePath));
        await Assert.That(linked.GetShallowCommits().Single().Value).IsEqualTo(second);
    }

    [Test]
    public async Task ConfiguredShallowLocationKeepsUnicodePathBytes()
    {
        using var fixture = new Fixture();
        const string relative = "info/日本語 shallow";
        fixture.Git("config", "gitoxide.core.shallowFile", relative);
        using var repo = GixRepository.Open(fixture.Root);
        var expected = Path.Combine(repo.CommonDirectory, "info", "日本語 shallow");
        await Assert.That(Path.GetFullPath(repo.ShallowFilePath)).IsEqualTo(Path.GetFullPath(expected));
        await Assert.That(Path.GetFullPath(Encoding.UTF8.GetString(repo.ShallowFile())))
            .IsEqualTo(Path.GetFullPath(expected));
        await Assert.That(File.Exists(expected)).IsFalse();
        Directory.CreateDirectory(Path.GetDirectoryName(expected)!);
        const string id = "1111111111111111111111111111111111111111";
        File.WriteAllText(expected, id + "\n");
        await Assert.That(repo.GetShallowCommits().Single().Value).IsEqualTo(id);
    }

    private static GixException CaptureGix(Action action)
    {
        try { action(); }
        catch (GixException exception) { return exception; }
        throw new InvalidOperationException("Expected a GixException.");
    }

    private sealed class Fixture : IDisposable
    {
        private readonly DirectoryInfo _parent = Directory.CreateTempSubdirectory("gixsharp-shallow-");
        public Fixture(bool bare = false)
        {
            Root = Path.Combine(_parent.FullName, "repository");
            Repository = GixRepository.Init(Root, bare);
        }
        public string Parent => _parent.FullName;
        public string Root { get; }
        public GixRepository Repository { get; }
        public string Git(params string[] arguments)
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
            var output = process.StandardOutput.ReadToEnd();
            var error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            if (process.ExitCode != 0) throw new InvalidOperationException($"git {string.Join(' ', arguments)}: {error}");
            return output.Trim();
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
