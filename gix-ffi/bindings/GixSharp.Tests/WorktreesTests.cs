using System.Diagnostics;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class WorktreesTests
{
    [Test]
    public async Task AddOverloadsKeepExplicitNamesBranchesAndCheckoutSelection()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        await Assert.That(repository.GetWorktrees().Count).IsEqualTo(0);
        var emptyPath = fixture.PathFor("different-basename");
        var empty = repository.AddWorktree("naked", emptyPath, null, false, false);
        await Assert.That(empty.Name).IsEqualTo("naked");
        await Assert.That(empty.IsValid).IsTrue();
        await Assert.That(File.Exists(Path.Combine(emptyPath, "tracked"))).IsFalse();
        await Assert.That(Directory.EnumerateFileSystemEntries(emptyPath).Count()).IsEqualTo(1);
        await Assert.That(fixture.GitAt(emptyPath, "symbolic-ref", "HEAD").Trim()).IsEqualTo("refs/heads/naked");

        var defaultPath = fixture.PathFor("default-checkout");
        var created = repository.AddWorktree("default", defaultPath);
        await Assert.That(Path.GetFullPath(created.Path)).IsEqualTo(Path.GetFullPath(defaultPath));
        await Assert.That(File.ReadAllText(Path.Combine(defaultPath, "tracked"))).IsEqualTo("committed\n");
        fixture.Git("branch", "existing");
        var attached = repository.AddWorktree("attached", fixture.PathFor("attached"), "refs/heads/existing");
        await Assert.That(fixture.GitAt(attached.Path, "symbolic-ref", "HEAD").Trim()).IsEqualTo("refs/heads/existing");
        await Assert.That(() => repository.AddWorktree("steal", fixture.PathFor("steal"), "existing")).Throws<GixException>();
        using var linked = GixRepository.Open(defaultPath);
        await Assert.That(Path.GetFullPath(linked.CommonDirectory)).IsEqualTo(Path.GetFullPath(repository.CommonDirectory));
        await Assert.That(linked.GetWorktrees().Select(item => item.Name)
            .SequenceEqual(repository.GetWorktrees().Select(item => item.Name))).IsTrue();
        await Assert.That(repository.GetWorktrees().Count).IsEqualTo(3);
    }

    [Test]
    public async Task DetachedCreationAndOwnedByteRecordsSurviveRepositoryDisposal()
    {
        foreach (var bare in new[] { false, true })
        {
            using var fixture = new Fixture(bare);
            var repository = fixture.Repository;
            var commit = new GixObjectId(fixture.Git("rev-parse", "HEAD").Trim());
            var path = fixture.PathFor("日本語 checkout");
            repository.AddDetachedWorktree("日本語"u8.ToArray(), Encoding.UTF8.GetBytes(path), commit, true);
            using (var linked = GixRepository.Open(path))
                await Assert.That(linked.Head().IsDetached).IsTrue();
            var lockPath = Path.Combine(repository.CommonDirectory, "worktrees", "日本語", "locked");
            byte[] reason = [32, 0xff, 0, 13, 10];
            File.WriteAllBytes(lockPath, reason);
            var info = repository.GetWorktrees().Single();
            repository.Dispose();
            await Assert.That(info.NameBytes.SequenceEqual("日本語"u8.ToArray())).IsTrue();
            await Assert.That(info.PathBytes.SequenceEqual(Encoding.UTF8.GetBytes(info.Path))).IsTrue();
            await Assert.That(info.LockReasonBytes!.SequenceEqual(reason)).IsTrue();
            var copy = info.LockReasonBytes!;
            copy[0] = 0;
            await Assert.That(info.LockReasonBytes![0]).IsEqualTo((byte)32);
            var edited = info with { Name = "renamed", Path = "new path", LockReason = "new reason" };
            await Assert.That(edited.NameBytes.SequenceEqual("renamed"u8.ToArray())).IsTrue();
            await Assert.That(edited.PathBytes.SequenceEqual("new path"u8.ToArray())).IsTrue();
            await Assert.That(edited.LockReasonBytes!.SequenceEqual("new reason"u8.ToArray())).IsTrue();
            await Assert.That((info with { LockReason = null }).LockReasonBytes).IsNull();
        }
    }

    [Test]
    public async Task ExactAndAllPruningRespectStaleValidLockedAndMissingStates()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var commit = new GixObjectId(fixture.Git("rev-parse", "HEAD").Trim());
        var stale = repository.AddDetachedWorktree("stale", fixture.PathFor("stale"), commit);
        var locked = repository.AddDetachedWorktree("locked", fixture.PathFor("locked"), commit, true);
        var valid = repository.AddWorktree("valid"u8.ToArray(), Encoding.UTF8.GetBytes(fixture.PathFor("valid")));
        Directory.Delete(stale.Path, true);
        Directory.Delete(locked.Path, true);
        await Assert.That(repository.GetWorktrees().Single(item => item.Name == "stale").IsValid).IsFalse();
        await Assert.That(repository.PruneWorktree("missing")).IsFalse();
        await Assert.That(repository.PruneWorktree("locked")).IsFalse();
        await Assert.That(repository.PruneWorktree("valid")).IsFalse();
        await Assert.That(repository.PruneWorktrees()).IsEqualTo(1);
        await Assert.That(repository.PruneWorktree("locked"u8.ToArray(), includeLocked: true)).IsTrue();
        await Assert.That(repository.PruneWorktree("locked", includeLocked: true)).IsFalse();
        await Assert.That(repository.PruneWorktrees(includeValid: true, removeWorkingTrees: true)).IsEqualTo(1);
        await Assert.That(Directory.Exists(valid.Path)).IsFalse();
        await Assert.That(repository.GetWorktrees().Count).IsEqualTo(0);
    }

    [Test]
    public async Task MetadataOnlyPruningKeepsFilesAndLockOverrideStillProtectsDirtyWork()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var metadata = repository.AddWorktree("metadata", fixture.PathFor("metadata"));
        File.WriteAllText(Path.Combine(metadata.Path, "tracked"), "edited");
        File.WriteAllBytes(Path.Combine(metadata.Path, "untracked"), [0xff, 0, 10]);
        await Assert.That(repository.PruneWorktree("metadata", includeValid: true)).IsTrue();
        await Assert.That(File.ReadAllText(Path.Combine(metadata.Path, "tracked"))).IsEqualTo("edited");
        await Assert.That(File.ReadAllBytes(Path.Combine(metadata.Path, "untracked")).SequenceEqual(new byte[] { 0xff, 0, 10 })).IsTrue();
        await Assert.That(File.Exists(Path.Combine(metadata.Path, ".git"))).IsTrue();

        var dirty = repository.AddWorktree("dirty", fixture.PathFor("dirty"), lockWorktree: true);
        File.WriteAllText(Path.Combine(dirty.Path, "untracked"), "keep me");
        fixture.Git("config", "status.showUntrackedFiles", "no");
        await Assert.That(() => repository.PruneWorktree("dirty", true, true, true)).Throws<GixException>();
        await Assert.That(File.ReadAllText(Path.Combine(dirty.Path, "untracked"))).IsEqualTo("keep me");
        await Assert.That(repository.GetWorktrees().Single().IsLocked).IsTrue();
        File.Delete(Path.Combine(dirty.Path, "untracked"));
        await Assert.That(repository.PruneWorktree("dirty", true, true, true)).IsTrue();
        await Assert.That(Directory.Exists(dirty.Path)).IsFalse();
    }

    [Test]
    public async Task InvalidNamesIdsAndDisposedCallsFailWithoutCreatingWorktrees()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var commit = new GixObjectId(fixture.Git("rev-parse", "HEAD").Trim());
        var path = fixture.PathFor("invalid");
        await Assert.That(() => repository.AddWorktree(" ", path)).Throws<ArgumentException>();
        await Assert.That(() => repository.AddWorktree([], Encoding.UTF8.GetBytes(path))).Throws<ArgumentException>();
        await Assert.That(() => repository.AddWorktree("name", path, " ")).Throws<ArgumentException>();
        await Assert.That(() => repository.AddDetachedWorktree("name", path, default)).Throws<ArgumentException>();
        await Assert.That(() => repository.AddDetachedWorktree("../escape", path, commit)).Throws<GixException>();
        await Assert.That(() => repository.PruneWorktree("../escape", true, true, true)).Throws<GixException>();
        var blob = repository.WriteBlob("not a commit"u8.ToArray());
        await Assert.That(() => repository.AddDetachedWorktree("name", path, blob)).Throws<GixException>();
        await Assert.That(Directory.Exists(path)).IsFalse();
        repository.Dispose();
        await Assert.That(() => repository.GetWorktrees()).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.AddWorktree("name", path)).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.AddDetachedWorktree("name", path, commit)).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.PruneWorktrees()).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.PruneWorktree("name")).Throws<ObjectDisposedException>();
    }

    private sealed class Fixture : IDisposable
    {
        private readonly DirectoryInfo _root = Directory.CreateTempSubdirectory("gixsharp-worktrees-");
        public Fixture(bool bare = false)
        {
            Main = Path.Combine(_root.FullName, "main");
            Directory.CreateDirectory(Main);
            Git("init", "-q", "-b", "main");
            Git("config", "user.name", "Worktree Tests");
            Git("config", "user.email", "worktree@example.com");
            Git("config", "core.autocrlf", "false");
            File.WriteAllText(Path.Combine(Main, "tracked"), "committed\n");
            Git("add", "tracked");
            Git("commit", "-q", "-m", "fixture");
            if (bare)
            {
                var barePath = Path.Combine(_root.FullName, "bare.git");
                Git("clone", "--bare", "-q", Main, barePath);
                Main = barePath;
                Git("config", "user.name", "Worktree Tests");
                Git("config", "user.email", "worktree@example.com");
                Git("config", "core.autocrlf", "false");
            }
            Repository = GixRepository.Open(Main);
        }
        public string Main { get; private set; }
        public GixRepository Repository { get; }
        public string PathFor(string name) => Path.Combine(_root.FullName, name);
        public string Git(params string[] arguments) => GitAt(Main, arguments);
        public string GitAt(string path, params string[] arguments)
        {
            var start = new ProcessStartInfo("git")
            {
                WorkingDirectory = path, RedirectStandardOutput = true, RedirectStandardError = true,
                UseShellExecute = false, CreateNoWindow = true, StandardOutputEncoding = Encoding.UTF8,
            };
            foreach (var argument in arguments) start.ArgumentList.Add(argument);
            start.Environment.Remove("GIT_DIR");
            start.Environment.Remove("GIT_WORK_TREE");
            start.Environment.Remove("GIT_CONFIG_COUNT");
            using var process = Process.Start(start) ?? throw new InvalidOperationException("Could not start git.");
            var output = process.StandardOutput.ReadToEnd();
            var error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            if (process.ExitCode != 0) throw new InvalidOperationException($"git {string.Join(' ', arguments)}: {error}");
            return output;
        }
        public void Dispose()
        {
            Repository.Dispose();
            foreach (var path in Directory.EnumerateFiles(_root.FullName, "*", SearchOption.AllDirectories))
                File.SetAttributes(path, FileAttributes.Normal);
            _root.Delete(recursive: true);
        }
    }
}
