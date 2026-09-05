using System.Reflection;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class IndexTests
{
    [Test]
    public async Task RepeatedStatusAndStage_UseCurrentIndexWithAnUnchangedTimestamp()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, "tracked.txt", "baseline\n");
        fixture.Repository.Stage();
        _ = Commit(fixture.Repository, "baseline");
        var baselineTree = fixture.Repository.WriteIndexTree();
        await Assert.That(fixture.Repository.GetStatus()).IsEmpty();
        var indexPath = Path.Combine(fixture.Repository.RepositoryPath, "index");
        var pinned = File.GetLastWriteTimeUtc(indexPath);

        Write(fixture.Root, "tracked.txt", "staged content with a different size\n");
        fixture.Repository.Stage("tracked.txt");
        File.SetLastWriteTimeUtc(indexPath, pinned);
        var staged = fixture.Repository.GetStatus().Single();

        Write(fixture.Root, "tracked.txt", "baseline\n");
        fixture.Repository.Stage("tracked.txt");
        var restoredTree = fixture.Repository.WriteIndexTree();
        var restoredStatus = fixture.Repository.GetStatus();

        await Assert.That(staged.Status).IsEqualTo(GitFileStatus.ModifiedInIndex);
        await Assert.That(restoredTree).IsEqualTo(baselineTree);
        await Assert.That(restoredStatus).IsEmpty();
    }

    [Test]
    public async Task Stage_HandlesPathspecsIgnoredFilesDeletionsAndRawPaths()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, ".gitignore", "ignored.txt\n");
        Write(fixture.Root, "tracked.txt", "baseline\n");
        fixture.Repository.Stage(".gitignore", "tracked.txt");
        _ = Commit(fixture.Repository, "baseline");

        File.Delete(Path.Combine(fixture.Root, "tracked.txt"));
        Write(fixture.Root, "nested/value.txt", "nested\n");
        Write(fixture.Root, "skipped.txt", "skipped\n");
        Write(fixture.Root, "ignored.txt", "ignored\n");

        fixture.Repository.Stage("nested/value.txt");
        var selected = ByPath(fixture.Repository.GetIndexEntries());
        await Assert.That(selected.ContainsKey("nested/value.txt")).IsTrue();
        await Assert.That(selected.ContainsKey("skipped.txt")).IsFalse();
        await Assert.That(selected.ContainsKey("tracked.txt")).IsTrue();

        fixture.Repository.Stage("*");
        var all = ByPath(fixture.Repository.GetIndexEntries());

        await Assert.That(all.Keys.Order().SequenceEqual(
                [".gitignore", "nested/value.txt", "skipped.txt"]))
            .IsTrue();
        await Assert.That(all.Values.All(static entry => entry.Stage == 0)).IsTrue();
        await Assert.That(all.ContainsKey("tracked.txt")).IsFalse();
        await Assert.That(all.ContainsKey("ignored.txt")).IsFalse();

        var rawPath = all["nested/value.txt"].PathBytes;
        await Assert.That(rawPath.SequenceEqual(
                Encoding.UTF8.GetBytes("nested/value.txt")))
            .IsTrue();

        rawPath[0] = (byte)'X';
        await Assert.That(all["nested/value.txt"].PathBytes[0])
            .IsEqualTo((byte)'n');
    }

    [Test]
    public async Task Unstage_RestoresHeadWithoutChangingWorktree_AndHandlesUnborn()
    {
        using (var fixture = new TempRepository())
        {
            Write(fixture.Root, "tracked.txt", "baseline\n");
            fixture.Repository.Stage();
            _ = Commit(fixture.Repository, "baseline");
            var headTree = fixture.Repository.GetCommitTreeId("HEAD").Value;

            Write(
                fixture.Root,
                "tracked.txt",
                "working copy is different and substantially longer\n");
            fixture.Repository.Stage("tracked.txt");
            var stagedTree = fixture.Repository.WriteIndexTree();

            await Assert.That(stagedTree).IsNotEqualTo(headTree);

            fixture.Repository.Unstage("tracked.txt");

            await Assert.That(fixture.Repository.WriteIndexTree())
                .IsEqualTo(headTree);
            await Assert.That(File.ReadAllText(
                    Path.Combine(fixture.Root, "tracked.txt")))
                .IsEqualTo(
                    "working copy is different and substantially longer\n");
        }

        using (var unborn = new TempRepository())
        {
            Write(unborn.Root, "new.txt", "unborn\n");
            unborn.Repository.Stage();
            await Assert.That(unborn.Repository.GetIndexEntries()).Count().IsEqualTo(1);

            unborn.Repository.Unstage();

            await Assert.That(unborn.Repository.GetIndexEntries()).IsEmpty();
            await Assert.That(File.Exists(Path.Combine(unborn.Root, "new.txt"))).IsTrue();
            await Assert.That(unborn.Repository.Head().IsUnborn).IsTrue();
        }
    }

    [Test]
    public async Task UpdateIndex_UpdatesTrackedPathsOnly_AndRefreshesPhysicalIndex()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, "tracked.txt", "baseline\n");
        fixture.Repository.Stage();
        _ = Commit(fixture.Repository, "baseline");
        var headTree = fixture.Repository.GetCommitTreeId("HEAD").Value;

        Write(
            fixture.Root,
            "tracked.txt",
            "updated tracked contents with a different size\n");
        Write(fixture.Root, "untracked.txt", "must remain untracked\n");

        fixture.Repository.UpdateIndex();
        var entries = ByPath(fixture.Repository.GetIndexEntries());

        await Assert.That(fixture.Repository.WriteIndexTree())
            .IsNotEqualTo(headTree);
        await Assert.That(entries.ContainsKey("tracked.txt")).IsTrue();
        await Assert.That(entries.ContainsKey("untracked.txt")).IsFalse();

        fixture.Repository.RefreshIndex();
    }

    [Test]
    public async Task IndexWrites_RespectThePhysicalLock()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, "tracked.txt", "baseline\n");
        fixture.Repository.Stage();
        _ = Commit(fixture.Repository, "baseline");
        var headTree = fixture.Repository.GetCommitTreeId("HEAD").Value;

        Write(
            fixture.Root,
            "tracked.txt",
            "changed while the index is locked and longer\n");
        var lockPath = Path.Combine(fixture.Repository.RepositoryPath, "index.lock");
        File.WriteAllText(lockPath, "held");

        try
        {
            var exception = CaptureGix(() => fixture.Repository.UpdateIndex());
            await Assert.That(exception.Operation).IsEqualTo("UpdateIndex");
            await Assert.That(fixture.Repository.WriteIndexTree())
                .IsEqualTo(headTree);
        }
        finally
        {
            File.Delete(lockPath);
        }

        fixture.Repository.UpdateIndex();
        await Assert.That(fixture.Repository.WriteIndexTree())
            .IsNotEqualTo(headTree);
    }

    [Test]
    public async Task RefreshIndex_RejectsAPhysicallyCorruptIndex()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, "tracked.txt", "baseline\n");
        fixture.Repository.Stage();

        File.WriteAllBytes(
            Path.Combine(fixture.Repository.RepositoryPath, "index"),
            [1, 2, 3]);

        var exception = CaptureGix(() => fixture.Repository.RefreshIndex());
        await Assert.That(exception.Operation).IsEqualTo("RefreshIndex");
    }

    [Test]
    public async Task ResolveConflictAsDeleted_UsesTheConsumerShapeAndKeepsWorktree()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, "victim.txt", "working copy\n");
        fixture.Repository.Stage("victim.txt");

        var index = fixture.Repository.GetIndexEntries();
        var conflictStages = index
            .Where(static entry => entry.Stage is 1 or 2 or 3)
            .ToArray();

        await Assert.That(index.Single().Path).IsEqualTo("victim.txt");
        await Assert.That(conflictStages).IsEmpty();

        fixture.Repository.ResolveConflictAsDeleted("victim.txt");

        await Assert.That(fixture.Repository.GetIndexEntries()).IsEmpty();
        await Assert.That(File.Exists(Path.Combine(fixture.Root, "victim.txt"))).IsTrue();

        var missing = CaptureGix(
            () => fixture.Repository.ResolveConflictAsDeleted("victim.txt"));
        await Assert.That(missing.Kind).IsEqualTo(GixErrorKind.NotFound);
        await Assert.That(missing.Operation)
            .IsEqualTo("ResolveConflictAsDeleted");
    }

    [Test]
    public async Task WriteIndexTree_IsNonMutatingAndPublicSurfaceIsManaged()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, "one.txt", "one\n");
        Write(fixture.Root, "nested/two.txt", "two\n");
        fixture.Repository.Stage();

        var before = fixture.Repository.GetIndexEntries()
            .Select(static entry => (entry.Path, entry.Stage))
            .ToArray();
        var worktreeContents = File.ReadAllText(
            Path.Combine(fixture.Root, "nested", "two.txt"));

        var treeId = fixture.Repository.WriteIndexTree();

        await Assert.That(treeId.Length).IsEqualTo(40);
        await Assert.That(treeId.All(Uri.IsHexDigit)).IsTrue();
        await Assert.That(fixture.Repository.Head().IsUnborn).IsTrue();
        await Assert.That(fixture.Repository.GetIndexEntries()
                .Select(static entry => (entry.Path, entry.Stage))
                .SequenceEqual(before))
            .IsTrue();
        await Assert.That(File.ReadAllText(
                Path.Combine(fixture.Root, "nested", "two.txt")))
            .IsEqualTo(worktreeContents);

        var entry = new GitIndexEntry("path.txt", 3);
        var bytes = entry.PathBytes;
        bytes[0] = (byte)'X';
        await Assert.That(entry.PathBytes[0]).IsEqualTo((byte)'p');

        await Assert.That(() => new GitIndexEntry("", 0))
            .Throws<ArgumentException>();
        await Assert.That(() => new GitIndexEntry("path", 4))
            .Throws<ArgumentOutOfRangeException>();
        await Assert.That(() => fixture.Repository.Stage(null!))
            .Throws<ArgumentNullException>();
        await Assert.That(() => fixture.Repository.Stage(" "))
            .Throws<ArgumentException>();
        await Assert.That(() => fixture.Repository.UpdateIndex("bad\0path"))
            .Throws<ArgumentException>();
        await Assert.That(() => fixture.Repository.Unstage("\ud800"))
            .Throws<ArgumentException>();
        await Assert.That(
                () => fixture.Repository.ResolveConflictAsDeleted(" "))
            .Throws<ArgumentException>();

        var generatedResources = new HashSet<Type>
        {
            typeof(Repo),
            typeof(IndexEntryRecord),
            typeof(VecIndexEntryRecord),
            typeof(Utf8String),
            typeof(VecByte),
            typeof(SliceByte),
        };
        var exposesGeneratedResource = typeof(GixRepository)
            .GetMethods(BindingFlags.Instance | BindingFlags.Public)
            .Where(static method =>
                method.DeclaringType == typeof(GixRepository))
            .Any(method =>
                generatedResources.Contains(method.ReturnType) ||
                method.GetParameters().Any(parameter =>
                    generatedResources.Contains(parameter.ParameterType)));

        await Assert.That(exposesGeneratedResource).IsFalse();

        fixture.Repository.Dispose();
        await Assert.That(() => fixture.Repository.GetIndexEntries())
            .Throws<ObjectDisposedException>();
        await Assert.That(() => fixture.Repository.WriteIndexTree())
            .Throws<ObjectDisposedException>();
    }

    private static GixObjectId Commit(
        GixRepository repository,
        string message) =>
        repository.CreateCommit(
            message,
            "Index Tests",
            "index-tests@example.com");

    private static Dictionary<string, GitIndexEntry> ByPath(
        IReadOnlyList<GitIndexEntry> entries) =>
        entries.ToDictionary(static entry => entry.Path);

    private static void Write(string root, string relativePath, string contents)
    {
        var path = Path.Combine(
            root,
            relativePath.Replace('/', Path.DirectorySeparatorChar));
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, contents);
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

    private sealed class TempRepository : IDisposable
    {
        private readonly DirectoryInfo _parent =
            Directory.CreateTempSubdirectory();

        public TempRepository()
        {
            Root = Path.Combine(_parent.FullName, "repository");
            Repository = GixRepository.Init(Root);
        }

        public string Root { get; }

        public GixRepository Repository { get; }

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
