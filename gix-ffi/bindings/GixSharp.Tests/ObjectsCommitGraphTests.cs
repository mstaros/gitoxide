using System.Diagnostics;
using GixSharp;

namespace GixSharp.Tests;

public sealed class ObjectsCommitGraphTests
{
    private static readonly GixObjectId EmptyTree =
        new("4b825dc642cb6eb9a060e54bf8d69288fbee4904");

    [Test]
    public async Task ObjectIds_ValidateSha1AndNormalizeHex()
    {
        var uppercase = new GixObjectId("ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD");

        await Assert.That(uppercase.Value)
            .IsEqualTo("abcdefabcdefabcdefabcdefabcdefabcdefabcd");
        await Assert.That(() => new GixObjectId(" "))
            .Throws<ArgumentException>();
        await Assert.That(() => new GixObjectId("abc"))
            .Throws<FormatException>();
        await Assert.That(() => new GixObjectId(
                "0000000000000000000000000000000000000000000000000000000000000000"))
            .Throws<FormatException>();
        await Assert.That(() => new GixObjectId(
                "gggggggggggggggggggggggggggggggggggggggg"))
            .Throws<FormatException>();
    }

    [Test]
    public async Task LookupCommit_PeelsAnnotatedTagsAndPreservesExactData()
    {
        using var fixture = new TempRepository();
        var author = Signature(
            "Author One",
            "author@example.com",
            1_700_000_000,
            330);
        var committer = Signature(
            "Committer One",
            "committer@example.com",
            1_700_000_100,
            -420);
        var id = fixture.Repository.CreateCommitObject(
            "exact message",
            EmptyTree,
            Array.Empty<GixObjectId>(),
            "HEAD",
            author,
            committer);

        Git(fixture.Root, "config", "user.name", "Tagger");
        Git(fixture.Root, "config", "user.email", "tagger@example.com");
        Git(fixture.Root, "tag", "-a", "annotated", "-m", "annotated tag");

        var commit = fixture.Repository.LookupCommit("annotated");

        await Assert.That(commit.Id).IsEqualTo(id);
        await Assert.That(commit.Message).IsEqualTo("exact message");
        await Assert.That(commit.Author).IsEqualTo(author);
        await Assert.That(commit.Committer).IsEqualTo(committer);
        await Assert.That(commit.ParentIds).IsEmpty();
        await Assert.That(fixture.Repository.GetCommitTreeId("annotated"))
            .IsEqualTo(EmptyTree);
    }

    [Test]
    public async Task CommitHistory_SupportsMergesSortsExclusionsReverseAndLimits()
    {
        using var fixture = new TempRepository();
        var repository = fixture.Repository;
        var root = Commit(repository, "root", [], "HEAD", 100);
        var left = Commit(repository, "left", [root], "HEAD", 300);
        var right = Commit(repository, "right", [root], null, 200);
        var merge = Commit(repository, "merge", [left, right], "HEAD", 400);

        var sorted = repository.GetCommitHistory(
            merge.Value,
            int.MaxValue,
            GixCommitSort.Topological | GixCommitSort.Time);
        var excluded = repository.GetCommitHistory(
            merge.Value,
            left.Value,
            int.MaxValue,
            GixCommitSort.Topological | GixCommitSort.Time);
        var reversed = repository.GetCommitHistory(
            merge.Value,
            2,
            GixCommitSort.Topological | GixCommitSort.Time | GixCommitSort.Reverse);
        var zero = repository.GetCommitHistory(merge.Value, 0);

        await Assert.That(sorted.Select(static commit => commit.Id)
                .SequenceEqual([merge, left, right, root]))
            .IsTrue();
        await Assert.That(excluded.Select(static commit => commit.Id)
                .SequenceEqual([merge, right]))
            .IsTrue();
        await Assert.That(reversed.Select(static commit => commit.Id)
                .SequenceEqual([root, right]))
            .IsTrue();
        await Assert.That(zero).IsEmpty();
        await Assert.That(() => repository.GetCommitHistory(
                merge.Value,
                10,
                (GixCommitSort)8))
            .Throws<ArgumentOutOfRangeException>();

        var mergeCommit = repository.LookupCommit(merge.Value);
        await Assert.That(mergeCommit.ParentIds.SequenceEqual([left, right])).IsTrue();
    }

