using System.Text;

namespace GixSharp;

/// <summary>Options controlling repository status traversal.</summary>
[Flags]
public enum GitStatusOptionFlags : uint
{
    None = 0,
    IncludeUntracked = 1U << 0,
    IncludeIgnored = 1U << 1,
    IncludeUnmodified = 1U << 2,
    ExcludeSubmodules = 1U << 3,
    RecurseUntrackedDirectories = 1U << 4,
    DisablePathspecMatch = 1U << 5,
    RecurseIgnoredDirectories = 1U << 6,
    RenamesHeadToIndex = 1U << 7,
    RenamesIndexToWorkingDirectory = 1U << 8,
    SortCaseSensitively = 1U << 9,
    SortCaseInsensitively = 1U << 10,
    RenamesFromRewrites = 1U << 11,
    NoRefresh = 1U << 12,
    UpdateIndex = 1U << 13,
    IncludeUnreadable = 1U << 14,
    IncludeUnreadableAsUntracked = 1U << 15,
}

/// <summary>Selects which sides of repository status are returned.</summary>
public enum GitStatusShow
{
    IndexAndWorkingDirectory = 0,
    IndexOnly = 1,
    WorkingDirectoryOnly = 2,
}

/// <summary>Managed repository status options.</summary>
public sealed record GitStatusOptions
{
    /// <summary>Gets the index/worktree view to return.</summary>
    public GitStatusShow Show { get; init; } =
        GitStatusShow.IndexAndWorkingDirectory;

    /// <summary>Gets the traversal and result flags.</summary>
    public GitStatusOptionFlags Flags { get; init; } =
        GitStatusOptionFlags.IncludeUntracked |
        GitStatusOptionFlags.RecurseUntrackedDirectories;

    /// <summary>
    /// Gets repository-relative Git pathspecs. Multiple values are combined
    /// without exposing the generated NUL-separated byte representation.
    /// </summary>
    public IReadOnlyList<string> Pathspecs { get; init; } =
        Array.Empty<string>();
}

/// <summary>Status bits for one repository-relative path.</summary>
[Flags]
public enum GitFileStatus : uint
{
    Current = 0,
    NewInIndex = 1U << 0,
    ModifiedInIndex = 1U << 1,
    DeletedFromIndex = 1U << 2,
    RenamedInIndex = 1U << 3,
    TypeChangedInIndex = 1U << 4,
    NewInWorkingDirectory = 1U << 7,
    ModifiedInWorkingDirectory = 1U << 8,
    DeletedFromWorkingDirectory = 1U << 9,
    TypeChangedInWorkingDirectory = 1U << 10,
    RenamedInWorkingDirectory = 1U << 11,
    UnreadableInWorkingDirectory = 1U << 12,
    Ignored = 1U << 14,
    Conflicted = 1U << 15,
}

/// <summary>A managed status entry with both decoded and exact raw Git path forms.</summary>
public sealed record GitStatusEntry(string Path, GitFileStatus Status)
{
    private const GitFileStatus IndexChanges =
        GitFileStatus.NewInIndex |
        GitFileStatus.ModifiedInIndex |
        GitFileStatus.DeletedFromIndex |
        GitFileStatus.RenamedInIndex |
        GitFileStatus.TypeChangedInIndex;

    private const GitFileStatus WorkingDirectoryChanges =
        GitFileStatus.NewInWorkingDirectory |
        GitFileStatus.ModifiedInWorkingDirectory |
        GitFileStatus.DeletedFromWorkingDirectory |
        GitFileStatus.TypeChangedInWorkingDirectory |
        GitFileStatus.RenamedInWorkingDirectory |
        GitFileStatus.UnreadableInWorkingDirectory;

    private readonly string _pathBytes =
        Convert.ToBase64String(ValidateAndEncodePath(Path));

    internal GitStatusEntry(byte[] pathBytes, GitFileStatus status)
        : this(DecodePath(pathBytes), status)
    {
        ArgumentNullException.ThrowIfNull(pathBytes);
        if (pathBytes.Length == 0 || pathBytes.Contains((byte)0))
            throw new ArgumentException(
                "A Git status path must be non-empty and contain no NUL bytes.",
                nameof(pathBytes));

        _pathBytes = Convert.ToBase64String(pathBytes);
    }

