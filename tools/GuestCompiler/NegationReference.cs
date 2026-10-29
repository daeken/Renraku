using System.Globalization;
using System.Reflection;
using System.Text.Json;

internal static class NegationReference
{
    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Guest.IntegerNegation", throwOnError: true)!;
        var cases = new List<object>();
        var random = new Random(0x4e4547);
        foreach (string name in new[] { "Neg32", "Neg64", "NegUInt32", "NegAsUInt64" })
        {
            var method = type.GetMethod(name)!;
            var parameter = method.GetParameters()[0].ParameterType;
            int width = parameter == typeof(int) || parameter == typeof(uint) ? 32 : 64;
            int resultWidth = method.ReturnType == typeof(int) ? 32 : 64;
            var values = new List<long> { 0, 1, -1, 2, -2, 10, -10, int.MinValue, int.MinValue + 1L,
                int.MaxValue, int.MaxValue - 1L, 2147483648L, 4294967295L, 4294967296L,
                long.MinValue, long.MinValue + 1, long.MaxValue, long.MaxValue - 1 };
            for (int i = 0; i < 128; i++) { byte[] bytes = new byte[8]; random.NextBytes(bytes); values.Add(BitConverter.ToInt64(bytes)); }
            foreach (long value in values)
            {
                long bits = width == 32 ? unchecked((int)value) : value;
                object argument = parameter == typeof(int) ? (object)unchecked((int)bits)
                    : parameter == typeof(uint) ? unchecked((uint)bits)
                    : parameter == typeof(ulong) ? unchecked((ulong)bits) : bits;
                object actual = method.Invoke(null, new[] { argument })!;
                long result = actual switch { int v => v, long v => v, ulong v => unchecked((long)v),
                    _ => throw new InvalidOperationException("unexpected negation result") };
                cases.Add(new { method = name, width, resultWidth, input = bits.ToString(CultureInfo.InvariantCulture),
                    value = result.ToString(CultureInfo.InvariantCulture) });
            }
        }
        File.WriteAllText(Path.Combine(output, "negation-reference.json"), JsonSerializer.Serialize(cases));
    }
}
