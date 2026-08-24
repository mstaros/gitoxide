namespace GixSharp;

/// <summary>Classifies an error reported by gitoxide.</summary>
public enum GixErrorKind
{
    NotARepository,
    Io,
    Config,
    InvalidPath,
    InvalidId,
    NotFound,
    InvalidReference,
    ReferenceConflict,
    Other,
}

/// <summary>Represents a gitoxide failure at the managed API boundary.</summary>
public sealed class GixException : Exception
{
    internal GixException(
        string operation,
        GixErrorKind kind,
        string nativeMessage,
        Exception? innerException = null)
        : base($"{operation} failed ({kind}): {nativeMessage}", innerException)
    {
        Operation = operation;
        Kind = kind;
        NativeMessage = nativeMessage;
    }

    /// <summary>Gets the managed operation that failed.</summary>
    public string Operation { get; }

    /// <summary>Gets the stable error category.</summary>
    public GixErrorKind Kind { get; }

    /// <summary>Gets the message returned by gitoxide.</summary>
    public string NativeMessage { get; }
}
