using System.Globalization;
using System.Collections.Immutable;
using System.Reflection;
using System.Reflection.Emit;
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
using System.Text.Json;

namespace SlopHawk.Core;

public sealed record ManagedBodyDifference(
    string Method, string Name, string Change, int CommonPrefixCount,
    IReadOnlyList<string> Candidate, IReadOnlyList<string> Reference, IReadOnlyList<long> IntegerLiterals);

public static class ManagedBodyComparison
{
    private static readonly Dictionary<ushort, OpCode> Opcodes = typeof(OpCodes).GetFields(BindingFlags.Public | BindingFlags.Static)
        .Select(field => (OpCode)field.GetValue(null)!).GroupBy(opcode => unchecked((ushort)opcode.Value))
        .ToDictionary(group => group.Key, group => group.First());

    public static IReadOnlyDictionary<string, long[]> IntegerLiterals(Stream assembly, ISet<string> methodNames) =>
        Read(assembly, methodNames).Values.GroupBy(body => body.Name, StringComparer.Ordinal)
            .ToDictionary(group => group.Key, group => group.SelectMany(body => body.Literals).Distinct().Order().ToArray(),
                StringComparer.Ordinal);

    public static IReadOnlyList<ManagedBodyDifference> Compare(Stream candidate, Stream reference)
    {
        var left = Read(candidate);
        var right = Read(reference);
        var changes = new List<ManagedBodyDifference>();
        foreach (var method in left.Keys.Union(right.Keys, StringComparer.Ordinal).Order(StringComparer.Ordinal))
        {
            var hasLeft = left.TryGetValue(method, out var lhs);
            var hasRight = right.TryGetValue(method, out var rhs);
            var before = rhs?.Instructions ?? [];
            var after = lhs?.Instructions ?? [];
            if (hasLeft && hasRight && after.SequenceEqual(before, StringComparer.Ordinal))
                continue;
            var prefix = 0;
            while (prefix < Math.Min(before.Length, after.Length) && before[prefix] == after[prefix])
                prefix++;
            var suffix = 0;
            while (suffix < Math.Min(before.Length, after.Length) - prefix && before[^(suffix + 1)] == after[^(suffix + 1)])
                suffix++;
            changes.Add(new(method, (lhs ?? rhs)!.Name, !hasLeft ? "removed" : !hasRight ? "added" : "changed", prefix,
                Excerpt(after, prefix, suffix), Excerpt(before, prefix, suffix),
                (lhs?.Literals ?? []).Concat(rhs?.Literals ?? []).Distinct().Order().ToArray()));
        }
        return changes;
    }

    private static string[] Excerpt(string[] body, int prefix, int suffix)
    {
        var start = Math.Max(0, prefix - 2);
        var end = Math.Min(body.Length, body.Length - suffix + 2);
        return body.Skip(start).Take(Math.Min(end - start, 24)).ToArray();
    }

    private sealed record MethodBody(string Name, string[] Instructions, long[] Literals);
    private sealed record Instruction(int Offset, string Opcode, string? Operand = null, int[]? Targets = null);