    /// <summary>
    /// Gets the exact repository-relative Git path bytes. The returned array is
    /// a new copy; <see cref="Path"/> uses UTF-8 replacement for invalid input.
    /// </summary>
    public byte[] PathBytes => Convert.FromBase64String(_pathBytes);

    /// <summary>Gets whether the entry contains an index-side change.</summary>
    public bool IsStaged => (Status & IndexChanges) != 0;

    /// <summary>Gets whether the entry contains a worktree-side change.</summary>
    public bool HasWorkingDirectoryChanges =>
        (Status & WorkingDirectoryChanges) != 0;

    private static byte[] ValidateAndEncodePath(string path)
    {
        ArgumentNullException.ThrowIfNull(path);
        if (path.Length == 0 || path.Contains('\0'))
            throw new ArgumentException(
                "A Git status path must be non-empty and contain no NUL characters.",
                nameof(path));

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
    private const GitStatusOptionFlags AllStatusOptionFlags =
        (GitStatusOptionFlags)((1U << 16) - 1);

    /// <summary>
    /// Returns a snapshot of repository status using managed options and values.
    /// </summary>
    public IReadOnlyList<GitStatusEntry> GetStatus(
        GitStatusOptions? options = null)
    {
        options ??= new GitStatusOptions();
        ValidateStatusOptions(options);
        var pathspecBytes = EncodeStatusPathspecs(options.Pathspecs);

        return Invoke("GetStatus", repo =>
        {
            using var nativePathspecs = pathspecBytes.Slice();
            using var nativeEntries = repo.Status(
                (uint)options.Show,
                (uint)options.Flags,
                nativePathspecs);
            var entries = new GitStatusEntry[nativeEntries.Count];

            for (var index = 0; index < entries.Length; index++)
            {
                // The indexed wrapper aliases an element still owned by the
                // vector. Copy its path now; disposing it separately would
                // double-free when nativeEntries is disposed.
                var nativeEntry = nativeEntries[index];
                entries[index] = new GitStatusEntry(
                    nativeEntry.path.ToArray(),
                    (GitFileStatus)nativeEntry.status);
            }

            return entries;
        });
    }

    private static void ValidateStatusOptions(GitStatusOptions options)
    {
        if (!Enum.IsDefined(options.Show))
            throw new ArgumentOutOfRangeException(
                nameof(options),
                options.Show,
                "Unknown status display mode.");

        if ((options.Flags & ~AllStatusOptionFlags) != 0)
            throw new ArgumentOutOfRangeException(
                nameof(options),
                options.Flags,
                "Unknown status option bits.");

        if (options.Flags.HasFlag(GitStatusOptionFlags.SortCaseSensitively) &&
            options.Flags.HasFlag(GitStatusOptionFlags.SortCaseInsensitively))
        {
            throw new ArgumentException(
                "Case-sensitive and case-insensitive status sorting are mutually exclusive.",
                nameof(options));
        }

        if (options.Flags.HasFlag(GitStatusOptionFlags.NoRefresh) &&
            options.Flags.HasFlag(GitStatusOptionFlags.UpdateIndex))
        {
            throw new ArgumentException(
                "NoRefresh and UpdateIndex are mutually exclusive.",
                nameof(options));
        }

        ArgumentNullException.ThrowIfNull(options.Pathspecs);
    }

    private static byte[] EncodeStatusPathspecs(
        IReadOnlyList<string> pathspecs)
    {
        if (pathspecs.Count == 0)
            return Array.Empty<byte>();

        var result = new List<byte>();
        for (var index = 0; index < pathspecs.Count; index++)
        {
            var pathspec = pathspecs[index];
            if (string.IsNullOrEmpty(pathspec))
                throw new ArgumentException(
                    "Status pathspecs must be non-empty.",
                    nameof(pathspecs));
            if (pathspec.Contains('\0'))
                throw new ArgumentException(
                    "Status pathspecs must not contain NUL characters.",
                    nameof(pathspecs));

            byte[] encoded;
            try
            {
                encoded = EncodePath(pathspec);
            }
            catch (EncoderFallbackException exception)
            {
                throw new ArgumentException(
                    "Status pathspecs must be valid UTF-16 strings.",
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
