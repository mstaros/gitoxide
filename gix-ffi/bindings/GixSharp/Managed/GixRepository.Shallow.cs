namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>Gets owned IDs at the shallow history boundary, or an empty list for complete history.</summary>
    public IReadOnlyList<GixObjectId> GetShallowCommits() =>
        Invoke("GetShallowCommits", static repo =>
        {
            using var nativeIds = repo.ShallowCommits();
            var ids = new GixObjectId[nativeIds.Count];
            for (var i = 0; i < ids.Length; i++)
                ids[i] = new GixObjectId(nativeIds[i].String);
            return ids;
        });

    /// <summary>Gets the configured shallow-file path, which may name a file that does not exist.</summary>
    public string ShallowFilePath => DecodePath(ShallowFile());

    /// <summary>Gets the configured shallow-file location as owned platform bytes.</summary>
    public byte[] ShallowFile() =>
        Invoke("ShallowFile", static repo =>
        {
            using var path = repo.ShallowFile();
            return path.ToArray();
        });
}
