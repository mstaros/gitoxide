using System.Diagnostics.CodeAnalysis;

namespace GixSharp;

public sealed partial class GixRepository
{
    /// <summary>Writes a note with one identity for author and committer, preserving message text exactly.</summary>
    public GixObjectId WriteNote(GixObjectId annotatedObjectId, string message, string notesRef,
        GixSignature signature, bool overwrite = false)
    {
        ArgumentNullException.ThrowIfNull(message);
        return WriteNoteCore(annotatedObjectId, EncodePath(message), EncodeNotesRef(notesRef), signature, overwrite);
    }

    /// <summary>Writes a note to the configured default notes ref.</summary>
    public GixObjectId WriteNote(GixObjectId annotatedObjectId, string message,
        GixSignature signature, bool overwrite = false)
    {
        ArgumentNullException.ThrowIfNull(message);
        return WriteNoteCore(annotatedObjectId, EncodePath(message), [], signature, overwrite);
    }

    /// <summary>Writes exact note bytes to an exact byte-oriented reference name.</summary>
    public GixObjectId WriteNote(GixObjectId annotatedObjectId, byte[] message, byte[] notesRef,
        GixSignature signature, bool overwrite = false) =>
        WriteNoteCore(annotatedObjectId, message, ValidateNotesRef(notesRef), signature, overwrite);

    public GixObjectId WriteNote(GixObjectId annotatedObjectId, byte[] message, string notesRef,
        GixSignature signature, bool overwrite = false) =>
        WriteNoteCore(annotatedObjectId, message, EncodeNotesRef(notesRef), signature, overwrite);

    public GixObjectId WriteNote(GixObjectId annotatedObjectId, byte[] message,
        GixSignature signature, bool overwrite = false) =>
        WriteNoteCore(annotatedObjectId, message, [], signature, overwrite);

    /// <summary>Reads a note, throwing KeyNotFoundException for absence. Null selects the configured default ref.</summary>
    public GitNote ReadNote(GixObjectId annotatedObjectId, string? notesRef = null) =>
        ReadNoteCore(annotatedObjectId, notesRef is null ? [] : EncodeNotesRef(notesRef), "ReadNote")
        ?? throw new KeyNotFoundException($"No note exists for object '{annotatedObjectId}' in the selected notes reference.");

    public GitNote ReadNote(GixObjectId annotatedObjectId, byte[] notesRef) =>
        ReadNoteCore(annotatedObjectId, ValidateNotesRef(notesRef), "ReadNote")
        ?? throw new KeyNotFoundException($"No note exists for object '{annotatedObjectId}' in the selected notes reference.");

    public bool TryReadNote(GixObjectId annotatedObjectId, string? notesRef, [NotNullWhen(true)] out GitNote? note)
    {
        note = ReadNoteCore(annotatedObjectId, notesRef is null ? [] : EncodeNotesRef(notesRef), "TryReadNote");
        return note is not null;
    }

    public bool TryReadNote(GixObjectId annotatedObjectId, byte[] notesRef, [NotNullWhen(true)] out GitNote? note)
    {
        note = ReadNoteCore(annotatedObjectId, ValidateNotesRef(notesRef), "TryReadNote");
        return note is not null;
    }

    public bool TryReadNote(GixObjectId annotatedObjectId, [NotNullWhen(true)] out GitNote? note) =>
        TryReadNote(annotatedObjectId, (string?)null, out note);

    /// <summary>Materializes notes ordered by annotated object ID. Null selects the configured default ref.</summary>
    public IReadOnlyList<GitNoteEntry> EnumerateNotes(string? notesRef = null) =>
        EnumerateNotesCore(notesRef is null ? [] : EncodeNotesRef(notesRef));

    public IReadOnlyList<GitNoteEntry> EnumerateNotes(byte[] notesRef) =>
        EnumerateNotesCore(ValidateNotesRef(notesRef));

    /// <summary>Removes a note and retains the notes reference; returns false when no note exists.</summary>
    public bool RemoveNote(GixObjectId annotatedObjectId, string notesRef, GixSignature signature) =>
        RemoveNoteCore(annotatedObjectId, EncodeNotesRef(notesRef), signature);

    public bool RemoveNote(GixObjectId annotatedObjectId, byte[] notesRef, GixSignature signature) =>
        RemoveNoteCore(annotatedObjectId, ValidateNotesRef(notesRef), signature);

