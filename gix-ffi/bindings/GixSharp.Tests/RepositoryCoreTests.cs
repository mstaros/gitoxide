using System.IO.Compression;
using System.Runtime.CompilerServices;
using System.Security.Cryptography;
using System.Text;
using GixSharp;

namespace GixSharp.Tests;

public sealed class RepositoryCoreTests
{
    [Test]
    public async Task Open_ExposesRepositoryLocationMetadata()
    {
        var root = RepositoryRoot();
        using var repository = GixRepository.Open(root);

        await Assert.That(Canonical(repository.RepositoryPath))
            .IsEqualTo(DecodePath(repository.GitDir()));
        await Assert.That(Directory.Exists(repository.RepositoryPath)).IsTrue();
        await Assert.That(repository.WorkingDirectory).IsNotNull();
        await Assert.That(Canonical(repository.WorkingDirectory!))
            .IsEqualTo(Canonical(root));
        await Assert.That(Directory.Exists(repository.CommonDirectory)).IsTrue();
        await Assert.That(repository.IsBare).IsFalse();

        var expectedLinkedWorktree = File.Exists(Path.Combine(root, ".git"));
        await Assert.That(repository.IsWorktree).IsEqualTo(expectedLinkedWorktree);

        if (expectedLinkedWorktree)
        {
            await Assert.That(Canonical(repository.CommonDirectory))
                .IsNotEqualTo(Canonical(repository.RepositoryPath));
        }
        else
        {
            await Assert.That(Canonical(repository.CommonDirectory))
                .IsEqualTo(Canonical(repository.RepositoryPath));
        }
    }

    [Test]
    public async Task Init_CreatesNormalAndBareRepositories()
    {
        var parent = Directory.CreateTempSubdirectory();

        try
        {
            var worktreePath = Path.Combine(parent.FullName, "worktree");
            using (var worktree = GixRepository.Init(worktreePath))
            {
                var head = worktree.Head();

                await Assert.That(worktree.IsBare).IsFalse();
                await Assert.That(worktree.IsWorktree).IsFalse();
                await Assert.That(Canonical(worktree.WorkingDirectory!))
                    .IsEqualTo(Canonical(worktreePath));
                await Assert.That(Directory.Exists(worktree.RepositoryPath)).IsTrue();
                await Assert.That(head.IsUnborn).IsTrue();
                await Assert.That(head.Target).IsEmpty();
                await Assert.That(Encoding.UTF8.GetString(head.Referent))
                    .StartsWith("refs/heads/");
            }

            var barePath = Path.Combine(parent.FullName, "bare.git");
            using (var bare = GixRepository.Init(barePath, bare: true))
            {
                var head = bare.Head();

                await Assert.That(bare.IsBare).IsTrue();
                await Assert.That(bare.IsWorktree).IsFalse();
                await Assert.That(bare.WorkingDirectory).IsNull();
                await Assert.That(Canonical(bare.RepositoryPath))
                    .IsEqualTo(Canonical(bare.CommonDirectory));
                await Assert.That(head.IsUnborn).IsTrue();
            }
        }
        finally
        {
            parent.Delete(recursive: true);
        }
    }

    [Test]
    public async Task Head_ReportsDetachedRepositoryState()
    {
        var parent = Directory.CreateTempSubdirectory();

        try
        {
            var root = Path.Combine(parent.FullName, "repository");
            string gitDirectory;
            using (var initialized = GixRepository.Init(root))
            {
                gitDirectory = initialized.RepositoryPath;
            }

            var treeId = await WriteLooseObject(
                gitDirectory,
                "tree",
                Array.Empty<byte>());
            var commitBody = Encoding.UTF8.GetBytes(
                $"tree {treeId}\nauthor Test <test@example.com> 0 +0000\n" +
                "committer Test <test@example.com> 0 +0000\n\ndetached\n");
            var commitId = await WriteLooseObject(
                gitDirectory,
                "commit",
                commitBody);
            await File.WriteAllTextAsync(
                Path.Combine(gitDirectory, "HEAD"),
                $"{commitId}\n",
                Encoding.ASCII);

            using var detached = GixRepository.Open(root);
            var head = detached.Head();

            await Assert.That(head.IsDetached).IsTrue();
            await Assert.That(head.IsUnborn).IsFalse();
            await Assert.That(head.Target).IsEqualTo(commitId);
            await Assert.That(head.Referent).IsEmpty();
        }
        finally
        {
            parent.Delete(recursive: true);
        }
    }

