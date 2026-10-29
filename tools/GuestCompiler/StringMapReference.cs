using System.Reflection;
using System.Text.Json;

internal static class StringMapReference
{
    public static void Write(Assembly assembly, string output)
    {
        var type = assembly.GetType("Renraku.Collections.StringMap", throwOnError: true)!;
        object map = Activator.CreateInstance(type)!;
        var dictionary = new Dictionary<string, string?>(StringComparer.Ordinal);
        var trace = new List<object>();
        void Apply(string operation, string? key, string? value = null)
        {
            bool success = false, found = false, changed = false;
            string? observed = null;
            string error = "none";
            if (key == null) error = "null_key";
            else if (key.Length > 128) error = "key_too_long";
            else if (operation == "Set" && value?.Length > 4096) error = "value_too_long";
            else
            {
                found = dictionary.TryGetValue(key, out string? previous);
                if (operation == "Get" || operation == "Remove")
                {
                    if (!found) error = "missing";
                    else { success = true; observed = previous; if (operation == "Remove") { dictionary.Remove(key); changed = true; } }
                }
                else if (!found && dictionary.Count == 64) error = "full";
                else
                {
                    success = true;
                    changed = !found || !StringComparer.Ordinal.Equals(previous, value);
                    if (changed) dictionary[key] = value;
                    observed = dictionary[key];
                }
            }
            object result = type.GetMethod(operation)!.Invoke(map, operation == "Set" ? new object?[] { key, value } : new object?[] { key })!;
            var resultType = result.GetType();
            object? Field(string name) => resultType.GetField(name)!.GetValue(result);
            if ((bool)Field("Success")! != success || (bool)Field("Found")! != found || (bool)Field("Changed")! != changed
                || !StringComparer.Ordinal.Equals((string?)Field("Value"), observed) || (string)Field("Error")! != error
                || (int)Field("Count")! != dictionary.Count || (int)type.GetMethod("Count")!.Invoke(map, null)! != dictionary.Count)
                throw new InvalidOperationException($"managed string map disagrees with ordinal dictionary: {operation}");
            trace.Add(new { operation, key = key?.Select(c => (ushort)c).ToArray(), value = value?.Select(c => (ushort)c).ToArray(),
                success, found, changed, observed = observed?.Select(c => (ushort)c).ToArray(), error, count = dictionary.Count });
        }
        Apply("Get", null); Apply("Set", null, new string('x', 4097)); Apply("Remove", null);
        Apply("Get", ""); Apply("Set", "", null); Apply("Get", ""); Apply("Set", "", null);
        foreach (string key in new[] { "a", "A", "I", "\u0131", "\u00e9", "e\u0301", "\ud800", "\udc00", "\ud83d\ude00", "\0", "a\0b", new string('k', 128) })
        { Apply("Set", key, "shared\ud800"); Apply("Get", key); Apply("Set", key, new string("shared\ud800".ToCharArray())); }
        Apply("Set", "a", "replacement"); Apply("Set", "a", null); Apply("Get", "a");
        Apply("Set", new string('k', 129), "bad"); Apply("Get", new string('k', 129)); Apply("Remove", new string('k', 129));
        Apply("Set", "large", new string('v', 4096)); Apply("Set", "large", new string('v', 4097));
        foreach (string key in dictionary.Keys.ToArray()) Apply("Remove", key);
        Apply("Remove", "absent");
        for (int i = 0; i < 64; i++) Apply("Set", "k" + i, i % 4 == 0 ? null : "v" + i);
        Apply("Set", "full", "blocked"); Apply("Set", "k0", "updated-at-capacity"); Apply("Set", "k0", "updated-at-capacity");
        foreach (string key in new[] { "k63", "k31", "k0" }) Apply("Remove", key);
        foreach (string key in new[] { "new-head", "new-middle", "new-tail" }) Apply("Set", key, "capacity-reused");
        var random = new Random(0x4d4150);
        for (int i = 0; i < 256; i++)
        { string operation = new[] { "Get", "Set", "Remove" }[random.Next(3)]; Apply(operation, "k" + random.Next(80), i % 7 == 0 ? null : "value" + random.Next(100)); }
        foreach (string key in dictionary.Keys.ToArray()) Apply("Get", key);
        File.WriteAllText(Path.Combine(output, "string-map-reference.json"), JsonSerializer.Serialize(trace));
    }
}
