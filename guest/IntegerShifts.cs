namespace Renraku.Guest
{
    public static class IntegerShifts
    {
        public static int Shl32(int value, int count) { return value << count; }
        public static uint ShlU32(uint value, int count) { return value << count; }
        public static long Shl64(long value, int count) { return value << count; }
        public static ulong ShlU64(ulong value, int count) { return value << count; }
        public static int Shr32(int value, int count) { return value >> count; }
        public static uint ShrU32(uint value, int count) { return value >> count; }
        public static long Shr64(long value, int count) { return value >> count; }
        public static ulong ShrU64(ulong value, int count) { return value >> count; }
        public static int ShrZero32(int value, int count) { return value >>> count; }
        public static long ShrZero64(long value, int count) { return value >>> count; }
        public static uint Mix32(uint value, int count)
        {
            count = count & 31;
            uint rotated = (value << count) | (value >> (32 - count));
            return rotated ^ (rotated >> 7);
        }
        public static int Mutations;
        public static int MutateMix(Counter counter, int count)
        {
            Mutations = Mutations + 1;
            uint mixed = Mix32((uint)counter.Value, count);
            counter.Add((int)mixed - counter.Value);
            return counter.Value;
        }
    }
}
