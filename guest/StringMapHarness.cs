using Renraku.Collections;
namespace Renraku.Tests
{
    public static class MapHarness
    {
        public static StringMap Saved;
        public static StringMapResult Last;
        public static int Operations;
        public static void Initialize() { Saved = new StringMap(); Last = null; Operations = 0; }
        public static StringMapResult Set(string key, string value)
        { Operations = Operations + 1; Last = Saved.Set(key, value); return Last; }
        public static StringMapResult Get(string key) { return Saved.Get(key); }
        public static StringMapResult Remove(string key)
        { Operations = Operations + 1; Last = Saved.Remove(key); return Last; }
        public static int Count() { return Saved.Count(); }
        public static StringMapResult GetOn(StringMap map, string key) { return map.Get(key); }
        public static StringMapResult ReplaceRootAndSet(StringMap map, string key, string value)
        { Saved = new StringMap(); Operations = Operations + 1; Last = map.Set(key, value); return Last; }
    }
}
