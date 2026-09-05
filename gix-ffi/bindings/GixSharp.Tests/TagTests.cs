using System.Diagnostics;
using System.Text;

namespace GixSharp.Tests;

public sealed class TagTests
{
    private static readonly GixSignature Tagger =
        new("Tagger", "tagger@example.com", DateTimeOffset.FromUnixTimeSeconds(1_700_000_000).ToOffset(TimeSpan.FromMinutes(330)));

    [Test]
    public async Task RawTagSnapshotsOwnEveryByteAndRecordEditsStayConsistent()
    {
        foreach (var bare in new[] { false, true })
        {
            using var fixture = new Fixture(bare);
            var repository = fixture.Repository;
            var target = repository.WriteBlob("target"u8.ToArray());
            byte[] body = [0x72, 0xff, 0, 13, 10];
            byte[] taggerName = [0x41, 0xfe];
            byte[] taggerEmail = [0x65, 0xfd, 0x40, 0x78];
            var tagId = repository.CreateAnnotatedTag("日本語/release"u8.ToArray(), target,
                taggerName, taggerEmail, Tagger.When, body);
            var tag = repository.ReadTag(tagId);
            var same = repository.ReadTag(tagId);
            await Assert.That(tag).IsEqualTo(same);
            await Assert.That(tag.TargetId).IsEqualTo(target);
            await Assert.That(tag.TargetType).IsEqualTo(GixObjectType.Blob);
            await Assert.That(tag.MessageBytes.SequenceEqual(body)).IsTrue();
            await Assert.That(tag.TaggerNameBytes!.SequenceEqual(taggerName)).IsTrue();
            await Assert.That(tag.TaggerEmailBytes!.SequenceEqual(taggerEmail)).IsTrue();
            await Assert.That(tag.Tagger!.When).IsEqualTo(Tagger.When);
            var copy = tag.MessageBytes;
            copy[0] = 0;
            body[0] = 0;
            taggerName[0] = 0;
            await Assert.That(tag.MessageBytes[0]).IsEqualTo((byte)0x72);
            await Assert.That(tag.TaggerNameBytes![0]).IsEqualTo((byte)0x41);
            var edited = tag with { Name = "new", Message = "new message", Tagger = Tagger };
            await Assert.That(edited.NameBytes.SequenceEqual("new"u8.ToArray())).IsTrue();
            await Assert.That(edited.MessageBytes.SequenceEqual("new message"u8.ToArray())).IsTrue();
            await Assert.That(edited.TaggerNameBytes!.SequenceEqual("Tagger"u8.ToArray())).IsTrue();
            var withoutTagger = tag with { Tagger = null };
            await Assert.That(withoutTagger.TaggerNameBytes).IsNull();
            await Assert.That(withoutTagger.TaggerEmailBytes).IsNull();
            repository.Dispose();
            await Assert.That(tag.Name).IsEqualTo("日本語/release");
            await Assert.That(tag.MessageBytes[1]).IsEqualTo((byte)0xff);
            await Assert.That(tag.TaggerEmailBytes![1]).IsEqualTo((byte)0xfd);
        }
    }

