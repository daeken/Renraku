namespace Renraku.Guest
{
    // Compiled unchanged against both Renraku.CoreLib and CoreCLR.
    public static class IntegerArithmetic
    {
        public static int Div32(int a, int b) { return a / b; }
        public static uint DivU32(uint a, uint b) { return a / b; }
        public static int Rem32(int a, int b) { return a % b; }
        public static uint RemU32(uint a, uint b) { return a % b; }
        public static long Div64(long a, long b) { return a / b; }
        public static ulong DivU64(ulong a, ulong b) { return a / b; }
        public static long Rem64(long a, long b) { return a % b; }
        public static ulong RemU64(ulong a, ulong b) { return a % b; }

        public static int Gcd32(int a, int b)
        {
            while (b != 0) { int next = Rem32(a, b); a = b; b = next; }
            return a;
        }
        public static int DecimalDigits64(long value)
        {
            int digits = 1;
            while (value <= -10 || value >= 10) { value = Div64(value, 10); digits = digits + 1; }
            return digits;
        }
        public static Counter ArithmeticState;
        public static int PersistentArithmetic()
        {
            Counter counter = new Counter(19);
            counter.Next = counter;
            ArithmeticState = counter;
            counter.Add(Gcd32(1071, 462));
            counter.Add(DecimalDigits64(-9223372036854775808));
            return counter.Value;
        }
        public static int MutateDiv32(Counter counter, int a, int b)
        { counter.Add(1); return Div32(a, b); }
        public static int MutateRem32(Counter counter, int a, int b)
        { counter.Add(1); return Rem32(a, b); }
        public static long MutateDiv64(Counter counter, long a, long b)
        { counter.Add(1); return Div64(a, b); }
        public static long MutateRem64(Counter counter, long a, long b)
        { counter.Add(1); return Rem64(a, b); }
    }
}
