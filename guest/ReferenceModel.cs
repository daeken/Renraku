// CoreCLR differential tests use the same deterministic response as local.echo.
// The Renraku guest instead references the explicit CoreLib capability intrinsic.
namespace Renraku
{
    public static class Model
    {
        public static string Complete(string prompt) { return System.String.Concat("echo: ", prompt); }
        public static string Chat(ModelBinding binding, ChatMessage messages)
        {
            while (messages.Next != null) messages = messages.Next;
            return System.String.Concat("echo: ", messages.Content);
        }
    }
    public sealed class ModelBinding
    {
        public string Capability;
        public string Model;
        public ModelBinding(string capability, string model) { Capability = capability; Model = model; }
    }
    public sealed class ChatMessage
    {
        public int Role;
        public string Content;
        public ChatMessage Next;
        public ChatMessage(int role, string content) { Role = role; Content = content; }
    }
}
