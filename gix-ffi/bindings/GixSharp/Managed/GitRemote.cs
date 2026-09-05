using System.Text;

namespace GixSharp;

/// <summary>An owned configured remote. Text properties decode UTF-8; byte properties retain exact Git data.</summary>
public sealed record GitRemote(string Name, string? Url)
{
    private static readonly UTF8Encoding StrictUtf8 = new(false, true);
    private readonly string _nameBytes = Encode(Name);
    private readonly string[] _fetchUrls = Url is null ? [] : [Encode(Url)];
    private readonly string[] _pushUrls = [];

    /// <summary>Gets the remote name. An init assignment replaces its exact bytes.</summary>
    public string Name
    {
        get => Decode(_nameBytes);
        init => _nameBytes = Encode(value);
    }

    /// <summary>Gets the first fetch URL, or null. An init assignment replaces that entry; null clears fetch URLs.</summary>
    public string? Url
    {
        get => _fetchUrls.Length == 0 ? null : Decode(_fetchUrls[0]);
        init => _fetchUrls = value is null ? [] : [Encode(value), .. _fetchUrls.Skip(1)];
    }

    /// <summary>Gets the first push URL, falling back to the fetch URL when no push URLs are supplied.</summary>
    public string? PushUrl => PushValues.Length == 0 ? null : Decode(PushValues[0]);

    /// <summary>Gets all fetch URLs in configuration order as an independent read-only list.</summary>
    public IReadOnlyList<string> FetchUrls => Array.AsReadOnly(_fetchUrls.Select(Decode).ToArray());

    /// <summary>Gets all push URLs, including fetch URL fallback, as an independent read-only list.</summary>
    public IReadOnlyList<string> PushUrls => Array.AsReadOnly(PushValues.Select(Decode).ToArray());

    public byte[] NameBytes => Convert.FromBase64String(_nameBytes);
    public byte[]? UrlBytes => _fetchUrls.Length == 0 ? null : Convert.FromBase64String(_fetchUrls[0]);
    public byte[]? PushUrlBytes => PushValues.Length == 0 ? null : Convert.FromBase64String(PushValues[0]);

    /// <summary>Gets independent copies of every exact fetch URL.</summary>
    public IReadOnlyList<byte[]> FetchUrlBytes => Array.AsReadOnly(_fetchUrls.Select(Convert.FromBase64String).ToArray());

    /// <summary>Gets independent copies of every exact push URL, including fetch URL fallback.</summary>
    public IReadOnlyList<byte[]> PushUrlsBytes => Array.AsReadOnly(PushValues.Select(Convert.FromBase64String).ToArray());

    private string[] PushValues => _pushUrls.Length == 0 ? _fetchUrls : _pushUrls;

    internal GitRemote(byte[] name, byte[][] fetchUrls, byte[][] pushUrls)
        : this(Encoding.UTF8.GetString(name), null)
    {
        _nameBytes = Convert.ToBase64String(name);
        _fetchUrls = fetchUrls.Select(Convert.ToBase64String).ToArray();
        _pushUrls = pushUrls.Select(Convert.ToBase64String).ToArray();
    }

    // Compare the immutable byte values, not the private array identities.
    public bool Equals(GitRemote? other) => other is not null
        && _nameBytes == other._nameBytes
        && _fetchUrls.AsSpan().SequenceEqual(other._fetchUrls)
        && PushValues.AsSpan().SequenceEqual(other.PushValues);

    public override int GetHashCode()
    {
        var hash = new HashCode();
        hash.Add(_nameBytes);
        hash.Add(_fetchUrls.Length);
        foreach (var value in _fetchUrls) hash.Add(value);
        hash.Add(PushValues.Length);
        foreach (var value in PushValues) hash.Add(value);
        return hash.ToHashCode();
    }

    private static string Encode(string value)
    {
        ArgumentNullException.ThrowIfNull(value);
        return Convert.ToBase64String(StrictUtf8.GetBytes(value));
    }

    private static string Decode(string value) => Encoding.UTF8.GetString(Convert.FromBase64String(value));
}
