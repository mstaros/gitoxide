using System.Text;

namespace GixSharp;

/// <summary>
/// One managed repository index entry, including its conflict stage.
/// </summary>
public sealed record GitIndexEntry(string Path, int Stage)
{
    private readonly string _pathBytes =
        Convert.ToBase64String(ValidateAndEncodePath(Path, Stage));

    internal GitIndexEntry(byte[] pathBytes, int stage)
        : this(DecodePath(pathBytes), stage)
    {
        ArgumentNullException.ThrowIfNull(pathBytes);
        if (pathBytes.Length == 0 || pathBytes.Contains((byte)0))
            throw new ArgumentException(
                "A Git index path must be non-empty and contain no NUL bytes.",
                nameof(pathBytes));

        _pathBytes = Convert.ToBase64String(pathBytes);
    }

    /// <summary>
    /// Gets the exact repository-relative Git path bytes as a new array.
    /// </summary>
    public byte[] PathBytes => Convert.FromBase64String(_pathBytes);

    private static byte[] ValidateAndEncodePath(string path, int stage)
    {
        ArgumentNullException.ThrowIfNull(path);
        if (path.Length == 0 || path.Contains('\0'))
            throw new ArgumentException(
                "A Git index path must be non-empty and contain no NUL characters.",
                nameof(path));
        if (stage is < 0 or > 3)
            throw new ArgumentOutOfRangeException(
                nameof(stage),
                stage,
                "A Git index stage must be between zero and three.");

        return Encoding.UTF8.GetBytes(path);
    }

    private static string DecodePath(byte[] pathBytes)
    {
        ArgumentNullException.ThrowIfNull(pathBytes);
        return Encoding.UTF8.GetString(pathBytes);
    }
}

public sealed partial class GixRepository
{
    /// <summary>
    /// Adds all matching working-directory changes to the index and writes it.
    /// An empty pathspec list selects every change.
    /// </summary>
    public void Stage(params string[] pathspecs)
    {
        var pathspecBytes = EncodeIndexPathspecs(pathspecs);

        Invoke("Stage", repo =>
        {
            using var nativePathspecs = pathspecBytes.Slice();
            repo.Stage(nativePathspecs);
        });
    }

    /// <summary>
    /// Restores matching index entries from HEAD without changing the worktree.
    /// An empty pathspec list selects every index path. In an unborn repository,
    /// selected entries are removed.
    /// </summary>
    public void Unstage(params string[] pathspecs)
    {
        var pathspecBytes = EncodeIndexPathspecs(pathspecs);

        Invoke("Unstage", repo =>
        {
            using var nativePathspecs = pathspecBytes.Slice();
            repo.Unstage(nativePathspecs);
        });
    }

    /// <summary>Physically reloads and validates the index from disk.</summary>
    public void RefreshIndex(bool force = true) =>
        Invoke("RefreshIndex", repo => repo.RefreshIndex(force));

    /// <summary>
    /// Updates matching tracked index entries from the worktree and writes the
    /// index. Untracked paths are never added. An empty pathspec list selects
    /// every tracked path.
    /// </summary>
    public void UpdateIndex(params string[] pathspecs)
    {
        var pathspecBytes = EncodeIndexPathspecs(pathspecs);

        Invoke("UpdateIndex", repo =>
        {
            using var nativePathspecs = pathspecBytes.Slice();
            repo.UpdateIndex(nativePathspecs);
        });
    }

    /// <summary>
    /// Returns every index entry, including separate unresolved conflict stages.
    /// </summary>
    public IReadOnlyList<GitIndexEntry> GetIndexEntries() =>
        Invoke("GetIndexEntries", static repo =>
        {
            using var nativeEntries = repo.IndexEntries();
            var entries = new GitIndexEntry[nativeEntries.Count];

            for (var index = 0; index < entries.Length; index++)
            {
                // The indexed wrapper aliases an element still owned by the
                // vector. Copy its path now; disposing it separately would
                // double-free when nativeEntries is disposed.
                var nativeEntry = nativeEntries[index];
                entries[index] = new GitIndexEntry(
                    nativeEntry.path.ToArray(),
                    checked((int)nativeEntry.stage));
            }

            return entries;
        });

    /// <summary>
    /// Resolves an unresolved path as deleted by removing all of its index stages.
    /// </summary>
    public void ResolveConflictAsDeleted(string path)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(path);
        if (path.Contains('\0'))
            throw new ArgumentException(
                "A conflict path must not contain NUL characters.",
                nameof(path));

        byte[] pathBytes;
        try
        {
            pathBytes = EncodePath(path);
        }
        catch (EncoderFallbackException exception)
        {
            throw new ArgumentException(
                "A conflict path must be a valid UTF-16 string.",
                nameof(path),
                exception);
        }

        Invoke("ResolveConflictAsDeleted", repo =>
        {
            using var nativePath = pathBytes.Slice();
            repo.ResolveConflictAsDeleted(nativePath);
        });
    }

    /// <summary>
    /// Writes the conflict-free index as a tree object without changing the
    /// index, worktree, HEAD, or another reference.
    /// </summary>
    public string WriteIndexTree() =>
        Invoke("WriteIndexTree", static repo =>
        {
            using var nativeId = repo.WriteIndexTree();
            return nativeId.String;
        });

    private void Invoke(string operation, Action<Repo> action) =>
        Invoke(operation, repo =>
        {
            action(repo);
            return true;
        });

    private static byte[] EncodeIndexPathspecs(IReadOnlyList<string> pathspecs)
    {
        ArgumentNullException.ThrowIfNull(pathspecs);
        if (pathspecs.Count == 0)
            return Array.Empty<byte>();

        var result = new List<byte>();
        for (var index = 0; index < pathspecs.Count; index++)
        {
            var pathspec = pathspecs[index];
            if (string.IsNullOrWhiteSpace(pathspec))
                throw new ArgumentException(
                    "Index pathspecs must be non-empty and not whitespace.",
                    nameof(pathspecs));
            if (pathspec.Contains('\0'))
                throw new ArgumentException(
                    "Index pathspecs must not contain NUL characters.",
                    nameof(pathspecs));

            byte[] encoded;
            try
            {
                encoded = EncodePath(pathspec);
            }
            catch (EncoderFallbackException exception)
            {
                throw new ArgumentException(
                    "Index pathspecs must be valid UTF-16 strings.",
                    nameof(pathspecs),
                    exception);
            }

            if (index != 0)
                result.Add(0);
            result.AddRange(encoded);
        }

        return [.. result];
    }
}
