using System.Text;

namespace GixSharp;

/// <summary>One direct or symbolic Git reference.</summary>
public sealed record GitReferenceInfo(
    string Name,
    string Shorthand,
    GixObjectId? Target,
    string? SymbolicTarget)
{
    private static readonly UTF8Encoding StrictUtf8 =
        new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);

    private readonly string _nameBytes =
        Convert.ToBase64String(ValidateAndEncode(Name, nameof(Name)));
    private readonly string _shorthandBytes =
        Convert.ToBase64String(ValidateAndEncode(Shorthand, nameof(Shorthand)));
    private readonly string? _symbolicTargetBytes =
        SymbolicTarget is null
            ? null
            : Convert.ToBase64String(
                ValidateAndEncode(SymbolicTarget, nameof(SymbolicTarget)));

    internal GitReferenceInfo(
        byte[] nameBytes,
        byte[] shorthandBytes,
        GixObjectId? target,
        byte[]? symbolicTargetBytes)
        : this(
            DecodeAndValidate(nameBytes, nameof(nameBytes)),
            DecodeAndValidate(shorthandBytes, nameof(shorthandBytes)),
            target,
            symbolicTargetBytes is null
                ? null
                : DecodeAndValidate(
                    symbolicTargetBytes,
                    nameof(symbolicTargetBytes)))
    {
        _nameBytes = Convert.ToBase64String(nameBytes);
        _shorthandBytes = Convert.ToBase64String(shorthandBytes);
        _symbolicTargetBytes = symbolicTargetBytes is null
            ? null
            : Convert.ToBase64String(symbolicTargetBytes);
    }

    /// <summary>Gets the exact full reference name bytes as a new array.</summary>
    public byte[] NameBytes => Convert.FromBase64String(_nameBytes);

    /// <summary>Gets the exact shortened reference name bytes as a new array.</summary>
    public byte[] ShorthandBytes => Convert.FromBase64String(_shorthandBytes);

    /// <summary>
    /// Gets the exact symbolic target bytes as a new array, or null for a
    /// direct reference.
    /// </summary>
    public byte[]? SymbolicTargetBytes =>
        _symbolicTargetBytes is null
            ? null
            : Convert.FromBase64String(_symbolicTargetBytes);

    private static byte[] ValidateAndEncode(string value, string parameterName)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(value, parameterName);
        if (value.Contains('\0'))
            throw new ArgumentException(
                "A reference name must not contain NUL characters.",
                parameterName);

        try
        {
            return StrictUtf8.GetBytes(value);
        }
        catch (EncoderFallbackException exception)
        {
            throw new ArgumentException(
                "A reference name must be a valid UTF-16 string.",
                parameterName,
                exception);
        }
    }

    private static string DecodeAndValidate(byte[] value, string parameterName)
    {
        ArgumentNullException.ThrowIfNull(value, parameterName);
        if (value.Length == 0 || value.Contains((byte)0))
            throw new ArgumentException(
                "Reference name bytes must be non-empty and contain no NUL bytes.",
                parameterName);

        return Encoding.UTF8.GetString(value);
    }
}

/// <summary>Selects local and/or remote-tracking branches.</summary>
[Flags]
public enum GitBranchFilter
{
    Local = 1,
    Remote = 2,
    All = Local | Remote,
}

/// <summary>One local or remote-tracking Git branch.</summary>
public sealed record GitBranch(
    string Name,
    bool IsRemote,
    GixObjectId? Target)
{
    private static readonly UTF8Encoding StrictUtf8 =
        new(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);

    private readonly string _nameBytes =
        Convert.ToBase64String(ValidateAndEncode(Name));

    internal GitBranch(byte[] nameBytes, bool isRemote, GixObjectId? target)
        : this(DecodeAndValidate(nameBytes), isRemote, target)
    {
        _nameBytes = Convert.ToBase64String(nameBytes);
    }

    /// <summary>Gets the exact shortened branch name bytes as a new array.</summary>
    public byte[] NameBytes => Convert.FromBase64String(_nameBytes);

    private static byte[] ValidateAndEncode(string value)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(value);
        if (value.Contains('\0'))
            throw new ArgumentException(
                "A branch name must not contain NUL characters.",
                nameof(value));

        try
        {
            return StrictUtf8.GetBytes(value);
        }
        catch (EncoderFallbackException exception)
        {
            throw new ArgumentException(
                "A branch name must be a valid UTF-16 string.",
                nameof(value),
                exception);
        }
    }

    private static string DecodeAndValidate(byte[] value)
    {
        ArgumentNullException.ThrowIfNull(value);
        if (value.Length == 0 || value.Contains((byte)0))
            throw new ArgumentException(
                "Branch name bytes must be non-empty and contain no NUL bytes.",
                nameof(value));

        return Encoding.UTF8.GetString(value);
    }
}
