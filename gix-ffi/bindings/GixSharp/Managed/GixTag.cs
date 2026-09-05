using System.Text;

namespace GixSharp;

/// <summary>An owned annotated tag. Message includes any in-body cryptographic signature.</summary>
public sealed record GixTag(
    GixObjectId Id, GixObjectId TargetId, GixObjectType TargetType,
    string Name, string Message, GixSignature? Tagger)
{
    private readonly string _name = Name;
    private readonly string _message = Message;
    private readonly GixSignature? _tagger = Tagger;
    private readonly string _nameBytes = Encode(Name);
    private readonly string _messageBytes = Encode(Message);
    private readonly string? _taggerNameBytes = Tagger is null ? null : Encode(Tagger.Name);
    private readonly string? _taggerEmailBytes = Tagger is null ? null : Encode(Tagger.Email);
    private readonly string? _signatureBytes;

    public string Name
    {
        get => _name;
        init { _name = value; _nameBytes = Encode(value); }
    }

    public string Message
    {
        get => _message;
        init { _message = value; _messageBytes = Encode(value); _signatureBytes = null; }
    }

    public GixSignature? Tagger
    {
        get => _tagger;
        init
        {
            _tagger = value;
            _taggerNameBytes = value is null ? null : Encode(value.Name);
            _taggerEmailBytes = value is null ? null : Encode(value.Email);
        }
    }

    internal GixTag(GixObjectId id, GixObjectId targetId, GixObjectType targetType,
        byte[] name, byte[] message, GixSignature? tagger, byte[]? taggerName,
        byte[]? taggerEmail, byte[]? signature)
        : this(id, targetId, targetType, Encoding.UTF8.GetString(name), Encoding.UTF8.GetString(message), tagger)
    {
        _nameBytes = Convert.ToBase64String(name);
        _messageBytes = Convert.ToBase64String(message);
        _taggerNameBytes = taggerName is null ? null : Convert.ToBase64String(taggerName);
        _taggerEmailBytes = taggerEmail is null ? null : Convert.ToBase64String(taggerEmail);
        _signatureBytes = signature is null ? null : Convert.ToBase64String(signature);
    }

    /// <summary>Gets an independent copy of the exact tag name bytes.</summary>
    public byte[] NameBytes => Convert.FromBase64String(_nameBytes);
    /// <summary>Gets the exact complete message body, including any signature armor.</summary>
    public byte[] MessageBytes => Convert.FromBase64String(_messageBytes);
    public byte[]? TaggerNameBytes => Decode(_taggerNameBytes);
    public byte[]? TaggerEmailBytes => Decode(_taggerEmailBytes);
    /// <summary>Gets the signature extracted by gix, without verifying it. Editing Message clears this metadata.</summary>
    public byte[]? SignatureBytes => Decode(_signatureBytes);

    private static string Encode(string text) => Convert.ToBase64String(Encoding.UTF8.GetBytes(text));
    private static byte[]? Decode(string? bytes) => bytes is null ? null : Convert.FromBase64String(bytes);
}
