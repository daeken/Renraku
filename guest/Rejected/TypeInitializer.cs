public static class TypeInitializer
{
    public static int Value;
    static TypeInitializer() { Value = 7; }
    public static int Run() { return Value; }
}
