namespace Renraku.RemoteTests
{
    // A local contract declaration, not the owner's Counter type or implementation.
    // Once bound, runtime dispatch must never execute this marker body.
    public sealed class CounterPort { public int Add(int delta) { return -999; } }
    public sealed class EchoPort { public string Echo(string text) { return "local fallback forbidden"; } }
    public sealed class FaultPort { public int Fault(int divisor) { return -999; } }
    public sealed class FieldPort { public int Value; public int Add(int delta) { return Value; } }
    public sealed class ExtraPort { public int Add(int delta) { return delta; } public int Other() { return 0; } }
    public class UnsealedPort { public int Add(int delta) { return delta; } }
    public sealed class MismatchPort { public long Add(long delta) { return delta; } }
    public sealed class WidePort { public long Wide(long value) { return -999; } }
    public sealed class FlagPort { public bool Flag(bool value) { return false; } }
    public sealed class UnitPort { public char Unit(char value) { return (char)0; } }
    public sealed class VoidPort { public void Void() { } }
    public static class Caller
    {
        public static long Wide(WidePort port, long value) { return port.Wide(value); }
        public static bool Flag(FlagPort port, bool value) { return port.Flag(value); }
        public static char Unit(UnitPort port, char value) { return port.Unit(value); }
        public static int Void(VoidPort port) { port.Void(); return 73; }
        public static int Zero() { return 123; }
        public static int Before;
        public static int After;
        public static int Run(CounterPort counter, int delta) { Before=Before+1; return Nested(counter,delta)+1; }
        private static int Nested(CounterPort counter, int delta) { int result=counter.Add(delta); After=After+1; return result; }
        public static int Repeat(CounterPort counter, int count) { int value=0; for(int i=0;i<count;i++) value=counter.Add(1); return value; }
        public static int Twice(CounterPort counter, int delta) { return counter.Add(delta)+counter.Add(delta); }
        public static CounterPort Construct() { return new CounterPort(); }
        public static string Echo(EchoPort port, string text) { return port.Echo(text); }
        public static int Fault(FaultPort port) { Before=Before+1; return port.Fault(0); }
    }
}