    [Test]
    public async Task MetadataAndFailures_DistinguishKindsMissingAndWrongType()
    {
        using var fixture = new TempRepository();
        var commitId = Commit(fixture.Repository, "metadata", [], "HEAD", 100);

        var commit = fixture.Repository.GetObjectMetadata(commitId);
        var tree = fixture.Repository.GetObjectMetadata(EmptyTree);
        var missing = CaptureGix(() => fixture.Repository.GetObjectMetadata(
            new GixObjectId("1111111111111111111111111111111111111111")));
        var wrongType = CaptureGix(() => fixture.Repository.LookupCommit(EmptyTree.Value));

        await Assert.That(commit.Type).IsEqualTo(GixObjectType.Commit);
        await Assert.That(commit.Size).IsGreaterThan(0);
        await Assert.That(tree).IsEqualTo(new GixObjectMetadata(GixObjectType.Tree, 0));
        await Assert.That(missing.Kind).IsEqualTo(GixErrorKind.NotFound);
        await Assert.That(missing.Operation).IsEqualTo("GetObjectMetadata");
        await Assert.That(wrongType.Kind).IsEqualTo(GixErrorKind.Other);
        await Assert.That(wrongType.Operation).IsEqualTo("LookupCommit");
    }

    [Test]
    public async Task CreateCommitObject_UsesConfiguredSignaturesIndependently()
    {
        using var fixture = new TempRepository();
        fixture.Repository.Dispose();
        Git(fixture.Root, "config", "user.name", "Configured User");
        Git(fixture.Root, "config", "user.email", "configured@example.com");
        using var repository = GixRepository.Open(fixture.Root);

        var explicitAuthor = Signature(
            "Explicit Author",
            "author@example.com",
            1_710_000_000,
            120);
        var first = repository.CreateCommitObject(
            "configured committer",
            EmptyTree,
            Array.Empty<GixObjectId>(),
            explicitAuthor,
            committer: null);
        var firstCommit = repository.LookupCommit(first.Value);

        var explicitCommitter = Signature(
            "Explicit Committer",
            "committer@example.com",
            1_710_000_100,
            -180);
        var second = repository.CreateCommitObject(
            "configured author",
            EmptyTree,
            [first],
            author: null,
            committer: explicitCommitter);
        var secondCommit = repository.LookupCommit(second.Value);

        await Assert.That(firstCommit.Author).IsEqualTo(explicitAuthor);
        await Assert.That(firstCommit.Committer.Name).IsEqualTo("Configured User");
        await Assert.That(firstCommit.Committer.Email)
            .IsEqualTo("configured@example.com");
        await Assert.That(secondCommit.Author.Name).IsEqualTo("Configured User");
        await Assert.That(secondCommit.Author.Email)
            .IsEqualTo("configured@example.com");
        await Assert.That(secondCommit.Committer).IsEqualTo(explicitCommitter);
        await Assert.That(secondCommit.ParentIds.SequenceEqual([first])).IsTrue();
    }

    [Test]
    public async Task CreateCommit_UsesTheRealIndexIdentityAndAllowEmpty()
    {
        using var fixture = new TempRepository();
        fixture.Repository.Dispose();
        Git(fixture.Root, "config", "user.name", "Configured User");
        Git(fixture.Root, "config", "user.email", "configured@example.com");
        using var repository = GixRepository.Open(fixture.Root);

        await File.WriteAllTextAsync(
            Path.Combine(fixture.Root, "tracked.txt"),
            "first\n");
        Git(fixture.Root, "add", "tracked.txt");
        var configured = repository.CreateCommit("configured commit");
        var configuredCommit = repository.LookupCommit("HEAD");

        await File.AppendAllTextAsync(
            Path.Combine(fixture.Root, "tracked.txt"),
            "second\n");
        Git(fixture.Root, "add", "tracked.txt");
        var explicitId = repository.CreateCommit(
            "explicit commit",
            "Index Author",
            "index@example.com");
        var explicitCommit = repository.LookupCommit("HEAD");

        var refused = CaptureGix(() => repository.CreateCommit(
            "refuse empty",
            "Index Author",
            "index@example.com"));
        var allowed = repository.CreateCommit(
            "allow empty",
            "Index Author",
            "index@example.com",
            allowEmpty: true);
        var allowedCommit = repository.LookupCommit("HEAD");

        await Assert.That(configuredCommit.Id).IsEqualTo(configured);
        await Assert.That(configuredCommit.Author.Name).IsEqualTo("Configured User");
        await Assert.That(configuredCommit.Author.Email)
            .IsEqualTo("configured@example.com");
        await Assert.That(explicitCommit.Id).IsEqualTo(explicitId);
        await Assert.That(explicitCommit.Author.Name).IsEqualTo("Index Author");
        await Assert.That(explicitCommit.Committer.Email)
            .IsEqualTo("index@example.com");
        await Assert.That(explicitCommit.ParentIds.SequenceEqual([configured])).IsTrue();
        await Assert.That(refused.Kind).IsEqualTo(GixErrorKind.Other);
        await Assert.That(allowedCommit.Id).IsEqualTo(allowed);
        await Assert.That(allowedCommit.ParentIds.SequenceEqual([explicitId])).IsTrue();
        await Assert.That(repository.GetCommitTreeId(allowed.Value))
            .IsEqualTo(repository.GetCommitTreeId(explicitId.Value));
    }

