using System.Text;

namespace GixSharp;

/// <summary>An owned snapshot of one linked registration. The main checkout is not a linked entry.</summary>
public sealed record GitWorktreeInfo(string Name, string Path, bool IsValid, bool IsLocked, string? LockReason)
{
    private readonly string _name = Name;
    private readonly string _path = Path;
    private readonly string? _lockReason = LockReason;
    private readonly string _nameBytes = Encode(Name);
    private readonly string _pathBytes = Encode(Path);
    private readonly string? _lockReasonBytes = LockReason is null ? null : Encode(LockReason);

    public string Name
    {
        get => _name;
        init { _name = value; _nameBytes = Encode(value); }
    }

    public string Path
    {
        get => _path;
        init { _path = value; _pathBytes = Encode(value); }
    }

    public string? LockReason
    {
        get => _lockReason;
        init { _lockReason = value; _lockReasonBytes = value is null ? null : Encode(value); }
    }

    internal GitWorktreeInfo(byte[] name, byte[] path, bool isValid, bool isLocked, byte[] lockReason)
        : this(Encoding.UTF8.GetString(name), Encoding.UTF8.GetString(path),
            isValid, isLocked, isLocked ? Encoding.UTF8.GetString(lockReason) : null)
    {
        _nameBytes = Convert.ToBase64String(name);
        _pathBytes = Convert.ToBase64String(path);
        _lockReasonBytes = isLocked ? Convert.ToBase64String(lockReason) : null;
    }

    public byte[] NameBytes => Convert.FromBase64String(_nameBytes);
    /// <summary>The owned platform path bytes; empty if a malformed registration has no readable checkout path.</summary>
    public byte[] PathBytes => Convert.FromBase64String(_pathBytes);
    /// <summary>The exact lock-file bytes, including any final newline; null when unlocked.</summary>
    public byte[]? LockReasonBytes => _lockReasonBytes is null ? null : Convert.FromBase64String(_lockReasonBytes);

    private static string Encode(string text) => Convert.ToBase64String(Encoding.UTF8.GetBytes(text));
}
