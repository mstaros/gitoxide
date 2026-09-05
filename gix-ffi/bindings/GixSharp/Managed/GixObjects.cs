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
public sealed record GixSignature(string Name, string Email, DateTimeOffset When)
{
    private readonly string _name = Name;
    private readonly string _email = Email;
    private readonly string _nameBytes = Encode(Name);
    private readonly string _emailBytes = Encode(Email);

    /// <summary>Gets the name as UTF-8 display text. Assigning text replaces its raw bytes.</summary>
    public string Name
    {
        get => _name;
        init { _name = value; _nameBytes = Encode(value); }
    }

    /// <summary>Gets the email as UTF-8 display text. Assigning text replaces its raw bytes.</summary>
    public string Email
    {
        get => _email;
        init { _email = value; _emailBytes = Encode(value); }
    }

    /// <summary>Gets an independent copy of the exact identity name bytes.</summary>
    public byte[] NameBytes => Convert.FromBase64String(_nameBytes);

    /// <summary>Gets an independent copy of the exact identity email bytes.</summary>
    public byte[] EmailBytes => Convert.FromBase64String(_emailBytes);

    private GixSignature(byte[] name, byte[] email, DateTimeOffset when)
        : this(System.Text.Encoding.UTF8.GetString(name), System.Text.Encoding.UTF8.GetString(email), when)
    {
        _nameBytes = Convert.ToBase64String(name);
        _emailBytes = Convert.ToBase64String(email);
    }

    /// <summary>Creates an owned signature from exact bytes, decoding display text as UTF-8.</summary>
    public static GixSignature FromBytes(byte[] name, byte[] email, DateTimeOffset when)
    {
        ArgumentNullException.ThrowIfNull(name);
        ArgumentNullException.ThrowIfNull(email);
        return new GixSignature(name, email, when);
    }

    private static string Encode(string text) =>
        Convert.ToBase64String(System.Text.Encoding.UTF8.GetBytes(text));
}

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
