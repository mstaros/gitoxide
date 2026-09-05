using System.Text;

namespace GixSharp;

/// <summary>An owned note and the author/committer of the current notes commit.</summary>
public sealed record GitNote(GixObjectId Id, string Message, GixSignature Author, GixSignature Committer)
{
    private readonly string _message = Message;
    private readonly GixSignature _author = Author;
    private readonly GixSignature _committer = Committer;
    private readonly string _messageBytes = Encode(Message);
    private readonly string _authorNameBytes = Encode(Author.Name);
    private readonly string _authorEmailBytes = Encode(Author.Email);
    private readonly string _committerNameBytes = Encode(Committer.Name);
    private readonly string _committerEmailBytes = Encode(Committer.Email);

    public string Message
    {
        get => _message;
        init { _message = value; _messageBytes = Encode(value); }
    }

    public GixSignature Author
    {
        get => _author;
        init
        {
            _author = value;
            _authorNameBytes = Encode(value.Name);
            _authorEmailBytes = Encode(value.Email);
        }
    }

    public GixSignature Committer
    {
        get => _committer;
        init
        {
            _committer = value;
            _committerNameBytes = Encode(value.Name);
            _committerEmailBytes = Encode(value.Email);
        }
    }

    internal GitNote(GixObjectId id, byte[] message, GixSignature author, GixSignature committer,
        byte[] authorName, byte[] authorEmail, byte[] committerName, byte[] committerEmail)
        : this(id, Encoding.UTF8.GetString(message), author, committer)
    {
        _messageBytes = Convert.ToBase64String(message);
        _authorNameBytes = Convert.ToBase64String(authorName);
        _authorEmailBytes = Convert.ToBase64String(authorEmail);
        _committerNameBytes = Convert.ToBase64String(committerName);
        _committerEmailBytes = Convert.ToBase64String(committerEmail);
    }

    /// <summary>Gets an independent copy of the exact note blob bytes.</summary>
    public byte[] MessageBytes => Convert.FromBase64String(_messageBytes);
    /// <summary>Gets the exact author name bytes; Author decodes text as UTF-8.</summary>
    public byte[] AuthorNameBytes => Convert.FromBase64String(_authorNameBytes);
    public byte[] AuthorEmailBytes => Convert.FromBase64String(_authorEmailBytes);
    public byte[] CommitterNameBytes => Convert.FromBase64String(_committerNameBytes);
    public byte[] CommitterEmailBytes => Convert.FromBase64String(_committerEmailBytes);

    private static string Encode(string text) => Convert.ToBase64String(Encoding.UTF8.GetBytes(text));
}

/// <summary>An annotated object and its owned note.</summary>
public sealed record GitNoteEntry(GixObjectId AnnotatedObjectId, GitNote Note);
