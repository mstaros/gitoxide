using System.Text;

namespace GixSharp;

public sealed partial class GixRepository
{
    private static readonly Encoding GitTextEncoding =
        new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: false);

    /// <summary>Resolves a revision, peels annotated tags, and returns a complete commit.</summary>
    public GixCommit LookupCommit(string revision)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(revision);

        return Invoke("LookupCommit", repo =>
        {
            using var nativeRevision = revision.Utf8();
            using var nativeCommit = repo.LookupCommit(nativeRevision);
            return ReadCommit(nativeCommit);
        });
    }

    /// <summary>
    /// Returns commits reachable from <paramref name="revision"/> while hiding
    /// <paramref name="excludedRevision"/> and all of its ancestors.
    /// </summary>
    public IReadOnlyList<GixCommit> GetCommitHistory(
        string revision,
        string excludedRevision,
        int maxCount = 100,
        GixCommitSort sort = GixCommitSort.None)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(excludedRevision);
        return GetCommitHistoryCore(revision, excludedRevision, maxCount, sort);
    }

    /// <summary>Returns commits reachable from a revision.</summary>
    public IReadOnlyList<GixCommit> GetCommitHistory(
        string revision = "HEAD",
        int maxCount = 100,
        GixCommitSort sort = GixCommitSort.None) =>
        GetCommitHistoryCore(revision, null, maxCount, sort);

    /// <summary>Returns the type and uncompressed size of an object.</summary>
    public GixObjectMetadata GetObjectMetadata(GixObjectId objectId) =>
        Invoke("GetObjectMetadata", repo =>
        {
            using var nativeId = objectId.Value.Utf8();
            var nativeMetadata = repo.ObjectMetadata(nativeId);
            return new GixObjectMetadata(
                ReadObjectType(nativeMetadata.object_type),
                checked((long)nativeMetadata.size));
        });

    /// <summary>Returns the tree ID referenced by a commit revision.</summary>
    public GixObjectId GetCommitTreeId(string revision)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(revision);

        return Invoke("GetCommitTreeId", repo =>
        {
            using var nativeRevision = revision.Utf8();
            using var nativeId = repo.CommitTreeId(nativeRevision);
            return new GixObjectId(nativeId.String);
        });
    }

    /// <summary>Creates a commit object without updating a reference.</summary>
    public GixObjectId CreateCommitObject(
        string message,
        GixObjectId treeId,
        IReadOnlyList<GixObjectId> parentIds,
        GixSignature? author = null,
        GixSignature? committer = null) =>
        CreateCommitObjectCore(message, treeId, parentIds, null, author, committer);

    /// <summary>Creates a commit object and atomically updates an explicit reference.</summary>
    public GixObjectId CreateCommitObject(
        string message,
        GixObjectId treeId,
        IReadOnlyList<GixObjectId> parentIds,
        string updateReference,
        GixSignature? author = null,
        GixSignature? committer = null)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(updateReference);
        return CreateCommitObjectCore(
            message,
            treeId,
            parentIds,
            updateReference,
            author,
            committer);
    }

    /// <summary>Creates a commit from the current index and advances HEAD.</summary>
    public GixObjectId CreateCommit(
        string message,
        string? authorName = null,
        string? authorEmail = null,
        bool allowEmpty = false)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(message);
        if ((authorName is null) != (authorEmail is null))
            throw new ArgumentException(
                "Author name and email must either both be supplied or both be omitted.");
        if (authorName is not null)
        {
            ArgumentException.ThrowIfNullOrWhiteSpace(authorName);
            ArgumentException.ThrowIfNullOrWhiteSpace(authorEmail);
        }

        return Invoke("CreateCommit", repo =>
        {
            using var nativeMessage = message.Utf8();
            using var nativeAuthorName =
                GitTextEncoding.GetBytes(authorName ?? string.Empty).Slice();
            using var nativeAuthorEmail =
                GitTextEncoding.GetBytes(authorEmail ?? string.Empty).Slice();
            using var nativeId = repo.CreateCommitFromIndex(
                nativeMessage,
                authorName is not null,
                nativeAuthorName,
                nativeAuthorEmail,
                allowEmpty);
            return new GixObjectId(nativeId.String);
        });
    }

    /// <summary>
    /// Returns true when <paramref name="ancestor"/> is reachable by following
    /// parents from <paramref name="descendant"/>.
    /// </summary>
    public bool IsAncestorOf(GixObjectId ancestor, GixObjectId descendant)
    {
        if (ancestor == descendant)
            return true;

        return Invoke("IsAncestorOf", repo =>
        {
            using var nativeAncestor = ancestor.Value.Utf8();
            using var nativeDescendant = descendant.Value.Utf8();
            return repo.IsAncestorOf(nativeAncestor, nativeDescendant);
        });
    }

    private IReadOnlyList<GixCommit> GetCommitHistoryCore(
        string revision,
        string? excludedRevision,
        int maxCount,
        GixCommitSort sort)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(revision);
        ArgumentOutOfRangeException.ThrowIfNegative(maxCount);
        const GixCommitSort allSortOptions =
            GixCommitSort.Topological | GixCommitSort.Time | GixCommitSort.Reverse;
        if ((sort & ~allSortOptions) != 0)
            throw new ArgumentOutOfRangeException(
                nameof(sort),
                sort,
                "Unknown commit sort option.");
        if (maxCount == 0)
            return Array.Empty<GixCommit>();

        return Invoke("GetCommitHistory", repo =>
        {
            using var nativeRevision = revision.Utf8();
            using var nativeExcluded = (excludedRevision ?? string.Empty).Utf8();
            using var nativeIds = repo.CommitHistory(
                nativeRevision,
                nativeExcluded,
                (ulong)maxCount,
                (uint)sort);
            var commits = new GixCommit[nativeIds.Count];

            for (var index = 0; index < commits.Length; index++)
            {
                var id = nativeIds[index].String;
                using var nativeId = id.Utf8();
                using var nativeCommit = repo.LookupCommit(nativeId);
                commits[index] = ReadCommit(nativeCommit);
            }

            return commits;
        });
    }

    private GixObjectId CreateCommitObjectCore(
        string message,
        GixObjectId treeId,
        IReadOnlyList<GixObjectId> parentIds,
        string? updateReference,
        GixSignature? author,
        GixSignature? committer)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(message);
        ArgumentNullException.ThrowIfNull(parentIds);

        return Invoke("CreateCommitObject", repo =>
        {
            var nativeParentIds = new Utf8String[parentIds.Count];
            try
            {
                for (var index = 0; index < parentIds.Count; index++)
                    nativeParentIds[index] = parentIds[index].Value.Utf8();

                using var nativeMessage = message.Utf8();
                using var nativeTreeId = treeId.Value.Utf8();
                using var nativeParents = nativeParentIds.IntoVec();
                using var nativeUpdateReference =
                    GitTextEncoding.GetBytes(updateReference ?? string.Empty).Slice();
                using var nativeAuthorName =
                    GitTextEncoding.GetBytes(author?.Name ?? string.Empty).Slice();
                using var nativeAuthorEmail =
                    GitTextEncoding.GetBytes(author?.Email ?? string.Empty).Slice();
                using var nativeCommitterName =
                    GitTextEncoding.GetBytes(committer?.Name ?? string.Empty).Slice();
                using var nativeCommitterEmail =
                    GitTextEncoding.GetBytes(committer?.Email ?? string.Empty).Slice();
                using var nativeId = repo.CreateCommitObject(
                    nativeMessage,
                    nativeTreeId,
                    nativeParents,
                    nativeUpdateReference,
                    author is not null,
                    nativeAuthorName,
                    nativeAuthorEmail,
                    author?.When.ToUnixTimeSeconds() ?? 0,
                    SignatureOffsetSeconds(author),
                    committer is not null,
                    nativeCommitterName,
                    nativeCommitterEmail,
                    committer?.When.ToUnixTimeSeconds() ?? 0,
                    SignatureOffsetSeconds(committer));
                return new GixObjectId(nativeId.String);
            }
            finally
            {
                foreach (var nativeParentId in nativeParentIds)
                    nativeParentId?.Dispose();
            }
        });
    }

    private static GixCommit ReadCommit(CommitRecord nativeCommit)
    {
        var parentIds = new GixObjectId[nativeCommit.parent_ids.Count];
        for (var index = 0; index < parentIds.Length; index++)
            parentIds[index] = new GixObjectId(nativeCommit.parent_ids[index].String);

        return new GixCommit(
            new GixObjectId(nativeCommit.id.String),
            GitTextEncoding.GetString(nativeCommit.message.ToArray()),
            ReadSignature(
                nativeCommit.author_name,
                nativeCommit.author_email,
                nativeCommit.author_time_seconds,
                nativeCommit.author_time_offset_seconds),
            ReadSignature(
                nativeCommit.committer_name,
                nativeCommit.committer_email,
                nativeCommit.committer_time_seconds,
                nativeCommit.committer_time_offset_seconds),
            parentIds);
    }

    private static GixSignature ReadSignature(
        VecByte name,
        VecByte email,
        long seconds,
        int offsetSeconds)
    {
        var when = DateTimeOffset
            .FromUnixTimeSeconds(seconds)
            .ToOffset(TimeSpan.FromSeconds(offsetSeconds));
        return new GixSignature(
            GitTextEncoding.GetString(name.ToArray()),
            GitTextEncoding.GetString(email.ToArray()),
            when);
    }

    private static GixObjectType ReadObjectType(FfiObjectType objectType)
    {
        if (objectType.IsCommit)
            return GixObjectType.Commit;
        if (objectType.IsTree)
            return GixObjectType.Tree;
        if (objectType.IsBlob)
            return GixObjectType.Blob;
        if (objectType.IsTag)
            return GixObjectType.Tag;

        throw new InvalidOperationException("gitoxide returned an unknown object type.");
    }

    private static int SignatureOffsetSeconds(GixSignature? signature) =>
        signature is null
            ? 0
            : checked((int)signature.When.Offset.TotalSeconds);
}
