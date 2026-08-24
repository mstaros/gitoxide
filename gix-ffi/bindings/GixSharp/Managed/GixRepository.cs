using System.Text;

namespace GixSharp;

/// <summary>Managed information about the repository's HEAD.</summary>
/// <param name="Target">Hex object id HEAD resolves to, or an empty string when unborn.</param>
/// <param name="Referent">Full reference name as raw bytes, or an empty array when detached.</param>
/// <param name="IsDetached">Whether HEAD points directly at an object.</param>
/// <param name="IsUnborn">Whether HEAD names a branch without commits.</param>
public sealed record GixHead(
    string Target,
    byte[] Referent,
    bool IsDetached,
    bool IsUnborn);

/// <summary>Managed information about a commit.</summary>
/// <param name="Id">Hex object id.</param>
/// <param name="AuthorName">Author name as raw bytes.</param>
/// <param name="AuthorEmail">Author email as raw bytes.</param>
/// <param name="TimeSeconds">Author time in seconds since the Unix epoch.</param>
/// <param name="TimeOffsetSeconds">Author timezone offset in seconds east of UTC.</param>
/// <param name="Message">Full commit message as raw bytes.</param>
public sealed record GixCommitInfo(
    string Id,
    byte[] AuthorName,
    byte[] AuthorEmail,
    long TimeSeconds,
    int TimeOffsetSeconds,
    byte[] Message);

