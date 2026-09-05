using System.Reflection;
using GixSharp;

namespace GixSharp.Tests;

public sealed class ManagedApiBoundaryTests
{
    // The generator owns this namespace, including nested cases and marshallers.
    private static readonly string GeneratedNamespace = typeof(Repo).Namespace!;
    private const BindingFlags Members = BindingFlags.Public | BindingFlags.NonPublic |
        BindingFlags.Instance | BindingFlags.Static | BindingFlags.DeclaredOnly;

    [Test]
    public async Task PublicManagedSurface_DoesNotExposeGeneratedTypes()
    {
        await Assert.That(GeneratedNamespace).IsEqualTo("GixSharp.Native");
        var publicTypes = typeof(GixRepository).Assembly.GetTypes()
            .Where(static type => IsExternallyVisible(type) && !IsGenerated(type));
        var violations = FindExposures(publicTypes);

        await Assert.That(string.Join(Environment.NewLine, violations)).IsEqualTo("");
    }

    [Test]
    public async Task BoundaryCheck_RejectsNestedCasesAcrossPublicSignatureShapes()
    {
        var violations = FindExposures(
            [typeof(LeakingSignatures), typeof(LeakingRecord), typeof(LeakingBase),
             typeof(LeakingConstraint<>), typeof(LeakingDelegate)]);

        foreach (var site in new[]
        {
            "field Callback", "property Value", "constructor", "event Changed",
            "method Return", "method Accept", "method Constraint", "base type",
            "LeakingConstraint", "method Invoke",
        })
        {
            await Assert.That(violations.Any(value => value.Contains(site, StringComparison.Ordinal)))
                .IsTrue();
        }
        await Assert.That(violations.Any(value =>
            value.Contains(nameof(GixError.InvalidIdCase), StringComparison.Ordinal))).IsTrue();
    }

    [Test]
    public async Task BoundaryCheck_AcceptsOwnedRecordsAndRecursiveManagedConstraints()
    {
        var violations = FindExposures([typeof(OwnedRecord), typeof(ManagedConstraint<>)]);
        await Assert.That(string.Join(Environment.NewLine, violations)).IsEqualTo("");
    }

    [Test]
    public async Task BoundaryCheck_IncludesProtectedNestedDeclarations()
    {
        var publicTypes = typeof(ManagedApiBoundaryTests).Assembly.GetTypes()
            .Where(IsExternallyVisible);
        var violations = FindExposures(publicTypes);

        await Assert.That(violations.Any(value =>
            value.Contains("HiddenSurface method Leak", StringComparison.Ordinal))).IsTrue();
    }

    [Test]
    public async Task ReferenceOutcomeMapping_PreservesThePublicContract()
    {
        await Assert.That(typeof(ReferenceUpdateOutcome).FullName)
            .IsEqualTo("GixSharp.ReferenceUpdateOutcome");
        await Assert.That(Enum.GetUnderlyingType(typeof(ReferenceUpdateOutcome)))
            .IsEqualTo(typeof(byte));
        await Assert.That(Enum.GetNames<ReferenceUpdateOutcome>()
            .SequenceEqual(new[] { "Applied", "Mismatch", "Absent" })).IsTrue();
        await Assert.That(Enum.GetValues<ReferenceUpdateOutcome>()
            .Select(static value => (byte)value).SequenceEqual(new byte[] { 0, 1, 2 })).IsTrue();

        var map = (typeof(GixRepository).GetMethod(
            "ReadReferenceUpdateOutcome", BindingFlags.NonPublic | BindingFlags.Static)
            ?? throw new InvalidOperationException("The reference outcome boundary was not found."))
            .CreateDelegate<Func<GixSharp.Native.ReferenceUpdateOutcome, ReferenceUpdateOutcome>>();

        // Every named native value must receive an intentional managed mapping.
        foreach (var native in Enum.GetValues<GixSharp.Native.ReferenceUpdateOutcome>())
            await Assert.That(map(native).ToString()).IsEqualTo(native.ToString());
        await Assert.That(() => map((GixSharp.Native.ReferenceUpdateOutcome)byte.MaxValue))
            .Throws<InteropException>();
    }

    private static bool IsExternallyVisible(Type type) =>
        type.DeclaringType is { } declaringType
            ? IsExternallyVisible(declaringType) &&
                (type.IsNestedPublic || type.IsNestedFamily || type.IsNestedFamORAssem)
            : type.IsPublic;

