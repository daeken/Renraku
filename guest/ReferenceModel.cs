// CoreCLR differential tests use the same deterministic response as local.echo.
// The Renraku guest instead references the explicit CoreLib capability intrinsic.
namespace Renraku
{
    public static class Model
    {
        public static string Complete(string prompt) { return System.String.Concat("echo: ", prompt); }
    }
}
