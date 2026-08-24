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

    [Test]
    public async Task Open_ResolvesGitDirectory()
    {
        using var repo = Open(RepositoryRoot());

        using var raw = repo.GitDir();
        var gitDir = Encoding.UTF8.GetString(raw.ToArray());

        // Deliberately not asserting the path ends in ".git": in a linked
        // worktree gix correctly resolves to <main>/.git/worktrees/<name>.
        // Asserting an existing, absolute, .git-containing path holds for
        // both a normal clone and a worktree.
        await Assert.That(gitDir).IsNotEmpty();
        await Assert.That(gitDir).Contains(".git");
        await Assert.That(Path.IsPathRooted(gitDir)).IsTrue();
        await Assert.That(Directory.Exists(gitDir)).IsTrue();
    }

    [Test]
    public async Task Open_ReportsNonBareWorkingTree()
    {
        using var repo = Open(RepositoryRoot());

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
}