    public bool RemoveNote(GixObjectId annotatedObjectId, GixSignature signature) =>
        RemoveNoteCore(annotatedObjectId, [], signature);

    private GixObjectId WriteNoteCore(GixObjectId objectId, byte[] message, byte[] notesRef,
        GixSignature signature, bool overwrite)
    {
        ValidateNoteId(objectId);
        ArgumentNullException.ThrowIfNull(message);
        ValidateNoteSignature(signature);
        return Invoke("WriteNote", repo =>
        {
            using var nativeId = objectId.Value.Utf8();
            using var nativeRef = notesRef.Slice();
            using var nativeMessage = message.Slice();
            using var nativeName = signature.NameBytes.Slice();
            using var nativeEmail = signature.EmailBytes.Slice();
            using var id = repo.WriteNote(nativeId, nativeRef, nativeMessage,
                nativeName, nativeEmail, signature.When.ToUnixTimeSeconds(), SignatureOffsetSeconds(signature),
                nativeName, nativeEmail, signature.When.ToUnixTimeSeconds(), SignatureOffsetSeconds(signature), overwrite);
            return new GixObjectId(id.String);
        });
    }

    private GitNote? ReadNoteCore(GixObjectId objectId, byte[] notesRef, string operation)
    {
        ValidateNoteId(objectId);
        try
        {
            return Invoke(operation, repo =>
            {
                using var nativeId = objectId.Value.Utf8();
                using var nativeRef = notesRef.Slice();
                using var note = repo.ReadNote(nativeId, nativeRef);
                return ReadNoteRecord(note);
            });
        }
        catch (GixException exception) when (exception.Kind == GixErrorKind.NotFound)
        {
            return null;
        }
    }

    private IReadOnlyList<GitNoteEntry> EnumerateNotesCore(byte[] notesRef) =>
        Invoke("EnumerateNotes", repo =>
        {
            using var nativeRef = notesRef.Slice();
            using var entries = repo.EnumerateNotes(nativeRef);
            var notes = new GitNoteEntry[entries.Count];
            for (var index = 0; index < notes.Length; index++)
            {
                var entry = entries[index];
                notes[index] = new GitNoteEntry(new GixObjectId(entry.annotated_object_id.String), ReadNoteRecord(entry.note));
            }
            return notes;
        });

    private bool RemoveNoteCore(GixObjectId objectId, byte[] notesRef, GixSignature signature)
    {
        ValidateNoteId(objectId);
        ValidateNoteSignature(signature);
        return Invoke("RemoveNote", repo =>
        {
            using var nativeId = objectId.Value.Utf8();
            using var nativeRef = notesRef.Slice();
            using var nativeName = signature.NameBytes.Slice();
            using var nativeEmail = signature.EmailBytes.Slice();
            return repo.RemoveNote(nativeId, nativeRef,
                nativeName, nativeEmail, signature.When.ToUnixTimeSeconds(), SignatureOffsetSeconds(signature),
                nativeName, nativeEmail, signature.When.ToUnixTimeSeconds(), SignatureOffsetSeconds(signature));
        });
    }

    private static GitNote ReadNoteRecord(NoteRecord note) =>
        new(new GixObjectId(note.id.String), note.message.ToArray(),
            ReadSignature(note.author_name, note.author_email, note.author_time_seconds, note.author_time_offset_seconds),
            ReadSignature(note.committer_name, note.committer_email, note.committer_time_seconds, note.committer_time_offset_seconds),
            note.author_name.ToArray(), note.author_email.ToArray(), note.committer_name.ToArray(), note.committer_email.ToArray());

    private static byte[] EncodeNotesRef(string notesRef)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(notesRef);
        return EncodePath(notesRef);
    }

    private static byte[] ValidateNotesRef(byte[] notesRef)
    {
        ArgumentNullException.ThrowIfNull(notesRef);
        if (notesRef.Length == 0) throw new ArgumentException("An explicit notes reference cannot be empty.", nameof(notesRef));
        return notesRef;
    }

    private static void ValidateNoteId(GixObjectId objectId)
    {
        if (string.IsNullOrEmpty(objectId.Value))
            throw new ArgumentException("The annotated object ID must be initialized.", nameof(objectId));
    }

    private static void ValidateNoteSignature(GixSignature signature)
    {
        ArgumentNullException.ThrowIfNull(signature);
        ArgumentException.ThrowIfNullOrWhiteSpace(signature.Name);
        ArgumentException.ThrowIfNullOrWhiteSpace(signature.Email);
    }
}
