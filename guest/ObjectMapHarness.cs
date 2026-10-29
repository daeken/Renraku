namespace Renraku.Tests
{
    using Renraku.Collections;
    public sealed class ObjectMapValue
    {
        public readonly int Label;
        public object Peer;
        public ObjectMapValue(int label) { Label = label; }
    }
    public static class ObjectMapHarness
    {
        public static ObjectMap Saved;
        public static ObjectMapResult Last;
        public static int Operations;
        public static object A;
        public static object B;
        public static object TextA;
        public static object TextB;
        public static void Initialize(string textA, string textB)
        {
            Saved = new ObjectMap(); Last = null; Operations = 0;
            ObjectMapValue a = new ObjectMapValue(1);
            ObjectMapValue b = new ObjectMapValue(2);
            a.Peer = b; b.Peer = a; A = a; B = b;
            TextA = textA; TextB = textB;
        }
        public static object Pool(int id)
        {
            switch (id) { case 0: return null; case 1: return A; case 2: return B;
                case 3: return TextA; case 4: return TextB; default: return Saved; }
        }
        public static ObjectMapResult Set(string key, object value)
        { Operations = Operations + 1; Last = Saved.Set(key, value); return Last; }
        public static ObjectMapResult Get(string key) { return Saved.Get(key); }
        public static ObjectMapResult Remove(string key)
        { Operations = Operations + 1; Last = Saved.Remove(key); return Last; }
        public static void Link(object value, object peer) { ((ObjectMapValue)value).Peer = peer; }
        public static int Count() { return Saved.Count(); }
        public static void ClearPool() { A = null; B = null; TextA = null; TextB = null; Last = null; }
        public static ObjectMapResult GetOn(ObjectMap map, string key) { return map.Get(key); }
        public static ObjectMapResult ReplaceRootAndSet(ObjectMap map, string key, object value)
        { Saved = new ObjectMap(); ClearPool(); Operations = Operations + 1; Last = map.Set(key, value); return Last; }
        public static ObjectMapResult DropRootsAndRemove(ObjectMap map, string key)
        { Saved = new ObjectMap(); ClearPool(); Operations = Operations + 1; return map.Remove(key); }
    }
}
