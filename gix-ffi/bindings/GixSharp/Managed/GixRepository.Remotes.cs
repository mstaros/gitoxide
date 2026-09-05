namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>Returns current configured remotes with stored URL bytes in ascending raw-byte name order.</summary>
    public IReadOnlyList<GitRemote> GetRemotes() => GetRemotes(resolveUrls: false);

    /// <summary>Returns owned remotes in ascending raw-byte name order.</summary>
    /// <param name="resolveUrls">
    /// False preserves configured URL bytes. True uses gix URL parsing and insteadOf/pushInsteadOf
    /// rules, including URL-shaped remote-name fallback. Invalid URLs or remote options then report a configuration error.
    /// </param>
    /// <remarks>
    /// Every call reads current configuration, including includes. Empty URL entries reset earlier
    /// values; absent fetch URLs produce a null Url. Push URLs fall back to fetch URLs.
    /// Text properties decode UTF-8 with replacement; byte properties always preserve the returned bytes.
    /// No generated resources escape this method.
    /// </remarks>
    public IReadOnlyList<GitRemote> GetRemotes(bool resolveUrls) =>
        Invoke("GetRemotes", repo =>
        {
            using var native = repo.Remotes(resolveUrls);
            var remotes = new GitRemote[native.Count];
            for (var i = 0; i < remotes.Length; i++)
            {
                // Nested vector views are borrowed from the outer owning vector.
                var entry = native[i];
                var fetch = new byte[entry.fetch_urls.Count][];
                var push = new byte[entry.push_urls.Count][];
                for (var j = 0; j < fetch.Length; j++) fetch[j] = entry.fetch_urls[j].ToArray();
                for (var j = 0; j < push.Length; j++) push[j] = entry.push_urls[j].ToArray();
                remotes[i] = new GitRemote(entry.name.ToArray(), fetch, push);
            }
            return Array.AsReadOnly(remotes);
        });
}
