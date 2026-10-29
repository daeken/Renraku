using System.Reflection;
using System.Reflection.Emit;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text.Json;

internal static class SwitchReference
{
    public static byte[] RawBody(string name)
    {
        var bytes = new List<byte>();
        void Table(params int[] deltas) {
            bytes.Add(0x45); bytes.AddRange(BitConverter.GetBytes(deltas.Length));
            foreach (int delta in deltas) bytes.AddRange(BitConverter.GetBytes(delta));
        }
        switch (name) {
            case "RawZero": bytes.Add(2); Table(); bytes.AddRange(new byte[] { 0x1f, 42, 0x2a }); break;
            case "RawDuplicate":
                bytes.Add(2); Table(3, 3, 6);
                bytes.AddRange(new byte[] { 0x1f, 255, 0x2a, 0x1f, 11, 0x2a, 0x1f, 22, 0x2a }); break;
            case "RawBackward":
                bytes.AddRange(new byte[] { 0x2b, 3, 0x1f, 77, 0x2a, 2 });
                Table(-17, 0); bytes.AddRange(new byte[] { 0x1f, 255, 0x2a }); break;
            default: throw new InvalidOperationException(name);
        }
        return bytes.ToArray();
    }
    public static void WriteRawFixture(byte[] original, string path)
    {
        byte[] bytes = original.ToArray();
        using var pe = new PEReader(new MemoryStream(original));
        var metadata = pe.GetMetadataReader();
        int patched = 0;
        foreach (var handle in metadata.MethodDefinitions) {
            var method = metadata.GetMethodDefinition(handle);
            string name = metadata.GetString(method.Name);
            if (!name.StartsWith("Raw", StringComparison.Ordinal)) continue;
            byte[] raw = RawBody(name);
            int rva = method.RelativeVirtualAddress;
            var section = pe.PEHeaders.SectionHeaders.Single(s => rva >= s.VirtualAddress && rva < s.VirtualAddress + s.VirtualSize);
            int offset = section.PointerToRawData + rva - section.VirtualAddress;
            int header = (bytes[offset] & 3) == 2 ? 1 : ((bytes[offset + 1] >> 4) * 4);
            int length = pe.GetMethodBody(rva).GetILBytes()!.Length;
            if (raw.Length > length) throw new InvalidOperationException("raw switch fixture exceeds reserved body");
            Array.Clear(bytes, offset + header, length);
            bytes[offset + header + length - 1] = 0x2a;
            raw.CopyTo(bytes, offset + header);
            patched++;
        }
        if (patched != 3) throw new InvalidOperationException("missing raw switch fixtures");
        File.WriteAllBytes(path, bytes);
    }
    private static DynamicMethod Raw(string name)
    {
        var method = new DynamicMethod(name, typeof(int), new[] { typeof(int) });
        var il = method.GetILGenerator();
        if (name == "RawBackward") {
            var dispatch = il.DefineLabel(); var backward = il.DefineLabel(); var missing = il.DefineLabel();
            il.Emit(OpCodes.Br, dispatch); il.MarkLabel(backward); il.Emit(OpCodes.Ldc_I4, 77); il.Emit(OpCodes.Ret);
            il.MarkLabel(dispatch); il.Emit(OpCodes.Ldarg_0); il.Emit(OpCodes.Switch, new[] { backward, missing });
            il.MarkLabel(missing); il.Emit(OpCodes.Ldc_I4_M1); il.Emit(OpCodes.Ret);
        } else if (name == "RawDuplicate") {
            var a = il.DefineLabel(); var b = il.DefineLabel();
            il.Emit(OpCodes.Ldarg_0); il.Emit(OpCodes.Switch, new[] { a, a, b });
            il.Emit(OpCodes.Ldc_I4_M1); il.Emit(OpCodes.Ret);
            il.MarkLabel(a); il.Emit(OpCodes.Ldc_I4, 11); il.Emit(OpCodes.Ret);
            il.MarkLabel(b); il.Emit(OpCodes.Ldc_I4, 22); il.Emit(OpCodes.Ret);
        } else {
            il.Emit(OpCodes.Ldarg_0); il.Emit(OpCodes.Switch, Array.Empty<Label>());
            il.Emit(OpCodes.Ldc_I4, 42); il.Emit(OpCodes.Ret);
        }
        return method;
    }
    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Guest.IntegerSwitch", throwOnError: true)!;
        var values = Enumerable.Range(-16, 49).Concat(new[] { int.MinValue, int.MaxValue }).ToList();
        var random = new Random(0x535749);
        for (int i = 0; i < 128; i++) { byte[] bytes = new byte[4]; random.NextBytes(bytes); values.Add(BitConverter.ToInt32(bytes)); }
        var cases = new List<object>();
        foreach (string name in new[] { "Select32", "SelectOffset", "SelectUnsigned", "SelectEnum", "SelectWide", "RawZero", "RawDuplicate", "RawBackward" }) {
            bool raw = name.StartsWith("Raw", StringComparison.Ordinal);
            MethodInfo method = raw ? Raw(name) : type.GetMethod(name)!;
            var parameter = method.GetParameters()[0].ParameterType;
            foreach (int value in values) {
                object argument = parameter == typeof(uint) ? (object)unchecked((uint)value)
                    : parameter.IsEnum ? Enum.ToObject(parameter, value) : value;
                object actual = method.Invoke(null, new[] { argument })!;
                cases.Add(new { method = name, raw, input = value, wide = actual is long,
                    value = Convert.ToInt64(actual).ToString(System.Globalization.CultureInfo.InvariantCulture) });
            }
        }
        File.WriteAllText(Path.Combine(output, "switch-reference.json"), JsonSerializer.Serialize(cases));
    }
}
