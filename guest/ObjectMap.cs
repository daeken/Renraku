namespace Renraku.Collections
{
    public sealed class ObjectMapResult
    {
        public readonly bool Success;
        public readonly bool Found;
        public readonly bool Changed;
        public readonly object Value;
        public readonly string Error;
        public readonly int Count;
        public ObjectMapResult(bool success, bool found, bool changed, object value, string error, int count)
        { Success = success; Found = found; Changed = changed; Value = value; Error = error; Count = count; }
    }
    internal sealed class ObjectMapNode
    {
        internal readonly string Key;
        internal object Value;
        internal ObjectMapNode Next;
        internal ObjectMapNode(string key, object value, ObjectMapNode next)
        { Key = key; Value = value; Next = next; }
    }
    // The graph and all active operations are ordinary checkpointable CIL.
    // Mutators have multiple safe points; only completed methods promise Count.
    public sealed class ObjectMap
    {
        private ObjectMapNode head;
        private int count;
        public ObjectMap() { }
        public static int EntryLimit() { return 64; }
        public static int KeyUnitLimit() { return 128; }
        public int Count() { return count; }
        private string KeyError(string key)
        {
            if (key == null) return "null_key";
            if (key.Length > KeyUnitLimit()) return "key_too_long";
            return "none";
        }
        private ObjectMapResult Failure(string error)
        { return new ObjectMapResult(false, false, false, null, error, count); }
        public ObjectMapResult Get(string key)
        {
            string error = KeyError(key);
            if (error != "none") return Failure(error);
            ObjectMapNode node = head;
            while (node != null)
            {
                if (node.Key == key) return new ObjectMapResult(true, true, false, node.Value, "none", count);
                node = node.Next;
            }
            return Failure("missing");
        }
        public ObjectMapResult Set(string key, object value)
        {
            string error = KeyError(key);
            if (error != "none") return Failure(error);
            ObjectMapNode node = head;
            while (node != null)
            {
                if (node.Key == key)
                {
                    // Object equality is reference identity, including string values.
                    if (node.Value == value) return new ObjectMapResult(true, true, false, node.Value, "none", count);
                    node.Value = value;
                    return new ObjectMapResult(true, true, true, value, "none", count);
                }
                node = node.Next;
            }
            if (count >= EntryLimit()) return Failure("full");
            head = new ObjectMapNode(key, value, head);
            count = count + 1;
            return new ObjectMapResult(true, false, true, value, "none", count);
        }
        public ObjectMapResult Remove(string key)
        {
            string error = KeyError(key);
            if (error != "none") return Failure(error);
            ObjectMapNode previous = null;
            ObjectMapNode node = head;
            while (node != null)
            {
                if (node.Key == key)
                {
                    if (previous == null) head = node.Next;
                    else previous.Next = node.Next;
                    count = count - 1;
                    return new ObjectMapResult(true, true, true, node.Value, "none", count);
                }
                previous = node;
                node = node.Next;
            }
            return Failure("missing");
        }
    }
}
