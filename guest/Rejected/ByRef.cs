public static class ByRef
{
    public static void Run(ref int value) { value = value + 1; }
}
