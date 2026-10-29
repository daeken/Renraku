namespace Renraku.Guest
{
    public static class IntegerConversions
    {
        public static int I64ToI32(long value) { return (int)value; }
        public static uint I64ToU32(long value) { return (uint)value; }
        public static int U64ToI32(ulong value) { return (int)value; }
        public static uint U64ToU32(ulong value) { return (uint)value; }
        public static long I32ToI64(int value) { return value; }
        public static ulong I32ToU64(int value) { return (ulong)value; }
        public static long U32ToI64(uint value) { return value; }
        public static ulong U32ToU64(uint value) { return value; }
        public static long U64ToI64(ulong value) { return (long)value; }
        public static ulong I64ToU64(long value) { return (ulong)value; }
        public static long TruncateThenSign(long value) { return (int)value; }
        public static ulong TruncateThenZero(long value) { return (uint)value; }
    }
}
