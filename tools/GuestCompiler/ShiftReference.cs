using System.Globalization;
using System.Reflection;
using System.Reflection.Emit;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text.Json;

internal static class ShiftReference
{
    // Test-only: replace Roslyn's ldc.i4.s mask/and with three nops. Offsets,
    // metadata, stack bounds and all other instructions stay unchanged. The
    // raw fixture tests Renraku's own count policy rather than a C# mask.
    public static void WriteRawFixture(byte[] original, string path)
    {
        byte[] bytes = (byte[])original.Clone();
        using var pe = new PEReader(new MemoryStream(original));
        var metadata = pe.GetMetadataReader();
        int patched = 0;
        foreach (var handle in metadata.MethodDefinitions)
        {
            var method = metadata.GetMethodDefinition(handle);
            string name = metadata.GetString(method.Name);
            if (!name.StartsWith("Shl", StringComparison.Ordinal) && !name.StartsWith("Shr", StringComparison.Ordinal)) continue;
            int rva = method.RelativeVirtualAddress;
            var section = pe.PEHeaders.SectionHeaders.Single(s => rva >= s.VirtualAddress && rva < s.VirtualAddress + Math.Max(s.VirtualSize, s.SizeOfRawData));
            int offset = section.PointerToRawData + rva - section.VirtualAddress;
            int header = (bytes[offset] & 3) == 2 ? 1 : ((bytes[offset] | bytes[offset + 1] << 8) >> 12) * 4;
            byte[] il = pe.GetMethodBody(rva).GetILBytes()!;
            if (il.Length != 7 || il[0] != 2 || il[1] != 3 || il[2] != 0x1f || il[3] != (name.EndsWith("32", StringComparison.Ordinal) ? 31 : 63) || il[4] != 0x5f
                || il[5] < 0x62 || il[5] > 0x64 || il[6] != 0x2a)
                throw new InvalidOperationException("unexpected Roslyn shift-mask shape");
            for (int i = 2; i <= 4; i++) bytes[offset + header + i] = 0;
            patched++;
        }
        if (patched != 10) throw new InvalidOperationException("missing direct shift methods");
        File.WriteAllBytes(path, bytes);
    }

    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Guest.IntegerShifts", throwOnError: true)!;
        var cases = new List<object>();
        var mixes = new List<object>();
        var random = new Random(0x534849);
        int[] counts = { int.MinValue, -65537, -129, -128, -127, -65, -64, -63, -33, -32, -31, -1,
            0, 1, 2, 7, 15, 16, 30, 31, 32, 33, 62, 63, 64, 65, 127, 128, 129, 255, 256, 65537, int.MaxValue };
        foreach (int width in new[] { 32, 64 })
        {
            long[] values = { 0, 1, -1, 2, -2, 0x55555555, unchecked((int)0xaaaaaaaa),
                width == 32 ? int.MinValue : long.MinValue, width == 32 ? int.MinValue + 1L : long.MinValue + 1,
                width == 32 ? int.MaxValue : long.MaxValue, width == 32 ? int.MaxValue - 1L : long.MaxValue - 1,
                width == 32 ? 0x33333333 : 0x5555555555555555, width == 32 ? unchecked((int)0xcccccccc) : unchecked((long)0xaaaaaaaaaaaaaaaa) };
            foreach (string operation in new[] { "Shl", "ShlU", "Shr", "ShrU", "ShrZero" })
            {
                string name = operation + width;
                var method = type.GetMethod(name)!;
                var valueType = width == 32 ? typeof(int) : typeof(long);
                var raw = new DynamicMethod("Raw" + name, valueType, new[] { valueType, typeof(int) });
                var il = raw.GetILGenerator();
                il.Emit(OpCodes.Ldarg_0); il.Emit(OpCodes.Ldarg_1);
                il.Emit(operation.StartsWith("Shl", StringComparison.Ordinal) ? OpCodes.Shl : operation == "Shr" ? OpCodes.Shr : OpCodes.Shr_Un);
                il.Emit(OpCodes.Ret);
                void Add(long input, int count)
                {
                    long bits = width == 32 ? unchecked((int)input) : input;
                    object argument = method.GetParameters()[0].ParameterType switch {
                        Type t when t == typeof(int) => (object)unchecked((int)bits),
                        Type t when t == typeof(uint) => unchecked((uint)bits),
                        Type t when t == typeof(ulong) => unchecked((ulong)bits), _ => bits };
                    long Storage(object value) => value switch {
                        int v => v, uint v => unchecked((int)v), long v => v, ulong v => unchecked((long)v),
                        _ => throw new InvalidOperationException("unexpected shift result") };
                    long expected = Storage(method.Invoke(null, new object[] { argument, count })!);
                    object rawArg = width == 32 ? (object)unchecked((int)bits) : bits;
                    long actualRaw = Storage(raw.Invoke(null, new object[] { rawArg, count })!);
                    // ECMA specifies these raw counts. Out-of-range raw host
                    // results are recorded but do not define Renraku's policy.
                    if (count >= 0 && count < width && actualRaw != expected)
                        throw new InvalidOperationException("specified raw shift disagrees with emitted C#");
                    cases.Add(new { method = name, width, input = bits.ToString(CultureInfo.InvariantCulture), count,
                        value = expected.ToString(CultureInfo.InvariantCulture), rawValue = actualRaw.ToString(CultureInfo.InvariantCulture) });
                }
                foreach (long value in values) foreach (int count in counts) Add(value, count);
                for (int i = 0; i < 128; i++) {
                    byte[] a = new byte[8], b = new byte[4]; random.NextBytes(a); random.NextBytes(b);
                    Add(BitConverter.ToInt64(a), BitConverter.ToInt32(b));
                }
            }
        }
        var mix = type.GetMethod("Mix32")!;
        foreach (uint value in new uint[] { 0, 1, 19, 612, 0x55555555, 0x80000000, uint.MaxValue })
        foreach (int count in counts) {
            uint actual = (uint)mix.Invoke(null, new object[] { value, count })!;
            uint rotated = System.Numerics.BitOperations.RotateLeft(value, count);
            if (actual != (rotated ^ (rotated >> 7))) throw new InvalidOperationException("managed mix differs from independent rotate oracle");
            mixes.Add(new { input = unchecked((int)value).ToString(CultureInfo.InvariantCulture), count,
                value = unchecked((int)actual).ToString(CultureInfo.InvariantCulture) });
        }
        File.WriteAllText(Path.Combine(output, "shift-reference.json"), JsonSerializer.Serialize(new { cases, mixes }));
    }
}