    private static Dictionary<string, MethodBody> Read(Stream stream, ISet<string>? methodNames = null)
    {
        using var pe = new PEReader(stream, PEStreamOptions.LeaveOpen);
        if (!pe.HasMetadata)
            throw new BadImageFormatException("A managed PE assembly was expected.");
        var metadata = pe.GetMetadataReader();
        var names = new Names(metadata);
        var methods = new Dictionary<string, MethodBody>(StringComparer.Ordinal);
        foreach (var handle in metadata.MethodDefinitions)
        {
            var method = metadata.GetMethodDefinition(handle);
            if (method.RelativeVirtualAddress == 0 || (method.ImplAttributes & MethodImplAttributes.CodeTypeMask) != MethodImplAttributes.IL)
                continue;
            var methodName = metadata.GetString(method.Name);
            var operation = methodNames is null ? methodName : RequestedOperation(methodName, methodNames);
            if (operation is null)
                continue;
            var body = pe.GetMethodBody(method.RelativeVirtualAddress);
            var bytes = body.GetILReader();
            var instructions = new List<Instruction>();
            while (bytes.RemainingBytes != 0)
                instructions.Add(Decode(ref bytes, metadata, names));
            var offsets = instructions.Select((item, index) => (item.Offset, index)).ToDictionary(item => item.Offset, item => item.index);
            offsets.Add(bytes.Length, instructions.Count);
            var normalized = new List<string>();
            if (!body.LocalSignature.IsNil)
            {
                var signature = metadata.GetStandaloneSignature(body.LocalSignature);
                normalized.Add((body.LocalVariablesInitialized ? "locals init " : "locals ") +
                    string.Join(", ", signature.DecodeLocalSignature(names, null)));
            }
            foreach (var item in instructions)
            {
                var operand = item.Targets is null ? item.Operand : string.Join(", ", item.Targets.Select(target => $"#{Position(target)}"));
                normalized.Add(operand is null ? item.Opcode : $"{item.Opcode} {operand}");
            }
            foreach (var region in body.ExceptionRegions)
                normalized.Add($"{region.Kind} #{Position(region.TryOffset)}..#{Position(region.TryOffset + region.TryLength)} " +
                    $"#{Position(region.HandlerOffset)}..#{Position(region.HandlerOffset + region.HandlerLength)} " +
                    (region.Kind == ExceptionRegionKind.Catch ? names.Entity(region.CatchType) :
                        region.Kind == ExceptionRegionKind.Filter ? $"#{Position(region.FilterOffset)}" : ""));

            var symbol = names.Entity(handle);
            var literals = instructions.Select(IntegerLiteral).OfType<long>().Distinct().ToArray();
            if (!methods.TryAdd(symbol, new MethodBody(operation, normalized.ToArray(), literals)))
                throw new BadImageFormatException($"Duplicate managed method {symbol}.");

            int Position(int offset) => offsets.TryGetValue(offset, out var index) ? index
                : throw new BadImageFormatException("A branch or exception region points outside an instruction boundary.");
        }
        return methods;
    }

    private static string? RequestedOperation(string name, ISet<string> methodNames)
    {
        if (methodNames.Contains(name))
            return name;
        if (!name.StartsWith('<'))
            return null;
        var close = name.IndexOf('>');
        if (close <= 1)
            return null;
        var suffix = name.AsSpan(close + 1);
        if (!suffix.StartsWith("g__", StringComparison.Ordinal) && !suffix.StartsWith("b__", StringComparison.Ordinal))
            return null;
        var containingMethod = name[1..close];
        return methodNames.Contains(containingMethod) ? containingMethod : null;
    }

    private static long? IntegerLiteral(Instruction instruction)
    {
        if (instruction.Opcode is "ldc.i4" or "ldc.i4.s" or "ldc.i8" &&
            long.TryParse(instruction.Operand, NumberStyles.Integer, CultureInfo.InvariantCulture, out var value))
            return value;
        if (instruction.Opcode is "ldc.i4.m1")
            return -1;
        return instruction.Opcode.StartsWith("ldc.i4.", StringComparison.Ordinal) &&
            instruction.Opcode.Length == 8 && instruction.Opcode[^1] is >= '0' and <= '8'
            ? instruction.Opcode[^1] - '0' : null;
    }

