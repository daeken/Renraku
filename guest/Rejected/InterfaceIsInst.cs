public interface IMarker { }
public static class InterfaceIsInst { public static bool Run(object value) { return value is IMarker; } }
