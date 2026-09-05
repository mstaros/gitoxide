using System.Diagnostics;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class DiffTests
{
    [Test]
    public async Task TargetsProduceOwnedPatchesStatisticsAndPathFilters()
    {
        using var fixture = new Fixture();
        fixture.Write("file.txt", "old\nkeep\n");
        fixture.Commit();
        fixture.Write("file.txt", "staged\nkeep\n");
        fixture.Write("added.txt", "added\n");
        fixture.Git("add", ".");
        fixture.Write("file.txt", "working\nkeep\n");
        fixture.Write("nested/new.txt", "untracked\n");
        using var repo = GixRepository.Open(fixture.Root);
        var index = File.ReadAllBytes(Path.Combine(fixture.Root, ".git", "index"));

        var staged = repo.GetDiff(GitDiffTarget.Staged);
        await Assert.That(staged.FileCount).IsEqualTo(2);
        await Assert.That(staged.LinesAdded).IsEqualTo(2);
        await Assert.That(staged.LinesDeleted).IsEqualTo(1);
        await Assert.That(staged.Patch.Contains("+staged\n")).IsTrue();
        var unstaged = repo.GetDiff(GitDiffTarget.Unstaged);
        await Assert.That(unstaged.FileCount).IsEqualTo(1);
        await Assert.That(unstaged.Patch.Contains("-staged\n+working\n")).IsTrue();

        var all = repo.GetDiff();
        await Assert.That(all.FileCount).IsEqualTo(3);
        await Assert.That(all.LinesAdded).IsEqualTo(3);
        await Assert.That(all.LinesDeleted).IsEqualTo(1);
        await Assert.That(repo.GetDiff(GitDiffTarget.WorkingTree, "nested/**").FileCount).IsEqualTo(1);
        await Assert.That(repo.GetPatch()).IsEqualTo(all.Patch);
        await Assert.That(index.SequenceEqual(
            File.ReadAllBytes(Path.Combine(fixture.Root, ".git", "index")))).IsTrue();

        fixture.Write(".git/all.patch", all.Patch);
        fixture.Git("apply", "--check", "--reverse", ".git/all.patch");
        repo.Dispose();
        var bytes = all.PatchBytes;
        bytes[0] = 0;
        await Assert.That(all.PatchBytes[0]).IsEqualTo((byte)'d');
        await Assert.That(Encoding.UTF8.GetString(all.PatchBytes)).IsEqualTo(all.Patch);
        var replacement = all with { Patch = "replacement\n" };
        await Assert.That(Encoding.UTF8.GetString(replacement.PatchBytes)).IsEqualTo("replacement\n");
        await Assert.That(Encoding.UTF8.GetString(all.PatchBytes)).IsEqualTo(all.Patch);
    }

    [Test]
    public async Task UnbornBinaryAndNoFinalNewlinePreservePatchSemantics()
    {
        using var fixture = new Fixture();
        fixture.Write("empty.txt", "");
        fixture.Write("missing-newline.txt", "last line");
        fixture.Write(".gitignore", "ignored\n");
        fixture.Write("ignored", "ignored content\n");
        using var repo = GixRepository.Open(fixture.Root);
        await Assert.That(repo.GetDiff(GitDiffTarget.Staged).FileCount).IsEqualTo(0);
        await Assert.That(repo.GetDiff(GitDiffTarget.Unstaged).FileCount).IsEqualTo(0);
        var patch = repo.GetDiff(GitDiffTarget.WorkingTree, "*.txt");
        await Assert.That(patch.FileCount).IsEqualTo(2);
        await Assert.That(patch.LinesAdded).IsEqualTo(1);
        await Assert.That(patch.Patch.Contains("\\ No newline at end of file")).IsTrue();
        fixture.Write(".git/new.patch", patch.Patch);
        fixture.Git("apply", "--check", "--reverse", ".git/new.patch");
        File.WriteAllBytes(Path.Combine(fixture.Root, "binary.dat"), [0, 1, 2, 3]);
        var binary = repo.GetDiff(GitDiffTarget.WorkingTree, "binary.dat");
        await Assert.That(binary.FileCount).IsEqualTo(1);
        await Assert.That(binary.LinesAdded).IsEqualTo(0);
        await Assert.That(binary.LinesDeleted).IsEqualTo(0);
        await Assert.That(binary.Patch.Contains("Binary files")).IsTrue();
    }

    [Test]
    public async Task TreeChangesOwnOptionalPathsIdsAndModesAfterNativeDisposal()
    {
        using var fixture = new Fixture();
        fixture.Write("rename.txt", "unique contents preserved through rename\n");
        fixture.Write("delete.txt", "delete me\n");
        fixture.Write("mode.txt", "mode\n");
        var first = fixture.Commit();
        fixture.Git("mv", "rename.txt", "renamed.txt");
        fixture.Git("rm", "-q", "delete.txt");
        fixture.Write("add.txt", "new\n");
        fixture.Git("add", ".");
        fixture.Git("update-index", "--chmod=+x", "mode.txt");
        fixture.Git("update-index", "--add", "--cacheinfo", $"160000,{first},submodule");
        fixture.Git("commit", "-qm", "second");
        var second = fixture.Git("rev-parse", "HEAD");
        fixture.Git("tag", "-am", "annotated", "second");
        using var repo = GixRepository.Open(fixture.Root);
        var changes = repo.GetTreeChanges(first, "second");
        var filtered = repo.GetTreeChanges(first, second, "rename.txt");
        await Assert.That(filtered.Count).IsEqualTo(1);
        await Assert.That(filtered[0].Path).IsEqualTo("renamed.txt");
        var root = repo.GetTreeChanges(null, repo.GetCommitTreeId("HEAD").Value);
        await Assert.That(root.Count).IsEqualTo(4);
        repo.Dispose();

        await Assert.That(changes.Select(x => x.Path).SequenceEqual(
            new[] { "add.txt", "delete.txt", "mode.txt", "renamed.txt", "submodule" })).IsTrue();
        var added = changes.Single(x => x.Path == "add.txt");
        await Assert.That(added.Kind).IsEqualTo(GitTreeChangeKind.Added);
        await Assert.That(added.OldObjectId).IsNull();
        await Assert.That(added.OldPath).IsNull();
        await Assert.That(added.ObjectId.HasValue).IsTrue();
        var deleted = changes.Single(x => x.Path == "delete.txt");
        await Assert.That(deleted.ObjectId).IsNull();
        await Assert.That(deleted.OldObjectId.HasValue).IsTrue();
        var renamed = changes.Single(x => x.Path == "renamed.txt");
        await Assert.That(renamed.Kind).IsEqualTo(GitTreeChangeKind.Renamed);
        await Assert.That(renamed.OldPath).IsEqualTo("rename.txt");
        await Assert.That(renamed.ObjectId).IsEqualTo(renamed.OldObjectId);
        await Assert.That(changes.Single(x => x.Path == "mode.txt").Mode).IsEqualTo(GitFileMode.BlobExecutable);
        var gitlink = changes.Single(x => x.Path == "submodule");
        await Assert.That(gitlink.Mode).IsEqualTo(GitFileMode.GitLink);
        await Assert.That(gitlink.ObjectId!.Value.Value).IsEqualTo(first);
        var bytes = renamed.PathBytes;
        bytes[0] = 0;
        await Assert.That(renamed.PathBytes[0]).IsEqualTo((byte)'r');
        var oldBytes = renamed.OldPathBytes!;
        oldBytes[0] = 0;
        await Assert.That(renamed.OldPathBytes![0]).IsEqualTo((byte)'r');
        var relocated = renamed with { Path = "another path.txt", OldPath = null };
        await Assert.That(Encoding.UTF8.GetString(relocated.PathBytes)).IsEqualTo("another path.txt");
        await Assert.That(relocated.OldPathBytes).IsNull();
        var changedSource = renamed with { OldPath = "another source.txt" };
        await Assert.That(Encoding.UTF8.GetString(changedSource.OldPathBytes!)).IsEqualTo("another source.txt");
        await Assert.That(Encoding.UTF8.GetString(renamed.PathBytes)).IsEqualTo("renamed.txt");
        await Assert.That(Encoding.UTF8.GetString(renamed.OldPathBytes!)).IsEqualTo("rename.txt");
    }

    [Test]
    public async Task InvalidArgumentsAndDisposedRepositoryFailAtManagedBoundary()
    {
        using var fixture = new Fixture();
        fixture.Write("file.txt", "base\n");
        fixture.Commit();
        using var repo = GixRepository.Open(fixture.Root);
        await Assert.That(() => repo.GetDiff((GitDiffTarget)99)).Throws<ArgumentOutOfRangeException>();
        await Assert.That(() => repo.GetDiff(GitDiffTarget.Staged, "")).Throws<ArgumentException>();
        await Assert.That(() => repo.GetTreeChanges("", "HEAD")).Throws<ArgumentException>();
        await Assert.That(() => repo.GetTreeChanges(null, "missing")).Throws<GixException>();
        repo.Dispose();
        await Assert.That(() => repo.GetDiff()).Throws<ObjectDisposedException>();
        await Assert.That(() => repo.GetTreeChanges(null, "HEAD")).Throws<ObjectDisposedException>();
    }

    private sealed class Fixture : IDisposable
    {
        public string Root { get; } = Path.Combine(Path.GetTempPath(), $"gixsharp-diff-{Guid.NewGuid():N}");
        public Fixture()
        {
            Directory.CreateDirectory(Root);
            Git("init", "-q");
            Git("config", "user.name", "Diff Tests");
            Git("config", "user.email", "diff@example.com");
            Git("config", "core.autocrlf", "false");
        }
        public void Write(string path, string contents)
        {
            var fullPath = Path.Combine(Root, path.Replace('/', Path.DirectorySeparatorChar));
            Directory.CreateDirectory(Path.GetDirectoryName(fullPath)!);
            File.WriteAllText(fullPath, contents, new UTF8Encoding(false));
        }
        public string Commit()
        {
            Git("add", ".");
            Git("commit", "-qm", "fixture");
            return Git("rev-parse", "HEAD");
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
            using var process = Process.Start(start) ?? throw new InvalidOperationException("Could not start git.");
            var output = process.StandardOutput.ReadToEnd();
            var error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            if (process.ExitCode != 0) throw new InvalidOperationException($"git {string.Join(' ', arguments)}: {error}");
            return output.Trim();
        }
        public void Dispose()
        {
            if (!Directory.Exists(Root)) return;
            foreach (var path in Directory.EnumerateFiles(Root, "*", SearchOption.AllDirectories))
                File.SetAttributes(path, FileAttributes.Normal);
            Directory.Delete(Root, true);
        }
    }
}
