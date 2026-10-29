namespace Renraku.Guest
{
    public enum SwitchChoice { Zero, One, Two, Three, Four, Five, Six, Seven }
    public static class IntegerSwitch
    {
        public static int Select32(int value)
        {
            switch (value) {
                case 0: return 3; case 1: return -7; case 2: return 11;
                case 3: return 19; case 4: return -23; case 5: return 31;
                case 6: return 43; case 7: return -47; default: return 2;
            }
        }
        public static int SelectOffset(int value)
        {
            switch (value) {
                case -4: return 8; case -3: return 6; case -2: return 7;
                case -1: return 5; case 0: return 3; case 1: return 0;
                case 2: return 9; case 3: return 1; default: return -1;
            }
        }
        public static int SelectUnsigned(uint value)
        {
            switch (value) {
                case 4294967288U: return 4; case 4294967289U: return 8;
                case 4294967290U: return 15; case 4294967291U: return 16;
                case 4294967292U: return 23; case 4294967293U: return 42;
                case 4294967294U: return -1; case 4294967295U: return -2;
                default: return 0;
            }
        }
        public static int SelectEnum(SwitchChoice value)
        {
            switch (value) {
                case SwitchChoice.Zero: return 10; case SwitchChoice.One: return 20;
                case SwitchChoice.Two: return 30; case SwitchChoice.Three: return 40;
                case SwitchChoice.Four: return 50; case SwitchChoice.Five: return 60;
                case SwitchChoice.Six: return 70; case SwitchChoice.Seven: return 80;
                default: return -10;
            }
        }
        public static long SelectWide(int value)
        {
            switch (value) {
                case 0: return 9223372036854775807L; case 1: return -9223372036854775808L;
                case 2: return 4294967296L; case 3: return -4294967296L;
                case 4: return 1234567890123L; case 5: return -1234567890123L;
                case 6: return 77L; case 7: return -88L; default: return 0L;
            }
        }
        // Test-only PE variants replace these bodies with naked jump tables.
        public static int RawZero(int value)
        {
            switch (value) {
                case 0: return 3; case 1: return -7; case 2: return 11;
                case 3: return 19; case 4: return -23; case 5: return 31;
                case 6: return 43; case 7: return -47; default: return 2;
            }
        }
        public static int RawDuplicate(int value)
        {
            switch (value) {
                case 0: return 3; case 1: return -7; case 2: return 11;
                case 3: return 19; case 4: return -23; case 5: return 31;
                case 6: return 43; case 7: return -47; default: return 2;
            }
        }
        public static int RawBackward(int value)
        {
            switch (value) {
                case 0: return 3; case 1: return -7; case 2: return 11;
                case 3: return 19; case 4: return -23; case 5: return 31;
                case 6: return 43; case 7: return -47; default: return 2;
            }
        }
        public static int Mutations;
        public static int MutateCounter(Counter counter, int choice)
        {
            Mutations = Mutations + 1;
            int delta = Select32(choice);
            counter.Add(delta);
            return counter.Value;
        }
    }
}
