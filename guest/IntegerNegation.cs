namespace Renraku.Guest
{
    public static class IntegerNegation
    {
        public static int Neg32(int value) { return -value; }
        public static long Neg64(long value) { return -value; }
        public static long NegUInt32(uint value) { return -value; }
        public static ulong NegAsUInt64(ulong value) { return (ulong)(-(long)value); }
        public static int Mutations;
        public static int NegateCounter(Counter counter)
        {
            Mutations = Mutations + 1;
            int next = Neg32(counter.Value);
            counter.Add(next - counter.Value);
            return counter.Value;
        }
    }
}
