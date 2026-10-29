namespace Renraku.Guest
{
    public static class CheckedIntegerConversions
    {
        public static int I32ToI32(int value) { return checked((int)value); }
        public static uint I32ToU32(int value) { return checked((uint)value); }
        public static long I32ToI64(int value) { return checked((long)value); }
        public static ulong I32ToU64(int value) { return checked((ulong)value); }
        public static int U32ToI32(uint value) { return checked((int)value); }
        public static uint U32ToU32(uint value) { return checked((uint)value); }
        public static long U32ToI64(uint value) { return checked((long)value); }
        public static ulong U32ToU64(uint value) { return checked((ulong)value); }
        public static int I64ToI32(long value) { return checked((int)value); }
        public static uint I64ToU32(long value) { return checked((uint)value); }
        public static long I64ToI64(long value) { return checked((long)value); }
        public static ulong I64ToU64(long value) { return checked((ulong)value); }
        public static int U64ToI32(ulong value) { return checked((int)value); }
        public static uint U64ToU32(ulong value) { return checked((uint)value); }
        public static long U64ToI64(ulong value) { return checked((long)value); }
        public static ulong U64ToU64(ulong value) { return checked((ulong)value); }
        // Test-only PE variants install each exact checked opcode, including redundant conversions.
        public static int Raw32ToI32(int value) { return (int)(value + value + value); }
        public static int Raw32ToI32Un(int value) { return (int)(value + value + value); }
        public static uint Raw32ToU32(int value) { return (uint)(value + value + value); }
        public static uint Raw32ToU32Un(int value) { return (uint)(value + value + value); }
        public static long Raw32ToI64(int value) { return (long)(value + value + value); }
        public static long Raw32ToI64Un(int value) { return (long)(value + value + value); }
        public static ulong Raw32ToU64(int value) { return (ulong)(value + value + value); }
        public static ulong Raw32ToU64Un(int value) { return (ulong)(value + value + value); }
        public static int Raw64ToI32(long value) { return (int)(value + value + value); }
        public static int Raw64ToI32Un(long value) { return (int)(value + value + value); }
        public static uint Raw64ToU32(long value) { return (uint)(value + value + value); }
        public static uint Raw64ToU32Un(long value) { return (uint)(value + value + value); }
        public static long Raw64ToI64(long value) { return (long)(value + value + value); }
        public static long Raw64ToI64Un(long value) { return (long)(value + value + value); }
        public static ulong Raw64ToU64(long value) { return (ulong)(value + value + value); }
        public static ulong Raw64ToU64Un(long value) { return (ulong)(value + value + value); }
        public static int Mutations;
        public static int Apply(Counter counter, long value)
        {
            int narrow = I64ToI32(value);
            uint positive = I32ToU32(narrow);
            long wide = U32ToI64(positive);
            counter.Add(I64ToI32(wide));
            Mutations = Mutations + 1;
            return counter.Value;
        }
        public static int MutateI32ToI32(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return I32ToI32(value); }
        public static uint MutateI32ToU32(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return I32ToU32(value); }
        public static long MutateI32ToI64(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return I32ToI64(value); }
        public static ulong MutateI32ToU64(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return I32ToU64(value); }
        public static int MutateU32ToI32(Counter counter, uint value)
        { counter.Add(1); Mutations = Mutations + 1; return U32ToI32(value); }
        public static uint MutateU32ToU32(Counter counter, uint value)
        { counter.Add(1); Mutations = Mutations + 1; return U32ToU32(value); }
        public static long MutateU32ToI64(Counter counter, uint value)
        { counter.Add(1); Mutations = Mutations + 1; return U32ToI64(value); }
        public static ulong MutateU32ToU64(Counter counter, uint value)
        { counter.Add(1); Mutations = Mutations + 1; return U32ToU64(value); }
        public static int MutateI64ToI32(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return I64ToI32(value); }
        public static uint MutateI64ToU32(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return I64ToU32(value); }
        public static long MutateI64ToI64(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return I64ToI64(value); }
        public static ulong MutateI64ToU64(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return I64ToU64(value); }
        public static int MutateU64ToI32(Counter counter, ulong value)
        { counter.Add(1); Mutations = Mutations + 1; return U64ToI32(value); }
        public static uint MutateU64ToU32(Counter counter, ulong value)
        { counter.Add(1); Mutations = Mutations + 1; return U64ToU32(value); }
        public static long MutateU64ToI64(Counter counter, ulong value)
        { counter.Add(1); Mutations = Mutations + 1; return U64ToI64(value); }
        public static ulong MutateU64ToU64(Counter counter, ulong value)
        { counter.Add(1); Mutations = Mutations + 1; return U64ToU64(value); }
        public static int MutateRaw32ToI32(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToI32(value); }
        public static int MutateRaw32ToI32Un(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToI32Un(value); }
        public static uint MutateRaw32ToU32(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToU32(value); }
        public static uint MutateRaw32ToU32Un(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToU32Un(value); }
        public static long MutateRaw32ToI64(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToI64(value); }
        public static long MutateRaw32ToI64Un(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToI64Un(value); }
        public static ulong MutateRaw32ToU64(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToU64(value); }
        public static ulong MutateRaw32ToU64Un(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToU64Un(value); }
        public static int MutateRaw64ToI32(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToI32(value); }
        public static int MutateRaw64ToI32Un(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToI32Un(value); }
        public static uint MutateRaw64ToU32(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToU32(value); }
        public static uint MutateRaw64ToU32Un(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToU32Un(value); }
        public static long MutateRaw64ToI64(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToI64(value); }
        public static long MutateRaw64ToI64Un(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToI64Un(value); }
        public static ulong MutateRaw64ToU64(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToU64(value); }
        public static ulong MutateRaw64ToU64Un(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToU64Un(value); }
    }
}
