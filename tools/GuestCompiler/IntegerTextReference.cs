using System.Globalization;
using System.Numerics;
using System.Reflection;
using System.Text.Json;

internal static class IntegerTextReference
{
    // Host-only independent oracle; none of these BCL operations enter guest code.
    private static (bool Success, long Value, string Error, int Position) Parse(string? text, int width)
    {
        if (text == null) return (false, 0, "null", -1);
        if (text.Length == 0) return (false, 0, "empty", 0);
        if (text.Length > 64) return (false, 0, "too_long", 64);
        int start = text[0] is '+' or '-' ? 1 : 0;
        if (start == text.Length) return (false, 0, "invalid_digit", start);
        BigInteger magnitude = 0;
        BigInteger min = width == 32 ? int.MinValue : long.MinValue;
        BigInteger max = width == 32 ? int.MaxValue : long.MaxValue;
        for (int i = start; i < text.Length; i++)
        {
            if (text[i] < '0' || text[i] > '9') return (false, 0, "invalid_digit", i);
            magnitude = magnitude * 10 + text[i] - '0';
            BigInteger signed = text[0] == '-' ? -magnitude : magnitude;
            if (signed < min || signed > max) return (false, 0, "overflow", i);
        }
        long value = (long)(text[0] == '-' ? -magnitude : magnitude);
        bool native = width == 32
            ? int.TryParse(text, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out int i32) && i32 == value
            : long.TryParse(text, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out long i64) && i64 == value;
        if (!native) throw new InvalidOperationException("CoreCLR disagrees with strict decimal oracle");
        return (true, value, "none", text.Length);
    }

    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Text.IntegerText", throwOnError: true)!;
        var formats = new List<object>();
        var inputs = new HashSet<string?> { null, "", "+", "-", "0", "+0", "-0", "-000", "+001", "-001",
            " 1", "1 ", "\t1", "1\n", "--1", "+-1", "++1", "1-", "1+", "1.0", "1,000", "0x10", "1e2",
            "\u22121", "\uff0b1", "\u0661", "\uff11", "\ud835\udfd9", "\ud800", "\udc00", "1\ud800", "1\0", "\0", "1\0X",
            "2147483648", "-2147483649", "9223372036854775808", "-9223372036854775809", "999999999999999999999X",
            new string('0', 64), new string('0', 65), "+" + new string('0', 63), "-" + new string('0', 64),
            new string('0', 63) + "1", new string('9', 64) };
        var random = new Random(0x52454e);
        foreach (int width in new[] { 32, 64 })
        {
            var values = new List<long> { 0, 1, -1, 9, -9, 10, -10, 99, -99, 100, -100,
                width == 32 ? int.MinValue : long.MinValue, width == 32 ? int.MaxValue : long.MaxValue,
                width == 32 ? int.MinValue + 1L : long.MinValue + 1, width == 32 ? int.MaxValue - 1L : long.MaxValue - 1 };
            for (int i = 0; i < 128; i++) values.Add(width == 32 ? random.NextInt64(int.MinValue, (long)int.MaxValue + 1) : random.NextInt64(long.MinValue, long.MaxValue));
            var method = type.GetMethod("FormatInt" + width)!;
            foreach (long value in values)
            {
                string expected = value.ToString(CultureInfo.InvariantCulture);
                string actual = (string)method.Invoke(null, new[] { width == 32 ? (object)(int)value : value })!;
                if (actual != expected) throw new InvalidOperationException("managed formatter differs from CoreCLR");
                formats.Add(new { width, value = expected, text = expected });
                inputs.Add(expected);
                inputs.Add(value < 0 ? "-000" + expected[1..] : "+000" + expected);
            }
        }
        var parses = new List<object>();
        foreach (int width in new[] { 32, 64 })
        foreach (string? text in inputs)
        {
            var expected = Parse(text, width);
            object actual = type.GetMethod("ParseInt" + width)!.Invoke(null, new object?[] { text })!;
            var result = actual.GetType();
            if ((bool)result.GetField("Success")!.GetValue(actual)! != expected.Success
                || Convert.ToInt64(result.GetField("Value")!.GetValue(actual)) != expected.Value
                || (string)result.GetField("Error")!.GetValue(actual)! != expected.Error
                || (int)result.GetField("Position")!.GetValue(actual)! != expected.Position)
                throw new InvalidOperationException("managed parser differs from independent oracle");
            parses.Add(new { width, units = text?.Select(c => (ushort)c).ToArray(), success = expected.Success,
                value = expected.Value.ToString(CultureInfo.InvariantCulture), error = expected.Error, position = expected.Position });
        }
        File.WriteAllText(Path.Combine(output, "integer-text-reference.json"), JsonSerializer.Serialize(new { formats, parses }));
    }
}
