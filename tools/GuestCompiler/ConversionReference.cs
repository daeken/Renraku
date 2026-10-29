using System.Globalization;
using System.Reflection;
using System.Text.Json;

internal static class ConversionReference
{
    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Guest.IntegerConversions", throwOnError: true)!;
        var cases = new List<object>();
        var random = new Random(0x434f4e);
        foreach (var method in type.GetMethods(BindingFlags.Static | BindingFlags.Public))
        {
            var parameter = method.GetParameters()[0].ParameterType;
            int width = parameter == typeof(int) || parameter == typeof(uint) ? 32 : 64;
            int resultWidth = method.ReturnType == typeof(int) || method.ReturnType == typeof(uint) ? 32 : 64;
            var values = new List<long> { 0, 1, -1, 2, -2, int.MinValue, int.MinValue + 1L, int.MaxValue - 1L,
                int.MaxValue, 2147483648L, 4294967295L, 4294967296L, 4294967297L,
                -4294967296L, -4294967297L, long.MinValue, long.MinValue + 1, long.MaxValue - 1, long.MaxValue };
            for (int i = 0; i < 128; i++)
            {
                byte[] bytes = new byte[8]; random.NextBytes(bytes);
                values.Add(BitConverter.ToInt64(bytes));
            }
            foreach (long bits in values)
            {
                long input = width == 32 ? unchecked((int)bits) : bits;
                object argument = parameter == typeof(int) ? (object)unchecked((int)input)
                    : parameter == typeof(uint) ? unchecked((uint)input)
                    : parameter == typeof(ulong) ? unchecked((ulong)input) : input;
                object actual = method.Invoke(null, new[] { argument })!;
                long result = actual switch {
                    uint v => unchecked((int)v), ulong v => unchecked((long)v), int v => v, long v => v,
                    _ => throw new InvalidOperationException("unexpected conversion result") };
                cases.Add(new { method = method.Name, width, resultWidth,
                    input = input.ToString(CultureInfo.InvariantCulture), value = result.ToString(CultureInfo.InvariantCulture) });
            }
        }
        File.WriteAllText(Path.Combine(output, "conversion-reference.json"), JsonSerializer.Serialize(cases));
    }
}
