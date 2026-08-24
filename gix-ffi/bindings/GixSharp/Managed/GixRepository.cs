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
public sealed class GixRepository : IDisposable
{
    private readonly object _sync = new();
    private Repo? _repo;

    private GixRepository(Repo repo)
    {
        _repo = repo;
    }

    /// <summary>Opens a repository from a managed path.</summary>
    public static GixRepository Open(string path)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path);

        try
        {
            using var nativePath = Encoding.UTF8.GetBytes(path).Slice();
            return new GixRepository(Repo.Open(nativePath));
        }
        catch (EnumException<GixError> exception)
        {
            throw Translate("Open", exception);
        }
        catch (InteropException exception)
        {
            throw Unexpected("Open", exception);
        }
    }

    /// <summary>Gets the absolute .git directory as raw platform bytes.</summary>
    public byte[] GitDir() =>
        Invoke("GitDir", static repo =>
        {
            using var nativePath = repo.GitDir();
            return nativePath.ToArray();
        });

    /// <summary>Gets whether the repository has no working tree.</summary>
    public bool IsBare =>
        Invoke("IsBare", static repo => repo.IsBare());

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
