namespace Renraku.Text
{
    public sealed class Int32ParseResult
    {
        public readonly bool Success;
        public readonly int Value;
        public readonly string Error;
        public readonly int Position;
        public Int32ParseResult(bool success, int value, string error, int position)
        { Success = success; Value = value; Error = error; Position = position; }
    }
    public sealed class Int64ParseResult
    {
        public readonly bool Success;
        public readonly long Value;
        public readonly string Error;
        public readonly int Position;
        public Int64ParseResult(bool success, long value, string error, int position)
        { Success = success; Value = value; Error = error; Position = position; }
    }
    // Pure ordinary CIL: no native formatter, culture, exceptions, or byrefs.
    public static class IntegerText
    {
        public static int MaxParseUnits() { return 64; }
        public static string FormatInt32(int value) { return FormatInt64(value); }
        public static string FormatInt64(long value)
        {
            bool negative = value < 0;
            // Stay negative so the minimum signed value needs no positive magnitude.
            if (value > 0) value = 0L - value;
            string text = "";
            do
            {
                text = string.Concat(Digit(value % 10), text);
                value = value / 10;
            } while (value != 0);
            if (negative) text = string.Concat("-", text);
            return text;
        }
        private static string Digit(long negativeDigit)
        {
            if (negativeDigit == -1) return "1";
            if (negativeDigit == -2) return "2";
            if (negativeDigit == -3) return "3";
            if (negativeDigit == -4) return "4";
            if (negativeDigit == -5) return "5";
            if (negativeDigit == -6) return "6";
            if (negativeDigit == -7) return "7";
            if (negativeDigit == -8) return "8";
            if (negativeDigit == -9) return "9";
            return "0";
        }
        public static Int32ParseResult ParseInt32(string text)
        {
            if (text == null) return new Int32ParseResult(false, 0, "null", -1);
            int length = text.Length;
            if (length == 0) return new Int32ParseResult(false, 0, "empty", 0);
            int maximum = MaxParseUnits();
            if (length > maximum) return new Int32ParseResult(false, 0, "too_long", maximum);
            int index = 0;
            bool negative = text[0] == '-';
            if (negative || text[0] == '+') index = 1;
            if (index == length) return new Int32ParseResult(false, 0, "invalid_digit", index);
            int limit = negative ? -2147483648 : -2147483647;
            int multiplyLimit = limit / 10;
            int value = 0;
            while (index < length)
            {
                int digit = text[index] - '0';
                if (digit < 0 || digit > 9) return new Int32ParseResult(false, 0, "invalid_digit", index);
                if (value < multiplyLimit) return new Int32ParseResult(false, 0, "overflow", index);
                int scaled = value * 10;
                if (scaled < limit + digit) return new Int32ParseResult(false, 0, "overflow", index);
                value = scaled - digit;
                index = index + 1;
            }
            if (!negative) value = 0 - value;
            return new Int32ParseResult(true, value, "none", length);
        }
        public static Int64ParseResult ParseInt64(string text)
        {
            if (text == null) return new Int64ParseResult(false, 0, "null", -1);
            int length = text.Length;
            if (length == 0) return new Int64ParseResult(false, 0, "empty", 0);
            int maximum = MaxParseUnits();
            if (length > maximum) return new Int64ParseResult(false, 0, "too_long", maximum);
            int index = 0;
            bool negative = text[0] == '-';
            if (negative || text[0] == '+') index = 1;
            if (index == length) return new Int64ParseResult(false, 0, "invalid_digit", index);
            long limit = negative ? -9223372036854775808 : -9223372036854775807;
            long multiplyLimit = limit / 10;
            long value = 0;
            while (index < length)
            {
                int digit = text[index] - '0';
                if (digit < 0 || digit > 9) return new Int64ParseResult(false, 0, "invalid_digit", index);
                if (value < multiplyLimit) return new Int64ParseResult(false, 0, "overflow", index);
                long scaled = value * 10;
                if (scaled < limit + digit) return new Int64ParseResult(false, 0, "overflow", index);
                value = scaled - digit;
                index = index + 1;
            }
            if (!negative) value = 0L - value;
            return new Int64ParseResult(true, value, "none", length);
        }
    }
}
