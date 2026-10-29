namespace Renraku.Guest
{
    // Identical source is compiled against Renraku.CoreLib and CoreCLR.
    public static class IntegerBitwise
    {
        public static int And32(int a, int b) { return a & b; }
        public static uint AndU32(uint a, uint b) { return a & b; }
        public static long And64(long a, long b) { return a & b; }
        public static ulong AndU64(ulong a, ulong b) { return a & b; }
        public static int Or32(int a, int b) { return a | b; }
        public static uint OrU32(uint a, uint b) { return a | b; }
        public static long Or64(long a, long b) { return a | b; }
        public static ulong OrU64(ulong a, ulong b) { return a | b; }
        public static int Xor32(int a, int b) { return a ^ b; }
        public static uint XorU32(uint a, uint b) { return a ^ b; }
        public static long Xor64(long a, long b) { return a ^ b; }
        public static ulong XorU64(ulong a, ulong b) { return a ^ b; }
        public static int Not32(int value) { return ~value; }
        public static uint NotU32(uint value) { return ~value; }
        public static long Not64(long value) { return ~value; }
        public static ulong NotU64(ulong value) { return ~value; }

        public static int Next32(int value, int set, int clear, int toggle)
        { return ((value | set) & ~clear) ^ toggle; }
        public static int Mutations;
        public static int MutateFlags(Counter counter, int set, int clear, int toggle)
        {
            Mutations = Mutations + 1;
            int next = Next32(counter.Value, set, clear, toggle);
            counter.Add(next - counter.Value);
            return counter.Value;
        }
    }
}
