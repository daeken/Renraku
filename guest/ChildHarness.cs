namespace Renraku.ChildTests
{
    public sealed class Node
    {
        public int Value;
        public Node Next;
        public Node(int value) { Value=value; }
    }
    public sealed class BigLayout { public int F0,F1,F2,F3,F4,F5,F6,F7,F8,F9,F10,F11,F12,F13,F14,F15,F16,F17,F18,F19,F20,F21,F22,F23,F24,F25,F26,F27,F28,F29,F30,F31,F32,F33,F34,F35,F36,F37,F38,F39,F40,F41,F42,F43,F44,F45,F46,F47,F48,F49,F50,F51,F52,F53,F54,F55,F56,F57,F58,F59,F60,F61,F62,F63,F64; public BigLayout() { } }
    public sealed class SecretThing { public SecretThing() { } }
    public static class PrivateParent
    {
        public static int Number;
        public static Node Hidden;
        public static string Text;
        private static int HiddenRead() { return Number; }
        public static int Anchor() { return HiddenRead(); }
    }
    public sealed class CounterPort { public int Add(int value) { return -999; } }
    public static class Child
    {
        public static Node Shared;
        public static Node Kept;
        public static int Runs;
        public static Renraku.ModelBinding Binding;
        public static Renraku.ChatMessage Head;
        public static void Seed()
        {
            PrivateParent.Number=8675309;
            PrivateParent.Text="parent-private-text";
            PrivateParent.Hidden=new Node(77);
            Shared=new Node(19); Shared.Next=PrivateParent.Hidden; PrivateParent.Hidden.Next=Shared;
            Binding=new Renraku.ModelBinding("local.echo","echo-v1");
            Head=new Renraku.ChatMessage(1,PrivateParent.Text);
        }
        public static int Use(Node node,int delta) { Runs=Runs+1; node.Value=node.Value+delta; return node.Value; }
        public static Node Leak(Node node) { Runs=Runs+1; return node.Next; }
        public static Node Identity(Node node) { return node; }
        public static Node Create(int value) { Node node=new Node(value); node.Next=node; Kept=node; return node; }
        public static int Allocate(int count) { for(int i=0;i<count;i++)new Node(i); return count; }
        public static int Drop() { Kept=null; return 0; }
        public static int AttackStatic(int value) { return PrivateParent.Number+value; }
        public static int AttackCall(int value) { return PrivateParent.Anchor()+value; }
        public static object AttackNew(int value) { return new SecretThing(); }
        public static object AttackCast(object value) { return value as SecretThing; }
        public static int Dead(bool condition) { if(condition)return PrivateParent.Number; return 0; }
        public static string Echo() { return Renraku.Model.Complete("child-echo"); }
        public static string Chat(Renraku.ModelBinding binding,Renraku.ChatMessage head) { return Renraku.Model.Chat(binding,head); }
        public static int Remote(CounterPort counter,int delta) { Runs=Runs+1; return counter.Add(delta)+1; }
        public static int RawCall(int value) { return ((value+1)*3+7)/2; }
        public static int RawField(int value) { return ((value+1)*3+7)/2; }
        public static object RawCast(object value,int n) { for(int i=0;i<n;i++)n=n^i; return value; }
        public static object RawNew(int n) { if(n>5)return new SecretThing(); return null; }
    }
}