    private static Instruction Decode(ref BlobReader bytes, MetadataReader metadata, Names names)
    {
        var offset = bytes.Offset;
        var code = (ushort)bytes.ReadByte();
        if (code == 0xfe)
            code = (ushort)(0xfe00 | bytes.ReadByte());
        if (!Opcodes.TryGetValue(code, out var instruction))
            throw new BadImageFormatException("An IL opcode is unknown.");
        var name = instruction.Name ?? throw new BadImageFormatException("An IL opcode has no name.");
        if (instruction.OperandType is OperandType.ShortInlineBrTarget or OperandType.InlineBrTarget)
        {
            var displacement = instruction.OperandType == OperandType.ShortInlineBrTarget ? bytes.ReadSByte() : bytes.ReadInt32();
            return new(offset, name.EndsWith(".s", StringComparison.Ordinal) ? name[..^2] : name,
                Targets: [checked(bytes.Offset + displacement)]);
        }
        if (instruction.OperandType == OperandType.InlineSwitch)
        {
            var count = bytes.ReadInt32();
            if (count < 0 || count > bytes.RemainingBytes / 4)
                throw new BadImageFormatException("An IL switch table is truncated.");
            var baseOffset = checked(bytes.Offset + count * 4);
            var targets = new int[count];
            for (var index = 0; index < count; index++)
                targets[index] = checked(baseOffset + bytes.ReadInt32());
            return new(offset, name, Targets: targets);
        }
        var operand = instruction.OperandType switch
        {
            OperandType.InlineNone => null,
            OperandType.ShortInlineI => bytes.ReadSByte().ToString(CultureInfo.InvariantCulture),
            OperandType.InlineI => bytes.ReadInt32().ToString(CultureInfo.InvariantCulture),
            OperandType.InlineI8 => bytes.ReadInt64().ToString(CultureInfo.InvariantCulture),
            OperandType.ShortInlineR => BitConverter.SingleToInt32Bits(bytes.ReadSingle()).ToString("X8", CultureInfo.InvariantCulture),
            OperandType.InlineR => BitConverter.DoubleToInt64Bits(bytes.ReadDouble()).ToString("X16", CultureInfo.InvariantCulture),
            OperandType.ShortInlineVar => bytes.ReadByte().ToString(CultureInfo.InvariantCulture),
            OperandType.InlineVar => bytes.ReadUInt16().ToString(CultureInfo.InvariantCulture),
            OperandType.InlineString => JsonSerializer.Serialize(metadata.GetUserString(MetadataTokens.UserStringHandle(bytes.ReadInt32() & 0xffffff))),
            OperandType.InlineField or OperandType.InlineMethod or OperandType.InlineSig or OperandType.InlineTok or OperandType.InlineType =>
                names.Entity(MetadataTokens.EntityHandle(bytes.ReadInt32())),
            _ => throw new BadImageFormatException($"Unsupported IL operand type {instruction.OperandType}.")
        };
        return new(offset, name, operand);
    }

    private sealed class Names(MetadataReader metadata) : ISignatureTypeProvider<string, object?>
    {
        private readonly Dictionary<EntityHandle, string> resolved = [];
        private readonly HashSet<EntityHandle> resolving = [];

        internal string Entity(EntityHandle handle)
        {
            if (resolved.TryGetValue(handle, out var known))
                return known;
            if (!resolving.Add(handle))
                throw new BadImageFormatException("A metadata signature refers to itself.");
            try
            {
                var name = handle.Kind switch
                {
                    HandleKind.TypeDefinition => Definition((TypeDefinitionHandle)handle),
                    HandleKind.TypeReference => Reference((TypeReferenceHandle)handle),
                    HandleKind.TypeSpecification => metadata.GetTypeSpecification((TypeSpecificationHandle)handle).DecodeSignature(this, null),
                    HandleKind.MethodDefinition => Method((MethodDefinitionHandle)handle),
                    HandleKind.FieldDefinition => Field((FieldDefinitionHandle)handle),
                    HandleKind.MemberReference => Member((MemberReferenceHandle)handle),
                    HandleKind.MethodSpecification => MethodSpecification((MethodSpecificationHandle)handle),
                    HandleKind.StandaloneSignature => Standalone((StandaloneSignatureHandle)handle),
                    HandleKind.AssemblyReference => metadata.GetString(metadata.GetAssemblyReference((AssemblyReferenceHandle)handle).Name),
                    HandleKind.ModuleReference => metadata.GetString(metadata.GetModuleReference((ModuleReferenceHandle)handle).Name),
                    HandleKind.ModuleDefinition => "<module>",
                    _ => throw new BadImageFormatException($"Unsupported metadata reference {handle.Kind}.")
                };
                resolved.Add(handle, name);
                return name;
            }
            finally
            {
                resolving.Remove(handle);
            }
        }

        private static string Qualified(string space, string name) => space.Length == 0 ? name : space + "." + name;

