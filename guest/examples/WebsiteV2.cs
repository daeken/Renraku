// Compile against a frozen reference view of the existing Website type. This
// caller uses its original logical identity, statics, and retained visit history.
public static class PublishedPage
{
    public static string Handle(string id, string method, string target, string body)
    {
        return string.Concat("<h2>Published v2</h2>",
            Renraku.Guest.Website.Handle(id, method, target, body));
    }
}
