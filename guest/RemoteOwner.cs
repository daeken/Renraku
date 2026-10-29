namespace Renraku.RemoteOwnerTests
{
    public sealed class Subject
    {
        public int Value;
        public int Mutations;
        public Subject() { Value=19; }
        public long Wide(long value) { Mutations=Mutations+1; return value; }
        public bool Flag(bool value) { Mutations=Mutations+1; return value; }
        public char Unit(char value) { Mutations=Mutations+1; return value; }
        public void Void() { Mutations=Mutations+1; Value=Value+1; }
    }
    public static class Harness
    {
        public static Subject Saved;
        public static void Initialize() { Saved=new Subject(); }
    }
}
