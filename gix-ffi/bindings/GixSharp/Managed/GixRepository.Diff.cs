namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>
    /// Returns a read-only unified diff. Staged compares HEAD to the index;
    /// Unstaged compares the index to tracked worktree files; WorkingTree compares
    /// HEAD to the worktree and includes non-ignored untracked files.
    /// </summary>
    /// <remarks>
    /// Patches use three context lines and repository diff attributes and filters.
    /// Binary changes have a summary and no line counts. Unmerged and sparse-directory
    /// indices are rejected. Rename/copy tracking is available in GetTreeChanges;
    /// patch diffs preserve additions/deletions. Returned data owns no native resources.
    /// </remarks>
    public GitDiffResult GetDiff(
        GitDiffTarget target = GitDiffTarget.WorkingTree, params string[] pathspecs)
    {
        if (!Enum.IsDefined(target))
            throw new ArgumentOutOfRangeException(nameof(target));
        ArgumentNullException.ThrowIfNull(pathspecs);
        var patterns = EncodeStatusPathspecs(pathspecs);
        return Invoke("GetDiff", repo =>
        {
            using var nativePatterns = patterns.Slice();
            using var native = repo.Diff((uint)target, nativePatterns);
            return new GitDiffResult(native.patch.ToArray(),
                checked((int)native.file_count),
                checked((int)native.lines_added),
                checked((int)native.lines_deleted));
        });
    }

    /// <summary>Returns the UTF-8 decoded unified patch for the requested comparison.</summary>
    public string GetPatch(
        GitDiffTarget target = GitDiffTarget.WorkingTree, params string[] pathspecs) =>
        GetDiff(target, pathspecs).Patch;

    /// <summary>
    /// Returns file-level changes between commits, tags or trees in raw Git path order.
    /// A null old revision selects the empty tree. Repository diff.renames and
    /// diff.renameLimit configuration controls rename/copy tracking.
    /// Pathspecs match either side of a rename.
    /// </summary>
    public IReadOnlyList<GitTreeChange> GetTreeChanges(
        string? oldRevision, string newRevision, params string[] pathspecs)
    {
        if (oldRevision is not null)
            ArgumentException.ThrowIfNullOrWhiteSpace(oldRevision);
        ArgumentException.ThrowIfNullOrWhiteSpace(newRevision);
        ArgumentNullException.ThrowIfNull(pathspecs);
        var patterns = EncodeStatusPathspecs(pathspecs);

        return Invoke("GetTreeChanges", repo =>
        {
            using var nativeOld = (oldRevision ?? string.Empty).Utf8();
            using var nativeNew = newRevision.Utf8();
            using var nativePatterns = patterns.Slice();
            using var native = repo.TreeChanges(nativeOld, nativeNew, nativePatterns);
            var result = new GitTreeChange[native.Count];
            for (var i = 0; i < result.Length; i++)
            {
                // Element and Option payload views are borrowed from the owning vector.
                var entry = native[i];
                result[i] = new GitTreeChange(entry.path.ToArray(),
                    entry.old_path.IsSome ? entry.old_path.AsSome().ToArray() : null,
                    (GitTreeChangeKind)entry.kind)
                {
                    ObjectId = entry.object_id.IsSome
                        ? new GixObjectId(entry.object_id.AsSome().String) : null,
                    OldObjectId = entry.old_object_id.IsSome
                        ? new GixObjectId(entry.old_object_id.AsSome().String) : null,
                    Mode = (GitFileMode)entry.mode,
                    OldMode = (GitFileMode)entry.old_mode,
                };
            }
            return result;
        });
    }
}