    [Test]
    public async Task TextCreationMatchesGitAndAbsentTaggersPreserveEmptyMessages()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var target = repository.WriteBlob("target"u8.ToArray());
        const string message = "日本語\r\nno final newline";
        var tagId = repository.CreateAnnotatedTag("release", target, Tagger, message);
        fixture.Git(Encoding.UTF8.GetBytes(message), "-c", "user.name=Tagger", "-c", "user.email=tagger@example.com",
            "tag", "--force", "--annotate", "--cleanup=verbatim", "--file=-", "release", target.Value);
        await Assert.That(fixture.GitText("rev-parse", "refs/tags/release").Trim()).IsEqualTo(tagId.Value);
        var signed = repository.ReadTag(tagId);
        await Assert.That(signed.Message).IsEqualTo(message);
        await Assert.That(signed.Tagger).IsEqualTo(Tagger);
        for (var index = 0; index < 3; index++)
        {
            var body = new string('\n', index);
            var id = repository.CreateAnnotatedTag($"empty-{index}", target, null, body);
            var tag = repository.ReadTag(id);
            await Assert.That(tag.Tagger).IsNull();
            await Assert.That(tag.TaggerNameBytes).IsNull();
            await Assert.That(tag.SignatureBytes).IsNull();
            await Assert.That(tag.Message).IsEqualTo(body);
        }
    }

    [Test]
    public async Task SignedBodiesKeepTheOriginalMessageAndExposeAnIndependentSignature()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var target = repository.WriteBlob("target"u8.ToArray());
        var signature = "-----BEGIN SSH SIGNATURE-----\nraw\n-----END SSH SIGNATURE-----\n"u8.ToArray();
        byte[] body = [0x61, 0xff, 10, 10, .. signature];
        var tagId = repository.CreateAnnotatedTag("signed"u8.ToArray(), target, null, body);
        var tag = repository.ReadTag(tagId);
        await Assert.That(tag.MessageBytes.SequenceEqual(body)).IsTrue();
        await Assert.That(tag.SignatureBytes!.SequenceEqual(signature)).IsTrue();
        var signatureCopy = tag.SignatureBytes!;
        signatureCopy[0] = 0;
        await Assert.That(tag.SignatureBytes![0]).IsEqualTo((byte)'-');
        var edited = tag with { Message = "replacement" };
        await Assert.That(edited.SignatureBytes).IsNull();
        await Assert.That(edited.MessageBytes.SequenceEqual("replacement"u8.ToArray())).IsTrue();
        repository.Dispose();
        await Assert.That(tag.SignatureBytes!.SequenceEqual(signature)).IsTrue();
        await Assert.That(tag.MessageBytes.SequenceEqual(body)).IsTrue();
    }

    [Test]
    public async Task NestedPeelingAndLightweightReferencesMatchGit()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var target = repository.WriteBlob("target"u8.ToArray());
        var tagId = repository.CreateAnnotatedTag("inner", target, Tagger, "inner");
        var outer = repository.CreateAnnotatedTag("outer", tagId, null, "outer");
        var tag = repository.ReadTag(outer);
        await Assert.That(tag.TargetId).IsEqualTo(tagId);
        await Assert.That(tag.TargetType).IsEqualTo(GixObjectType.Tag);
        await Assert.That(repository.PeelTags(outer)).IsEqualTo(target);
        await Assert.That(repository.PeelTags(target)).IsEqualTo(target);
        await Assert.That(fixture.GitText("rev-parse", "refs/tags/outer^{}").Trim()).IsEqualTo(target.Value);
        repository.CreateTagReference("lightweight", outer);
        await Assert.That(fixture.GitText("rev-parse", "refs/tags/lightweight").Trim()).IsEqualTo(outer.Value);
        var conflict = CaptureGix(() => repository.CreateTagReference("lightweight", outer));
        await Assert.That(conflict.Kind).IsEqualTo(GixErrorKind.ReferenceConflict);
        await Assert.That(conflict.Operation).IsEqualTo("CreateTagReference");
        var tagConflict = CaptureGix(() => repository.CreateAnnotatedTag("inner", target, Tagger, "inner"));
        await Assert.That(tagConflict.Kind).IsEqualTo(GixErrorKind.ReferenceConflict);
        repository.CreateTagReference("lightweight"u8.ToArray(), target, force: true);
        await Assert.That(fixture.GitText("rev-parse", "refs/tags/lightweight").Trim()).IsEqualTo(target.Value);
    }

    [Test]
    public async Task ForeignLocksAndWrongKindOrMissingObjectsAreDistinguishable()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var target = repository.WriteBlob("target"u8.ToArray());
        repository.CreateTagReference("locked", target);
        var lockPath = Path.Combine(repository.CommonDirectory, "refs", "tags", "locked.lock");
        File.WriteAllText(lockPath, "foreign lock");
        try
        {
            var error = CaptureGix(() => repository.CreateAnnotatedTag("locked", target, Tagger, "new", force: true));
            await Assert.That(error.Kind).IsEqualTo(GixErrorKind.ReferenceLocked);
            await Assert.That(error.Operation).IsEqualTo("CreateAnnotatedTag");
            await Assert.That(File.ReadAllText(lockPath)).IsEqualTo("foreign lock");
            await Assert.That(fixture.GitText("rev-parse", "refs/tags/locked").Trim()).IsEqualTo(target.Value);
        }
        finally { File.Delete(lockPath); }
        var wrong = CaptureGix(() => repository.ReadTag(target));
        await Assert.That(wrong.Kind).IsEqualTo(GixErrorKind.Other);
        await Assert.That(wrong.Operation).IsEqualTo("ReadTag");
        var missing = CaptureGix(() => repository.ReadTag(new GixObjectId(new string('1', 40))));
        await Assert.That(missing.Kind).IsEqualTo(GixErrorKind.NotFound);
        var invalidName = CaptureGix(() => repository.CreateTagReference("invalid name", target));
        await Assert.That(invalidName.Kind).IsEqualTo(GixErrorKind.InvalidReference);
    }

    [Test]
    public async Task ManagedValidationAndDisposalApplyToEveryTagOperation()
    {
        using var fixture = new Fixture();
        var repository = fixture.Repository;
        var target = repository.WriteBlob("target"u8.ToArray());
        await Assert.That(() => repository.ReadTag(default)).Throws<ArgumentException>();
        await Assert.That(() => repository.PeelTags(default)).Throws<ArgumentException>();
        await Assert.That(() => repository.CreateTagReference(Array.Empty<byte>(), target)).Throws<ArgumentException>();
        await Assert.That(() => repository.CreateAnnotatedTag("valid", target, null, (string)null!)).Throws<ArgumentNullException>();
        await Assert.That(() => repository.CreateAnnotatedTag((byte[])null!, target, null, [])).Throws<ArgumentNullException>();
        var invalidIdentity = CaptureGix(() => repository.CreateAnnotatedTag("identity"u8.ToArray(), target,
            "invalid\nname"u8.ToArray(), "email"u8.ToArray(), Tagger.When, []));
        await Assert.That(invalidIdentity.Kind).IsEqualTo(GixErrorKind.Other);
        if (OperatingSystem.IsWindows())
        {
            var invalidPath = CaptureGix(() => repository.CreateAnnotatedTag(new byte[] { 0x72, 0xff },
                target, Tagger, "message"u8.ToArray()));
            await Assert.That(invalidPath.Kind).IsEqualTo(GixErrorKind.InvalidPath);
        }
        repository.Dispose();
        await Assert.That(() => repository.ReadTag(target)).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.PeelTags(target)).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.CreateTagReference("valid", target)).Throws<ObjectDisposedException>();
        await Assert.That(() => repository.CreateAnnotatedTag("valid", target, Tagger, "message")).Throws<ObjectDisposedException>();
    }

    private static GixException CaptureGix(Action action)
    {
        try { action(); }
        catch (GixException exception) { return exception; }
        throw new InvalidOperationException("Expected a GixException.");
    }

    private sealed class Fixture : IDisposable
    {
        private readonly DirectoryInfo _root = Directory.CreateTempSubdirectory("gixsharp-tags-");
        public Fixture(bool bare = false)
        {
            using (var repository = GixRepository.Init(Root, bare)) { }
            GitText("config", "user.name", "Tag Fixture");
            GitText("config", "user.email", "fixture@example.com");
            Repository = GixRepository.Open(Root);
        }
        public string Root => _root.FullName;
        public GixRepository Repository { get; }
        public string GitText(params string[] arguments) => Encoding.UTF8.GetString(Git(null, arguments));
        public byte[] Git(byte[]? input, params string[] arguments)
        {
            var start = new ProcessStartInfo("git")
            {
                WorkingDirectory = Root, RedirectStandardOutput = true, RedirectStandardError = true,
                RedirectStandardInput = true, UseShellExecute = false, CreateNoWindow = true,
            };
            start.Environment["GIT_COMMITTER_DATE"] = "1700000000 +0530";
            foreach (var argument in arguments) start.ArgumentList.Add(argument);
            using var process = Process.Start(start) ?? throw new InvalidOperationException("Could not start git.");
            if (input is not null) process.StandardInput.BaseStream.Write(input);
            process.StandardInput.Close();
            using var output = new MemoryStream();
            process.StandardOutput.BaseStream.CopyTo(output);
            var error = process.StandardError.ReadToEnd();
            process.WaitForExit();
            if (process.ExitCode != 0) throw new InvalidOperationException($"git {string.Join(' ', arguments)}: {error}");
            return output.ToArray();
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
