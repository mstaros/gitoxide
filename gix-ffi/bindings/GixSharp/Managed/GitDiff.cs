using System.Text;

namespace GixSharp;

/// <summary>Selects the old and new sides of a repository diff.</summary>
public enum GitDiffTarget
{
    Staged,
    Unstaged,
    WorkingTree,
}

/// <summary>A unified patch and its file and line counts.</summary>
public sealed record GitDiffResult(string Patch, int FileCount, int LinesAdded, int LinesDeleted)
{
    private readonly string _patch = Patch;
    private readonly string _patchBytes = Convert.ToBase64String(Encoding.UTF8.GetBytes(Patch));

    /// <summary>Gets the decoded patch; assigning a new value also replaces its byte representation.</summary>
    public string Patch
    {
        get => _patch;
        init
        {
            _patch = value;
            _patchBytes = Convert.ToBase64String(Encoding.UTF8.GetBytes(value));
        }
    }

    internal GitDiffResult(byte[] patch, int fileCount, int added, int deleted)
        : this(Encoding.UTF8.GetString(patch), fileCount, added, deleted)
    {
        _patchBytes = Convert.ToBase64String(patch);
    }

    /// <summary>Gets an independent copy of the exact patch bytes; Patch decodes as UTF-8.</summary>
    public byte[] PatchBytes => Convert.FromBase64String(_patchBytes);
}

/// <summary>The kind of a path-level tree delta.</summary>
public enum GitTreeChangeKind
{
    Unmodified = 0,
    Added = 1,
    Deleted = 2,
    Modified = 3,
    Renamed = 4,
    Copied = 5,
    Ignored = 6,
    Untracked = 7,
    TypeChanged = 8,
    Unreadable = 9,
    Conflicted = 10,
}

/// <summary>The Git tree entry mode, including executable files, links and submodules.</summary>
public enum GitFileMode
{
    Unreadable = 0,
    Tree = 16384,
    Blob = 33188,
    BlobExecutable = 33261,
    SymbolicLink = 40960,
    GitLink = 57344,
}

/// <summary>A path-level change between tree-resolving revisions.</summary>
public sealed record GitTreeChange(string Path, string? OldPath, GitTreeChangeKind Kind)
{
    private readonly string _path = Path;
    private readonly string? _oldPath = OldPath;
    private readonly string _pathBytes = Convert.ToBase64String(Encoding.UTF8.GetBytes(Path));
    private readonly string? _oldPathBytes = OldPath is null ? null :
        Convert.ToBase64String(Encoding.UTF8.GetBytes(OldPath));

    /// <summary>Gets the current decoded Git path; an init assignment replaces the exact bytes.</summary>
    public string Path
    {
        get => _path;
        init
        {
            _path = value;
            _pathBytes = Convert.ToBase64String(Encoding.UTF8.GetBytes(value));
        }
    }

    /// <summary>Gets the old decoded Git path; an init assignment replaces the exact bytes.</summary>
    public string? OldPath
    {
        get => _oldPath;
        init
        {
            _oldPath = value;
            _oldPathBytes = value is null ? null : Convert.ToBase64String(Encoding.UTF8.GetBytes(value));
        }
    }

    internal GitTreeChange(byte[] path, byte[]? oldPath, GitTreeChangeKind kind)
        : this(Encoding.UTF8.GetString(path),
            oldPath is null ? null : Encoding.UTF8.GetString(oldPath), kind)
    {
        _pathBytes = Convert.ToBase64String(path);
        _oldPathBytes = oldPath is null ? null : Convert.ToBase64String(oldPath);
    }

    /// <summary>Gets the new object ID, absent for deletions.</summary>
    public GixObjectId? ObjectId { get; init; }

    /// <summary>Gets the old object ID, absent for additions.</summary>
    public GixObjectId? OldObjectId { get; init; }

    /// <summary>Gets the new mode, or Unreadable for deletions.</summary>
    public GitFileMode Mode { get; init; }

    /// <summary>Gets the old mode, or Unreadable for additions.</summary>
    public GitFileMode OldMode { get; init; }

    /// <summary>Gets an independent copy of the exact current Git path bytes.</summary>
    public byte[] PathBytes => Convert.FromBase64String(_pathBytes);

    /// <summary>Gets an independent copy of the old Git path bytes, absent for additions.</summary>
    public byte[]? OldPathBytes => _oldPathBytes is null ? null : Convert.FromBase64String(_oldPathBytes);
}
