namespace System
{
    public class Object { public Object() { } }
    public abstract class ValueType { }
    public abstract class Enum : ValueType { }
    public class Attribute { }
    public enum AttributeTargets
    {
        Assembly = 1, Module = 2, Class = 4, Struct = 8, Enum = 16,
        Constructor = 32, Method = 64, Property = 128, Field = 256,
        Event = 512, Interface = 1024, Parameter = 2048, Delegate = 4096,
        ReturnValue = 8192, GenericParameter = 16384, All = 32767
    }
    public sealed class AttributeUsageAttribute : Attribute
    {
        public AttributeUsageAttribute(AttributeTargets validOn) { }
        public bool AllowMultiple { get; set; }
        public bool Inherited { get; set; }
    }
    public abstract class Array { }
    public abstract class Delegate { }
    public sealed class String
    {
        private String() { }
        public extern int Length
        {
            [Runtime.CompilerServices.MethodImpl(Runtime.CompilerServices.MethodImplOptions.InternalCall)]
            get;
        }
        [Runtime.CompilerServices.IndexerName("Chars")]
        public extern char this[int index]
        {
            [Runtime.CompilerServices.MethodImpl(Runtime.CompilerServices.MethodImplOptions.InternalCall)]
            get;
        }
        [Runtime.CompilerServices.MethodImpl(Runtime.CompilerServices.MethodImplOptions.InternalCall)]
        public static extern string Concat(string left, string right);
        [Runtime.CompilerServices.MethodImpl(Runtime.CompilerServices.MethodImplOptions.InternalCall)]
        public static extern bool operator ==(string left, string right);
        [Runtime.CompilerServices.MethodImpl(Runtime.CompilerServices.MethodImplOptions.InternalCall)]
        public static extern bool operator !=(string left, string right);
    }
    public struct Void { }
    public struct Boolean { }
    public struct Char { }
    public struct SByte { }
    public struct Byte { }
    public struct Int16 { }
    public struct UInt16 { }
    public struct Int32 { }
    public struct UInt32 { }
    public struct Int64 { }
    public struct UInt64 { }
    public struct IntPtr { }
    public struct UIntPtr { }
    public struct Single { }
    public struct Double { }
}
namespace System.Runtime.CompilerServices
{
    public sealed class ReferenceAssemblyAttribute : System.Attribute
    {
        public ReferenceAssemblyAttribute() { }
    }
    public enum MethodImplOptions { InternalCall = 4096 }
    public sealed class MethodImplAttribute : System.Attribute
    {
        public MethodImplAttribute(MethodImplOptions options) { }
    }
    public sealed class IndexerNameAttribute : System.Attribute
    {
        public IndexerNameAttribute(string name) { }
    }
}
namespace System.Reflection
{
    public sealed class DefaultMemberAttribute : System.Attribute
    {
        public DefaultMemberAttribute(string name) { }
    }
}
namespace Renraku
{
    public static class Model
    {
        [System.Runtime.CompilerServices.MethodImpl(System.Runtime.CompilerServices.MethodImplOptions.InternalCall)]
        public static extern string Complete(string prompt);
        [System.Runtime.CompilerServices.MethodImpl(System.Runtime.CompilerServices.MethodImplOptions.InternalCall)]
        public static extern string Chat(ModelBinding binding, ChatMessage messages);
    }
    public sealed class ModelBinding
    {
        public string Capability;
        public string Model;
        public ModelBinding(string capability, string model) { Capability = capability; Model = model; }
    }
    public sealed class ChatMessage
    {
        public int Role; // 0 = system, 1 = user, 2 = assistant.
        public string Content;
        public ChatMessage Next;
        public ChatMessage(int role, string content) { Role = role; Content = content; }
    }
}
