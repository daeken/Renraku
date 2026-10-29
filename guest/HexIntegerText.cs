namespace Renraku.Text
{
    // Result declarations belong to Renraku.UnsignedText, not this module.
    public static class HexIntegerText
    {
        public static int MaxParseUnits() { return 64; }
        public static string FormatUInt32(uint value) { return FormatUInt64(value); }
        public static string FormatUInt64(ulong value)
        {
            string text = "";
            do
            {
                text = string.Concat(Digit((int)(value & 15UL)), text);
                value = value >> 4;
            } while (value != 0);
            return text;
        }
        private static string Digit(int digit)
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
            if (digit == 10) return "a";
            if (digit == 11) return "b";
            if (digit == 12) return "c";
            if (digit == 13) return "d";
            if (digit == 14) return "e";
            if (digit == 15) return "f";
            return "0";
        }
        private static int Decode(int unit)
        {
            if (unit >= '0' && unit <= '9') return unit - '0';
            if (unit >= 'a' && unit <= 'f') return unit - 'a' + 10;
            if (unit >= 'A' && unit <= 'F') return unit - 'A' + 10;
            return -1;
        }
        private static int Start(string text, int length)
        {
            if (length >= 2 && text[0] == '0' && (text[1] == 'x' || text[1] == 'X')) return 2;
            return 0;
        }
        public static UInt32ParseResult ParseUInt32(string text)
        {
            if (text == null) return new UInt32ParseResult(false, 0, "null", -1);
            int length = text.Length;
            if (length == 0) return new UInt32ParseResult(false, 0, "empty", 0);
            int maximum = MaxParseUnits();
            if (length > maximum) return new UInt32ParseResult(false, 0, "too_long", maximum);
            int index = Start(text, length);
            if (index == length) return new UInt32ParseResult(false, 0, "invalid_digit", index);
            uint value = 0;
            while (index < length)
            {
                int digit = Decode(text[index]);
                if (digit < 0) return new UInt32ParseResult(false, 0, "invalid_digit", index);
                if (value > 0x0fffffffU) return new UInt32ParseResult(false, 0, "overflow", index);
                value = (value << 4) | (uint)digit;
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
            int index = Start(text, length);
            if (index == length) return new UInt64ParseResult(false, 0, "invalid_digit", index);
            ulong value = 0;
            while (index < length)
            {
                int digit = Decode(text[index]);
                if (digit < 0) return new UInt64ParseResult(false, 0, "invalid_digit", index);
                if (value > 0x0fffffffffffffffUL) return new UInt64ParseResult(false, 0, "overflow", index);
                value = (value << 4) | (uint)digit;
                index = index + 1;
            }
            return new UInt64ParseResult(true, value, "none", length);
        }
    }
}
