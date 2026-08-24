using System.Diagnostics;
using System.Reflection;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class StatusTests
{
    [Test]
    public async Task DefaultOptions_MatchCSharpMpcAndExposeEntryProperties()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, "staged.txt", "baseline\n");
        Write(fixture.Root, "working.txt", "baseline\n");
        Write(fixture.Root, "both.txt", "baseline\n");
        Write(fixture.Root, "clean.txt", "baseline\n");
        CommitAll(fixture.Root, "baseline");

        Write(fixture.Root, "staged.txt", "staged version with a different size\n");
        Git(fixture.Root, "add", "staged.txt");
        Write(fixture.Root, "working.txt", "working version with a different size\n");
        Write(fixture.Root, "both.txt", "staged version\n");
        Git(fixture.Root, "add", "both.txt");
        Write(fixture.Root, "both.txt", "working version after staging and longer\n");
        Write(fixture.Root, "nested/untracked.txt", "untracked\n");

        var entries = ByPath(fixture.Repository.GetStatus());

        await Assert.That(entries["staged.txt"].Status)
            .IsEqualTo(GitFileStatus.ModifiedInIndex);
        await Assert.That(entries["staged.txt"].IsStaged).IsTrue();
        await Assert.That(entries["staged.txt"].HasWorkingDirectoryChanges).IsFalse();
        await Assert.That(entries["working.txt"].Status)
            .IsEqualTo(GitFileStatus.ModifiedInWorkingDirectory);
        await Assert.That(entries["working.txt"].IsStaged).IsFalse();
        await Assert.That(entries["working.txt"].HasWorkingDirectoryChanges).IsTrue();
        await Assert.That(entries["both.txt"].Status)
            .IsEqualTo(
                GitFileStatus.ModifiedInIndex |
                GitFileStatus.ModifiedInWorkingDirectory);
        await Assert.That(entries["both.txt"].IsStaged).IsTrue();
        await Assert.That(entries["both.txt"].HasWorkingDirectoryChanges).IsTrue();
        await Assert.That(entries["nested/untracked.txt"].Status)
            .IsEqualTo(GitFileStatus.NewInWorkingDirectory);
        await Assert.That(entries.ContainsKey("clean.txt")).IsFalse();

        var rawPath = entries["nested/untracked.txt"].PathBytes;
        await Assert.That(rawPath.SequenceEqual(
                Encoding.UTF8.GetBytes("nested/untracked.txt")))
            .IsTrue();

        rawPath[0] = (byte)'X';
        await Assert.That(entries["nested/untracked.txt"].PathBytes[0])
            .IsEqualTo((byte)'n');
    }

    [Test]
    public async Task ShowModes_MergeAndFilterDeletedIgnoredAndCurrentEntries()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, ".gitignore", "ignored-*\n");
        Write(fixture.Root, "staged-delete.txt", "baseline\n");
        Write(fixture.Root, "working-delete.txt", "baseline\n");
        Write(fixture.Root, "clean.txt", "baseline\n");
        CommitAll(fixture.Root, "baseline");

        Git(fixture.Root, "rm", "--quiet", "staged-delete.txt");
        File.Delete(Path.Combine(fixture.Root, "working-delete.txt"));
        Write(fixture.Root, "ignored-output", "ignored\n");
        Write(fixture.Root, "untracked.txt", "untracked\n");

        const GitStatusOptionFlags flags =
            GitStatusOptionFlags.IncludeUntracked |
            GitStatusOptionFlags.IncludeIgnored |
            GitStatusOptionFlags.IncludeUnmodified |
            GitStatusOptionFlags.RecurseUntrackedDirectories;

        var combined = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions { Flags = flags }));
        await Assert.That(combined["staged-delete.txt"].Status)
            .IsEqualTo(GitFileStatus.DeletedFromIndex);
        await Assert.That(combined["working-delete.txt"].Status)
            .IsEqualTo(GitFileStatus.DeletedFromWorkingDirectory);
        await Assert.That(combined["ignored-output"].Status)
            .IsEqualTo(GitFileStatus.Ignored);
        await Assert.That(combined["untracked.txt"].Status)
            .IsEqualTo(GitFileStatus.NewInWorkingDirectory);
        await Assert.That(combined["clean.txt"].Status)
            .IsEqualTo(GitFileStatus.Current);

        var indexOnly = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Show = GitStatusShow.IndexOnly,
                Flags = flags,
            }));
        await Assert.That(indexOnly["staged-delete.txt"].Status)
            .IsEqualTo(GitFileStatus.DeletedFromIndex);
        await Assert.That(indexOnly["clean.txt"].Status)
            .IsEqualTo(GitFileStatus.Current);
        await Assert.That(indexOnly["working-delete.txt"].Status)
            .IsEqualTo(GitFileStatus.Current);
        await Assert.That(indexOnly.ContainsKey("ignored-output")).IsFalse();
        await Assert.That(indexOnly.ContainsKey("untracked.txt")).IsFalse();

        var workingOnly = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Show = GitStatusShow.WorkingDirectoryOnly,
                Flags = flags,
            }));
        await Assert.That(workingOnly["working-delete.txt"].Status)
            .IsEqualTo(GitFileStatus.DeletedFromWorkingDirectory);
        await Assert.That(workingOnly["ignored-output"].Status)
            .IsEqualTo(GitFileStatus.Ignored);
        await Assert.That(workingOnly["untracked.txt"].Status)
            .IsEqualTo(GitFileStatus.NewInWorkingDirectory);
        await Assert.That(workingOnly["clean.txt"].Status)
            .IsEqualTo(GitFileStatus.Current);
        await Assert.That(workingOnly.ContainsKey("staged-delete.txt")).IsFalse();
    }

    [Test]
    public async Task RecursionPathspecLiteralMatchingAndSorting_AreManaged()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, ".gitignore", "ignored/\n");
        CommitAll(fixture.Root, "ignore rules");

        Write(fixture.Root, "untracked/nested/value.txt", "untracked\n");
        Write(fixture.Root, "ignored/nested/value.txt", "ignored\n");
        Write(fixture.Root, "wild[a].txt", "literal wildcard\n");
        Write(fixture.Root, "wilda.txt", "wildcard match\n");
        Write(fixture.Root, "Beta", "uppercase\n");
        Write(fixture.Root, "alpha", "lowercase\n");

        var collapsed = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Flags =
                    GitStatusOptionFlags.IncludeUntracked |
                    GitStatusOptionFlags.IncludeIgnored,
            }));
        await Assert.That(collapsed["untracked/"].Status)
            .IsEqualTo(GitFileStatus.NewInWorkingDirectory);
        await Assert.That(collapsed["ignored/"].Status)
            .IsEqualTo(GitFileStatus.Ignored);
        await Assert.That(collapsed.ContainsKey("untracked/nested/value.txt"))
            .IsFalse();
        await Assert.That(collapsed.ContainsKey("ignored/nested/value.txt"))
            .IsFalse();

        var recursive = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Flags =
                    GitStatusOptionFlags.IncludeUntracked |
                    GitStatusOptionFlags.IncludeIgnored |
                    GitStatusOptionFlags.RecurseUntrackedDirectories |
                    GitStatusOptionFlags.RecurseIgnoredDirectories,
            }));
        await Assert.That(recursive["untracked/nested/value.txt"].Status)
            .IsEqualTo(GitFileStatus.NewInWorkingDirectory);
        await Assert.That(recursive["ignored/nested/value.txt"].Status)
            .IsEqualTo(GitFileStatus.Ignored);

        var wildcard = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Pathspecs = ["wild[a].txt"],
            }));
        await Assert.That(wildcard.ContainsKey("wild[a].txt")).IsTrue();
        await Assert.That(wildcard.ContainsKey("wilda.txt")).IsTrue();

        var literal = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Flags =
                    GitStatusOptionFlags.IncludeUntracked |
                    GitStatusOptionFlags.RecurseUntrackedDirectories |
                    GitStatusOptionFlags.DisablePathspecMatch,
                Pathspecs = ["wild[a].txt"],
            }));
        await Assert.That(literal.ContainsKey("wild[a].txt")).IsTrue();
        await Assert.That(literal.ContainsKey("wilda.txt")).IsFalse();

        var sensitive = fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Flags =
                    GitStatusOptionFlags.IncludeUntracked |
                    GitStatusOptionFlags.SortCaseSensitively,
                Pathspecs = ["Beta", "alpha"],
            });
        await Assert.That(sensitive.Select(static entry => entry.Path)
                .SequenceEqual(["Beta", "alpha"]))
            .IsTrue();

        var insensitive = fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Flags =
                    GitStatusOptionFlags.IncludeUntracked |
                    GitStatusOptionFlags.SortCaseInsensitively,
                Pathspecs = ["Beta", "alpha"],
            });
        await Assert.That(insensitive.Select(static entry => entry.Path)
                .SequenceEqual(["alpha", "Beta"]))
            .IsTrue();
    }

    [Test]
    public async Task RenameFlags_DetectBothDirections()
    {
        using var fixture = new TempRepository();
        Write(
            fixture.Root,
            "old-index.txt",
            "exact staged rename contents with enough bytes\n");
        Write(
            fixture.Root,
            "old-worktree.txt",
            "exact worktree rename contents with enough bytes\n");
        CommitAll(fixture.Root, "rename baseline");

        Git(fixture.Root, "mv", "old-index.txt", "new-index-name.txt");
        File.Move(
            Path.Combine(fixture.Root, "old-worktree.txt"),
            Path.Combine(fixture.Root, "new-worktree-name.txt"));

        var withoutRenames = ByPath(fixture.Repository.GetStatus());
        await Assert.That(withoutRenames["old-index.txt"].Status)
            .IsEqualTo(GitFileStatus.DeletedFromIndex);
        await Assert.That(withoutRenames["new-index-name.txt"].Status)
            .IsEqualTo(GitFileStatus.NewInIndex);
        await Assert.That(withoutRenames["old-worktree.txt"].Status)
            .IsEqualTo(GitFileStatus.DeletedFromWorkingDirectory);
        await Assert.That(withoutRenames["new-worktree-name.txt"].Status)
            .IsEqualTo(GitFileStatus.NewInWorkingDirectory);

        var withRenames = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Flags =
                    GitStatusOptionFlags.IncludeUntracked |
                    GitStatusOptionFlags.RecurseUntrackedDirectories |
                    GitStatusOptionFlags.RenamesHeadToIndex |
                    GitStatusOptionFlags.RenamesIndexToWorkingDirectory,
            }));
        await Assert.That(withRenames["new-index-name.txt"].Status)
            .IsEqualTo(GitFileStatus.RenamedInIndex);
        await Assert.That(withRenames["new-worktree-name.txt"].Status)
            .IsEqualTo(GitFileStatus.RenamedInWorkingDirectory);
        await Assert.That(withRenames.ContainsKey("old-index.txt")).IsFalse();
        await Assert.That(withRenames.ContainsKey("old-worktree.txt")).IsFalse();
    }

    [Test]
    public async Task Conflicts_AreReportedInEveryShowMode()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, "conflict.txt", "base\n");
        CommitAll(fixture.Root, "base");
        var mainBranch = Git(fixture.Root, "branch", "--show-current");
        Git(fixture.Root, "branch", "other");

        Write(fixture.Root, "conflict.txt", "main side\n");
        CommitAll(fixture.Root, "main change");
        Git(fixture.Root, "checkout", "--quiet", "other");
        Write(fixture.Root, "conflict.txt", "other side\n");
        CommitAll(fixture.Root, "other change");
        Git(fixture.Root, "checkout", "--quiet", mainBranch);
        GitMustFail(fixture.Root, "merge", "--quiet", "other");

        foreach (var show in Enum.GetValues<GitStatusShow>())
        {
            var entries = ByPath(fixture.Repository.GetStatus(
                new GitStatusOptions
                {
                    Show = show,
                    Flags = GitStatusOptionFlags.None,
                }));
            await Assert.That(entries["conflict.txt"].Status)
                .IsEqualTo(GitFileStatus.Conflicted);
        }
    }

    [Test]
    public async Task TypeChangesSubmoduleExclusionAndAllFlags_WorkEndToEnd()
    {
        using var fixture = new TempRepository();
        Write(fixture.Root, "typed", "regular file used as a blob\n");
        Write(
            fixture.Root,
            "typed-to-module",
            "regular file becoming a gitlink\n");
        CommitAll(fixture.Root, "baseline");

        var blob = Git(fixture.Root, "rev-parse", "HEAD:typed");
        Git(
            fixture.Root,
            "update-index",
            "--cacheinfo",
            $"120000,{blob},typed");

        var head = Git(fixture.Root, "rev-parse", "HEAD");
        Git(
            fixture.Root,
            "update-index",
            "--add",
            "--cacheinfo",
            $"160000,{head},module");
        Git(
            fixture.Root,
            "update-index",
            "--cacheinfo",
            $"160000,{head},typed-to-module");

        var index = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Show = GitStatusShow.IndexOnly,
                Flags = GitStatusOptionFlags.None,
            }));
        await Assert.That(index["typed"].Status)
            .IsEqualTo(GitFileStatus.TypeChangedInIndex);
        await Assert.That(index["module"].Status)
            .IsEqualTo(GitFileStatus.NewInIndex);
        await Assert.That(index["typed-to-module"].Status)
            .IsEqualTo(GitFileStatus.TypeChangedInIndex);

        var excluded = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Show = GitStatusShow.IndexOnly,
                Flags = GitStatusOptionFlags.ExcludeSubmodules,
            }));
        await Assert.That(excluded.ContainsKey("module")).IsFalse();
        await Assert.That(excluded["typed-to-module"].Status)
            .IsEqualTo(GitFileStatus.TypeChangedInIndex);

        var combined = ByPath(fixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Flags = GitStatusOptionFlags.None,
                Pathspecs = ["typed"],
            }));
        await Assert.That(combined["typed"].Status)
            .IsEqualTo(
                GitFileStatus.TypeChangedInIndex |
                GitFileStatus.TypeChangedInWorkingDirectory);

        using var allFlagsFixture = new TempRepository();
        Write(allFlagsFixture.Root, "tracked", "tracked\n");
        CommitAll(allFlagsFixture.Root, "baseline");
        const GitStatusOptionFlags allCompatible =
            GitStatusOptionFlags.IncludeUntracked |
            GitStatusOptionFlags.IncludeIgnored |
            GitStatusOptionFlags.IncludeUnmodified |
            GitStatusOptionFlags.ExcludeSubmodules |
            GitStatusOptionFlags.RecurseUntrackedDirectories |
            GitStatusOptionFlags.DisablePathspecMatch |
            GitStatusOptionFlags.RecurseIgnoredDirectories |
            GitStatusOptionFlags.RenamesHeadToIndex |
            GitStatusOptionFlags.RenamesIndexToWorkingDirectory |
            GitStatusOptionFlags.SortCaseInsensitively |
            GitStatusOptionFlags.RenamesFromRewrites |
            GitStatusOptionFlags.UpdateIndex |
            GitStatusOptionFlags.IncludeUnreadable |
            GitStatusOptionFlags.IncludeUnreadableAsUntracked;

        _ = allFlagsFixture.Repository.GetStatus(
            new GitStatusOptions { Flags = allCompatible });
        _ = allFlagsFixture.Repository.GetStatus(
            new GitStatusOptions
            {
                Flags = GitStatusOptionFlags.NoRefresh,
            });
    }

    [Test]
    public async Task EnumsValidationRawPathsAndPublicSurface_AreStable()
    {
        var optionValues = Enum.GetValues<GitStatusOptionFlags>()
            .Select(static value => (uint)value);
        await Assert.That(optionValues.SequenceEqual(
            [
                0U,
                1U << 0,
                1U << 1,
                1U << 2,
                1U << 3,
                1U << 4,
                1U << 5,
                1U << 6,
                1U << 7,
                1U << 8,
                1U << 9,
                1U << 10,
                1U << 11,
                1U << 12,
                1U << 13,
                1U << 14,
                1U << 15,
            ]))
            .IsTrue();

        var fileValues = Enum.GetValues<GitFileStatus>()
            .Select(static value => (uint)value);
        await Assert.That(fileValues.SequenceEqual(
            [
                0U,
                1U << 0,
                1U << 1,
                1U << 2,
                1U << 3,
                1U << 4,
                1U << 7,
                1U << 8,
                1U << 9,
                1U << 10,
                1U << 11,
                1U << 12,
                1U << 14,
                1U << 15,
            ]))
            .IsTrue();
        await Assert.That(Enum.GetValues<GitStatusShow>()
                .Select(static value => (int)value)
                .SequenceEqual([0, 1, 2]))
            .IsTrue();

        var raw = new byte[] { (byte)'x', 0xFF };
        var rawEntry = new GitStatusEntry(
            raw,
            GitFileStatus.NewInWorkingDirectory);
        await Assert.That(rawEntry.PathBytes.SequenceEqual(raw)).IsTrue();
        await Assert.That(rawEntry.Path.Contains('\uFFFD')).IsTrue();

        using var fixture = new TempRepository();
        await Assert.That(() => fixture.Repository.GetStatus(
                new GitStatusOptions { Show = (GitStatusShow)99 }))
            .Throws<ArgumentOutOfRangeException>();
        await Assert.That(() => fixture.Repository.GetStatus(
                new GitStatusOptions
                {
                    Flags = (GitStatusOptionFlags)(1U << 20),
                }))
            .Throws<ArgumentOutOfRangeException>();
        await Assert.That(() => fixture.Repository.GetStatus(
                new GitStatusOptions
                {
                    Flags =
                        GitStatusOptionFlags.SortCaseSensitively |
                        GitStatusOptionFlags.SortCaseInsensitively,
                }))
            .Throws<ArgumentException>();
        await Assert.That(() => fixture.Repository.GetStatus(
                new GitStatusOptions
                {
                    Flags =
                        GitStatusOptionFlags.NoRefresh |
                        GitStatusOptionFlags.UpdateIndex,
                }))
            .Throws<ArgumentException>();
        await Assert.That(() => fixture.Repository.GetStatus(
                new GitStatusOptions { Pathspecs = null! }))
            .Throws<ArgumentNullException>();
        await Assert.That(() => fixture.Repository.GetStatus(
                new GitStatusOptions { Pathspecs = [""] }))
            .Throws<ArgumentException>();
        await Assert.That(() => fixture.Repository.GetStatus(
                new GitStatusOptions { Pathspecs = ["bad\0path"] }))
            .Throws<ArgumentException>();
        await Assert.That(() => fixture.Repository.GetStatus(
                new GitStatusOptions { Pathspecs = ["\uD800"] }))
            .Throws<ArgumentException>();

        var generatedResources = new HashSet<Type>
        {
            typeof(Repo),
            typeof(StatusRecord),
            typeof(VecStatusRecord),
            typeof(SliceByte),
            typeof(VecByte),
            typeof(GixError),
        };
        var exposesGeneratedResource = typeof(GixRepository)
            .GetMethods(BindingFlags.Public | BindingFlags.Instance)
            .Where(static method =>
                method.DeclaringType == typeof(GixRepository))
            .Any(method =>
                generatedResources.Contains(method.ReturnType) ||
                method.GetParameters().Any(parameter =>
                    generatedResources.Contains(parameter.ParameterType)));

        await Assert.That(exposesGeneratedResource).IsFalse();
    }

    [Test]
    public async Task UnicodeStatusPath_RetainsExactUtf8Bytes()
    {
        using var fixture = new TempRepository();
        Git(fixture.Root, "commit", "--allow-empty", "--quiet", "-m", "base");
        const string path = "unicodé/λ.txt";
        Write(fixture.Root, path, "unicode\n");

        var entry = fixture.Repository.GetStatus().Single();

        await Assert.That(entry.Path).IsEqualTo(path);
        await Assert.That(entry.PathBytes.SequenceEqual(
                Encoding.UTF8.GetBytes(path)))
            .IsTrue();
    }

    private static Dictionary<string, GitStatusEntry> ByPath(
        IReadOnlyList<GitStatusEntry> entries) =>
        entries.ToDictionary(
            static entry => entry.Path,
            StringComparer.Ordinal);

    private static void Write(
        string root,
        string relativePath,
        string contents)
    {
        var path = Path.Combine(
            root,
            relativePath.Replace('/', Path.DirectorySeparatorChar));
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, contents, new UTF8Encoding(false));
    }

    private static void CommitAll(string root, string message)
    {
        Git(root, "add", "--all");
        Git(root, "commit", "--quiet", "-m", message);
    }

    private static string Git(string repository, params string[] arguments)
    {
        var result = RunGit(repository, arguments);
        if (result.ExitCode != 0)
        {
            throw new InvalidOperationException(
                $"git {string.Join(' ', arguments)} failed: " +
                $"{result.StandardError}{result.StandardOutput}");
        }

        return result.StandardOutput.Trim();
    }

    private static void GitMustFail(
        string repository,
        params string[] arguments)
    {
        var result = RunGit(repository, arguments);
        if (result.ExitCode == 0)
        {
            throw new InvalidOperationException(
                $"git {string.Join(' ', arguments)} unexpectedly succeeded.");
        }
    }

    private static GitResult RunGit(
        string repository,
        IReadOnlyList<string> arguments)
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
        return new GitResult(
            process.ExitCode,
            standardOutput,
            standardError);
    }

    private sealed record GitResult(
        int ExitCode,
        string StandardOutput,
        string StandardError);

    private sealed class TempRepository : IDisposable
    {
        private readonly DirectoryInfo _parent =
            Directory.CreateTempSubdirectory();

        public TempRepository()
        {
            Root = Path.Combine(_parent.FullName, "repository");
            Repository = GixRepository.Init(Root);
            Git(Root, "config", "user.name", "Status Tests");
            Git(
                Root,
                "config",
                "user.email",
                "status-tests@example.com");
            Git(Root, "config", "core.autocrlf", "false");
            Git(Root, "config", "core.ignorecase", "false");
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
