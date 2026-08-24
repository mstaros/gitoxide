using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class ReferenceTests
{
    private static readonly GixObjectId EmptyTree =
        new("4b825dc642cb6eb9a060e54bf8d69288fbee4904");

    [Test]
    public async Task Enumeration_PreservesDirectSymbolicPackedAndFilteredReferences()
    {
        using var fixture = new TempRepository();
        var repository = fixture.Repository;

        await Assert.That(repository.TryCreateReference(
                "refs/remotes/origin/main",
                fixture.First))
            .IsTrue();

        var symbolicPath = Path.Combine(
            repository.RepositoryPath,
            "refs",
            "heads",
            "symbolic");
        Directory.CreateDirectory(Path.GetDirectoryName(symbolicPath)!);
        File.WriteAllText(
            symbolicPath,
            "ref: refs/heads/main\n",
            new UTF8Encoding(false));

        File.WriteAllText(
            Path.Combine(repository.RepositoryPath, "packed-refs"),
            string.Concat(
                "# pack-refs with: sorted\n",
                fixture.Second.Value,
                " refs/heads/packed\n",
                fixture.First.Value,
                " refs/tags/v1\n"),
            new UTF8Encoding(false));

        repository.Dispose();
        using var reopened = GixRepository.Open(fixture.Root);

        var all = reopened.GetReferences();
        var byName = all.ToDictionary(
            static reference => reference.Name,
            StringComparer.Ordinal);
        var orderedNames = all.Select(static reference => reference.Name).ToArray();

        await Assert.That(orderedNames.SequenceEqual(
                orderedNames.OrderBy(static name => name, StringComparer.Ordinal)))
            .IsTrue();

        var main = byName["refs/heads/main"];
        await Assert.That(main.Target).IsEqualTo(fixture.First);
        await Assert.That(main.SymbolicTarget).IsNull();

        var symbolic = byName["refs/heads/symbolic"];
        await Assert.That(symbolic.Target).IsNull();
        await Assert.That(symbolic.SymbolicTarget)
            .IsEqualTo("refs/heads/main");
        await Assert.That(symbolic.SymbolicTargetBytes!
                .SequenceEqual("refs/heads/main"u8.ToArray()))
            .IsTrue();

        await Assert.That(byName["refs/heads/packed"].Target)
            .IsEqualTo(fixture.Second);
        await Assert.That(byName["refs/tags/v1"].Target)
            .IsEqualTo(fixture.First);
        await Assert.That(byName["refs/remotes/origin/main"].Target)
            .IsEqualTo(fixture.First);

        var heads = reopened.GetReferences("refs/heads/*");
        await Assert.That(heads.All(
                static reference =>
                    reference.Name.StartsWith(
                        "refs/heads/",
                        StringComparison.Ordinal)))
            .IsTrue();
        await Assert.That(heads.Any(
                static reference => reference.Name == "refs/heads/packed"))
            .IsTrue();
        await Assert.That(heads.Any(
                static reference => reference.Name == "refs/heads/symbolic"))
            .IsTrue();

        var local = reopened.GetBranches(GitBranchFilter.Local);
        var remote = reopened.GetBranches(GitBranchFilter.Remote);
        var both = reopened.GetBranches();

        var mainBranch = local.Single(
            static branch =>
                string.Equals(branch.Name, "main", StringComparison.Ordinal));
        await Assert.That(mainBranch.Target?.Value)
            .IsEqualTo(fixture.First.Value);
        await Assert.That(mainBranch.IsRemote).IsFalse();

        var symbolicBranch = local.Single(
            static branch => branch.Name == "symbolic");
        await Assert.That(symbolicBranch.Target).IsNull();

        var remoteBranch = remote.Single();
        await Assert.That(remoteBranch.Name).IsEqualTo("origin/main");
        await Assert.That(remoteBranch.IsRemote).IsTrue();
        await Assert.That(remoteBranch.Target?.Value)
            .IsEqualTo(fixture.First.Value);
        await Assert.That(both.Count).IsEqualTo(local.Count + remote.Count);

        var clone = main.NameBytes;
        clone[0] = (byte)'X';
        await Assert.That(main.NameBytes[0]).IsEqualTo((byte)'r');
    }

    [Test]
    public async Task BranchMutation_HandlesForceHeadStatesAndValidation()
    {
        using var fixture = new TempRepository();
        var repository = fixture.Repository;

        var created = repository.CreateBranch("topic", fixture.Second.Value);
        await Assert.That(created.Name).IsEqualTo("topic");
        await Assert.That(created.Target).IsEqualTo(fixture.Second);

        var duplicate = CaptureGix(
            () => repository.CreateBranch("topic", fixture.First.Value));
        await Assert.That(duplicate.Kind)
            .IsEqualTo(GixErrorKind.ReferenceConflict);

        var replaced = repository.CreateBranch(
            "topic",
            fixture.First.Value,
            force: true);
        await Assert.That(replaced.Target).IsEqualTo(fixture.First);

        repository.SetHead("refs/heads/topic");
        var topicHead = repository.Head();
        await Assert.That(topicHead.IsDetached).IsFalse();
        await Assert.That(topicHead.IsUnborn).IsFalse();
        await Assert.That(Encoding.UTF8.GetString(topicHead.Referent))
            .IsEqualTo("refs/heads/topic");

        var checkedOut = CaptureGix(
            () => repository.DeleteBranch("topic"));
        await Assert.That(checkedOut.Kind)
            .IsEqualTo(GixErrorKind.ReferenceConflict);

        repository.SetHead("unborn");
        var unborn = repository.Head();
        await Assert.That(unborn.IsUnborn).IsTrue();
        await Assert.That(unborn.IsDetached).IsFalse();
        await Assert.That(Encoding.UTF8.GetString(unborn.Referent))
            .IsEqualTo("refs/heads/unborn");

        var nonBranchHead = CaptureGix(
            () => repository.SetHead("refs/tags/v1"));
        await Assert.That(nonBranchHead.Kind)
            .IsEqualTo(GixErrorKind.InvalidReference);

        var invalidBranch = CaptureGix(
            () => repository.CreateBranch(
                "bad..name",
                fixture.First.Value));
        await Assert.That(invalidBranch.Kind)
            .IsEqualTo(GixErrorKind.InvalidReference);

        repository.DeleteBranch("topic");
        var missingBranch = CaptureGix(
            () => repository.DeleteBranch("topic"));
        await Assert.That(missingBranch.Kind)
            .IsEqualTo(GixErrorKind.NotFound);

        File.WriteAllText(
            Path.Combine(repository.RepositoryPath, "HEAD"),
            fixture.First.Value + "\n",
            new UTF8Encoding(false));
        repository.Dispose();

        using var detached = GixRepository.Open(fixture.Root);
        var detachedHead = detached.Head();
        await Assert.That(detachedHead.IsDetached).IsTrue();
        await Assert.That(detachedHead.IsUnborn).IsFalse();
        await Assert.That(detachedHead.Target)
            .IsEqualTo(fixture.First.Value);

        detached.DeleteBranch("main");
        await Assert.That(
                detached.GetBranches(GitBranchFilter.Local)
                    .Any(static branch => branch.Name == "main"))
            .IsFalse();
    }

    [Test]
    public async Task ExactReferenceOperations_UseExpectedOldAndMessageFreeReflogs()
    {
        using var fixture = new TempRepository();
        var repository = fixture.Repository;
        const string name = "refs/heads/cas";

        await Assert.That(repository.TryGetReferenceTarget(name, out _))
            .IsFalse();
        await Assert.That(repository.TryCreateReference(name, fixture.First))
            .IsTrue();
        await Assert.That(repository.TryCreateReference(name, fixture.First))
            .IsFalse();
        await Assert.That(repository.TryGetReferenceTarget(name, out var first))
            .IsTrue();
        await Assert.That(first).IsEqualTo(fixture.First);

        await Assert.That(repository.CompareExchangeReference(
                name,
                fixture.Second,
                fixture.Second))
            .IsFalse();
        await Assert.That(repository.TryGetReferenceTarget(name, out var unchanged))
            .IsTrue();
        await Assert.That(unchanged).IsEqualTo(fixture.First);

        await Assert.That(repository.CompareExchangeReference(
                name,
                fixture.Second,
                fixture.First))
            .IsTrue();
        await Assert.That(repository.TryGetReferenceTarget(name, out var second))
            .IsTrue();
        await Assert.That(second).IsEqualTo(fixture.Second);

        var aliasPath = Path.Combine(
            repository.RepositoryPath,
            "refs",
            "meta",
            "alias");
        Directory.CreateDirectory(Path.GetDirectoryName(aliasPath)!);
        File.WriteAllText(
            aliasPath,
            "ref: refs/heads/cas\n",
            new UTF8Encoding(false));
        await Assert.That(repository.TryGetReferenceTarget(
                "refs/meta/alias",
                out var followed))
            .IsTrue();
        await Assert.That(followed).IsEqualTo(fixture.Second);

        var reflog = File.ReadAllLines(
            Path.Combine(
                repository.RepositoryPath,
                "logs",
                "refs",
                "heads",
                "cas"));
        await Assert.That(reflog.Length).IsEqualTo(2);
        await Assert.That(reflog.All(static line => !line.Contains('\t')))
            .IsTrue();

        await Assert.That(repository.TryDeleteReference(
                name,
                fixture.First))
            .IsFalse();
        await Assert.That(repository.TryDeleteReference(
                name,
                fixture.Second))
            .IsTrue();
        await Assert.That(repository.TryDeleteReference(
                name,
                fixture.Second))
            .IsFalse();
        await Assert.That(repository.TryGetReferenceTarget(name, out _))
            .IsFalse();

        var invalid = CaptureGix(
            () => repository.TryGetReferenceTarget(
                "refs/heads/bad..name",
                out _));
        await Assert.That(invalid.Kind)
            .IsEqualTo(GixErrorKind.InvalidReference);
    }

    [Test]
    public async Task ReferenceLocks_AreAtomicCooperativeAndIdempotent()
    {
        using var fixture = new TempRepository();
        var repository = fixture.Repository;
        var oneLock = Path.Combine(
            repository.RepositoryPath,
            "refs",
            "heads",
            "one.lock");
        var twoLock = Path.Combine(
            repository.RepositoryPath,
            "refs",
            "heads",
            "two.lock");

        var lease = repository.AcquireReferenceLocks(
            "refs/heads/two",
            "refs/heads/one",
            "refs/heads/two");
        await Assert.That(File.Exists(oneLock)).IsTrue();
        await Assert.That(File.Exists(twoLock)).IsTrue();

        var contention = CaptureGix(
            () => repository.TryCreateReference(
                "refs/heads/one",
                fixture.First));
        await Assert.That(contention.Kind)
            .IsEqualTo(GixErrorKind.ReferenceConflict);

        lease.Dispose();
        lease.Dispose();
        await Assert.That(File.Exists(oneLock)).IsFalse();
        await Assert.That(File.Exists(twoLock)).IsFalse();
        await Assert.That(repository.TryCreateReference(
                "refs/heads/one",
                fixture.First))
            .IsTrue();

        var externalLock = Path.Combine(
            repository.RepositoryPath,
            "refs",
            "heads",
            "z.lock");
        File.WriteAllText(externalLock, "held", new UTF8Encoding(false));
        var partial = CaptureGix(
            () => repository.AcquireReferenceLocks(
                "refs/heads/z",
                "refs/heads/a"));
        await Assert.That(partial.Kind)
            .IsEqualTo(GixErrorKind.ReferenceConflict);
        await Assert.That(File.Exists(Path.Combine(
                repository.RepositoryPath,
                "refs",
                "heads",
                "a.lock")))
            .IsFalse();
        await Assert.That(File.Exists(externalLock)).IsTrue();
        File.Delete(externalLock);

        await Assert.That(() => repository.AcquireReferenceLocks())
            .Throws<ArgumentException>();
        await Assert.That(() => repository.AcquireReferenceLocks(
                "refs/heads/a",
                string.Empty))
            .Throws<ArgumentException>();

        var outlivingLease = repository.AcquireReferenceLocks(
            "refs/heads/outliving");
        var outlivingLock = Path.Combine(
            repository.RepositoryPath,
            "refs",
            "heads",
            "outliving.lock");
        repository.Dispose();
        await Assert.That(File.Exists(outlivingLock)).IsTrue();
        outlivingLease.Dispose();
        await Assert.That(File.Exists(outlivingLock)).IsFalse();
    }

    [Test]
    public async Task ManagedSurface_PreservesRawNamesAndLeaksNoGeneratedResources()
    {
        var rawName = "refs/heads/raw-"u8.ToArray();
        rawName = [.. rawName, 0xff];
        var rawShorthand = "raw-"u8.ToArray();
        rawShorthand = [.. rawShorthand, 0xff];
        var rawSymbolic = "refs/heads/target-"u8.ToArray();
        rawSymbolic = [.. rawSymbolic, 0xfe];

        var reference = new GitReferenceInfo(
            rawName,
            rawShorthand,
            null,
            rawSymbolic);
        var branch = new GitBranch(rawShorthand, false, null);

        var returnedName = reference.NameBytes;
        returnedName[^1] = 0;
        var returnedBranch = branch.NameBytes;
        returnedBranch[^1] = 0;

        await Assert.That(reference.NameBytes.SequenceEqual(rawName)).IsTrue();
        await Assert.That(reference.ShorthandBytes.SequenceEqual(rawShorthand))
            .IsTrue();
        await Assert.That(reference.SymbolicTargetBytes!
                .SequenceEqual(rawSymbolic))
            .IsTrue();
        await Assert.That(branch.NameBytes.SequenceEqual(rawShorthand)).IsTrue();
        await Assert.That(reference.Name.Contains('\ufffd')).IsTrue();

        var generatedResources = new HashSet<Type>
        {
            typeof(Repo),
            typeof(ReferenceRecord),
            typeof(BranchRecord),
            typeof(OptionalObjectId),
            typeof(ReferenceLockLease),
            typeof(VecReferenceRecord),
            typeof(VecBranchRecord),
            typeof(Utf8String),
            typeof(VecByte),
            typeof(SliceByte),
        };
        var methods = typeof(GixRepository)
            .GetMethods()
            .Where(static method =>
                method.DeclaringType == typeof(GixRepository))
            .ToArray();
        var exposesGeneratedResource = methods.Any(method =>
            generatedResources.Contains(method.ReturnType) ||
            method.GetParameters().Any(parameter =>
                generatedResources.Contains(
                    parameter.ParameterType.IsByRef
                        ? parameter.ParameterType.GetElementType()!
                        : parameter.ParameterType)));

        await Assert.That(exposesGeneratedResource).IsFalse();
        await Assert.That(methods.Any(
                static method => method.Name == nameof(GixRepository.GetReferences)))
            .IsTrue();
        await Assert.That(methods.Any(
                static method => method.Name == nameof(GixRepository.GetBranches)))
            .IsTrue();
        await Assert.That(methods.Any(
                static method =>
                    method.Name == nameof(GixRepository.AcquireReferenceLocks)))
            .IsTrue();

        await Assert.That(() => new GitBranch("bad\0name", false, null))
            .Throws<ArgumentException>();
        await Assert.That(() => new GitReferenceInfo(
                string.Empty,
                "short",
                null,
                null))
            .Throws<ArgumentException>();
    }

    private static GixObjectId Commit(
        GixRepository repository,
        string message,
        IReadOnlyList<GixObjectId> parents,
        string? updateReference,
        long seconds)
    {
        var signature = new GixSignature(
            "Reference Tests",
            "references@example.com",
            DateTimeOffset.FromUnixTimeSeconds(seconds));
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
            using (var created = GixRepository.Init(Root))
            {
                File.AppendAllText(
                    Path.Combine(created.RepositoryPath, "config"),
                    "\n[user]\n\tname = Reference Tests\n" +
                    "\temail = references@example.com\n",
                    new UTF8Encoding(false));
            }

            Repository = GixRepository.Open(Root);
            Repository.SetHead("main");
            First = Commit(Repository, "first", [], "HEAD", 100);
            Second = Commit(Repository, "second", [First], null, 200);
        }

        public string Root { get; }

        public GixRepository Repository { get; }

        public GixObjectId First { get; }

        public GixObjectId Second { get; }

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
