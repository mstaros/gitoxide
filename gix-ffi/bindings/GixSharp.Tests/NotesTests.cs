using System.Diagnostics;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class NotesTests
{
    private static readonly GixSignature Signature = new("Notes Author", "notes@example.com",
        DateTimeOffset.FromUnixTimeSeconds(1_700_000_000).ToOffset(TimeSpan.FromMinutes(330)));

    [Test]
    public async Task OwnedNotesKeepExactBytesSignaturesAndRecordValueSemanticsAfterDisposal()
    {
        foreach (var bare in new[] { false, true })
        {
            using var fixture = new Fixture(bare);
            var repository = fixture.Repository;
            var objectId = repository.WriteBlob("annotated"u8.ToArray());
            byte[] bytes = [0xff, 0, 13, 10, (byte)'x'];
            var noteId = repository.WriteNote(objectId, bytes, Signature);
            bytes[0] = 0;
            var note = repository.ReadNote(objectId);
            var entries = repository.EnumerateNotes();
            await Assert.That(note.Id).IsEqualTo(noteId);
            await Assert.That(note.Author).IsEqualTo(Signature);
            await Assert.That(note.Committer).IsEqualTo(Signature);
            await Assert.That(entries.Single()).IsEqualTo(new GitNoteEntry(objectId, note));
            await Assert.That(repository.ReadNote(objectId, "refs/notes/commits"u8.ToArray())).IsEqualTo(note);
            repository.Dispose();

            await Assert.That(note.MessageBytes.SequenceEqual(new byte[] { 0xff, 0, 13, 10, (byte)'x' })).IsTrue();
            var copy = note.MessageBytes;
            copy[0] = 0;
            await Assert.That(note.MessageBytes[0]).IsEqualTo((byte)0xff);
            await Assert.That(note.AuthorNameBytes.SequenceEqual(Encoding.UTF8.GetBytes(Signature.Name))).IsTrue();
            var renamed = note with { Message = "replacement", Author = Signature with { Name = "New author" } };
            await Assert.That(renamed.MessageBytes.SequenceEqual("replacement"u8.ToArray())).IsTrue();
            await Assert.That(renamed.AuthorNameBytes.SequenceEqual("New author"u8.ToArray())).IsTrue();
            await Assert.That(note.AuthorNameBytes.SequenceEqual("Notes Author"u8.ToArray())).IsTrue();
        }
    }

    [Test]
    public async Task DefaultAndCustomRefsSupportMissingOverwriteEnumerationAndDeletion()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var first = repository.WriteBlob("first"u8.ToArray());
        var second = repository.WriteBlob("second"u8.ToArray());
        const string reference = "refs/notes/review";
        await Assert.That(repository.TryReadNote(first, reference, out var missing)).IsFalse();
        await Assert.That(missing).IsNull();
        await Assert.That(() => repository.ReadNote(first, reference)).Throws<KeyNotFoundException>();
        await Assert.That(repository.EnumerateNotes(reference).Count).IsEqualTo(0);
        await Assert.That(repository.RemoveNote(first, reference, Signature)).IsFalse();

        repository.WriteNote(first, "review", reference, Signature);
        var before = fixture.Git("rev-parse", reference).Trim();
        var overwrite = CaptureGix(() => repository.WriteNote(first, "refused", reference, Signature));
        await Assert.That(overwrite.Kind).IsEqualTo(GixErrorKind.ReferenceConflict);
        await Assert.That(overwrite.Operation).IsEqualTo("WriteNote");
        await Assert.That(fixture.Git("rev-parse", reference).Trim()).IsEqualTo(before);
        repository.WriteNote(first, [], reference, Signature, overwrite: true);
        repository.WriteNote(second, "other", reference, Signature);
        await Assert.That(repository.TryReadNote(first, reference, out var empty)).IsTrue();
        await Assert.That(empty!.MessageBytes.Length).IsEqualTo(0);
        var entries = repository.EnumerateNotes(reference);
        await Assert.That(entries.Select(entry => entry.AnnotatedObjectId.Value)
            .SequenceEqual(entries.Select(entry => entry.AnnotatedObjectId.Value).Order(StringComparer.Ordinal))).IsTrue();
        await Assert.That(entries.Count).IsEqualTo(2);
        await Assert.That(repository.EnumerateNotes().Count).IsEqualTo(0);
        await Assert.That(repository.RemoveNote(first, Encoding.UTF8.GetBytes(reference), Signature)).IsTrue();
        await Assert.That(repository.RemoveNote(first, reference, Signature)).IsFalse();
        await Assert.That(repository.RemoveNote(second, reference, Signature)).IsTrue();
        await Assert.That(repository.TryGetReferenceTarget(reference, out _)).IsTrue();
        await Assert.That(repository.EnumerateNotes(reference).Count).IsEqualTo(0);
    }

    [Test]
    public async Task ConfiguredDefaultsAndUnicodeReferenceNamesMatchGit()
    {
        using var fixture = new Fixture(notesRef: "refs/notes/日本語");
        var repository = fixture.Repository;
        var objectId = repository.WriteBlob("annotated"u8.ToArray());
        var noteId = repository.WriteNote(objectId, "日本語 note\0exact", Signature);
        await Assert.That(repository.ReadNote(objectId, "refs/notes/日本語").Id).IsEqualTo(noteId);
        await Assert.That(repository.ReadNote(objectId, "refs/notes/日本語"u8.ToArray()).Message)
            .IsEqualTo("日本語 note\0exact");
        await Assert.That(fixture.Git("notes", "show", objectId.Value)).IsEqualTo("日本語 note\0exact");
        await Assert.That(repository.RemoveNote(objectId, Signature)).IsTrue();
        await Assert.That(repository.TryReadNote(objectId, out _)).IsFalse();
    }

    [Test]
    public async Task SymbolicWritesRetainAliasesAndLockOrCorruptionErrorsAreNotMissingNotes()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var objectId = repository.WriteBlob("annotated"u8.ToArray());
        var noteId = repository.WriteNote(objectId, "original", "refs/notes/direct", Signature);
        fixture.Git("symbolic-ref", "refs/notes/alias", "refs/notes/direct");
        repository.WriteNote(objectId, "alias update"u8.ToArray(), "refs/notes/alias"u8.ToArray(), Signature, overwrite: true);
        await Assert.That(fixture.Git("symbolic-ref", "refs/notes/alias").Trim()).IsEqualTo("refs/notes/direct");
        var lockPath = Path.Combine(repository.CommonDirectory, "refs", "notes", "direct.lock");
        File.WriteAllText(lockPath, "foreign lock");
        try
        {
            var locked = CaptureGix(() => repository.RemoveNote(objectId, "refs/notes/alias", Signature));
            await Assert.That(locked.Kind).IsEqualTo(GixErrorKind.ReferenceLocked);
            await Assert.That(locked.Operation).IsEqualTo("RemoveNote");
            await Assert.That(File.ReadAllText(lockPath)).IsEqualTo("foreign lock");
        }
        finally
        {
            File.Delete(lockPath);
        }
        noteId = repository.ReadNote(objectId, "refs/notes/direct").Id;
        File.Delete(Path.Combine(repository.CommonDirectory, "objects", noteId.Value[..2], noteId.Value[2..]));
        var corrupt = CaptureGix(() => repository.TryReadNote(objectId, "refs/notes/direct", out _));
        await Assert.That(corrupt.Kind).IsEqualTo(GixErrorKind.Other);
        await Assert.That(corrupt.Operation).IsEqualTo("TryReadNote");
    }

    [Test]
    public async Task InvalidArgumentsAndDisposedRepositoryFailAtTheManagedBoundary()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var objectId = repository.WriteBlob("annotated"u8.ToArray());
        await Assert.That(() => repository.ReadNote(default)).Throws<ArgumentException>();
        await Assert.That(() => repository.WriteNote(objectId, (byte[])null!, Signature)).Throws<ArgumentNullException>();
        await Assert.That(() => repository.WriteNote(objectId, "text", (GixSignature)null!)).Throws<ArgumentNullException>();
        await Assert.That(() => repository.EnumerateNotes(" ")).Throws<ArgumentException>();
        await Assert.That(() => repository.ReadNote(objectId, Array.Empty<byte>())).Throws<ArgumentException>();
        var invalid = CaptureGix(() => repository.ReadNote(objectId, "refs/notes/invalid ref"));
        await Assert.That(invalid.Kind).IsEqualTo(GixErrorKind.InvalidReference);
        repository.Dispose();
        await Assert.That(() => repository.ReadNote(objectId)).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.TryReadNote(objectId, out _)).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.EnumerateNotes()).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.WriteNote(objectId, "valid", Signature)).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.RemoveNote(objectId, Signature)).Throws<ObjectDisposedException>();
    }

    private static GixException CaptureGix(Action action)
    {
        try { action(); }
        catch (GixException exception) { return exception; }
        throw new InvalidOperationException("Expected a GixException.");
    }

    private sealed class Fixture : IDisposable
    {
        private readonly DirectoryInfo _root = Directory.CreateTempSubdirectory("gixsharp-notes-");
        public Fixture(bool bare = false, string? notesRef = null)
        {
            using (var repository = GixRepository.Init(Root, bare)) { }
            Git("config", "user.name", "Notes Fixture");
            Git("config", "user.email", "fixture@example.com");
            if (notesRef is not null) Git("config", "core.notesRef", notesRef);
            Repository = GixRepository.Open(Root);
        }
        public string Root => _root.FullName;
        public GixRepository Repository { get; }
        public string Git(params string[] arguments)
        {
            var start = new ProcessStartInfo("git")
            {
                WorkingDirectory = Root, RedirectStandardOutput = true, RedirectStandardError = true,
                UseShellExecute = false, CreateNoWindow = true, StandardOutputEncoding = Encoding.UTF8,
            };
            foreach (var argument in arguments) start.ArgumentList.Add(argument);
            using var process = Process.Start(start) ?? throw new InvalidOperationException("Could not start git.");
            var output = process.StandardOutput.ReadToEnd();
            var error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            if (process.ExitCode != 0) throw new InvalidOperationException($"git {string.Join(' ', arguments)}: {error}");
            return output;
        }
        public void Dispose()
        {
            Repository.Dispose();
            foreach (var path in Directory.EnumerateFiles(Root, "*", SearchOption.AllDirectories))
                File.SetAttributes(path, FileAttributes.Normal);
            _root.Delete(recursive: true);
        }
    }
}
