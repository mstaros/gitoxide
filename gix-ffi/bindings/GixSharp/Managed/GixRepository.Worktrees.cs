namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>Gets linked registrations in raw name order, including stale entries, from the common repository.</summary>
    public IReadOnlyList<GitWorktreeInfo> GetWorktrees() =>
        Invoke("GetWorktrees", static repo =>
        {
            using var native = repo.GetWorktrees();
            var entries = new GitWorktreeInfo[native.Count];
            for (var i = 0; i < entries.Length; i++)
                entries[i] = ReadWorktreeRecord(native[i]);
            return entries;
        });

    /// <summary>Adds a linked worktree and checks it out. Null referenceName creates a new name branch at HEAD.</summary>
    public GitWorktreeInfo AddWorktree(string name, string path, string? referenceName = null, bool lockWorktree = false) =>
        AddWorktree(name, path, referenceName, lockWorktree, checkoutWorktree: true);

    public GitWorktreeInfo AddWorktree(string name, string path, string? referenceName, bool lockWorktree, bool checkoutWorktree)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(name);
        ArgumentException.ThrowIfNullOrWhiteSpace(path);
        if (referenceName is not null) ArgumentException.ThrowIfNullOrWhiteSpace(referenceName);
        return AddWorktree(EncodePath(name), EncodePath(path),
            referenceName is null ? null : EncodePath(referenceName), lockWorktree, checkoutWorktree);
    }

    public GitWorktreeInfo AddWorktree(byte[] name, byte[] path, byte[]? referenceName = null, bool lockWorktree = false) =>
        AddWorktree(name, path, referenceName, lockWorktree, checkoutWorktree: true);

    /// <summary>Adds a linked worktree using exact name/path/reference bytes. No-checkout leaves only registration files.</summary>
    public GitWorktreeInfo AddWorktree(byte[] name, byte[] path, byte[]? referenceName, bool lockWorktree, bool checkoutWorktree)
    {
        ValidateWorktreeBytes(name, nameof(name));
        ValidateWorktreeBytes(path, nameof(path));
        if (referenceName is not null) ValidateWorktreeBytes(referenceName, nameof(referenceName));
        return Invoke("AddWorktree", repo =>
        {
            using var nativeName = name.Slice();
            using var nativePath = path.Slice();
            using var nativeReference = (referenceName ?? []).Slice();
            using var result = repo.AddWorktree(nativeName, nativePath, nativeReference, lockWorktree, checkoutWorktree);
            return ReadWorktreeRecord(result);
        });
    }

    /// <summary>Adds and checks out a detached linked worktree at an exact commit.</summary>
    public GitWorktreeInfo AddDetachedWorktree(string name, string path, GixObjectId commitId, bool lockWorktree = false)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(name);
        ArgumentException.ThrowIfNullOrWhiteSpace(path);
        return AddDetachedWorktree(EncodePath(name), EncodePath(path), commitId, lockWorktree);
    }

    public GitWorktreeInfo AddDetachedWorktree(byte[] name, byte[] path, GixObjectId commitId, bool lockWorktree = false)
    {
        ValidateWorktreeBytes(name, nameof(name));
        ValidateWorktreeBytes(path, nameof(path));
        if (string.IsNullOrEmpty(commitId.Value))
            throw new ArgumentException("The commit ID must be initialized.", nameof(commitId));
        return Invoke("AddDetachedWorktree", repo =>
        {
            using var nativeName = name.Slice();
            using var nativePath = path.Slice();
            using var nativeId = commitId.Value.Utf8();
            using var result = repo.AddDetachedWorktree(nativeName, nativePath, nativeId, lockWorktree);
            return ReadWorktreeRecord(result);
        });
    }

    /// <summary>
    /// Prunes eligible linked registrations. Checkout files are retained unless removeWorkingTrees is true;
    /// working-tree removal refuses dirty/untracked files even when includeLocked is true.
    /// </summary>
    public int PruneWorktrees(bool includeValid = false, bool includeLocked = false, bool removeWorkingTrees = false) =>
        Invoke("PruneWorktrees", repo => checked((int)repo.PruneWorktrees(includeValid, includeLocked, removeWorkingTrees)));

    /// <summary>Prunes one exact registration; absence or ineligibility returns false.</summary>
    public bool PruneWorktree(string name, bool includeValid = false, bool includeLocked = false, bool removeWorkingTree = false)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(name);
        return PruneWorktree(EncodePath(name), includeValid, includeLocked, removeWorkingTree);
    }

    public bool PruneWorktree(byte[] name, bool includeValid = false, bool includeLocked = false, bool removeWorkingTree = false)
    {
        ValidateWorktreeBytes(name, nameof(name));
        return Invoke("PruneWorktree", repo =>
        {
            using var nativeName = name.Slice();
            return repo.PruneWorktree(nativeName, includeValid, includeLocked, removeWorkingTree);
        });
    }

    private static GitWorktreeInfo ReadWorktreeRecord(WorktreeRecord value) =>
        new(value.name.ToArray(), value.path.ToArray(), value.is_valid, value.is_locked, value.lock_reason.ToArray());

    private static void ValidateWorktreeBytes(byte[] bytes, string parameterName)
    {
        ArgumentNullException.ThrowIfNull(bytes, parameterName);
        if (bytes.Length == 0) throw new ArgumentException("The value cannot be empty.", parameterName);
    }
}