    private static SortedSet<string> FindExposures(IEnumerable<Type> publicTypes)
    {
        var violations = new SortedSet<string>(StringComparer.Ordinal);
        foreach (var type in publicTypes)
        {
            var owner = type.FullName ?? type.Name;
            Check(type, owner);
            if (type.BaseType is { } baseType)
                Check(baseType, $"{owner} base type");
            foreach (var implemented in type.GetInterfaces())
                Check(implemented, $"{owner} interface");

            foreach (var field in type.GetFields(Members).Where(static field =>
                field.IsPublic || field.IsFamily || field.IsFamilyOrAssembly))
                Check(field.FieldType, $"{owner} field {field.Name}");

            foreach (var property in type.GetProperties(Members).Where(static property =>
                property.GetAccessors(nonPublic: true).Any(IsPublicOrProtected)))
            {
                Check(property.PropertyType, $"{owner} property {property.Name}");
                foreach (var parameter in property.GetIndexParameters())
                    Check(parameter.ParameterType, $"{owner} property {property.Name} index");
            }

            foreach (var @event in type.GetEvents(Members).Where(static @event =>
                IsPublicOrProtected(@event.GetAddMethod(nonPublic: true)) ||
                IsPublicOrProtected(@event.GetRemoveMethod(nonPublic: true))))
            {
                if (@event.EventHandlerType is { } handlerType)
                    Check(handlerType, $"{owner} event {@event.Name}");
            }

            foreach (var constructor in type.GetConstructors(Members).Where(IsPublicOrProtected))
                foreach (var parameter in constructor.GetParameters())
                    Check(parameter.ParameterType, $"{owner} constructor {parameter.Name}");

            foreach (var method in type.GetMethods(Members).Where(IsPublicOrProtected))
            {
                var site = $"{owner} method {method.Name}";
                Check(method.ReturnType, $"{site} return");
                foreach (var parameter in method.GetParameters())
                    Check(parameter.ParameterType, $"{site} parameter {parameter.Name}");
                foreach (var argument in method.GetGenericArguments())
                    Check(argument, $"{site} constraint");
            }
        }
        return violations;

        void Check(Type signatureType, string site)
        {
            foreach (var generatedType in GeneratedTypes(signatureType, new HashSet<Type>()))
                violations.Add($"{site} -> {generatedType}");
        }
    }

    private static bool IsPublicOrProtected(MethodBase? method) =>
        method is not null && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly);

    private static bool IsGenerated(Type type)
    {
        while (type.DeclaringType is { } declaringType)
            type = declaringType;
        return type.Namespace == GeneratedNamespace ||
            type.Namespace?.StartsWith(GeneratedNamespace + ".", StringComparison.Ordinal) == true;
    }

    private static IEnumerable<Type> GeneratedTypes(Type type, HashSet<Type> visited)
    {
        if (!visited.Add(type))
            yield break;
        if (IsGenerated(type))
        {
            yield return type;
            yield break;
        }
        if (type.HasElementType)
        {
            foreach (var generated in GeneratedTypes(type.GetElementType()!, visited))
                yield return generated;
        }
        if (type.IsFunctionPointer)
        {
            foreach (var generated in GeneratedTypes(type.GetFunctionPointerReturnType(), visited))
                yield return generated;
            foreach (var parameter in type.GetFunctionPointerParameterTypes())
                foreach (var generated in GeneratedTypes(parameter, visited))
                    yield return generated;
        }
        foreach (var argument in type.GetGenericArguments())
            foreach (var generated in GeneratedTypes(argument, visited))
                yield return generated;
        if (type.IsGenericParameter)
            foreach (var constraint in type.GetGenericParameterConstraints())
                foreach (var generated in GeneratedTypes(constraint, visited))
                    yield return generated;
    }

    // External derived consumers can reach a protected nested declaration.
    public class ProtectedNestedFixture
    {
        protected class HiddenSurface
        {
            public Repo Leak() => throw new NotImplementedException();
        }
    }

    // These deliberately invalid shapes live only in the test assembly.
    private sealed class LeakingSignatures
    {
        public Func<GixError.InvalidIdCase[]>? Callback = null;
        public IReadOnlyList<GixError.InvalidIdCase[]> Value => throw new NotImplementedException();
        public event Action<GixError.InvalidIdCase>? Changed { add { } remove { } }
        public IReadOnlyList<GixError.InvalidIdCase[]> Return() => throw new NotImplementedException();
        public void Accept(ref Dictionary<string, List<GixError.InvalidIdCase[,]>>[] value) { }
        public void Constraint<T>() where T : Repo { }
    }

    private sealed record LeakingRecord(GixError.InvalidIdCase Error);
    private sealed class LeakingBase : List<GixError.InvalidIdCase> { }
    private sealed class LeakingConstraint<T> where T : Repo { }
    private delegate GixError.InvalidIdCase LeakingDelegate();
    private sealed record OwnedRecord(IReadOnlyList<GixSignature[]> Values);
    private sealed class ManagedConstraint<T> where T : IComparable<T> { }
}
