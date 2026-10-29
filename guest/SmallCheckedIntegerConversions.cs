namespace Renraku.Guest
{
    public static class SmallCheckedIntegerConversions
    {
        public static int I32ToI8(int value) { return checked((sbyte)value); }
        public static int I32ToU8(int value) { return checked((byte)value); }
        public static int I32ToI16(int value) { return checked((short)value); }
        public static int I32ToU16(int value) { return checked((ushort)value); }
        public static char I32ToChar(int value) { return checked((char)value); }
        public static int U32ToI8(uint value) { return checked((sbyte)value); }
        public static int U32ToU8(uint value) { return checked((byte)value); }
        public static int U32ToI16(uint value) { return checked((short)value); }
        public static int U32ToU16(uint value) { return checked((ushort)value); }
        public static char U32ToChar(uint value) { return checked((char)value); }
        public static int I64ToI8(long value) { return checked((sbyte)value); }
        public static int I64ToU8(long value) { return checked((byte)value); }
        public static int I64ToI16(long value) { return checked((short)value); }
        public static int I64ToU16(long value) { return checked((ushort)value); }
        public static char I64ToChar(long value) { return checked((char)value); }
        public static int U64ToI8(ulong value) { return checked((sbyte)value); }
        public static int U64ToU8(ulong value) { return checked((byte)value); }
        public static int U64ToI16(ulong value) { return checked((short)value); }
        public static int U64ToU16(ulong value) { return checked((ushort)value); }
        public static char U64ToChar(ulong value) { return checked((char)value); }
        // Raw fixture bodies are patched only in the separate test PE variant.
        public static int Raw32ToI8(int value) { return (int)(value + value + value); }
        public static int Raw32ToI8Un(int value) { return (int)(value + value + value); }
        public static int Raw32ToU8(int value) { return (int)(value + value + value); }
        public static int Raw32ToU8Un(int value) { return (int)(value + value + value); }
        public static int Raw32ToI16(int value) { return (int)(value + value + value); }
        public static int Raw32ToI16Un(int value) { return (int)(value + value + value); }
        public static int Raw32ToU16(int value) { return (int)(value + value + value); }
        public static int Raw32ToU16Un(int value) { return (int)(value + value + value); }
        public static int Raw64ToI8(long value) { return (int)(value + value + value); }
        public static int Raw64ToI8Un(long value) { return (int)(value + value + value); }
        public static int Raw64ToU8(long value) { return (int)(value + value + value); }
        public static int Raw64ToU8Un(long value) { return (int)(value + value + value); }
        public static int Raw64ToI16(long value) { return (int)(value + value + value); }
        public static int Raw64ToI16Un(long value) { return (int)(value + value + value); }
        public static int Raw64ToU16(long value) { return (int)(value + value + value); }
        public static int Raw64ToU16Un(long value) { return (int)(value + value + value); }
        public static int Mutations;
        public static char SavedUnit;
        public static int StoreChar(Counter counter, long value)
        {
            counter.Add(1);
            Mutations = Mutations + 1;
            char unit = I64ToChar(value);
            SavedUnit = unit;
            return unit;
        }
        public static int MutateI32ToI8(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return I32ToI8(value); }
        public static int MutateI32ToU8(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return I32ToU8(value); }
        public static int MutateI32ToI16(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return I32ToI16(value); }
        public static int MutateI32ToU16(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return I32ToU16(value); }
        public static char MutateI32ToChar(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return I32ToChar(value); }
        public static int MutateU32ToI8(Counter counter, uint value)
        { counter.Add(1); Mutations = Mutations + 1; return U32ToI8(value); }
        public static int MutateU32ToU8(Counter counter, uint value)
        { counter.Add(1); Mutations = Mutations + 1; return U32ToU8(value); }
        public static int MutateU32ToI16(Counter counter, uint value)
        { counter.Add(1); Mutations = Mutations + 1; return U32ToI16(value); }
        public static int MutateU32ToU16(Counter counter, uint value)
        { counter.Add(1); Mutations = Mutations + 1; return U32ToU16(value); }
        public static char MutateU32ToChar(Counter counter, uint value)
        { counter.Add(1); Mutations = Mutations + 1; return U32ToChar(value); }
        public static int MutateI64ToI8(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return I64ToI8(value); }
        public static int MutateI64ToU8(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return I64ToU8(value); }
        public static int MutateI64ToI16(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return I64ToI16(value); }
        public static int MutateI64ToU16(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return I64ToU16(value); }
        public static char MutateI64ToChar(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return I64ToChar(value); }
        public static int MutateU64ToI8(Counter counter, ulong value)
        { counter.Add(1); Mutations = Mutations + 1; return U64ToI8(value); }
        public static int MutateU64ToU8(Counter counter, ulong value)
        { counter.Add(1); Mutations = Mutations + 1; return U64ToU8(value); }
        public static int MutateU64ToI16(Counter counter, ulong value)
        { counter.Add(1); Mutations = Mutations + 1; return U64ToI16(value); }
        public static int MutateU64ToU16(Counter counter, ulong value)
        { counter.Add(1); Mutations = Mutations + 1; return U64ToU16(value); }
        public static char MutateU64ToChar(Counter counter, ulong value)
        { counter.Add(1); Mutations = Mutations + 1; return U64ToChar(value); }
        public static int MutateRaw32ToI8(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToI8(value); }
        public static int MutateRaw32ToI8Un(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToI8Un(value); }
        public static int MutateRaw32ToU8(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToU8(value); }
        public static int MutateRaw32ToU8Un(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToU8Un(value); }
        public static int MutateRaw32ToI16(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToI16(value); }
        public static int MutateRaw32ToI16Un(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToI16Un(value); }
        public static int MutateRaw32ToU16(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToU16(value); }
        public static int MutateRaw32ToU16Un(Counter counter, int value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw32ToU16Un(value); }
        public static int MutateRaw64ToI8(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToI8(value); }
        public static int MutateRaw64ToI8Un(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToI8Un(value); }
        public static int MutateRaw64ToU8(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToU8(value); }
        public static int MutateRaw64ToU8Un(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToU8Un(value); }
        public static int MutateRaw64ToI16(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToI16(value); }
        public static int MutateRaw64ToI16Un(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToI16Un(value); }
        public static int MutateRaw64ToU16(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToU16(value); }
        public static int MutateRaw64ToU16Un(Counter counter, long value)
        { counter.Add(1); Mutations = Mutations + 1; return Raw64ToU16Un(value); }
    }
}
