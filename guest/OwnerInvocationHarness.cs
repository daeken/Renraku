namespace Renraku.Tests
{
    public sealed class OwnerSubject
    {
        public int Value;
        public int Mutations;
        public string Last;
        public OwnerSubject Peer;
        public OwnerSubject() { Value = 19; Peer = this; }
        public int Run(int delta) { Mutations = Mutations + 1; return Inner(delta); }
        private int Inner(int delta) { Value = Value + delta; return Value; }
        public int Fault(int divisor) { Mutations = Mutations + 1; Value = Value + 1; return Value / divisor; }
        public int Overflow(int delta) { Mutations = Mutations + 1; Value = Value + 1; int maximum = 2147483647; return checked(maximum + delta); }
        public string Echo(string text) { Mutations = Mutations + 1; Last = text; return text; }
        public string Null() { Mutations = Mutations + 1; return null; }
        public void Void() { Mutations = Mutations + 1; Value = Value + 1; }
        public string Large() { Mutations = Mutations + 1; Value = Value + 1; string text = "xx"; for (int i = 0; i < 12; i++) text = string.Concat(text, text); return text; }
        public string Denied() { Mutations = Mutations + 1; return Renraku.Model.Complete("owner must not submit"); }
        public string IndirectDenied() { return Denied(); }
    }
    public static class OwnerInvocationHarness
    {
        public static OwnerSubject Saved;
        public static void Initialize() { Saved = new OwnerSubject(); }
        public static void Replace() { Saved = new OwnerSubject(); }
    }
}
