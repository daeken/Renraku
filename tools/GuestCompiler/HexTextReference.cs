using System.Globalization;
using System.Numerics;
using System.Reflection;
using System.Text.Json;

internal static class HexTextReference
{
    private static (bool Success, ulong Value, string Error, int Position) Parse(string? text, int width)
    {
        if (text == null) return (false, 0, "null", -1);
        if (text.Length == 0) return (false, 0, "empty", 0);
        if (text.Length > 64) return (false, 0, "too_long", 64);
        int start = text.StartsWith("0x", StringComparison.OrdinalIgnoreCase) ? 2 : 0;
        if (start == text.Length) return (false, 0, "invalid_digit", start);
        BigInteger magnitude = 0;
        BigInteger max = width == 32 ? uint.MaxValue : ulong.MaxValue;
        for (int i = start; i < text.Length; i++)
        {
            int digit = "0123456789abcdef".IndexOf(char.ToLowerInvariant(text[i]));
            // Limit acceptance to ASCII, even if a culture mapping ever expands.
            if (text[i] > 127 || digit < 0) return (false, 0, "invalid_digit", i);
            magnitude = magnitude * 16 + digit;
            if (magnitude > max) return (false, 0, "overflow", i);
        }
        ulong value = (ulong)magnitude;
        string digits = text[start..];
        bool native = width == 32
            ? uint.TryParse(digits, NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture, out uint u32) && u32 == value
            : ulong.TryParse(digits, NumberStyles.AllowHexSpecifier, CultureInfo.InvariantCulture, out ulong u64) && u64 == value;
        if (!native) throw new InvalidOperationException("CoreCLR differs from strict hex prefix oracle");
        return (true, value, "none", text.Length);
    }
    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Text.HexIntegerText", throwOnError: true)!;
        var formats = new List<object>();
        var inputs = new HashSet<string?> { null, "", "0", "00", "a", "F", "aBcDeF", "0x", "0X", "0x0", "0Xf", "0xg", "0xx1",
            "+0", "-0", "+1", "-1", "0x-1", "0x+1", " 1", "1 ", "\t1", "1\n", "1.0", "1,000", "0b11", "1e2",
            "\u22121", "\uff0b1", "\u0661", "\uff11", "\uff21", "\ud835\udfd9", "\ud800", "\udc00", "1\ud800", "1\0", "\0", "0x\0X",
            "7fffffff", "80000000", "ffffffff", "100000000", "ffffffff0", "100000000X", "ffffffffX",
            "7fffffffffffffff", "8000000000000000", "ffffffffffffffff", "10000000000000000", "ffffffffffffffff0",
            "10000000000000000X", "ffffffffffffffffX", new string('0', 64), new string('0', 65), "0x" + new string('0', 62),
            "0X" + new string('0', 63), "!" + new string('0', 64), new string('0', 63) + "f", new string('f', 64) };
        var random = new Random(0x484558);
        foreach (int width in new[] { 32, 64 })
        {
            var values = new List<ulong> { 0, 1, 9, 10, 15, 16, 255, 256,
                width == 32 ? (ulong)int.MaxValue : (ulong)long.MaxValue,
                width == 32 ? (ulong)int.MaxValue + 1 : (ulong)long.MaxValue + 1,
                width == 32 ? uint.MaxValue - 1UL : ulong.MaxValue - 1, width == 32 ? uint.MaxValue : ulong.MaxValue };
            for (int i = 0; i < 128; i++) { byte[] bytes = new byte[8]; random.NextBytes(bytes); ulong value = BitConverter.ToUInt64(bytes); values.Add(width == 32 ? (uint)value : value); }
            foreach (ulong value in values)
            {
                string expected = value.ToString("x", CultureInfo.InvariantCulture);
                string actual = (string)type.GetMethod("FormatUInt" + width)!.Invoke(null, new[] { width == 32 ? (object)(uint)value : value })!;
                if (actual != expected) throw new InvalidOperationException("managed hex formatter differs from CoreCLR");
                formats.Add(new { width, value = value.ToString(CultureInfo.InvariantCulture), text = expected });
                inputs.Add(expected); inputs.Add("0x000" + expected); inputs.Add("0X" + expected.ToUpperInvariant());
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
                throw new InvalidOperationException("managed hex parser differs from BigInteger prefix oracle");
            parses.Add(new { width, units = text?.Select(c => (ushort)c).ToArray(), success = expected.Success,
                value = expected.Value.ToString(CultureInfo.InvariantCulture), error = expected.Error, position = expected.Position });
        }
        File.WriteAllText(Path.Combine(output, "hex-text-reference.json"), JsonSerializer.Serialize(new { formats, parses }));
    }
}
