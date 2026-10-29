using System.Globalization;
using System.Reflection;
using System.Text.Json;

internal static class SmallConversionReference
{
    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Guest.SmallIntegerConversions", throwOnError: true)!;
        var cases = new List<object>();
        var random = new Random(0x534d41);
        foreach (string name in new[] { "I32ToI8", "I32ToU8", "I32ToI16", "I32ToU16", "I64ToI8", "I64ToU8", "I64ToI16", "I64ToU16", "I32ToChar", "I64ToChar" })
        {
            var method = type.GetMethod(name)!;
            int width = name.StartsWith("I32", StringComparison.Ordinal) ? 32 : 64;
            var values = new List<long> { 0, 1, -1, 127, 128, 129, 255, 256, 257, -128, -129, -256, -257,
                32767, 32768, 32769, 65535, 65536, 65537, -32768, -32769, -65536, -65537,
                0xd7ff, 0xd800, 0xdbff, 0xdc00, 0xdfff, 0xe000, 0xfffe,
                int.MinValue, int.MinValue + 1L, int.MaxValue, int.MaxValue - 1L,
                0x123456788000ffff, -4294967297L, long.MinValue, long.MinValue + 1, long.MaxValue, long.MaxValue - 1 };
            for (int i = 0; i < 128; i++) { byte[] bytes = new byte[8]; random.NextBytes(bytes); values.Add(BitConverter.ToInt64(bytes)); }
            foreach (long value in values)
            {
                long bits = width == 32 ? unchecked((int)value) : value;
                object argument = width == 32 ? (object)unchecked((int)bits) : bits;
                object actual = method.Invoke(null, new[] { argument })!;
                int result = actual is char c ? c : (int)actual;
                cases.Add(new { method = name, width, input = bits.ToString(CultureInfo.InvariantCulture), value = result });
            }
        }
        File.WriteAllText(Path.Combine(output, "small-conversion-reference.json"), JsonSerializer.Serialize(cases));
    }
}
