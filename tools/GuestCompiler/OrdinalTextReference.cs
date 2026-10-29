using System.Reflection;
using System.Text.Json;

internal static class OrdinalTextReference
{
    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Text.OrdinalText", throwOnError: true)!;
        if ((int)type.GetMethod("MaxTextUnits")!.Invoke(null, null)! != 256
            || (int)type.GetMethod("MaxNeedleUnits")!.Invoke(null, null)! != 32)
            throw new InvalidOperationException("ordinal limits changed");
        var cases = new List<object>();
        void Add(string operation, string? text, string? needle, int start)
        {
            string error = text == null ? "null_text" : needle == null ? "null_needle"
                : text.Length > 256 ? "text_too_long" : needle.Length > 32 ? "needle_too_long"
                : operation == "IndexOf" && (start < 0 || start > text.Length) ? "start_out_of_range" : "none";
            int index = -1;
            if (error == "none") {
                if (operation == "IndexOf") index = text!.IndexOf(needle!, start, StringComparison.Ordinal);
                else if (operation == "StartsWith" && text!.StartsWith(needle!, StringComparison.Ordinal)) index = 0;
                else if (operation == "EndsWith" && text!.EndsWith(needle!, StringComparison.Ordinal)) index = text.Length - needle!.Length;
            }
            object actual = type.GetMethod(operation)!.Invoke(null, operation == "IndexOf"
                ? new object?[] { text, needle, start } : new object?[] { text, needle })!;
            var result = actual.GetType();
            if ((bool)result.GetField("Success")!.GetValue(actual)! != (error == "none")
                || (bool)result.GetField("Found")!.GetValue(actual)! != (index >= 0)
                || (int)result.GetField("Index")!.GetValue(actual)! != index
                || (string)result.GetField("Error")!.GetValue(actual)! != error)
                throw new InvalidOperationException("managed ordinal search differs from CoreCLR ordinal API");
            cases.Add(new { operation, text = text?.Select(c => (ushort)c).ToArray(), needle = needle?.Select(c => (ushort)c).ToArray(),
                start, success = error == "none", found = index >= 0, index, error });
        }
        string?[] texts = { null, "", "a", "Aa", "ababa", "aaaaab", "abc\0xyz", "\0", "e\u0301", "\u00e9", "\ud800x\udc00", "\ud83d\ude00x\ud83d\ude00",
            new string('a', 256), new string('a', 257) };
        string?[] needles = { null, "", "a", "A", "aba", "aaaaab", "b", "\0", "\0x", "e", "\u0301", "\u00e9", "\ud800", "\udc00", "\ud83d\ude00", "\ude00",
            new string('a', 32), new string('a', 33) };
        foreach (string? text in texts)
        foreach (string? needle in needles) {
            foreach (string operation in new[] { "StartsWith", "EndsWith" }) Add(operation, text, needle, 0);
            foreach (int start in new[] { int.MinValue, -1, 0, 1, text?.Length ?? 0, (text?.Length ?? 0) + 1, int.MaxValue }.Distinct()) Add("IndexOf", text, needle, start);
        }
        foreach (string needle in new[] { new string('a', 31) + "b", new string('a', 32), new string('a', 33) })
            Add("IndexOf", new string('a', 256), needle, 0);
        var random = new Random(0x4f5244);
        char[] alphabet = { 'a', 'b', 'A', '\0', '\ud800', '\udc00', '\ud83d', '\ude00', '\uffff' };
        string RandomText(int maximum) => new string(Enumerable.Range(0, random.Next(maximum + 1)).Select(_ => alphabet[random.Next(alphabet.Length)]).ToArray());
        for (int i = 0; i < 256; i++) {
            string text = RandomText(32), needle = i % 2 == 0 && text.Length > 0
                ? text.Substring(random.Next(text.Length), random.Next(1, 2)) : RandomText(8);
            foreach (string operation in new[] { "IndexOf", "StartsWith", "EndsWith" }) Add(operation, text, needle, random.Next(-1, text.Length + 2));
        }
        File.WriteAllText(Path.Combine(output, "ordinal-text-reference.json"), JsonSerializer.Serialize(cases));
    }
}
