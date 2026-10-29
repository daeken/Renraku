using System.Globalization;
using System.Numerics;
using System.Reflection;
using System.Text.Json;

internal static class CheckedArithmeticReference
{
    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Guest.CheckedIntegerArithmetic", throwOnError: true)!;
        var random = new Random(0x43484b);
        var cases = new List<object>();
        foreach (int width in new[] { 32, 64 }) {
            long min = width == 32 ? int.MinValue : long.MinValue;
            long max = width == 32 ? int.MaxValue : long.MaxValue;
            long[] values = { 0,1,-1,2,-2,3,-3,min,min+1,max,max-1,
                width == 32 ? 65535 : 4294967295, width == 32 ? 65536 : 4294967296,
                width == 32 ? 46340 : 3037000499, width == 32 ? 46341 : 3037000500,
                width == 32 ? 0x55555555 : 0x5555555555555555 };
            foreach (string op in new[] { "Add", "Sub", "Mul" })
            foreach (bool unsigned in new[] { false, true }) {
                string name = op + (unsigned ? "U" : "") + width;
                var method = type.GetMethod(name)!;
                void Add(long a, long b) {
                    long left = width == 32 ? unchecked((int)a) : a;
                    long right = width == 32 ? unchecked((int)b) : b;
                    object Box(long bits) => method.GetParameters()[0].ParameterType switch {
                        Type t when t == typeof(int) => (object)unchecked((int)bits),
                        Type t when t == typeof(uint) => unchecked((uint)bits),
                        Type t when t == typeof(ulong) => unchecked((ulong)bits), _ => bits };
                    BigInteger Number(long bits) => unsigned
                        ? width == 32 ? new BigInteger(unchecked((uint)bits)) : new BigInteger(unchecked((ulong)bits))
                        : new BigInteger(bits);
                    BigInteger x = Number(left), y = Number(right);
                    BigInteger exact = op == "Add" ? x + y : op == "Sub" ? x - y : x * y;
                    BigInteger low = unsigned ? BigInteger.Zero : -(BigInteger.One << (width - 1));
                    BigInteger high = (BigInteger.One << (unsigned ? width : width - 1)) - 1;
                    bool overflow = exact < low || exact > high;
                    string? value = null, fault = null;
                    try {
                        object actual = method.Invoke(null, new[] { Box(left), Box(right) })!;
                        long bits = actual switch { int v => v, uint v => unchecked((int)v),
                            ulong v => unchecked((long)v), long v => v, _ => throw new InvalidOperationException("unexpected result") };
                        value = bits.ToString(CultureInfo.InvariantCulture);
                        BigInteger mathematical = Number(bits);
                        if (overflow || mathematical != exact) throw new InvalidOperationException("CoreCLR and BigInteger disagree");
                    } catch (TargetInvocationException error) {
                        fault = error.InnerException!.GetType().FullName;
                        if (!overflow || fault != "System.OverflowException") throw;
                    }
                    cases.Add(new { method = name, width, left = left.ToString(CultureInfo.InvariantCulture),
                        right = right.ToString(CultureInfo.InvariantCulture), value, fault });
                }
                foreach (long a in values) foreach (long b in values) Add(a,b);
                for (int i = 0; i < 128; i++) {
                    byte[] a = new byte[8], b = new byte[8]; random.NextBytes(a); random.NextBytes(b);
                    Add(BitConverter.ToInt64(a),BitConverter.ToInt64(b));
                }
            }
        }
        File.WriteAllText(Path.Combine(output, "checked-arithmetic-reference.json"), JsonSerializer.Serialize(cases));
    }
}
