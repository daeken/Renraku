using Renraku.Text;

// Compile against the frozen Website view and the frozen IntegerText library view.
public static class IntegerWebsite
{
    private static string Page(string message)
    {
        string heading = string.Concat("<h1>Persistent numbered visits</h1><p>Visits: ", IntegerText.FormatInt32(Renraku.Guest.Website.Visits));
        string controls = string.Concat("</p><p>", string.Concat(message, "</p><button id='visit'>Record a visit</button> <button id='refresh'>Refresh</button><ol>"));
        return string.Concat(string.Concat(heading, controls), string.Concat(Renraku.Guest.Website.History, "</ol>"));
    }
    public static string Handle(string id, string method, string target, string body)
    {
        if (method == "GET" && (target == "/" || target == "/page") && body == "") return Page("");
        if (method != "POST" || target != "/visit") return Page("Unsupported route.");
        // An optional decimal body is the expected current count. It provides
        // a managed precondition while keeping one accepted request/one visit.
        if (body != "")
        {
            Int32ParseResult expected = IntegerText.ParseInt32(body);
            if (!expected.Success || expected.Value < 0 || expected.Value > 64) return Page("Expected count must be an ASCII integer from 0 to 64.");
            if (expected.Value != Renraku.Guest.Website.Visits) return Page("Count changed; refresh before recording a visit.");
        }
        if (Renraku.Guest.Website.Visits >= 64) return Page("Visit limit reached.");
        Renraku.Guest.Website.Handle(id, method, target, "");
        return Page("Visit recorded.");
    }
}
