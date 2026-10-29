public class Holder<T> { }
public static class GenericIsInst { public static bool Run(object value) { return value is Holder<int>; } }
