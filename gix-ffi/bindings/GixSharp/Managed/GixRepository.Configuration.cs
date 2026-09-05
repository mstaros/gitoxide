namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>Gets a UTF-8 configuration string, or null when the key is absent.</summary>
    /// <remarks>
    /// Reads current repository configuration, including includes and conditional includes.
    /// Relative repository paths retain their original location after process directory changes.
    /// The last value wins; an implicit key has an empty string value.
    /// Invalid UTF-8 values are reported without replacement or data loss.
    /// </remarks>
    public string? GetConfigString(string name) => ReadConfigString(name, "GetConfigString");

    /// <summary>Gets a UTF-8 configuration string; a missing key returns false and an empty out value.</summary>
    public bool TryGetConfigString(string name, out string value)
    {
        var found = ReadConfigString(name, "TryGetConfigString");
        value = found ?? string.Empty;
        return found is not null;
    }

    /// <summary>Atomically sets a unique direct key in the repository-local config.</summary>
    /// <remarks>
    /// Linked worktrees write the common config, even when config.worktree is enabled.
    /// The config and its write lock stay with the originally opened repository after directory changes.
    /// Repeated keys and keys originating from local includes are rejected.
    /// Strings use strict UTF-8 encoding and cannot contain NUL.
    /// Raw values can be read and repaired even if a typed setting is invalid.
    /// After persistence, the handle refreshes typed settings when the configuration is valid.
    /// If reopening fails, the write still succeeds and other operations retain the previous
    /// cached settings until a later successful refresh; configuration reads always reload files.
    /// </remarks>
    public void SetConfigString(string name, string value)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(name);
        ArgumentNullException.ThrowIfNull(value);
        if (value.Contains('\0'))
            throw new ArgumentException("A configuration string cannot contain NUL.", nameof(value));
        var nameBytes = EncodePath(name);
        var valueBytes = EncodePath(value);
        Invoke("SetConfigString", repo =>
        {
            using var nativeName = nameBytes.Slice();
            using var nativeValue = valueBytes.Slice();
            repo.SetConfigString(nativeName, nativeValue);
        });
    }

    /// <summary>Deletes a unique direct repository-local key, returning false if it is absent at that level.</summary>
    /// <remarks>
    /// Included or repeated keys are rejected. Removing a local value may reveal a
    /// lower-priority value; worktree-specific values remain unchanged.
    /// Persistence and typed cache refresh follow the same rules as SetConfigString.
    /// </remarks>
    public bool DeleteConfigValue(string name)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(name);
        var bytes = EncodePath(name);
        return Invoke("DeleteConfigValue", repo =>
        {
            using var nativeName = bytes.Slice();
            return repo.DeleteConfigValue(nativeName);
        });
    }

    private string? ReadConfigString(string name, string operation)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(name);
        var bytes = EncodePath(name);
        try
        {
            return Invoke(operation, repo =>
            {
                using var nativeName = bytes.Slice();
                using var nativeValue = repo.GetConfigString(nativeName);
                return PathEncoding.GetString(nativeValue.ToArray());
            });
        }
        catch (GixException exception) when (exception.Kind == GixErrorKind.NotFound)
        {
            return null;
        }
    }
}
