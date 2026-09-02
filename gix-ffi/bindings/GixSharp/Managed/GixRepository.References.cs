using System.Text;

namespace GixSharp;

public sealed partial class GixRepository
{
    private static readonly UTF8Encoding ReferenceEncoding =
        new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);

    /// <summary>Returns every ordinary reference in bytewise name order.</summary>
    public IReadOnlyList<GitReferenceInfo> GetReferences() =>
        GetReferencesCore(Array.Empty<byte>());

    /// <summary>
    /// Returns references matching a Git wildcard pattern. An asterisk may
    /// cross reference-name separators.
    /// </summary>
    public IReadOnlyList<GitReferenceInfo> GetReferences(string glob)
    {
        ArgumentNullException.ThrowIfNull(glob);
        if (glob.Contains('\0'))
            throw new ArgumentException(
                "A reference glob must not contain NUL characters.",
                nameof(glob));

        return GetReferencesCore(EncodeReferenceText(glob, nameof(glob)));
    }

    /// <summary>Returns local and/or remote-tracking branches.</summary>
    public IReadOnlyList<GitBranch> GetBranches(
        GitBranchFilter filter = GitBranchFilter.All)
    {
        if (filter is < GitBranchFilter.Local or > GitBranchFilter.All)
            throw new ArgumentOutOfRangeException(
                nameof(filter),
                filter,
                "A branch filter must select local, remote, or both.");

        return Invoke("GetBranches", repo =>
        {
            using var nativeBranches = repo.Branches((uint)filter);
            var branches = new GitBranch[nativeBranches.Count];

            for (var index = 0; index < branches.Length; index++)
            {
                var nativeBranch = nativeBranches[index];
                branches[index] = new GitBranch(
                    nativeBranch.name.ToArray(),
                    nativeBranch.is_remote,
                    nativeBranch.has_target
                        ? new GixObjectId(nativeBranch.target.String)
                        : null);
            }

            return branches;
        });
    }

    /// <summary>
    /// Creates a local branch at a commit-resolving revision. Existing branches
    /// are replaced only when <paramref name="force"/> is true.
    /// </summary>
    public GitBranch CreateBranch(
        string name,
        string targetRevision = "HEAD",
        bool force = false)
    {
        var nameBytes = EncodeRequiredReferenceText(name, nameof(name));
        ArgumentException.ThrowIfNullOrWhiteSpace(targetRevision);

        return Invoke("CreateBranch", repo =>
        {
            using var nativeName = nameBytes.Slice();
            using var nativeRevision = targetRevision.Utf8();
            using var nativeBranch =
                repo.CreateBranch(nativeName, nativeRevision, force);
            return new GitBranch(
                nativeBranch.name.ToArray(),
                nativeBranch.is_remote,
                nativeBranch.has_target
                    ? new GixObjectId(nativeBranch.target.String)
                    : null);
        });
    }

    /// <summary>Deletes a local or remote-tracking branch.</summary>
    public void DeleteBranch(string name, bool remote = false)
    {
        var nameBytes = EncodeRequiredReferenceText(name, nameof(name));

        Invoke("DeleteBranch", repo =>
        {
            using var nativeName = nameBytes.Slice();
            repo.DeleteBranch(nativeName, remote);
        });
    }

    /// <summary>
    /// Points HEAD symbolically at a local branch without checking out files.
    /// The branch may be unborn.
    /// </summary>
    public void SetHead(string branchName)
    {
        var branchNameBytes =
            EncodeRequiredReferenceText(branchName, nameof(branchName));

        Invoke("SetHead", repo =>
        {
            using var nativeBranchName = branchNameBytes.Slice();
            repo.SetHead(nativeBranchName);
        });
    }

    /// <summary>
    /// Resolves an exact reference through symbolic links to its first object ID.
    /// </summary>
    public bool TryGetReferenceTarget(
        string name,
        out GixObjectId target)
    {
        var nameBytes = EncodeRequiredReferenceText(name, nameof(name));
        var resolved = Invoke("TryGetReferenceTarget", repo =>
        {
            using var nativeName = nameBytes.Slice();
            using var nativeTarget = repo.TryGetReferenceTarget(nativeName);
            return nativeTarget.found
                ? new GixObjectId(nativeTarget.id.String)
                : (GixObjectId?)null;
        });

        if (resolved is { } objectId)
        {
            target = objectId;
            return true;
        }

        target = default;
        return false;
    }

    /// <summary>Creates an exact direct reference only when it is absent.</summary>
    public bool TryCreateReference(string name, GixObjectId target)
    {
        var nameBytes = EncodeRequiredReferenceText(name, nameof(name));

        return Invoke("TryCreateReference", repo =>
        {
            using var nativeName = nameBytes.Slice();
            using var nativeTarget = target.Value.Utf8();
            return repo.TryCreateReference(nativeName, nativeTarget);
        });
    }

    /// <summary>
    /// Atomically replaces an exact direct reference when its current object ID
    /// equals <paramref name="expected"/>.
    /// </summary>
    public ReferenceUpdateOutcome CompareExchangeReference(
        string name,
        GixObjectId target,
        GixObjectId expected)
    {
        var nameBytes = EncodeRequiredReferenceText(name, nameof(name));

        return Invoke("CompareExchangeReference", repo =>
        {
            using var nativeName = nameBytes.Slice();
            using var nativeTarget = target.Value.Utf8();
            using var nativeExpected = expected.Value.Utf8();
            return repo.CompareExchangeReference(
                nativeName,
                nativeTarget,
                nativeExpected);
        });
    }

    /// <summary>
    /// Deletes an exact direct reference when its current object ID equals
    /// <paramref name="expected"/>.
    /// </summary>
    public ReferenceUpdateOutcome DeleteReference(string name, GixObjectId expected)
    {
        var nameBytes = EncodeRequiredReferenceText(name, nameof(name));

        return Invoke("DeleteReference", repo =>
        {
            using var nativeName = nameBytes.Slice();
            using var nativeExpected = expected.Value.Utf8();
            return repo.DeleteReference(nativeName, nativeExpected);
        });
    }

    /// <summary>
    /// Atomically acquires cooperative loose-reference locks and holds them until
    /// the returned lease is disposed.
    /// </summary>
    public IDisposable AcquireReferenceLocks(params string[] referenceNames)
    {
        var encodedNames = EncodeReferenceNames(referenceNames);

        return Invoke("AcquireReferenceLocks", _ =>
        {
            using var nativeRepositoryPath = _repositoryPathBytes.Slice();
            using var nativeNames = encodedNames.Slice();
            return new ManagedReferenceLock(
                ReferenceLockLease.Acquire(
                    nativeRepositoryPath,
                    nativeNames));
        });
    }

    private IReadOnlyList<GitReferenceInfo> GetReferencesCore(byte[] glob)
    {
        return Invoke("GetReferences", repo =>
        {
            using var nativeGlob = glob.Slice();
            using var nativeReferences = repo.References(nativeGlob);
            var references = new GitReferenceInfo[nativeReferences.Count];

            for (var index = 0; index < references.Length; index++)
            {
                var nativeReference = nativeReferences[index];
                references[index] = new GitReferenceInfo(
                    nativeReference.name.ToArray(),
                    nativeReference.shorthand.ToArray(),
                    nativeReference.has_target
                        ? new GixObjectId(nativeReference.target.String)
                        : null,
                    nativeReference.has_symbolic_target
                        ? nativeReference.symbolic_target.ToArray()
                        : null);
            }

            return references;
        });
    }

    private static byte[] EncodeRequiredReferenceText(
        string value,
        string parameterName)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(value, parameterName);
        if (value.Contains('\0'))
            throw new ArgumentException(
                "A reference name must not contain NUL characters.",
                parameterName);

        return EncodeReferenceText(value, parameterName);
    }

    private static byte[] EncodeReferenceText(
        string value,
        string parameterName)
    {
        try
        {
            return ReferenceEncoding.GetBytes(value);
        }
        catch (EncoderFallbackException exception)
        {
            throw new ArgumentException(
                "Reference text must be a valid UTF-16 string.",
                parameterName,
                exception);
        }
    }

    private static byte[] EncodeReferenceNames(
        IReadOnlyList<string> referenceNames)
    {
        ArgumentNullException.ThrowIfNull(referenceNames);
        if (referenceNames.Count == 0)
            throw new ArgumentException(
                "At least one reference name is required.",
                nameof(referenceNames));

        var result = new List<byte>();
        for (var index = 0; index < referenceNames.Count; index++)
        {
            if (index != 0)
                result.Add(0);
            result.AddRange(EncodeRequiredReferenceText(
                referenceNames[index],
                nameof(referenceNames)));
        }

        return [.. result];
    }

    private sealed class ManagedReferenceLock : IDisposable
    {
        private ReferenceLockLease? _lease;

        internal ManagedReferenceLock(ReferenceLockLease lease)
        {
            _lease = lease;
        }

        public void Dispose() =>
            Interlocked.Exchange(ref _lease, null)?.Dispose();
    }
}
