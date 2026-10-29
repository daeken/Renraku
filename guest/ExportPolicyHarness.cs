// Test-only declarations for the export preflight value profile.
namespace Renraku.Tests
{
    public sealed class ExportSubject
    {
        public int Value;
        public ExportSubject() { Value = 19; }
        public int Add(int delta) { Value = Value + delta; return Value; }
        public string Echo(string text) { return text; }
        public string Three(string a, string b, string c) { return string.Concat(string.Concat(a, b), c); }
        public long Wide(long value) { return value; }
        public bool Flag(bool value) { return value; }
        public char Unit(char value) { return value; }
        public object Object(object value) { return value; }
        public ExportSubject Self() { return this; }
        public int Many(int a, int b, int c, int d, int e, int f, int g, int h, int i, int j, int k, int l, int m, int n, int o, int p, int q) { return a; }
        public static int Static(int value) { return value; }
    }
    public static class ExportPolicyHarness
    {
        public static ExportSubject Saved;
        public static void Initialize() { Saved = new ExportSubject(); }
    }
}
