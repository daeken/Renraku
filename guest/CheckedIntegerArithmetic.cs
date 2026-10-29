namespace Renraku.Guest
{
    public static class CheckedIntegerArithmetic
    {
        public static int Add32(int a, int b) { return checked(a + b); }
        public static uint AddU32(uint a, uint b) { return checked(a + b); }
        public static int Sub32(int a, int b) { return checked(a - b); }
        public static uint SubU32(uint a, uint b) { return checked(a - b); }
        public static int Mul32(int a, int b) { return checked(a * b); }
        public static uint MulU32(uint a, uint b) { return checked(a * b); }
        public static long Add64(long a, long b) { return checked(a + b); }
        public static ulong AddU64(ulong a, ulong b) { return checked(a + b); }
        public static long Sub64(long a, long b) { return checked(a - b); }
        public static ulong SubU64(ulong a, ulong b) { return checked(a - b); }
        public static long Mul64(long a, long b) { return checked(a * b); }
        public static ulong MulU64(ulong a, ulong b) { return checked(a * b); }
        public static int Mutations;
        public static int Apply(Counter counter, int value)
        {
            int delta = Add32(Mul32(value, 3), Sub32(11, 4));
            counter.Add(delta);
            Mutations = Mutations + 1;
            return counter.Value;
        }
        public static int MutateAdd32(Counter counter, int a, int b)
        { counter.Add(1); Mutations = Mutations + 1; return Add32(a, b); }
        public static uint MutateAddU32(Counter counter, uint a, uint b)
        { counter.Add(1); Mutations = Mutations + 1; return AddU32(a, b); }
        public static int MutateSub32(Counter counter, int a, int b)
        { counter.Add(1); Mutations = Mutations + 1; return Sub32(a, b); }
        public static uint MutateSubU32(Counter counter, uint a, uint b)
        { counter.Add(1); Mutations = Mutations + 1; return SubU32(a, b); }
        public static int MutateMul32(Counter counter, int a, int b)
        { counter.Add(1); Mutations = Mutations + 1; return Mul32(a, b); }
        public static uint MutateMulU32(Counter counter, uint a, uint b)
        { counter.Add(1); Mutations = Mutations + 1; return MulU32(a, b); }
        public static long MutateAdd64(Counter counter, long a, long b)
        { counter.Add(1); Mutations = Mutations + 1; return Add64(a, b); }
        public static ulong MutateAddU64(Counter counter, ulong a, ulong b)
        { counter.Add(1); Mutations = Mutations + 1; return AddU64(a, b); }
        public static long MutateSub64(Counter counter, long a, long b)
        { counter.Add(1); Mutations = Mutations + 1; return Sub64(a, b); }
        public static ulong MutateSubU64(Counter counter, ulong a, ulong b)
        { counter.Add(1); Mutations = Mutations + 1; return SubU64(a, b); }
        public static long MutateMul64(Counter counter, long a, long b)
        { counter.Add(1); Mutations = Mutations + 1; return Mul64(a, b); }
        public static ulong MutateMulU64(Counter counter, ulong a, ulong b)
        { counter.Add(1); Mutations = Mutations + 1; return MulU64(a, b); }
    }
}
