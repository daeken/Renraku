namespace Renraku.Collections
{
    public sealed class StringMapResult
    {
        public readonly bool Success;
        public readonly bool Found;
        public readonly bool Changed;
        public readonly string Value;
        public readonly string Error;
        public readonly int Count;
        public StringMapResult(bool success, bool found, bool changed, string value, string error, int count)
        { Success = success; Found = found; Changed = changed; Value = value; Error = error; Count = count; }
    }
    internal sealed class StringMapNode
    {
        internal readonly string Key;
        internal string Value;
        internal StringMapNode Next;
        internal StringMapNode(string key, string value, StringMapNode next)
        { Key = key; Value = value; Next = next; }
    }
    // The graph and all active operations are ordinary checkpointable CIL.
    // Mutators have multiple safe points; only completed methods promise Count.
    public sealed class StringMap
    {
        private StringMapNode head;
        private int count;
        public StringMap() { }
        public static int EntryLimit() { return 64; }
        public static int KeyUnitLimit() { return 128; }
        public static int ValueUnitLimit() { return 4096; }
        public int Count() { return count; }
        private string KeyError(string key)
        {
            if (key == null) return "null_key";
            if (key.Length > KeyUnitLimit()) return "key_too_long";
            return "none";
        }
        private StringMapResult Failure(string error)
        { return new StringMapResult(false, false, false, null, error, count); }
        public StringMapResult Get(string key)
        {
            string error = KeyError(key);
            if (error != "none") return Failure(error);
            StringMapNode node = head;
            while (node != null)
            {
                if (node.Key == key) return new StringMapResult(true, true, false, node.Value, "none", count);
                node = node.Next;
            }
            return Failure("missing");
        }
        public StringMapResult Set(string key, string value)
        {
            string error = KeyError(key);
            if (error != "none") return Failure(error);
            if (value != null && value.Length > ValueUnitLimit()) return Failure("value_too_long");
            StringMapNode node = head;
            while (node != null)
            {
                if (node.Key == key)
                {
                    // Equal content is a no-op and preserves the stored reference.
                    if (node.Value == value) return new StringMapResult(true, true, false, node.Value, "none", count);
                    node.Value = value;
                    return new StringMapResult(true, true, true, value, "none", count);
                }
                node = node.Next;
            }
            if (count >= EntryLimit()) return Failure("full");
            head = new StringMapNode(key, value, head);
            count = count + 1;
            return new StringMapResult(true, false, true, value, "none", count);
        }
        public StringMapResult Remove(string key)
        {
            string error = KeyError(key);
            if (error != "none") return Failure(error);
            StringMapNode previous = null;
            StringMapNode node = head;
            while (node != null)
            {
                if (node.Key == key)
                {
                    if (previous == null) head = node.Next;
                    else previous.Next = node.Next;
                    count = count - 1;
                    return new StringMapResult(true, true, true, node.Value, "none", count);
                }
                previous = node;
                node = node.Next;
            }
            return Failure("missing");
        }
    }
}
