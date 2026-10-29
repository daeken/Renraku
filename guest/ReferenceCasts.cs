namespace Renraku.Guest
{
    public class CastBase { public int Value; public CastBase Next; public CastBase(int value) { Value = value; } }
    public class CastDerived : CastBase { public int Extra; public CastDerived(int value) : base(value) { Extra = value + 1; } }
    public class CastLeaf : CastDerived { public CastLeaf(int value) : base(value) { } }
    public class CastSibling : CastBase { public CastSibling(int value) : base(value) { } }
    public class CastOther { public int Value; public CastOther(int value) { Value = value; } }
    public static class ReferenceCasts
    {
        public static object Saved;
        public static object Alias;
        public static int Mutations;
        public static object Create(int kind)
        {
            object value;
            switch (kind) {
                case 0: value = null; break;
                case 1: value = new CastBase(10); break;
                case 2: value = new CastDerived(20); break;
                case 3: value = new CastLeaf(30); break;
                case 4: value = new CastSibling(40); break;
                case 5: value = new CastOther(50); break;
                case 6: value = new Counter(19); break;
                case 7: value = "cast\0\ud800"; break;
                default: value = new object(); break;
            }
            CastBase cycle = value as CastBase;
            if (cycle != null) cycle.Next = cycle;
            Saved = value; Alias = value; return value;
        }
        public static CastBase AsBase(object value) { return value as CastBase; }
        public static CastDerived AsDerived(object value) { return value as CastDerived; }
        public static CastLeaf AsLeaf(object value) { return value as CastLeaf; }
        public static CastOther AsOther(object value) { return value as CastOther; }
        public static Counter AsCounter(object value) { return value as Counter; }
        public static string AsString(object value) { return value as string; }
        public static CastBase CastBase(object value) { return (CastBase)value; }
        public static CastDerived CastDerived(object value) { return (CastDerived)value; }
        public static CastLeaf CastLeaf(object value) { return (CastLeaf)value; }
        public static CastOther CastOther(object value) { return (CastOther)value; }
        public static Counter CastCounter(object value) { return (Counter)value; }
        public static string CastString(object value) { return (string)value; }
        public static bool IsBase(object value) { return value is CastBase; }
        public static bool IsDerived(object value) { return value is CastDerived; }
        public static bool IsCounter(object value) { return value is Counter; }
        public static bool IsString(object value) { return value is string; }
        public static int ReadDerived(object value) { CastDerived typed = (CastDerived)value; return typed == null ? -1 : typed.Value + typed.Extra; }
        public static CastDerived ClearThenAs(object value) { Saved = null; Alias = null; return value as CastDerived; }
        public static CastDerived ClearThenCast(object value) { Saved = null; Alias = null; return (CastDerived)value; }
        public static int MutateThenCast(Counter counter, object value)
        { counter.Add(1); Mutations = Mutations + 1; CastDerived typed = (CastDerived)value; return typed == null ? -1 : typed.Value; }
    }
}
