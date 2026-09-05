using System.Diagnostics.CodeAnalysis;

namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>Returns the cached configured author, or null when gix cannot resolve a complete identity.</summary>
    public GixSignature? GetAuthor() =>
        Invoke("GetAuthor", static repo =>
        {
            using var signature = repo.GetAuthor();
            return ReadOptionalSignature(signature);
        });

    /// <summary>Returns the cached configured committer, or null when gix cannot resolve a complete identity.</summary>
    public GixSignature? GetCommitter() =>
        Invoke("GetCommitter", static repo =>
        {
            using var signature = repo.GetCommitter();
            return ReadOptionalSignature(signature);
        });

    /// <summary>
    /// Returns the configured committer or installs a fallback in this repository session.
    /// Existing configured name and email take precedence independently. No configuration file is written.
    /// </summary>
    public GixSignature GetCommitterOrSetFallback(string name, string email)
    {
        ArgumentNullException.ThrowIfNull(name);
        ArgumentNullException.ThrowIfNull(email);
        return GetCommitterOrSetFallback(EncodePath(name), EncodePath(email));
    }

    /// <summary>Returns the configured committer or retains exact fallback identity bytes in this session.</summary>
    public GixSignature GetCommitterOrSetFallback(byte[] name, byte[] email)
    {
        ArgumentNullException.ThrowIfNull(name);
        ArgumentNullException.ThrowIfNull(email);
        return Invoke("GetCommitterOrSetFallback", repo =>
        {
            using var nativeName = name.Slice();
            using var nativeEmail = email.Slice();
            using var signature = repo.GetCommitterOrSetFallback(nativeName, nativeEmail);
            return ReadSignature(signature.name, signature.email, signature.time_seconds, signature.time_offset_seconds);
        });
    }

    /// <summary>Returns the configured committer or retains gix's generic fallback in this repository session.</summary>
    public GixSignature GetCommitterOrSetGenericFallback() =>
        Invoke("GetCommitterOrSetGenericFallback", static repo =>
        {
            using var signature = repo.GetCommitterOrSetGenericFallback();
            return ReadSignature(signature.name, signature.email, signature.time_seconds, signature.time_offset_seconds);
        });

    /// <summary>
    /// Resolves an identity through gix's repository mailmap, retaining its timestamp and unmapped fields.
    /// Like gix open_mailmap, loading and parsing errors are ignored and usable partial mappings are retained.
    /// </summary>
    public GixSignature ResolveMailmap(GixSignature signature)
    {
        ArgumentNullException.ThrowIfNull(signature);
        return Invoke("ResolveMailmap", repo =>
        {
            using var name = signature.NameBytes.Slice();
            using var email = signature.EmailBytes.Slice();
            using var mapped = repo.ResolveMailmap(name, email,
                signature.When.ToUnixTimeSeconds(), SignatureOffsetSeconds(signature));
            // Mailmap never changes time; retain even sub-second managed precision.
            return GixSignature.FromBytes(mapped.name.ToArray(), mapped.email.ToArray(), signature.When);
        });
    }

    /// <summary>
    /// Returns true when gix's lenient repository mailmap resolves this identity.
    /// False means no mapping applies; it does not report whether every mailmap source loaded successfully.
    /// </summary>
    public bool TryResolveMailmap(GixSignature signature, [NotNullWhen(true)] out GixSignature? resolved)
    {
        ArgumentNullException.ThrowIfNull(signature);
        resolved = Invoke("TryResolveMailmap", repo =>
        {
            using var name = signature.NameBytes.Slice();
            using var email = signature.EmailBytes.Slice();
            using var mapped = repo.TryResolveMailmap(name, email,
                signature.When.ToUnixTimeSeconds(), SignatureOffsetSeconds(signature));
            return mapped.is_present
                ? GixSignature.FromBytes(mapped.name.ToArray(), mapped.email.ToArray(), signature.When)
                : null;
        });
        return resolved is not null;
    }

    private static GixSignature? ReadOptionalSignature(SignatureRecord signature) =>
        signature.is_present
            ? ReadSignature(signature.name, signature.email, signature.time_seconds, signature.time_offset_seconds)
            : null;
}
