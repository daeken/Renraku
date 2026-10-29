using Renraku.Collections;
using Renraku.Text;

public static class GroupedWebsite
{
    public static StringMap Counts;
    private static string Page(string message, string groupCount)
    {
        string totals = string.Concat("<h1>Persistent grouped visits</h1><p>Visits: ", IntegerText.FormatInt32(Renraku.Guest.Website.Visits));
        totals = string.Concat(totals, string.Concat("; groups: ", IntegerText.FormatInt32(Counts.Count())));
        totals = string.Concat(totals, string.Concat("; selected group: ", groupCount));
        string controls = string.Concat("</p><p>", string.Concat(message, "</p><button id='visit'>Record a visit</button> <button id='refresh'>Refresh</button><ol>"));
        return string.Concat(string.Concat(totals, controls), string.Concat(Renraku.Guest.Website.History, "</ol>"));
    }
    public static string Handle(string id, string method, string target, string body)
    {
        if (Counts == null) Counts = new StringMap();
        if (method == "GET" && (target == "/" || target == "/page") && body == "")
        {
            StringMapResult current = Counts.Get("");
            return Page("", current.Success ? current.Value : "0");
        }
        if (method != "POST" || target != "/visit") return Page("Unsupported route.", "0");
        StringMapResult previous = Counts.Get(body);
        if (!previous.Success && previous.Error != "missing") return Page("Invalid group key.", "0");
        int count = 0;
        if (previous.Success)
        {
            Int32ParseResult parsed = IntegerText.ParseInt32(previous.Value);
            if (!parsed.Success || parsed.Value < 0 || parsed.Value >= 64) return Page("Stored group count is invalid or at its limit.", "0");
            count = parsed.Value;
        }
        if (Renraku.Guest.Website.Visits >= 64) return Page("Visit limit reached.", "0");
        string next = IntegerText.FormatInt32(count + 1);
        StringMapResult updated = Counts.Set(body, next);
        if (!updated.Success) return Page("Group map is full or invalid.", "0");
        Renraku.Guest.Website.Handle(id, method, target, "");
        return Page("Visit recorded.", next);
    }
}
