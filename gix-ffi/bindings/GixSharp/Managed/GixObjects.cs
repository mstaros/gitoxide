namespace GixSharp;

/// <summary>A validated SHA-1 Git object identifier.</summary>
public readonly record struct GixObjectId
{
    public GixObjectId(string value)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(value);
        if (value.Length != 40 || value.Any(static character => !Uri.IsHexDigit(character)))
            throw new FormatException("A Git object ID must contain exactly 40 hexadecimal characters.");

        Value = value.ToLowerInvariant();
    }

    /// <summary>Gets the lowercase 40-character hexadecimal identifier.</summary>
    public string Value { get; }

    public override string ToString() => Value;
}

/// <summary>The type of an object stored in the Git object database.</summary>
public enum GixObjectType
{
    Invalid = -1,
    Commit = 1,
    Tree = 2,
    Blob = 3,
    Tag = 4,
    OffsetDelta = 6,
    ReferenceDelta = 7,
}

/// <summary>Metadata for an object stored in the Git object database.</summary>
public sealed record GixObjectMetadata(GixObjectType Type, long Size);

/// <summary>A Git identity and its exact timestamp and timezone offset.</summary>
public sealed record GixSignature(string Name, string Email, DateTimeOffset When);

/// <summary>A complete managed commit record.</summary>
public sealed record GixCommit(
    GixObjectId Id,
    string Message,
    GixSignature Author,
    GixSignature Committer,
    IReadOnlyList<GixObjectId> ParentIds);

/// <summary>Commit traversal ordering options.</summary>
[Flags]
public enum GixCommitSort
{
    None = 0,
    Topological = 1,
    Time = 2,
    Reverse = 4,
}
