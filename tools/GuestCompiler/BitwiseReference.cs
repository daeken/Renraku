using System.Globalization;
using System.Reflection;
using System.Text.Json;

internal static class BitwiseReference
{
    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Guest.IntegerBitwise", throwOnError: true)!;
        var cases = new List<object>();
        var random = new Random(0x424954);
        foreach (int width in new[] { 32, 64 })
        {
            var values = new List<long> { 0, 1, -1, 2, -2, 0x55555555, unchecked((int)0xaaaaaaaa),
                width == 32 ? int.MinValue : long.MinValue, width == 32 ? int.MinValue + 1L : long.MinValue + 1,
                width == 32 ? int.MaxValue : long.MaxValue, width == 32 ? int.MaxValue - 1L : long.MaxValue - 1,
                width == 32 ? 0x33333333 : 0x5555555555555555, width == 32 ? unchecked((int)0xcccccccc) : unchecked((long)0xaaaaaaaaaaaaaaaa) };
            foreach (bool unsigned in new[] { false, true })
            foreach (string operation in new[] { "And", "Or", "Xor", "Not" })
            {
                var method = type.GetMethod(operation + (unsigned ? "U" : "") + width)!;
                object Box(long value) => method.GetParameters()[0].ParameterType switch {
                    Type t when t == typeof(int) => (object)unchecked((int)value),
                    Type t when t == typeof(uint) => unchecked((uint)value),
                    Type t when t == typeof(ulong) => unchecked((ulong)value), _ => value };
                void Add(long a, long b)
                {
                    long left = width == 32 ? unchecked((int)a) : a;
                    long right = width == 32 ? unchecked((int)b) : b;
                    object actual = method.Invoke(null, operation == "Not" ? new[] { Box(left) } : new[] { Box(left), Box(right) })!;
                    long result = actual switch { int v => v, uint v => unchecked((int)v), long v => v,
                        ulong v => unchecked((long)v), _ => throw new InvalidOperationException("unexpected bitwise result") };
                    cases.Add(new { method = method.Name, width, left = left.ToString(CultureInfo.InvariantCulture),
                        right = operation == "Not" ? null : right.ToString(CultureInfo.InvariantCulture),
                        value = result.ToString(CultureInfo.InvariantCulture) });
                }
                foreach (long a in values)
                    if (operation == "Not") Add(a, 0); else foreach (long b in values) Add(a, b);
                for (int i = 0; i < 128; i++) {
                    byte[] a = new byte[8], b = new byte[8]; random.NextBytes(a); random.NextBytes(b);
                    Add(BitConverter.ToInt64(a), BitConverter.ToInt64(b));
                }
            }
        }
        File.WriteAllText(Path.Combine(output, "bitwise-reference.json"), JsonSerializer.Serialize(cases));
    }
}
