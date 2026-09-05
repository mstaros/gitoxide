using System.Reflection;
using GixSharp;

namespace GixSharp.Tests;

public sealed class ErrorMappingTests
{
    private static readonly Func<string, EnumException<GixError>, GixException> Translate =
        (typeof(GixRepository).GetMethod("Translate", BindingFlags.NonPublic | BindingFlags.Static)
            ?? throw new InvalidOperationException("The error translation boundary was not found."))
        .CreateDelegate<Func<string, EnumException<GixError>, GixException>>();

    [Test]
    public async Task GeneratedCases_PreserveKindOperationAndOwnedMessage()
    {
        (GixErrorKind Kind, Func<Utf8String, GixError> Create)[] cases =
        [
            (GixErrorKind.NotARepository, static value => new GixError(new GixError.NotARepositoryCase(value))),
            (GixErrorKind.Io, static value => new GixError(new GixError.IoCase(value))),
            (GixErrorKind.Config, static value => new GixError(new GixError.ConfigCase(value))),
            (GixErrorKind.InvalidPath, static value => new GixError(new GixError.InvalidPathCase(value))),
            (GixErrorKind.InvalidId, static value => new GixError(new GixError.InvalidIdCase(value))),
            (GixErrorKind.NotFound, static value => new GixError(new GixError.NotFoundCase(value))),
            (GixErrorKind.InvalidReference, static value => new GixError(new GixError.InvalidReferenceCase(value))),
            (GixErrorKind.ReferenceConflict, static value => new GixError(new GixError.ReferenceConflictCase(value))),
            (GixErrorKind.ReferenceLocked, static value => new GixError(new GixError.ReferenceLockedCase(value))),
            (GixErrorKind.Other, static value => new GixError(new GixError.OtherCase(value))),
        ];

        foreach (var item in cases)
        {
            var message = $"native {item.Kind}: Unicode Ω and context";
            // The generated error owns this string. Translate copies the text and
            // disposes the error before the returned exception is inspected.
            var error = item.Create(message.Utf8());
            var translated = Translate("MappedOperation", new EnumException<GixError>(error));

            await Assert.That(translated.Kind).IsEqualTo(item.Kind);
            await Assert.That(translated.Operation).IsEqualTo("MappedOperation");
            await Assert.That(translated.NativeMessage).IsEqualTo(message);
        }
    }

    [Test]
    public async Task NullNativeError_IsAnExplicitInteropFailure()
    {
        GixException? failure = null;
        try
        {
            Translate("NullError", new EnumException<GixError>(null!));
        }
        catch (GixException exception)
        {
            failure = exception;
        }

        await Assert.That(failure).IsNotNull();
        await Assert.That(failure!.Operation).IsEqualTo("NullError");
        await Assert.That(failure.Kind).IsEqualTo(GixErrorKind.Other);
        await Assert.That(failure.NativeMessage)
            .IsEqualTo("The native error has no active case.");
        await Assert.That(failure.InnerException is InteropException).IsTrue();
    }
}
