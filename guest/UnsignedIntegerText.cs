namespace Renraku.Text
{
    public sealed class UInt32ParseResult
    {
        public readonly bool Success;
        public readonly uint Value;
        public readonly string Error;
        public readonly int Position;
        public UInt32ParseResult(bool success, uint value, string error, int position)
        { Success = success; Value = value; Error = error; Position = position; }
    }
    public sealed class UInt64ParseResult
    {
        public readonly bool Success;
        public readonly ulong Value;
        public readonly string Error;
        public readonly int Position;
        public UInt64ParseResult(bool success, ulong value, string error, int position)
        { Success = success; Value = value; Error = error; Position = position; }
    }
    // Separate installable module: saved signed-library and CoreLib bodies stay intact.
    public static class UnsignedIntegerText
    {
        public static int MaxParseUnits() { return 64; }
        public static string FormatUInt32(uint value) { return FormatUInt64(value); }
        public static string FormatUInt64(ulong value)
        {
            string text = "";
            do
            {
                text = string.Concat(Digit(value % 10UL), text);
                value = value / 10UL;
            } while (value != 0);
            return text;
        }
        private static string Digit(ulong digit)
        {
            if (digit == 1) return "1";
            if (digit == 2) return "2";
            if (digit == 3) return "3";
            if (digit == 4) return "4";
            if (digit == 5) return "5";
            if (digit == 6) return "6";
            if (digit == 7) return "7";
            if (digit == 8) return "8";
            if (digit == 9) return "9";
            return "0";
        }
        public static UInt32ParseResult ParseUInt32(string text)
        {
            if (text == null) return new UInt32ParseResult(false, 0, "null", -1);
            int length = text.Length;
            if (length == 0) return new UInt32ParseResult(false, 0, "empty", 0);
            int maximum = MaxParseUnits();
            if (length > maximum) return new UInt32ParseResult(false, 0, "too_long", maximum);
            int index = text[0] == '+' ? 1 : 0;
            if (index == length) return new UInt32ParseResult(false, 0, "invalid_digit", index);
            uint value = 0;
            while (index < length)
            {
                int digit = text[index] - '0';
                if (digit < 0 || digit > 9) return new UInt32ParseResult(false, 0, "invalid_digit", index);
                if (value > 429496729U || (value == 429496729U && digit > 5))
                    return new UInt32ParseResult(false, 0, "overflow", index);
                value = value * 10U + (uint)digit;
                index = index + 1;
            }
            return new UInt32ParseResult(true, value, "none", length);
        }
        public static UInt64ParseResult ParseUInt64(string text)
        {
            if (text == null) return new UInt64ParseResult(false, 0, "null", -1);
            int length = text.Length;
            if (length == 0) return new UInt64ParseResult(false, 0, "empty", 0);
            int maximum = MaxParseUnits();
            if (length > maximum) return new UInt64ParseResult(false, 0, "too_long", maximum);
            int index = text[0] == '+' ? 1 : 0;
            if (index == length) return new UInt64ParseResult(false, 0, "invalid_digit", index);
            ulong value = 0;
            while (index < length)
            {
                int digit = text[index] - '0';
                if (digit < 0 || digit > 9) return new UInt64ParseResult(false, 0, "invalid_digit", index);
                if (value > 1844674407370955161UL || (value == 1844674407370955161UL && digit > 5))
                    return new UInt64ParseResult(false, 0, "overflow", index);
                value = value * 10UL + (uint)digit;
                index = index + 1;
            }
            return new UInt64ParseResult(true, value, "none", length);
        }
    }
}
