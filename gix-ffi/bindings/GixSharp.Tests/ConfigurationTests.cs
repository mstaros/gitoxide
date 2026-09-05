using System.Diagnostics;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class ConfigurationTests
{
    [Test]
    public async Task ReadsDistinguishMissingEmptyAndImplicitAndRefreshIncludes()
    {
        using var fixture = new Fixture();
        fixture.Append("\n[wrapper]\n empty =\n implicit\n repeated = first\n repeated = last\n");
        await Assert.That(fixture.Repository.GetConfigString("wrapper.missing")).IsNull();
        await Assert.That(fixture.Repository.TryGetConfigString("wrapper.missing", out var missing)).IsFalse();
        await Assert.That(missing).IsEqualTo(string.Empty);
        foreach (var name in new[] { "wrapper.empty", "wrapper.implicit" })
        {
            await Assert.That(fixture.Repository.TryGetConfigString(name, out var found)).IsTrue();
            await Assert.That(found).IsEqualTo(string.Empty);
        }
        await Assert.That(fixture.Repository.GetConfigString("WRAPPER.REPEATED")).IsEqualTo("last");
        fixture.Write(".git/included", "[wrapper]\n included = 日本語\n");
        fixture.Append("[include]\n path = included\n");
        await Assert.That(fixture.Repository.GetConfigString("wrapper.included")).IsEqualTo("日本語");
        fixture.Write(".git/included", "[wrapper]\n included = changed\n");
        await Assert.That(fixture.Repository.GetConfigString("wrapper.included")).IsEqualTo("changed");
    }

    [Test]
    public async Task WritesRoundTripStrictStringsPreserveUnrelatedBytesAndRefreshCommitIdentity()
    {
        foreach (var bare in new[] { false, true })
        {
            using var fixture = new Fixture(bare);
            byte[] suffix = [(byte)'\n', (byte)'#', (byte)' ', 0xff, 13, 10];
            File.WriteAllBytes(fixture.Config, File.ReadAllBytes(fixture.Config).Concat(suffix).ToArray());
            const string value = " 日本語\nline\t\"quote\"\\slash;#comment ";
            fixture.Repository.SetConfigString("wrapper.Case.value", value);
            await Assert.That(fixture.Repository.GetConfigString("wrapper.Case.value")).IsEqualTo(value);
            await Assert.That(fixture.Repository.GetConfigString("wrapper.case.value")).IsNull();
            var written = File.ReadAllBytes(fixture.Config);
            await Assert.That(written.AsSpan().IndexOf(suffix) >= 0).IsTrue();
            fixture.Repository.SetConfigString("wrapper.Case.value", value);
            await Assert.That(File.ReadAllBytes(fixture.Config).SequenceEqual(written)).IsTrue();
            await Assert.That(fixture.Repository.DeleteConfigValue("wrapper.Case.value")).IsTrue();
            await Assert.That(fixture.Repository.DeleteConfigValue("wrapper.Case.value")).IsFalse();
            fixture.Repository.SetConfigString("user.name", "Configured Managed");
            fixture.Repository.SetConfigString("user.email", "configured@example.com");
            var id = fixture.Repository.CreateCommit("configured", allowEmpty: true);
            var info = fixture.Repository.CommitInfo(id.Value);
            await Assert.That(Encoding.UTF8.GetString(info.AuthorName)).IsEqualTo("Configured Managed");
            await Assert.That(Encoding.UTF8.GetString(info.AuthorEmail)).IsEqualTo("configured@example.com");
        }
    }

    [Test]
    public async Task AmbiguousIncludedAndInvalidEncodedValuesFailWithoutChangingConfig()
    {
        using var fixture = new Fixture();
        fixture.Append("\n[wrapper]\n repeated = one\n repeated = two\n");
        fixture.Write(".git/included", "[wrapper]\n included = original\n");
        fixture.Append("[include]\n path = included\n");
        var original = File.ReadAllBytes(fixture.Config);
        foreach (var key in new[] { "wrapper.repeated", "wrapper.included" })
        {
            var set = CaptureGix(() => fixture.Repository.SetConfigString(key, "replacement"));
            await Assert.That(set.Kind).IsEqualTo(GixErrorKind.Config);
            await Assert.That(set.Operation).IsEqualTo("SetConfigString");
            var delete = CaptureGix(() => fixture.Repository.DeleteConfigValue(key));
            await Assert.That(delete.Kind).IsEqualTo(GixErrorKind.Config);
            await Assert.That(delete.Operation).IsEqualTo("DeleteConfigValue");
        }
        await Assert.That(File.ReadAllBytes(fixture.Config).SequenceEqual(original)).IsTrue();
        byte[] rawValue = [.. "\n[wrapper]\n invalid = "u8.ToArray(), 0xff, (byte)'\n'];
        File.WriteAllBytes(fixture.Config, original.Concat(rawValue).ToArray());
        await Assert.That(() => fixture.Repository.GetConfigString("wrapper.invalid")).Throws<DecoderFallbackException>();
        await Assert.That(() => fixture.Repository.TryGetConfigString("wrapper.invalid", out _)).Throws<DecoderFallbackException>();
        fixture.Repository.SetConfigString("wrapper.valid", "usable after decode failure");
        await Assert.That(fixture.Repository.GetConfigString("wrapper.valid")).IsEqualTo("usable after decode failure");
    }

    [Test]
    public async Task LinkedWorktreesWriteCommonConfigWhilePrivateOverridesRemainAuthoritative()
    {
        using var fixture = new Fixture();
        fixture.Git("-c", "user.name=Configuration Tests", "-c", "user.email=config@example.com",
            "commit", "--allow-empty", "-qm", "fixture");
        var linkedPath = Path.Combine(fixture.Parent, "linked");
        fixture.Git("worktree", "add", "--detach", linkedPath, "HEAD");
        fixture.Git("config", "extensions.worktreeConfig", "true");
        fixture.Repository.SetConfigString("wrapper.value", "common");
        using var linked = GixRepository.Open(linkedPath);
        fixture.Write(".git/private-include", "[wrapper]\n conditional = selected\n");
        var privateGitDir = linked.RepositoryPath.Replace('\\', '/');
        fixture.Append($"[includeIf \"gitdir:{privateGitDir}\"]\n path = private-include\n");
        await Assert.That(linked.GetConfigString("wrapper.conditional")).IsEqualTo("selected");
        await Assert.That(fixture.Repository.GetConfigString("wrapper.conditional")).IsNull();
        var privateConfig = Path.Combine(linked.RepositoryPath, "config.worktree");
        File.WriteAllText(privateConfig, "[wrapper]\n value = private\n", new UTF8Encoding(false));
        await Assert.That(linked.GetConfigString("wrapper.value")).IsEqualTo("private");
        linked.SetConfigString("wrapper.value", "updated common");
        await Assert.That(fixture.Repository.GetConfigString("wrapper.value")).IsEqualTo("updated common");
        await Assert.That(linked.GetConfigString("wrapper.value")).IsEqualTo("private");
        await Assert.That(linked.DeleteConfigValue("wrapper.value")).IsTrue();
        await Assert.That(linked.DeleteConfigValue("wrapper.value")).IsFalse();
        await Assert.That(linked.GetConfigString("wrapper.value")).IsEqualTo("private");
        await Assert.That(File.ReadAllText(privateConfig)).IsEqualTo("[wrapper]\n value = private\n");
        await Assert.That(File.Exists(Path.Combine(linked.RepositoryPath, "config"))).IsFalse();
    }

    [Test]
    public async Task InvalidTypedSettingsCanBeReadRepairedAndDeletedOnTheSameHandle()
    {
        using var fixture = new Fixture();
        fixture.Repository.SetConfigString("core.repositoryFormatVersion", "not-an-integer");
        await Assert.That(fixture.Repository.GetConfigString("core.repositoryFormatVersion")).IsEqualTo("not-an-integer");
        await Assert.That(fixture.Repository.TryGetConfigString("core.repositoryFormatVersion", out var invalid)).IsTrue();
        await Assert.That(invalid).IsEqualTo("not-an-integer");
        await Assert.That(() => GixRepository.Open(fixture.Root)).Throws<GixException>();
        fixture.Repository.SetConfigString("wrapper.repair", "still writable");
        await Assert.That(fixture.Repository.GetConfigString("wrapper.repair")).IsEqualTo("still writable");
        fixture.Repository.SetConfigString("core.repositoryFormatVersion", "0");
        using (var repaired = GixRepository.Open(fixture.Root))
            await Assert.That(repaired.GetConfigString("core.repositoryFormatVersion")).IsEqualTo("0");
        fixture.Repository.SetConfigString("core.repositoryFormatVersion", "not-an-integer");
        await Assert.That(fixture.Repository.DeleteConfigValue("core.repositoryFormatVersion")).IsTrue();
        await Assert.That(fixture.Repository.GetConfigString("core.repositoryFormatVersion")).IsNull();
        using var deleted = GixRepository.Open(fixture.Root);
        await Assert.That(deleted.GetConfigString("core.repositoryFormatVersion")).IsNull();
    }

    [Test]
    public async Task LocksArgumentsAndDisposalKeepTheirManagedContracts()
    {
        using var fixture = new Fixture();
        fixture.Repository.SetConfigString("wrapper.value", "original");
        var original = File.ReadAllBytes(fixture.Config);
        var lockPath = fixture.Config + ".lock";
        File.WriteAllText(lockPath, "foreign owner");
        try
        {
            var failure = CaptureGix(() => fixture.Repository.SetConfigString("wrapper.value", "replacement"));
            await Assert.That(failure.Kind).IsEqualTo(GixErrorKind.Io);
            await Assert.That(failure.Operation).IsEqualTo("SetConfigString");
            await Assert.That(File.ReadAllText(lockPath)).IsEqualTo("foreign owner");
            await Assert.That(File.ReadAllBytes(fixture.Config).SequenceEqual(original)).IsTrue();
            await Assert.That(fixture.Repository.DeleteConfigValue("wrapper.missing")).IsFalse();
        }
        finally { File.Delete(lockPath); }
        await Assert.That(() => fixture.Repository.GetConfigString(null!)).Throws<ArgumentNullException>();
        await Assert.That(() => fixture.Repository.SetConfigString("wrapper.value", null!)).Throws<ArgumentNullException>();
        await Assert.That(() => fixture.Repository.SetConfigString("wrapper.value", "nul\0tail")).Throws<ArgumentException>();
        await Assert.That(() => fixture.Repository.SetConfigString("wrapper.value", "\ud800")).Throws<ArgumentException>();
        await Assert.That(() => fixture.Repository.TryGetConfigString(" ", out _)).Throws<ArgumentException>();
        var invalid = CaptureGix(() => fixture.Repository.GetConfigString("without-dot"));
        await Assert.That(invalid.Kind).IsEqualTo(GixErrorKind.Config);
        await Assert.That(invalid.Operation).IsEqualTo("GetConfigString");
        fixture.Repository.Dispose();
        await Assert.That(() => fixture.Repository.GetConfigString("wrapper.value")).Throws<ObjectDisposedException>();
        await Assert.That(() => fixture.Repository.TryGetConfigString("wrapper.value", out _)).Throws<ObjectDisposedException>();
        await Assert.That(() => fixture.Repository.SetConfigString("wrapper.value", "x")).Throws<ObjectDisposedException>();
        await Assert.That(() => fixture.Repository.DeleteConfigValue("wrapper.value")).Throws<ObjectDisposedException>();
    }

    private static GixException CaptureGix(Action action)
    {
        try { action(); }
        catch (GixException exception) { return exception; }
        throw new InvalidOperationException("Expected a GixException.");
    }

    private sealed class Fixture : IDisposable
    {
        private readonly DirectoryInfo _parent = Directory.CreateTempSubdirectory("gixsharp-config-");
        public Fixture(bool bare = false)
        {
            Root = Path.Combine(_parent.FullName, "repository");
            Repository = GixRepository.Init(Root, bare);
            Config = Path.Combine(Repository.CommonDirectory, "config");
        }
        public string Parent => _parent.FullName;
        public string Root { get; }
        public string Config { get; }
        public GixRepository Repository { get; }
        public void Append(string contents) => File.AppendAllText(Config, contents, new UTF8Encoding(false));
        public void Write(string path, string contents)
        {
            var fullPath = Path.Combine(Root, path.Replace('/', Path.DirectorySeparatorChar));
            Directory.CreateDirectory(Path.GetDirectoryName(fullPath)!);
            File.WriteAllText(fullPath, contents, new UTF8Encoding(false));
        }
        public void Git(params string[] arguments)
        {
            var start = new ProcessStartInfo("git")
            {
                WorkingDirectory = Root, RedirectStandardOutput = true, RedirectStandardError = true,
                UseShellExecute = false, CreateNoWindow = true,
            };
            foreach (var argument in arguments) start.ArgumentList.Add(argument);
            using var process = Process.Start(start) ?? throw new InvalidOperationException("Git did not start.");
            var output = process.StandardOutput.ReadToEndAsync();
            var error = process.StandardError.ReadToEndAsync();
            if (!process.WaitForExit(30000))
                throw new TimeoutException("Git fixture command timed out.");
            if (process.ExitCode != 0)
                throw new InvalidOperationException($"git {string.Join(' ', arguments)}: {error.GetAwaiter().GetResult()}");
            _ = output.GetAwaiter().GetResult();
        }
        public void Dispose()
        {
            Repository.Dispose();
            foreach (var path in Directory.EnumerateFiles(_parent.FullName, "*", SearchOption.AllDirectories))
                File.SetAttributes(path, FileAttributes.Normal);
            _parent.Delete(recursive: true);
        }
    }
}
