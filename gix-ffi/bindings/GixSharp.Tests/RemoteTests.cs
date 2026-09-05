using System.Diagnostics;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class RemoteTests
{
    [Test]
    public async Task StoredMetadataRetainsAllUrlsAndResolutionMatchesGit()
    {
        foreach (var bare in new[] { false, true })
        {
            using var fixture = new Fixture(bare);
            fixture.AppendConfig("\n[url \"https://fetch.example/\"]\n insteadOf = short:\n"
                + "[url \"ssh://push.example/\"]\n pushInsteadOf = short:\n"
                + "[remote \"z-empty\"]\n prune = true\n"
                + "[remote \"origin\"]\n url = short:first\n url = short:second\n"
                + "[remote \"push-only\"]\n pushurl = short:push\n"
                + "[remote \"origin\"]\n url = short:third\n");
            Func<IReadOnlyList<GitRemote>> getRemotes = fixture.Repository.GetRemotes;
            var remotes = getRemotes();
            await Assert.That(remotes.Select(x => x.Name).SequenceEqual(["origin", "push-only", "z-empty"])).IsTrue();
            await Assert.That(remotes[0].Url).IsEqualTo("short:first");
            await Assert.That(remotes[0].FetchUrls.SequenceEqual(["short:first", "short:second", "short:third"])).IsTrue();
            await Assert.That(remotes[0].PushUrls.SequenceEqual(remotes[0].FetchUrls)).IsTrue();
            await Assert.That(remotes[1].Url).IsNull();
            await Assert.That(remotes[1].PushUrl).IsEqualTo("short:push");
            await Assert.That(remotes[2].Url).IsNull();
            await Assert.That(remotes[2].PushUrls.Count).IsEqualTo(0);

            var resolved = fixture.Repository.GetRemotes(resolveUrls: true);
            await Assert.That(resolved[0].FetchUrls.SequenceEqual(
                fixture.Git("remote", "get-url", "--all", "origin").Split('\n', StringSplitOptions.RemoveEmptyEntries))).IsTrue();
            await Assert.That(resolved[0].PushUrls.SequenceEqual(
                fixture.Git("remote", "get-url", "--push", "--all", "origin").Split('\n', StringSplitOptions.RemoveEmptyEntries))).IsTrue();
            await Assert.That(resolved[1].PushUrl).IsEqualTo("https://fetch.example/push");
            await Assert.That(remotes[0].Url).IsEqualTo("short:first");
        }
    }

    [Test]
    public async Task NonUtf8BytesAndRecordCopiesRemainOwnedAndConsistentAfterDisposal()
    {
        using var fixture = new Fixture();
        var config = Encoding.UTF8.GetBytes("\n[remote \"raw")
            .Concat(new byte[] { 0xff }).Concat(Encoding.UTF8.GetBytes("\"]\n url = ../first-"))
            .Concat(new byte[] { 0xfe }).Concat(Encoding.UTF8.GetBytes("\n url = ../second\n pushurl = ../push\n")).ToArray();
        fixture.AppendConfig(config);
        var first = fixture.Repository.GetRemotes().Single();
        var same = fixture.Repository.GetRemotes().Single();
        var resolved = fixture.Repository.GetRemotes(resolveUrls: true).Single();
        await Assert.That(first.Equals(same)).IsTrue();
        await Assert.That(first.GetHashCode()).IsEqualTo(same.GetHashCode());
        await Assert.That(resolved.UrlBytes!.SequenceEqual(first.UrlBytes!)).IsTrue();
        var name = first.NameBytes;
        var url = first.FetchUrlBytes[0];
        var push = first.PushUrlsBytes[0];
        name[0] = 0;
        url[0] = 0;
        push[0] = 0;
        await Assert.That(first.NameBytes[^1]).IsEqualTo((byte)0xff);
        await Assert.That(first.UrlBytes![0]).IsEqualTo((byte)'.');
        await Assert.That(first.PushUrlBytes![0]).IsEqualTo((byte)'.');

        var changed = first with { Name = "日本語", Url = "../updated" };
        await Assert.That(changed.NameBytes.SequenceEqual("日本語"u8.ToArray())).IsTrue();
        await Assert.That(changed.UrlBytes!.SequenceEqual("../updated"u8.ToArray())).IsTrue();
        await Assert.That(changed.FetchUrls.SequenceEqual(["../updated", "../second"])).IsTrue();
        await Assert.That(first.UrlBytes![^1]).IsEqualTo((byte)0xfe);
        var cleared = first with { Url = null };
        await Assert.That(cleared.FetchUrls.Count).IsEqualTo(0);
        await Assert.That(cleared.UrlBytes).IsNull();
        await Assert.That(new GitRemote("origin", "../path").PushUrl).IsEqualTo("../path");

        fixture.Repository.Dispose();
        await Assert.That(first.PushUrls.Single()).IsEqualTo("../push");
        await Assert.That(first.FetchUrlBytes[0][^1]).IsEqualTo((byte)0xfe);
        await Assert.That(changed.Url).IsEqualTo("../updated");
    }

    [Test]
    public async Task IncludesAndLinkedWorktreeConfigRefreshWithoutReopening()
    {
        using var fixture = new Fixture();
        fixture.Git("-c", "user.name=Remote Tests", "-c", "user.email=remote@example.com",
            "commit", "--allow-empty", "-qm", "initial");
        fixture.Git("config", "extensions.worktreeConfig", "true");
        var included = Path.Combine(fixture.Parent, "日本語.config");
        File.WriteAllText(included, "[remote \"included\"]\n url = ../before\n", new UTF8Encoding(false));
        fixture.Git("config", "include.path", included.Replace('\\', '/'));
        var linkedPath = Path.Combine(fixture.Parent, "linked");
        fixture.Git("worktree", "add", "-q", "-b", "linked", linkedPath);
        fixture.Git("-C", linkedPath, "config", "--worktree", "remote.private.url", "../private");
        using var linked = GixRepository.Open(linkedPath);
        var snapshot = linked.GetRemotes();
        await Assert.That(snapshot.Count).IsEqualTo(2);
        await Assert.That(fixture.Repository.GetRemotes().Count).IsEqualTo(1);
        var timestamp = File.GetLastWriteTimeUtc(included);
        File.WriteAllText(included, "[remote \"included\"]\n url = ../after!\n", new UTF8Encoding(false));
        File.SetLastWriteTimeUtc(included, timestamp);
        await Assert.That(linked.GetRemotes()[0].Url).IsEqualTo("../after!");
        await Assert.That(linked.GetRemotes(resolveUrls: true)[0].Url).IsEqualTo("../after!");
        await Assert.That(snapshot[0].Url).IsEqualTo("../before");
    }

    [Test]
    public async Task ConfigurationFailuresAreTypedAndStoredUrlsCanBeInspectedBeforeRepair()
    {
        using var fixture = new Fixture();
        fixture.AppendConfig("\n[remote \"broken\"]\n url = http://[\n");
        await Assert.That(fixture.Repository.GetRemotes().Single().Url).IsEqualTo("http://[");
        var invalidUrl = Capture(() => fixture.Repository.GetRemotes(resolveUrls: true));
        await Assert.That(invalidUrl.Kind).IsEqualTo(GixErrorKind.Config);
        await Assert.That(invalidUrl.Operation).IsEqualTo("GetRemotes");

        var config = Path.Combine(fixture.Repository.CommonDirectory, "config");
        var valid = File.ReadAllBytes(config);
        fixture.AppendConfig("\n[core]\n repositoryFormatVersion = broken\n");
        await Assert.That(fixture.Repository.GetRemotes().Count).IsEqualTo(1);
        File.WriteAllText(config, "[broken", new UTF8Encoding(false));
        var invalidSyntax = Capture(() => fixture.Repository.GetRemotes());
        await Assert.That(invalidSyntax.Kind).IsEqualTo(GixErrorKind.Config);
        File.WriteAllBytes(config, valid);
        await Assert.That(fixture.Repository.GetRemotes().Count).IsEqualTo(1);
        fixture.Repository.Dispose();
        await Assert.That(() => fixture.Repository.GetRemotes()).Throws<ObjectDisposedException>();
        await Assert.That(() => fixture.Repository.GetRemotes(true)).Throws<ObjectDisposedException>();
    }

    private static GixException Capture(Action action)
    {
        try { action(); }
        catch (GixException error) { return error; }
        throw new InvalidOperationException("Expected a GixException.");
    }

    private sealed class Fixture : IDisposable
    {
        private readonly DirectoryInfo _parent = Directory.CreateTempSubdirectory("gixsharp-remotes-");
        public Fixture(bool bare = false)
        {
            Root = Path.Combine(_parent.FullName, "repository");
            Repository = GixRepository.Init(Root, bare);
        }
        public string Parent => _parent.FullName;
        public string Root { get; }
        public GixRepository Repository { get; }
        public void AppendConfig(string contents) => AppendConfig(Encoding.UTF8.GetBytes(contents));
        public void AppendConfig(byte[] contents)
        {
            using var file = File.Open(Path.Combine(Repository.CommonDirectory, "config"), FileMode.Append);
            file.Write(contents);
        }
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
            using var process = Process.Start(start) ?? throw new InvalidOperationException("Could not start Git.");
            var output = process.StandardOutput.ReadToEnd();
            var error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            if (process.ExitCode != 0) throw new InvalidOperationException($"git {string.Join(' ', arguments)}: {error}");
            return output.Replace("\r\n", "\n");
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
