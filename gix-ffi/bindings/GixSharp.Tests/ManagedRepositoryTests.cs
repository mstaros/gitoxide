using System.Runtime.CompilerServices;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class ManagedRepositoryTests
{
    [Test]
    public async Task ManagedSurface_CoversTheCompletePoc()
    {
        using var repository = GixRepository.Open(RepositoryRoot());

        var gitDirectory = Encoding.UTF8.GetString(repository.GitDir());
        var head = repository.Head();
        var ids = repository.RevWalk(head.Target, 5);
        var commit = repository.CommitInfo(ids[0]);

        await Assert.That(Path.IsPathRooted(gitDirectory)).IsTrue();
        await Assert.That(Directory.Exists(gitDirectory)).IsTrue();
        await Assert.That(repository.IsBare).IsFalse();
        await Assert.That(head.IsUnborn).IsFalse();
        await Assert.That(head.IsDetached).IsFalse();
        await Assert.That(head.Target.Length).IsEqualTo(40);
        await Assert.That(Encoding.UTF8.GetString(head.Referent)).StartsWith("refs/");
        await Assert.That(ids.Count).IsGreaterThan(0);
        await Assert.That(ids.Count).IsLessThanOrEqualTo(5);
        await Assert.That(ids[0]).IsEqualTo(head.Target);
        await Assert.That(commit.Id).IsEqualTo(head.Target);
        await Assert.That(commit.AuthorName.Length).IsGreaterThan(0);
        await Assert.That(commit.Message.Length).IsGreaterThan(0);
    }

    [Test]
    public async Task ManagedStrings_CanBeReusedAcrossCalls()
    {
        using var repository = GixRepository.Open(RepositoryRoot());
        var tip = repository.Head().Target;

        var firstWalk = repository.RevWalk(tip, 3);
        var secondWalk = repository.RevWalk(tip, 3);
        var firstCommit = repository.CommitInfo(tip);
        var secondCommit = repository.CommitInfo(tip);

        await Assert.That(firstWalk.SequenceEqual(secondWalk)).IsTrue();
        await Assert.That(firstCommit.Id).IsEqualTo(tip);
        await Assert.That(secondCommit.Id).IsEqualTo(tip);
        await Assert.That(tip.Length).IsEqualTo(40);
    }

    [Test]
    public async Task NativeErrors_BecomeManagedGixExceptions()
    {
        var directory = Directory.CreateTempSubdirectory();

        try
        {
            GixException? exception = null;
            try
            {
                using var repository = GixRepository.Open(directory.FullName);
            }
            catch (GixException caught)
            {
                exception = caught;
            }

            await Assert.That(exception).IsNotNull();
            await Assert.That(exception!.Kind).IsEqualTo(GixErrorKind.NotARepository);
            await Assert.That(exception.Operation).IsEqualTo("Open");
            await Assert.That(exception.NativeMessage).IsNotEmpty();
        }
        finally
        {
            directory.Delete(recursive: true);
        }
    }

    [Test]
    public async Task InvalidObjectIds_BecomeManagedGixExceptions()
    {
        using var repository = GixRepository.Open(RepositoryRoot());

        GixException? exception = null;
        try
        {
            repository.CommitInfo("not-a-valid-object-id");
        }
        catch (GixException caught)
        {
            exception = caught;
        }

        await Assert.That(exception).IsNotNull();
        await Assert.That(exception!.Kind).IsEqualTo(GixErrorKind.InvalidId);
        await Assert.That(exception.Operation).IsEqualTo("CommitInfo");
        await Assert.That(exception.NativeMessage).IsNotEmpty();
    }

    [Test]
    public async Task PublicArguments_AreValidatedBeforeInterop()
    {
        using var repository = GixRepository.Open(RepositoryRoot());

        await Assert.That(() => GixRepository.Open(" ")).Throws<ArgumentException>();
        await Assert.That(() => repository.RevWalk(" ", 1)).Throws<ArgumentException>();
        await Assert.That(() => repository.RevWalk(repository.Head().Target, -1))
            .Throws<ArgumentOutOfRangeException>();
        await Assert.That(() => repository.CommitInfo(" ")).Throws<ArgumentException>();
    }

    [Test]
    public async Task ManagedResults_OutliveTheRepository()
    {
        GixHead head;
        IReadOnlyList<string> ids;
        GixCommitInfo commit;

        using (var repository = GixRepository.Open(RepositoryRoot()))
        {
            head = repository.Head();
            ids = repository.RevWalk(head.Target, 2);
            commit = repository.CommitInfo(ids[0]);
        }

        await Assert.That(head.Target.Length).IsEqualTo(40);
        await Assert.That(ids.Count).IsGreaterThan(0);
        await Assert.That(commit.Id).IsEqualTo(ids[0]);
        await Assert.That(commit.AuthorName.Length).IsGreaterThan(0);
    }

    [Test]
    public async Task Dispose_IsIdempotentAndGuardsFurtherUse()
    {
        var repository = GixRepository.Open(RepositoryRoot());

        repository.Dispose();
        repository.Dispose();

        await Assert.That(() => repository.Head()).Throws<ObjectDisposedException>();
    }

    private static string RepositoryRoot([CallerFilePath] string sourceFile = "") =>
        Path.GetFullPath(Path.Combine(
            Path.GetDirectoryName(sourceFile)!,
            "..",
            "..",
            ".."));
}
