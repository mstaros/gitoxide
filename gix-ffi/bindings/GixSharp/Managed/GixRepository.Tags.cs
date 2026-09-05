namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>Creates an annotated tag with a short name, explicit optional tagger and exact message.</summary>
    /// <remarks>Force replaces the tag through the normal ref transaction. Use reference CAS methods for an expected-old guard.
    /// Failed reference publication can leave an unreachable tag object, as with gix's tag creation.</remarks>
    public GixObjectId CreateAnnotatedTag(string name, GixObjectId targetId, GixSignature? tagger,
        string message, bool force = false)
    {
        ArgumentNullException.ThrowIfNull(message);
        return CreateAnnotatedTag(EncodeRequiredReferenceText(name, nameof(name)), targetId, tagger,
            EncodePath(message), force);
    }

    /// <summary>Creates an annotated tag while preserving exact tag-name and message bytes.</summary>
    public GixObjectId CreateAnnotatedTag(byte[] name, GixObjectId targetId, GixSignature? tagger,
        byte[] message, bool force = false)
    {
        if (tagger is not null)
        {
            ArgumentException.ThrowIfNullOrWhiteSpace(tagger.Name);
            ArgumentException.ThrowIfNullOrWhiteSpace(tagger.Email);
        }
        return CreateAnnotatedTagCore(name, targetId, message,
            tagger is null ? null : EncodePath(tagger.Name),
            tagger is null ? null : EncodePath(tagger.Email), tagger?.When ?? default, force);
    }

    /// <summary>Creates an annotated tag with exact tagger identity bytes and an explicit timestamp.</summary>
    public GixObjectId CreateAnnotatedTag(byte[] name, GixObjectId targetId,
        byte[] taggerName, byte[] taggerEmail, DateTimeOffset when, byte[] message, bool force = false)
    {
        ArgumentNullException.ThrowIfNull(taggerName);
        ArgumentNullException.ThrowIfNull(taggerEmail);
        return CreateAnnotatedTagCore(name, targetId, message, taggerName, taggerEmail, when, force);
    }

    /// <summary>Creates a lightweight tag without peeling the supplied object. The name excludes refs/tags/.</summary>
    public void CreateTagReference(string name, GixObjectId targetId, bool force = false) =>
        CreateTagReference(EncodeRequiredReferenceText(name, nameof(name)), targetId, force);

    public void CreateTagReference(byte[] name, GixObjectId targetId, bool force = false)
    {
        ValidateTagName(name);
        ValidateTagId(targetId);
        Invoke("CreateTagReference", repo =>
        {
            using var nativeName = name.Slice();
            using var nativeId = targetId.Value.Utf8();
            repo.CreateTagReference(nativeName, nativeId, force);
            return true;
        });
    }

    /// <summary>Reads a complete owned annotated-tag snapshot by object ID. Missing and wrong-kind objects are errors.</summary>
    public GixTag ReadTag(GixObjectId tagId)
    {
        ValidateTagId(tagId);
        return Invoke("ReadTag", repo =>
        {
            using var nativeId = tagId.Value.Utf8();
            using var tag = repo.ReadTag(nativeId);
            var tagger = tag.tagger switch
            {
                OptionTagSignatureRecord.SomeCase { Value: var value } => value,
                OptionTagSignatureRecord.NoneCase => null,
                null => throw new InvalidOperationException("gix returned an empty tagger option."),
            };
            var signature = tag.signature switch
            {
                OptionVecByte.SomeCase { Value: var value } => value.ToArray(),
                OptionVecByte.NoneCase => null,
                null => throw new InvalidOperationException("gix returned an empty tag signature option."),
            };
            return new GixTag(new GixObjectId(tag.id.String), new GixObjectId(tag.target_id.String),
                ReadObjectType(tag.target_type), tag.name.ToArray(), tag.message.ToArray(),
                tagger is null ? null : ReadSignature(tagger.name, tagger.email, tagger.time_seconds, tagger.time_offset_seconds),
                tagger?.name.ToArray(), tagger?.email.ToArray(), signature);
        });
    }

    /// <summary>Follows annotated tags to the first non-tag object. Other object kinds return their own ID.</summary>
    public GixObjectId PeelTags(GixObjectId objectId)
    {
        ValidateTagId(objectId);
        return Invoke("PeelTags", repo =>
        {
            using var nativeId = objectId.Value.Utf8();
            using var result = repo.PeelTags(nativeId);
            return new GixObjectId(result.String);
        });
    }

    private GixObjectId CreateAnnotatedTagCore(byte[] name, GixObjectId targetId, byte[] message,
        byte[]? taggerName, byte[]? taggerEmail, DateTimeOffset when, bool force)
    {
        ValidateTagName(name);
        ValidateTagId(targetId);
        ArgumentNullException.ThrowIfNull(message);
        return Invoke("CreateAnnotatedTag", repo =>
        {
            using var nativeName = name.Slice();
            using var nativeTargetId = targetId.Value.Utf8();
            using var nativeMessage = message.Slice();
            using var nativeTaggerName = taggerName?.Vec();
            using var nativeTaggerEmail = taggerEmail?.Vec();
            using var nativeTagger = taggerName is null
                ? OptionTagSignatureRecord.None
                : OptionTagSignatureRecord.Some(new TagSignatureRecord
                {
                    name = nativeTaggerName!,
                    email = nativeTaggerEmail!,
                    time_seconds = when.ToUnixTimeSeconds(),
                    time_offset_seconds = checked((int)when.Offset.TotalSeconds),
                });
            using var result = repo.CreateAnnotatedTag(nativeName, nativeTargetId, nativeMessage, nativeTagger, force);
            return new GixObjectId(result.String);
        });
    }

    private static void ValidateTagId(GixObjectId objectId)
    {
        if (string.IsNullOrEmpty(objectId.Value))
            throw new ArgumentException("The object ID must be initialized.", nameof(objectId));
    }

    private static void ValidateTagName(byte[] name)
    {
        ArgumentNullException.ThrowIfNull(name);
        if (name.Length == 0) throw new ArgumentException("A tag name cannot be empty.", nameof(name));
    }
}