    [Test]
    public async Task Discovery_FindsRepositoryFromNestedDirectoryAndFile()
    {
        var parent = Directory.CreateTempSubdirectory();

        try
        {
            var root = Path.Combine(parent.FullName, "repository");
            using (var initialized = GixRepository.Init(root))
            {
                var nested = Directory.CreateDirectory(
                    Path.Combine(root, "one", "two"));
                var nestedFile = Path.Combine(nested.FullName, "file.txt");
                await File.WriteAllTextAsync(nestedFile, "content");

                var found = GixRepository.TryDiscover(
                    nested.FullName,
                    out var discoveredPath);

                await Assert.That(found).IsTrue();
                await Assert.That(Canonical(discoveredPath))
                    .IsEqualTo(Canonical(initialized.RepositoryPath));
                await Assert.That(Canonical(GixRepository.Discover(nested.FullName)))
                    .IsEqualTo(Canonical(initialized.RepositoryPath));
                await Assert.That(Canonical(GixRepository.FindWorktreeRoot(nestedFile)))
                    .IsEqualTo(Canonical(root));

                using var opened = GixRepository.OpenDiscovered(nested.FullName);
                await Assert.That(Canonical(opened.RepositoryPath))
                    .IsEqualTo(Canonical(initialized.RepositoryPath));
                await Assert.That(Canonical(opened.WorkingDirectory!))
                    .IsEqualTo(Canonical(root));
            }
        }
        finally
        {
            parent.Delete(recursive: true);
        }
    }

    [Test]
    public async Task Discovery_HonorsCeilingDirectoriesAndAcrossFileSystemsOption()
    {
        var parent = Directory.CreateTempSubdirectory();

        try
        {
            var root = Path.Combine(parent.FullName, "repository");
            using (var initialized = GixRepository.Init(root))
            {
                var ceiling = Directory.CreateDirectory(Path.Combine(root, "one"));
                var nested = Directory.CreateDirectory(Path.Combine(ceiling.FullName, "two"));

                var found = GixRepository.TryDiscover(
                    nested.FullName,
                    out var repositoryPath,
                    ceilingDirectories: ceiling.FullName);

                await Assert.That(found).IsFalse();
                await Assert.That(repositoryPath).IsEmpty();
                await Assert.That(() => GixRepository.Discover(
                        nested.FullName,
                        ceilingDirectories: ceiling.FullName))
                    .Throws<DirectoryNotFoundException>();
                await Assert.That(Canonical(GixRepository.Discover(
                        nested.FullName,
                        acrossFileSystems: true)))
                    .IsEqualTo(Canonical(initialized.RepositoryPath));
            }
        }
        finally
        {
            parent.Delete(recursive: true);
        }
    }

    [Test]
    public async Task Discovery_ReportsMissingRepositoryWithoutLeakingInteropErrors()
    {
        var parent = Directory.CreateTempSubdirectory();

        try
        {
            var found = GixRepository.TryDiscover(
                parent.FullName,
                out var repositoryPath);

            await Assert.That(found).IsFalse();
            await Assert.That(repositoryPath).IsEmpty();
            await Assert.That(() => GixRepository.Discover(parent.FullName))
                .Throws<DirectoryNotFoundException>();
            await Assert.That(() => GixRepository.OpenDiscovered(parent.FullName))
                .Throws<DirectoryNotFoundException>();
            await Assert.That(() => GixRepository.FindWorktreeRoot(parent.FullName))
                .Throws<DirectoryNotFoundException>();
        }
        finally
        {
            parent.Delete(recursive: true);
        }
    }

    [Test]
    public async Task RepositoryMetadata_IsGuardedAfterDispose()
    {
        var repository = GixRepository.Open(RepositoryRoot());
        repository.Dispose();

        await Assert.That(() => repository.RepositoryPath)
            .Throws<ObjectDisposedException>();
        await Assert.That(() => repository.WorkingDirectory)
            .Throws<ObjectDisposedException>();
        await Assert.That(() => repository.CommonDirectory)
            .Throws<ObjectDisposedException>();
        await Assert.That(() => repository.IsBare)
            .Throws<ObjectDisposedException>();
        await Assert.That(() => repository.IsWorktree)
            .Throws<ObjectDisposedException>();
        await Assert.That(() => repository.GitDir())
            .Throws<ObjectDisposedException>();
    }

    private static async Task<string> WriteLooseObject(
        string gitDirectory,
        string objectType,
        byte[] content)
    {
        var header = Encoding.ASCII.GetBytes($"{objectType} {content.Length}\0");
        var payload = new byte[header.Length + content.Length];
        Buffer.BlockCopy(header, 0, payload, 0, header.Length);
        Buffer.BlockCopy(content, 0, payload, header.Length, content.Length);

        var objectId = Convert.ToHexString(SHA1.HashData(payload)).ToLowerInvariant();
        var objectDirectory = Path.Combine(
            gitDirectory,
            "objects",
            objectId[..2]);
        Directory.CreateDirectory(objectDirectory);

        await using var output = File.Create(
            Path.Combine(objectDirectory, objectId[2..]));
        await using var compressed = new ZLibStream(
            output,
            CompressionLevel.Optimal);
        await compressed.WriteAsync(payload);
        return objectId;
    }

    private static string DecodePath(byte[] path) =>
        Canonical(Encoding.UTF8.GetString(path));

    private static string Canonical(string path) =>
        Path.TrimEndingDirectorySeparator(Path.GetFullPath(path));

    private static string RepositoryRoot([CallerFilePath] string sourceFile = "") =>
        Path.GetFullPath(Path.Combine(
            Path.GetDirectoryName(sourceFile)!,
            "..",
            "..",
            ".."));
}
