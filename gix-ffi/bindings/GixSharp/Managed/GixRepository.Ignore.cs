namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>Tests ignore rules independently of whether the path is tracked.</summary>
    /// <remarks>Use repository-relative paths with '/' separators. The directory flag is explicit.</remarks>
    public bool IsPathIgnored(string pathFromRepoRoot, bool isDirectory)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(pathFromRepoRoot);
        return IsPathIgnored(EncodePath(pathFromRepoRoot), isDirectory);
    }

    /// <summary>Tests ignore rules against exact repository-relative Git path bytes.</summary>
    public bool IsPathIgnored(byte[] pathFromRepoRoot, bool isDirectory)
    {
        ValidateExcludePath(pathFromRepoRoot);
        return Invoke("IsPathIgnored", repo =>
        {
            using var nativePath = pathFromRepoRoot.Slice();
            return repo.IsPathIgnored(nativePath, isDirectory);
        });
    }

    /// <summary>Adds an exact root-anchored rule to the common Git directory's info/exclude file.</summary>
    /// <remarks>
    /// Existing file bytes are preserved; repeated additions are idempotent.
    /// Glob metacharacters in the path are escaped. Linked worktrees share the rule.
    /// If a higher-priority ignore rule overrides it, the local rule remains written
    /// and the method reports the failed ignore check.
    /// </remarks>
    public void EnsureLocalExclude(string pathFromRepoRoot, bool isDirectory)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(pathFromRepoRoot);
        EnsureLocalExclude(EncodePath(pathFromRepoRoot), isDirectory);
    }

    /// <summary>Adds a local exclude rule for exact Git path bytes.</summary>
    public void EnsureLocalExclude(byte[] pathFromRepoRoot, bool isDirectory)
    {
        ValidateExcludePath(pathFromRepoRoot);
        Invoke("EnsureLocalExclude", repo =>
        {
            using var nativePath = pathFromRepoRoot.Slice();
            repo.EnsureLocalExclude(nativePath, isDirectory);
        });
    }

    private static void ValidateExcludePath(byte[] path)
    {
        ArgumentNullException.ThrowIfNull(path);
        if (path.Length == 0 || path.Any(static value => value is 0 or 10 or 13 or 92))
            throw new ArgumentException(
                "A Git path must be non-empty, use '/' separators, and contain no NUL or line breaks.",
                nameof(path));

        var start = 0;
        for (var i = 0; i <= path.Length; i++)
        {
            if (i < path.Length && path[i] != (byte)'/')
                continue;

            var segment = path.AsSpan(start, i - start);
            if (segment.IsEmpty || segment.SequenceEqual("."u8) || segment.SequenceEqual(".."u8))
                throw new ArgumentException(
                    "A Git path must have no leading/trailing slash, empty segments or dot segments.",
                    nameof(path));
            start = i + 1;
        }
    }
}