        private string Definition(TypeDefinitionHandle handle)
        {
            var type = metadata.GetTypeDefinition(handle);
            var name = metadata.GetString(type.Name);
            var enclosing = type.GetDeclaringType();
            return enclosing.IsNil ? Qualified(metadata.GetString(type.Namespace), name) : Entity(enclosing) + "+" + name;
        }

        private string Reference(TypeReferenceHandle handle)
        {
            var type = metadata.GetTypeReference(handle);
            var name = metadata.GetString(type.Name);
            var scope = type.ResolutionScope;
            return scope.Kind == HandleKind.TypeReference ? Entity(scope) + "+" + name :
                (scope.IsNil ? "" : $"[{Entity(scope)}]") + Qualified(metadata.GetString(type.Namespace), name);
        }

        private string Method(MethodDefinitionHandle handle)
        {
            var method = metadata.GetMethodDefinition(handle);
            return Entity(method.GetDeclaringType()) + "::" + metadata.GetString(method.Name) +
                Signature(method.DecodeSignature(this, null));
        }

        private string Field(FieldDefinitionHandle handle)
        {
            var field = metadata.GetFieldDefinition(handle);
            return Entity(field.GetDeclaringType()) + "::" + metadata.GetString(field.Name) + ":" + field.DecodeSignature(this, null);
        }

        private string Member(MemberReferenceHandle handle)
        {
            var member = metadata.GetMemberReference(handle);
            return Entity(member.Parent) + "::" + metadata.GetString(member.Name) + (member.GetKind() == MemberReferenceKind.Field
                ? ":" + member.DecodeFieldSignature(this, null) : Signature(member.DecodeMethodSignature(this, null)));
        }

        private string MethodSpecification(MethodSpecificationHandle handle)
        {
            var method = metadata.GetMethodSpecification(handle);
            return Entity(method.Method) + "<" + string.Join(", ", method.DecodeSignature(this, null)) + ">";
        }

        private string Standalone(StandaloneSignatureHandle handle)
        {
            var signature = metadata.GetStandaloneSignature(handle);
            return signature.GetKind() == StandaloneSignatureKind.LocalVariables
                ? "locals " + string.Join(", ", signature.DecodeLocalSignature(this, null)) : Signature(signature.DecodeMethodSignature(this, null));
        }

        private static string Signature(MethodSignature<string> signature) =>
            $"({string.Join(", ", signature.ParameterTypes)}):{signature.ReturnType}" +
            $"[{signature.Header.RawValue:X2}:{signature.GenericParameterCount}:{signature.RequiredParameterCount}]";

        public string GetArrayType(string elementType, ArrayShape shape) => elementType +
            $"[rank:{shape.Rank}; bounds:{string.Join(',', shape.LowerBounds)}; sizes:{string.Join(',', shape.Sizes)}]";
        public string GetByReferenceType(string elementType) => elementType + "&";
        public string GetFunctionPointerType(MethodSignature<string> signature) => "fn" + Signature(signature);
        public string GetGenericInstantiation(string genericType, ImmutableArray<string> arguments) =>
            genericType + "<" + string.Join(", ", arguments) + ">";
        public string GetGenericMethodParameter(object? context, int index) => "!!" + index.ToString(CultureInfo.InvariantCulture);
        public string GetGenericTypeParameter(object? context, int index) => "!" + index.ToString(CultureInfo.InvariantCulture);
        public string GetModifiedType(string modifier, string type, bool required) => $"{type} {(required ? "modreq" : "modopt")}({modifier})";
        public string GetPinnedType(string elementType) => elementType + " pinned";
        public string GetPointerType(string elementType) => elementType + "*";
        public string GetPrimitiveType(PrimitiveTypeCode typeCode) => typeCode.ToString();
        public string GetSZArrayType(string elementType) => elementType + "[]";
        public string GetTypeFromDefinition(MetadataReader _, TypeDefinitionHandle handle, byte rawTypeKind) => Entity(handle);
        public string GetTypeFromReference(MetadataReader _, TypeReferenceHandle handle, byte rawTypeKind) => Entity(handle);
        public string GetTypeFromSpecification(MetadataReader _, object? context, TypeSpecificationHandle handle, byte rawTypeKind) => Entity(handle);
    }
}
