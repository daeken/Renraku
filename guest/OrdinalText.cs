namespace Renraku.Text
{
    public sealed class StringMatchResult
    {
        public readonly bool Success;
        public readonly bool Found;
        public readonly int Index;
        public readonly string Error;
        public StringMatchResult(bool success, bool found, int index, string error)
        { Success = success; Found = found; Index = index; Error = error; }
    }
    public static class OrdinalText
    {
        public static int MaxTextUnits() { return 256; }
        public static int MaxNeedleUnits() { return 32; }
        private static string Validate(string text, string needle)
        {
            if (text == null) return "null_text";
            if (needle == null) return "null_needle";
            if (text.Length > MaxTextUnits()) return "text_too_long";
            if (needle.Length > MaxNeedleUnits()) return "needle_too_long";
            return null;
        }
        private static StringMatchResult Failure(string error)
        { return new StringMatchResult(false, false, -1, error); }
        private static StringMatchResult Missing()
        { return new StringMatchResult(true, false, -1, "none"); }
        private static StringMatchResult Match(int index)
        { return new StringMatchResult(true, true, index, "none"); }
        private static bool MatchAt(string text, string needle, int index, int length)
        {
            int unit = 0;
            while (unit < length)
            {
                if (text[index + unit] != needle[unit]) return false;
                unit = unit + 1;
            }
            return true;
        }
        public static StringMatchResult IndexOf(string text, string needle, int startIndex)
        {
            string error = Validate(text, needle);
            if (error != null) return Failure(error);
            int length = text.Length;
            if (startIndex < 0 || startIndex > length) return Failure("start_out_of_range");
            int needleLength = needle.Length;
            if (needleLength == 0) return Match(startIndex);
            int last = length - needleLength;
            int index = startIndex;
            while (index <= last)
            {
                if (MatchAt(text, needle, index, needleLength)) return Match(index);
                index = index + 1;
            }
            return Missing();
        }
        public static StringMatchResult StartsWith(string text, string needle)
        {
            string error = Validate(text, needle);
            if (error != null) return Failure(error);
            int length = needle.Length;
            if (length > text.Length) return Missing();
            if (MatchAt(text, needle, 0, length)) return Match(0);
            return Missing();
        }
        public static StringMatchResult EndsWith(string text, string needle)
        {
            string error = Validate(text, needle);
            if (error != null) return Failure(error);
            int length = needle.Length;
            int index = text.Length - length;
            if (index < 0) return Missing();
            if (MatchAt(text, needle, index, length)) return Match(index);
            return Missing();
        }
    }
}
