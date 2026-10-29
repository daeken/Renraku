using System.Globalization;
using System.Numerics;
using System.Reflection;
using System.Text.Json;

internal static class UnsignedTextReference
{
    // Host-only prefix oracle uses arbitrary precision, not the guest's overflow guards.
    private static (bool Success, ulong Value, string Error, int Position) Parse(string? text, int width)
    {
        if (text == null) return (false, 0, "null", -1);
        if (text.Length == 0) return (false, 0, "empty", 0);
        if (text.Length > 64) return (false, 0, "too_long", 64);
        int start = text[0] == '+' ? 1 : 0;
        if (start == text.Length) return (false, 0, "invalid_digit", start);
        BigInteger magnitude = 0;
        BigInteger max = width == 32 ? uint.MaxValue : ulong.MaxValue;
        for (int i = start; i < text.Length; i++)
        {
            if (text[i] < '0' || text[i] > '9') return (false, 0, "invalid_digit", i);
            magnitude = magnitude * 10 + text[i] - '0';
            if (magnitude > max) return (false, 0, "overflow", i);
        }
        ulong value = (ulong)magnitude;
        bool native = width == 32
            ? uint.TryParse(text, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out uint u32) && u32 == value
            : ulong.TryParse(text, NumberStyles.AllowLeadingSign, CultureInfo.InvariantCulture, out ulong u64) && u64 == value;
        if (!native) throw new InvalidOperationException("CoreCLR disagrees with strict unsigned decimal oracle");
        return (true, value, "none", text.Length);
    }

    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Text.UnsignedIntegerText", throwOnError: true)!;
        var formats = new List<object>();
        var inputs = new HashSet<string?> { null, "", "+", "-", "0", "+0", "-0", "-000", "+001", "-001",
            " 1", "1 ", "\t1", "1\n", "--1", "+-1", "++1", "1-", "1+", "1.0", "1,000", "0x10", "1e2",
            "\u22121", "\uff0b1", "\u0661", "\uff11", "\ud835\udfd9", "\ud800", "\udc00", "1\ud800", "1\0", "\0", "1\0X",
            "2147483648", "4294967294", "4294967295", "4294967296", "42949672950", "4294967296X", "429496729X",
            "9223372036854775808", "18446744073709551614", "18446744073709551615", "18446744073709551616",
            "184467440737095516150", "18446744073709551616X", "1844674407370955161X", "999999999999999999999X",
            new string('0', 64), new string('0', 65), "+" + new string('0', 63), "+" + new string('0', 64),
            "-" + new string('0', 64), new string('0', 63) + "1", new string('9', 64) };
        var random = new Random(0x554e53);
        foreach (int width in new[] { 32, 64 })
        {
            var values = new List<ulong> { 0, 1, 9, 10, 99, 100, 999999999,
                width == 32 ? (ulong)int.MaxValue : (ulong)long.MaxValue,
                width == 32 ? (ulong)int.MaxValue + 1 : (ulong)long.MaxValue + 1,
                width == 32 ? uint.MaxValue - 1UL : ulong.MaxValue - 1,
                width == 32 ? uint.MaxValue : ulong.MaxValue };
            for (int i = 0; i < 128; i++)
            {
                byte[] bytes = new byte[8]; random.NextBytes(bytes);
                ulong value = BitConverter.ToUInt64(bytes);
                values.Add(width == 32 ? (uint)value : value);
            }
            var method = type.GetMethod("FormatUInt" + width)!;
            foreach (ulong value in values)
            {
                string expected = value.ToString(CultureInfo.InvariantCulture);
                string actual = (string)method.Invoke(null, new[] { width == 32 ? (object)(uint)value : value })!;
                if (actual != expected) throw new InvalidOperationException("managed unsigned formatter differs from CoreCLR");
                formats.Add(new { width, value = expected, text = expected });
                inputs.Add(expected); inputs.Add("+000" + expected);
            }
        }
        var parses = new List<object>();
        foreach (int width in new[] { 32, 64 })
        foreach (string? text in inputs)
        {
            var expected = Parse(text, width);
            object actual = type.GetMethod("ParseUInt" + width)!.Invoke(null, new object?[] { text })!;
            var result = actual.GetType();
            if ((bool)result.GetField("Success")!.GetValue(actual)! != expected.Success
                || Convert.ToUInt64(result.GetField("Value")!.GetValue(actual)) != expected.Value
                || (string)result.GetField("Error")!.GetValue(actual)! != expected.Error
                || (int)result.GetField("Position")!.GetValue(actual)! != expected.Position)
                throw new InvalidOperationException("managed unsigned parser differs from independent oracle");
            parses.Add(new { width, units = text?.Select(c => (ushort)c).ToArray(), success = expected.Success,
                value = expected.Value.ToString(CultureInfo.InvariantCulture), error = expected.Error, position = expected.Position });
        }
        File.WriteAllText(Path.Combine(output, "unsigned-text-reference.json"), JsonSerializer.Serialize(new { formats, parses }));
    }
}
