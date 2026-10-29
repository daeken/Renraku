namespace Renraku.Guest
{
    // Narrow conversions, not new Byte/SByte/Int16/UInt16 storage signatures.
    public static class SmallIntegerConversions
    {
        public static int I32ToI8(int value) { return (sbyte)value; }
        public static int I32ToU8(int value) { return (byte)value; }
        public static int I32ToI16(int value) { return (short)value; }
        public static int I32ToU16(int value) { return (ushort)value; }
        public static int I64ToI8(long value) { return (sbyte)value; }
        public static int I64ToU8(long value) { return (byte)value; }
        public static int I64ToI16(long value) { return (short)value; }
        public static int I64ToU16(long value) { return (ushort)value; }
        public static char I32ToChar(int value) { return (char)value; }
        public static char I64ToChar(long value) { return (char)value; }
        public static char SavedUnit;
        public static int Mutations;
        public static int StoreChar(Counter counter, long value)
        {
            Mutations = Mutations + 1;
            char unit = I64ToChar(value);
            SavedUnit = unit;
            counter.Add(1);
            return unit;
        }
    }
}