    [Test]
    public async Task Ancestry_HandlesTrueFalseEqualMissingAndMerges()
    {
        using var fixture = new TempRepository();
        var repository = fixture.Repository;
        var root = Commit(repository, "root", [], "HEAD", 100);
        var left = Commit(repository, "left", [root], "HEAD", 200);
        var right = Commit(repository, "right", [root], null, 300);
        var merge = Commit(repository, "merge", [left, right], null, 400);

        await Assert.That(repository.IsAncestorOf(root, merge)).IsTrue();
        await Assert.That(repository.IsAncestorOf(left, merge)).IsTrue();
        await Assert.That(repository.IsAncestorOf(right, merge)).IsTrue();
        await Assert.That(repository.IsAncestorOf(merge, merge)).IsTrue();
        await Assert.That(repository.IsAncestorOf(right, left)).IsFalse();

        var missing = CaptureGix(() => repository.IsAncestorOf(
            new GixObjectId("1111111111111111111111111111111111111111"),
            merge));
        await Assert.That(missing.Kind).IsEqualTo(GixErrorKind.NotFound);
        await Assert.That(missing.Operation).IsEqualTo("IsAncestorOf");
    }

    [Test]
    public async Task PublicArgumentsAndSurface_AreManagedAndValidated()
    {
        using var fixture = new TempRepository();
        var repository = fixture.Repository;

        await Assert.That(() => repository.LookupCommit(" "))
            .Throws<ArgumentException>();
        await Assert.That(() => repository.GetCommitHistory("HEAD", -1))
            .Throws<ArgumentOutOfRangeException>();
        await Assert.That(() => repository.GetCommitHistory("HEAD", " "))
            .Throws<ArgumentException>();
        await Assert.That(() => repository.CreateCommit("message", "name", null))
            .Throws<ArgumentException>();
        await Assert.That(() => repository.CreateCommitObject(
                "message",
                EmptyTree,
                Array.Empty<GixObjectId>(),
                " "))
            .Throws<ArgumentException>();
        await Assert.That(() => repository.CreateCommitObject(
                "message",
                EmptyTree,
                null!))
            .Throws<ArgumentNullException>();

        var generatedResources = new HashSet<Type>
        {
            typeof(Repo),
            typeof(CommitRecord),
            typeof(ObjectMetadata),
            typeof(FfiObjectType),
            typeof(Utf8String),
            typeof(VecByte),
            typeof(VecUtf8String),
            typeof(SliceByte),
        };
        var exposesGeneratedResource = typeof(GixRepository)
            .GetMethods()
            .Where(static method => method.DeclaringType == typeof(GixRepository))
            .Any(method =>
                generatedResources.Contains(method.ReturnType) ||
                method.GetParameters().Any(parameter =>
                    generatedResources.Contains(parameter.ParameterType)));

        await Assert.That(exposesGeneratedResource).IsFalse();
    }

    private static GixObjectId Commit(
        GixRepository repository,
        string message,
        IReadOnlyList<GixObjectId> parents,
        string? updateReference,
        long seconds)
    {
        var signature = Signature(
            "Test User",
            "test@example.com",
            seconds,
            0);
        return updateReference is null
            ? repository.CreateCommitObject(
                message,
                EmptyTree,
                parents,
                signature,
                signature)
            : repository.CreateCommitObject(
                message,
                EmptyTree,
                parents,
                updateReference,
                signature,
                signature);
    }

    private static GixSignature Signature(
        string name,
        string email,
        long seconds,
        int offsetMinutes) =>
        new(
            name,
            email,
            DateTimeOffset
                .FromUnixTimeSeconds(seconds)
                .ToOffset(TimeSpan.FromMinutes(offsetMinutes)));

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

    private static void Git(string repository, params string[] arguments)
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