/// <summary>
/// Provides an idiomatic managed API over the generated gitoxide interop surface.
/// </summary>
public sealed partial class GixRepository : IDisposable
{
    private static readonly UTF8Encoding PathEncoding =
        new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);

    private readonly object _sync = new();
    private readonly byte[] _repositoryPathBytes;
    private readonly string _repositoryPath;
    private readonly string? _workingDirectory;
    private readonly string _commonDirectory;
    private readonly bool _isBare;
    private readonly bool _isWorktree;
    private Repo? _repo;

    private GixRepository(Repo repo)
    {
        try
        {
            using var info = repo.Info();
            _repositoryPathBytes = info.repository_path.ToArray();
            _repositoryPath = DecodePath(_repositoryPathBytes);
            _workingDirectory = info.has_working_directory
                ? DecodePath(info.working_directory.ToArray())
                : null;
            _commonDirectory = DecodePath(info.common_directory.ToArray());
            _isBare = info.is_bare;
            _isWorktree = info.is_worktree;
            _repo = repo;
        }
        catch
        {
            repo.Dispose();
            throw;
        }
    }

    /// <summary>Initializes a repository at a managed path.</summary>
    public static GixRepository Init(string path, bool bare = false)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path);
        using var nativePath = EncodePath(path).Slice();

        return InvokeStatic(
            "Init",
            () => new GixRepository(Repo.Create(nativePath, bare)));
    }

    /// <summary>Opens a repository from a managed path.</summary>
    public static GixRepository Open(string path)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path);
        using var nativePath = EncodePath(path).Slice();

        return InvokeStatic(
            "Open",
            () => new GixRepository(Repo.Open(nativePath)));
    }

    /// <summary>Discovers and opens the nearest repository above a path.</summary>
    public static GixRepository OpenDiscovered(
        string startPath,
        bool acrossFileSystems = false,
        string? ceilingDirectories = null) =>
        Open(Discover(startPath, acrossFileSystems, ceilingDirectories));

    /// <summary>Finds the nearest physical worktree by its .git directory or gitfile.</summary>
    public static string FindWorktreeRoot(string path)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path);

        var directoryPath = Path.GetFullPath(path);
        if (File.Exists(directoryPath))
        {
            directoryPath = Path.GetDirectoryName(directoryPath)!;
        }

        while (!Directory.Exists(directoryPath))
        {
            var parent = Path.GetDirectoryName(directoryPath);
            if (string.IsNullOrWhiteSpace(parent) ||
                string.Equals(parent, directoryPath, StringComparison.OrdinalIgnoreCase))
            {
                throw new DirectoryNotFoundException(
                    $"No Git worktree was found from '{path}'.");
            }

            directoryPath = parent;
        }

        for (var directory = new DirectoryInfo(directoryPath);
             directory is not null;
             directory = directory.Parent)
        {
            var dotGit = Path.Combine(directory.FullName, ".git");
            if (Directory.Exists(dotGit) || File.Exists(dotGit))
            {
                return NormalizePathSeparators(directory.FullName);
            }
        }

        throw new DirectoryNotFoundException(
            $"No Git worktree was found from '{path}'.");
    }

    /// <summary>Discovers the nearest repository's private Git directory.</summary>
    public static string Discover(
        string startPath,
        bool acrossFileSystems = false,
        string? ceilingDirectories = null)
    {
        try
        {
            return DiscoverRepositoryPath(
                startPath,
                acrossFileSystems,
                ceilingDirectories);
        }
        catch (GixException exception)
            when (exception.Kind == GixErrorKind.NotARepository)
        {
            throw new DirectoryNotFoundException(
                $"No Git repository was found from '{startPath}'.",
                exception);
        }
    }

    /// <summary>Attempts to discover the nearest repository's private Git directory.</summary>
    public static bool TryDiscover(
        string startPath,
        out string repositoryPath,
        bool acrossFileSystems = false,
        string? ceilingDirectories = null)
    {
        try
        {
            repositoryPath = DiscoverRepositoryPath(
                startPath,
                acrossFileSystems,
                ceilingDirectories);
            return true;
        }
        catch (GixException exception)
            when (exception.Kind == GixErrorKind.NotARepository)
        {
            repositoryPath = string.Empty;
            return false;
        }
    }

    /// <summary>Gets the repository-private Git directory.</summary>
    public string RepositoryPath =>
        ReadMetadata(() => _repositoryPath);

    /// <summary>Gets the worktree directory, or null for a bare repository.</summary>
    public string? WorkingDirectory =>
        ReadMetadata(() => _workingDirectory);

    /// <summary>Gets the common Git directory shared by linked worktrees.</summary>
    public string CommonDirectory =>
        ReadMetadata(() => _commonDirectory);

    /// <summary>Gets whether the repository has no working tree.</summary>
    public bool IsBare =>
        ReadMetadata(() => _isBare);

    /// <summary>Gets whether this repository belongs to a linked worktree.</summary>
    public bool IsWorktree =>
        ReadMetadata(() => _isWorktree);

    /// <summary>Gets the repository-private Git directory as raw platform bytes.</summary>
    public byte[] GitDir() =>
        ReadMetadata(() => (byte[])_repositoryPathBytes.Clone());

    /// <summary>Gets the current HEAD information as managed values.</summary>
    public GixHead Head() =>
        Invoke("Head", static repo =>
        {
            using var nativeHead = repo.Head();
            return new GixHead(
                nativeHead.target.String,
                nativeHead.referent.ToArray(),
                nativeHead.is_detached,
                nativeHead.is_unborn);
        });

    /// <summary>
    /// Walks commits from a hex object id. A maximum count of zero means unlimited.
    /// </summary>
    public IReadOnlyList<string> RevWalk(string tip, int maxCount = 0)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(tip);
        ArgumentOutOfRangeException.ThrowIfNegative(maxCount);

        return Invoke("RevWalk", repo =>
        {
            using var nativeTip = tip.Utf8();
            using var nativeIds = repo.RevWalk(nativeTip, (ulong)maxCount);
            var ids = new string[nativeIds.Count];

            for (var index = 0; index < ids.Length; index++)
            {
                ids[index] = nativeIds[index].String;
            }

            return ids;
        });
    }

    /// <summary>Loads one commit by hex object id.</summary>
    public GixCommitInfo CommitInfo(string id)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(id);

        return Invoke("CommitInfo", repo =>
        {
            using var nativeId = id.Utf8();
            using var nativeCommit = repo.CommitInfo(nativeId);
            return new GixCommitInfo(
                nativeCommit.id.String,
                nativeCommit.author_name.ToArray(),
                nativeCommit.author_email.ToArray(),
                nativeCommit.time_seconds,
                nativeCommit.time_offset_seconds,
                nativeCommit.message.ToArray());
        });
    }

    /// <summary>Releases the native repository.</summary>
    public void Dispose()
    {
        lock (_sync)
        {
            var repo = _repo;
            _repo = null;
            repo?.Dispose();
        }
    }

    private static string DiscoverRepositoryPath(
        string startPath,
        bool acrossFileSystems,
        string? ceilingDirectories)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(startPath);
        using var nativeStartPath = EncodePath(startPath).Slice();
        using var nativeCeilingDirectories =
            EncodePath(ceilingDirectories ?? string.Empty).Slice();

        return InvokeStatic(
            "Discover",
            () =>
            {
                using var repo = Repo.Discover(
                    nativeStartPath,
                    acrossFileSystems,
                    nativeCeilingDirectories);
                using var info = repo.Info();
                return DecodePath(info.repository_path.ToArray());
            });
    }

    private T Invoke<T>(string operation, Func<Repo, T> action)
    {
        lock (_sync)
        {
            var repo = _repo ?? throw new ObjectDisposedException(nameof(GixRepository));

            try
            {
                return action(repo);
            }
            catch (EnumException<GixError> exception)
            {
                throw Translate(operation, exception);
            }
            catch (InteropException exception)
            {
                throw Unexpected(operation, exception);
            }
        }
    }

    private T ReadMetadata<T>(Func<T> read)
    {
        lock (_sync)
        {
            _ = _repo ?? throw new ObjectDisposedException(nameof(GixRepository));
            return read();
        }
    }

    private static T InvokeStatic<T>(string operation, Func<T> action)
    {
        try
        {
            return action();
        }
        catch (EnumException<GixError> exception)
        {
            throw Translate(operation, exception);
        }
        catch (InteropException exception)
        {
            throw Unexpected(operation, exception);
        }
    }

    private static byte[] EncodePath(string path) =>
        PathEncoding.GetBytes(path);

    private static string DecodePath(byte[] path) =>
        NormalizePathSeparators(PathEncoding.GetString(path));

    private static string NormalizePathSeparators(string path) =>
        Path.DirectorySeparatorChar == Path.AltDirectorySeparatorChar
            ? path
            : path.Replace(Path.AltDirectorySeparatorChar, Path.DirectorySeparatorChar);

    private static GixException Translate(
        string operation,
        EnumException<GixError> exception)
    {
        using var error = exception.Value;
        var (kind, message) = error switch
        {
            { IsNotARepository: true } =>
                (GixErrorKind.NotARepository, error.AsNotARepository().String),
            { IsIo: true } =>
                (GixErrorKind.Io, error.AsIo().String),
            { IsConfig: true } =>
                (GixErrorKind.Config, error.AsConfig().String),
            { IsInvalidPath: true } =>
                (GixErrorKind.InvalidPath, error.AsInvalidPath().String),
            { IsInvalidId: true } =>
                (GixErrorKind.InvalidId, error.AsInvalidId().String),
            { IsNotFound: true } =>
                (GixErrorKind.NotFound, error.AsNotFound().String),
            { IsOther: true } =>
                (GixErrorKind.Other, error.AsOther().String),
            _ =>
                (GixErrorKind.Other, error.ToString()),
        };

        return new GixException(operation, kind, message);
    }

    private static GixException Unexpected(
        string operation,
        InteropException exception) =>
        new(operation, GixErrorKind.Other, exception.Message, exception);
}
