using System.Text;
using GixSharp;

namespace GixSharp.Tests;

/// <summary>
/// End-to-end tests over the generated interop layer.
///
/// These are deliberately the first tests written: until managed code has
/// actually loaded gix_ffi and called through it, nothing about the binding
/// is proven. That the generated C# compiles is not evidence that
/// marshalling, the API guard, or disposal work.
/// </summary>
public class RepoTests
{
    /// <summary>
    /// Locates the git checkout this test assembly was built from, by walking
    /// up from the output directory until a .git entry appears. Note this may
    /// be a linked worktree, where .git is a file rather than a directory.
    /// </summary>
    private static string RepositoryRoot()
    {
        var dir = new DirectoryInfo(AppContext.BaseDirectory);
        while (dir is not null)
        {
            var dotGit = Path.Combine(dir.FullName, ".git");
            if (Directory.Exists(dotGit) || File.Exists(dotGit))
            {
                return dir.FullName;
            }
            dir = dir.Parent;
        }
        throw new InvalidOperationException(
            $"No git repository found above {AppContext.BaseDirectory}");
    }

    /// <summary>
    /// Opens a repository from a managed path string.
    ///
    /// Paths cross the boundary as raw bytes, not UTF-8 strings, because git
    /// stores them as BString. SliceByte pins the array via GCHandle, so it
    /// must be disposed once the call returns.
    /// </summary>
    private static Repo Open(string path)
    {
        using var bytes = Encoding.UTF8.GetBytes(path).Slice();
        return Repo.Open(bytes);
    }

    private static Repo OpenSelf() => Open(RepositoryRoot());

    /// <summary>
    /// Decodes a Rust-owned byte vector as UTF-8 and disposes it.
    /// </summary>
    private static string Text(VecByte bytes)
    {
        using (bytes)
        {
            return Encoding.UTF8.GetString(bytes.ToArray());
        }
    }

    /// <summary>
    /// Reads the hex id that HEAD resolves to, as a managed string.
    ///
    /// Deliberately does not hand back the Utf8String: passing one as an
    /// argument MOVES it (the marshaller calls IntoUnmanaged, transferring
    /// the pointer to Rust and nulling the managed side), so a single
    /// Utf8String cannot be both read and passed. Managed callers should
    /// hold the decoded string and build a fresh Utf8String per call.
    /// </summary>
    private static string HeadTarget(Repo repo)
    {
        using var head = repo.Head();
        return head.target.String;
    }

    [Test]
    public async Task Open_ResolvesGitDirectory()
    {
        using var repo = OpenSelf();

        var gitDir = Text(repo.GitDir());

        // Deliberately not asserting the path ends in ".git": in a linked
        // worktree gix correctly resolves to <main>/.git/worktrees/<name>.
        await Assert.That(gitDir).IsNotEmpty();
        await Assert.That(gitDir).Contains(".git");
        await Assert.That(Path.IsPathRooted(gitDir)).IsTrue();
        await Assert.That(Directory.Exists(gitDir)).IsTrue();
    }

    [Test]
    public async Task Open_ReportsNonBareWorkingTree()
    {
        using var repo = OpenSelf();

        await Assert.That((bool)repo.IsBare()).IsFalse();
    }

    /// <summary>
    /// A path that exists but is not a repository must surface as an
    /// exception rather than a crash. First check that the error path across
    /// the boundary works at all.
    /// </summary>
    [Test]
    public async Task Open_NonRepositoryPath_Throws()
    {
        var temp = Directory.CreateTempSubdirectory();
        try
        {
            await Assert.That(() => Open(temp.FullName)).Throws<Exception>();
        }
        finally
        {
            temp.Delete(recursive: true);
        }
    }

    [Test]
    public async Task Head_ResolvesToBornCommitOnABranch()
    {
        using var repo = OpenSelf();

        using var head = repo.Head();

        await Assert.That((bool)head.is_unborn).IsFalse();

        // A full sha1 object id in hex.
        await Assert.That(head.target.String.Length).IsEqualTo(40);

        // The test runs from a checkout on a branch, so HEAD is symbolic.
        await Assert.That((bool)head.is_detached).IsFalse();
        await Assert.That(Text(head.referent)).StartsWith("refs/");
    }

    [Test]
    public async Task RevWalk_FromHead_IsBoundedAndStartsAtHead()
    {
        using var repo = OpenSelf();
        var tip = HeadTarget(repo);

        using var ids = repo.RevWalk(tip.Utf8(), 5);

        await Assert.That(ids.Count).IsGreaterThan(0);
        await Assert.That(ids.Count).IsLessThanOrEqualTo(5);

        // A walk starts at its tip.
        await Assert.That(ids[0].String).IsEqualTo(tip);
    }

    [Test]
    public async Task CommitInfo_RoundTripsHeadCommit()
    {
        using var repo = OpenSelf();
        var tip = HeadTarget(repo);

        using var commit = repo.CommitInfo(tip.Utf8());

        await Assert.That(commit.id.String).IsEqualTo(tip);
        await Assert.That(Text(commit.author_name)).IsNotEmpty();
        await Assert.That(Text(commit.message)).IsNotEmpty();

        // gitoxide's history predates this test but is not in the future.
        await Assert.That(commit.time_seconds).IsGreaterThan(1_000_000_000L);
        await Assert.That(commit.time_seconds)
                    .IsLessThan(DateTimeOffset.UtcNow.ToUnixTimeSeconds() + 86_400);
    }

    /// <summary>
    /// Malformed object ids must be rejected before reaching gix.
    /// </summary>
    [Test]
    public async Task CommitInfo_InvalidId_Throws()
    {
        using var repo = OpenSelf();

        await Assert.That(() => repo.CommitInfo("not-a-valid-object-id".Utf8()))
                    .Throws<Exception>();
    }
}
