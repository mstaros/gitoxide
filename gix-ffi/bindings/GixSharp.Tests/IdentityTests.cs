using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class IdentityTests
{
    private static readonly DateTimeOffset When =
        DateTimeOffset.FromUnixTimeSeconds(1_700_000_000).ToOffset(TimeSpan.FromMinutes(330));

    [Test]
    public async Task SignaturesOwnTheirBytesAndKeepConstructorDeconstructionAndWithConsistent()
    {
        byte[] name = [(byte)'N', 0xff];
        byte[] email = [(byte)'e', 0xfe, (byte)'@', (byte)'x'];
        var signature = GixSignature.FromBytes(name, email, When);
        name[0] = 0;
        email[0] = 0;
        var nameCopy = signature.NameBytes;
        var emailCopy = signature.EmailBytes;
        nameCopy[0] = 0;
        emailCopy[0] = 0;
        await Assert.That(signature.NameBytes.SequenceEqual(new byte[] { (byte)'N', 0xff })).IsTrue();
        await Assert.That(signature.EmailBytes.SequenceEqual(new byte[] { (byte)'e', 0xfe, (byte)'@', (byte)'x' })).IsTrue();
        await Assert.That(signature).IsEqualTo(GixSignature.FromBytes(signature.NameBytes, signature.EmailBytes, When));
        var (displayName, displayEmail, when) = signature;
        await Assert.That(displayName).IsEqualTo(Encoding.UTF8.GetString(signature.NameBytes));
        await Assert.That(displayEmail).IsEqualTo(Encoding.UTF8.GetString(signature.EmailBytes));
        await Assert.That(when).IsEqualTo(When);
        var retimed = signature with { When = When.AddTicks(1234) };
        await Assert.That(retimed.NameBytes.SequenceEqual(signature.NameBytes)).IsTrue();
        await Assert.That(retimed.EmailBytes.SequenceEqual(signature.EmailBytes)).IsTrue();
        var renamed = signature with { Name = "日本語" };
        await Assert.That(renamed.NameBytes.SequenceEqual("日本語"u8.ToArray())).IsTrue();
        await Assert.That(renamed.EmailBytes.SequenceEqual(signature.EmailBytes)).IsTrue();
        var remailed = signature with { Email = "replacement@example.com" };
        await Assert.That(remailed.EmailBytes.SequenceEqual("replacement@example.com"u8.ToArray())).IsTrue();
        await Assert.That(remailed.NameBytes.SequenceEqual(signature.NameBytes)).IsTrue();
        var constructed = new GixSignature("Name", "email@example.com", When) { Name = "Edited" };
        await Assert.That(constructed.NameBytes.SequenceEqual("Edited"u8.ToArray())).IsTrue();
        await Assert.That(constructed).IsEqualTo(GixSignature.FromBytes("Edited"u8.ToArray(), "email@example.com"u8.ToArray(), When));
    }

    [Test]
    public async Task CommitNoteAndTagSignaturesRoundTripRawBytesAfterRepositoryDisposal()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var signature = GixSignature.FromBytes([(byte)'N', 0xff], [(byte)'e', 0xfe, (byte)'@', (byte)'x'], When);
        // An empty tree has no worktree or index dependency.
        var emptyTree = new GixObjectId("4b825dc642cb6eb9a060e54bf8d69288fbee4904");
        var tree = new GixObjectId(repository.WriteIndexTree());
        await Assert.That(tree).IsEqualTo(emptyTree);
        var commitId = repository.CreateCommitObject("raw identity", tree, [], signature, signature);
        var commit = repository.LookupCommit(commitId.Value);
        var blob = repository.WriteBlob("annotated"u8.ToArray());
        var tagId = repository.CreateAnnotatedTag("raw-identity", blob, signature, "tag");
        var tag = repository.ReadTag(tagId);
        var copiedTagId = repository.CreateAnnotatedTag("copied-identity", blob, tag.Tagger, "tag");
        var copiedTag = repository.ReadTag(copiedTagId);
        repository.WriteNote(blob, "note", signature);
        var note = repository.ReadNote(blob);
        await Assert.That(repository.RemoveNote(blob, signature)).IsTrue();
        repository.Dispose();
        await Assert.That(commit.Author).IsEqualTo(signature);
        await Assert.That(commit.Committer).IsEqualTo(signature);
        await Assert.That(note.Author).IsEqualTo(signature);
        await Assert.That(tag.Tagger).IsEqualTo(signature);
        await Assert.That(copiedTag.Tagger).IsEqualTo(signature);
        await Assert.That(tag.TaggerNameBytes!.SequenceEqual(signature.NameBytes)).IsTrue();
        var editedTag = tag with { Tagger = signature with { Email = "changed@example.com" } };
        await Assert.That(editedTag.TaggerNameBytes!.SequenceEqual(signature.NameBytes)).IsTrue();
        await Assert.That(editedTag.TaggerEmailBytes!.SequenceEqual("changed@example.com"u8.ToArray())).IsTrue();
        var constructedTag = new GixTag(tag.Id, tag.TargetId, tag.TargetType, tag.Name, tag.Message, signature);
        await Assert.That(constructedTag.TaggerNameBytes!.SequenceEqual(signature.NameBytes)).IsTrue();
        await Assert.That((tag with { Tagger = null }).TaggerNameBytes).IsNull();
        await Assert.That(note.AuthorNameBytes.SequenceEqual(signature.NameBytes)).IsTrue();
        var edited = note with { Author = signature with { Email = "changed@example.com" } };
        await Assert.That(edited.AuthorNameBytes.SequenceEqual(signature.NameBytes)).IsTrue();
        await Assert.That(edited.AuthorEmailBytes.SequenceEqual("changed@example.com"u8.ToArray())).IsTrue();
        var constructed = new GitNote(note.Id, note.Message, signature, signature);
        await Assert.That(constructed.AuthorNameBytes.SequenceEqual(signature.NameBytes)).IsTrue();
    }

    [Test]
    public async Task ConfiguredRoleReadsAndFallbackCallsReturnOwnedSignatures()
    {
        using var fixture = new Fixture(configured: true);
        var repository = fixture.Repository;
        var author = repository.GetAuthor();
        var committer = repository.GetCommitter();
        await Assert.That(author).IsNotNull();
        await Assert.That(committer).IsNotNull();
        await Assert.That(author!.Name).IsEqualTo("Configured Author");
        await Assert.That(author.Email).IsEqualTo("user@example.com");
        await Assert.That(committer!.Name).IsEqualTo("Configured User");
        await Assert.That(committer.Email).IsEqualTo("committer@example.com");
        var before = File.ReadAllBytes(fixture.Config);
        await Assert.That(repository.GetCommitterOrSetFallback("Unused", "unused@example.com")).IsEqualTo(committer);
        await Assert.That(repository.GetCommitterOrSetFallback("Unused"u8.ToArray(), "unused@example.com"u8.ToArray())).IsEqualTo(committer);
        await Assert.That(repository.GetCommitterOrSetGenericFallback()).IsEqualTo(committer);
        await Assert.That(File.ReadAllBytes(fixture.Config).SequenceEqual(before)).IsTrue();
        repository.Dispose();
        await Assert.That(author.NameBytes.SequenceEqual("Configured Author"u8.ToArray())).IsTrue();
        await Assert.That(committer.EmailBytes.SequenceEqual("committer@example.com"u8.ToArray())).IsTrue();
    }

    [Test]
    public async Task MailmapResolutionRetainsExactBytesAndTimestampAfterDisposal()
    {
        using var fixture = new Fixture();
        File.WriteAllBytes(Path.Combine(fixture.Root, ".mailmap"),
            "invalid entry\nMapped-"u8.ToArray().Concat(new byte[] { 0xff })
                .Concat(" <mapped-"u8.ToArray()).Concat(new byte[] { 0xfe })
                .Concat("@example.com> Original <old@example.com>\n"u8.ToArray()).ToArray());
        fixture.Repository.SetConfigString("mailmap.blob", "missing-revision");
        var input = new GixSignature("Original", "old@example.com", When.AddTicks(1234));
        var mapped = fixture.Repository.ResolveMailmap(input);
        await Assert.That(fixture.Repository.TryResolveMailmap(input, out var tried)).IsTrue();
        await Assert.That(tried).IsEqualTo(mapped);
        var unchanged = GixSignature.FromBytes([(byte)'N', 0xfd], "unknown@example.com"u8.ToArray(), input.When);
        await Assert.That(fixture.Repository.TryResolveMailmap(unchanged, out var missing)).IsFalse();
        await Assert.That(missing).IsNull();
        await Assert.That(fixture.Repository.ResolveMailmap(unchanged)).IsEqualTo(unchanged);
        fixture.Repository.Dispose();
        await Assert.That(mapped.NameBytes.SequenceEqual("Mapped-"u8.ToArray().Concat(new byte[] { 0xff }))).IsTrue();
        await Assert.That(mapped.EmailBytes.SequenceEqual("mapped-"u8.ToArray().Concat(new byte[] { 0xfe }).Concat("@example.com"u8.ToArray()))).IsTrue();
        await Assert.That(mapped.When.EqualsExact(input.When)).IsTrue();
    }

    [Test]
    public async Task IdentityArgumentsAndDisposedHandlesFailAtTheManagedBoundary()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var signature = new GixSignature("Name", "email@example.com", When);
        await Assert.That(() => GixSignature.FromBytes(null!, [], When)).Throws<ArgumentNullException>();
        await Assert.That(() => GixSignature.FromBytes([], null!, When)).Throws<ArgumentNullException>();
        await Assert.That(() => repository.GetCommitterOrSetFallback((string)null!, "email")).Throws<ArgumentNullException>();
        await Assert.That(() => repository.GetCommitterOrSetFallback([], (byte[])null!)).Throws<ArgumentNullException>();
        await Assert.That(() => repository.GetCommitterOrSetFallback("\ud800", "email")).Throws<EncoderFallbackException>();
        await Assert.That(() => repository.ResolveMailmap(null!)).Throws<ArgumentNullException>();
        await Assert.That(() => repository.TryResolveMailmap(null!, out _)).Throws<ArgumentNullException>();
        repository.Dispose();
        await Assert.That(() => repository.GetAuthor()).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.GetCommitter()).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.GetCommitterOrSetFallback("Name", "email")).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.GetCommitterOrSetGenericFallback()).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.ResolveMailmap(signature)).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.TryResolveMailmap(signature, out _)).Throws<ObjectDisposedException>();
    }

    private sealed class Fixture : IDisposable
    {
        private readonly DirectoryInfo _root = Directory.CreateTempSubdirectory("gixsharp-identity-");
        public Fixture(bool configured = false)
        {
            using (var repository = GixRepository.Init(Root)) { }
            if (configured)
                File.AppendAllText(Config,
                    "\n[user]\n name = Configured User\n email = user@example.com\n[author]\n name = Configured Author\n[committer]\n email = committer@example.com\n",
                    new UTF8Encoding(false));
            Repository = GixRepository.Open(Root);
        }
        public string Root => _root.FullName;
        public string Config => Path.Combine(Root, ".git", "config");
        public GixRepository Repository { get; }
        public void Dispose()
        {
            Repository.Dispose();
            foreach (var path in Directory.EnumerateFiles(Root, "*", SearchOption.AllDirectories))
                File.SetAttributes(path, FileAttributes.Normal);
            _root.Delete(recursive: true);
        }
    }
}
