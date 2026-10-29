using Renraku.Collections;
using Renraku.Guest;
using Renraku.Text;

public static class ObjectWebsite
{
    public static ObjectMap State;
    public static void Initialize(Counter counter)
    {
        if (State == null) State = new ObjectMap();
        ObjectMapResult previous = State.Get("counter");
        if (!previous.Found) State.Set("counter", counter);
    }
    private static string Page(Counter counter, string message)
    {
        string totals = string.Concat("<h1>Persistent object state</h1><p>Counter: ", IntegerText.FormatInt32(counter.Value));
        totals = string.Concat(totals, string.Concat("; visits: ", IntegerText.FormatInt32(Website.Visits)));
        totals = string.Concat(totals, string.Concat("; entries: ", IntegerText.FormatInt32(State.Count())));
        string controls = string.Concat("</p><p>", string.Concat(message, "</p><button id='visit'>Record a visit</button> <button id='refresh'>Refresh</button><ol>"));
        return string.Concat(string.Concat(totals, controls), string.Concat(Website.History, "</ol>"));
    }
    public static string Handle(string id, string method, string target, string body)
    {
        if (State == null) return "<h1>Object state is not initialized</h1>";
        ObjectMapResult entry = State.Get("counter");
        Counter counter = entry.Value as Counter;
        if (!entry.Success || counter == null) return "<h1>Saved counter is missing or has the wrong type</h1>";
        if (method == "GET" && (target == "/" || target == "/page") && body == "") return Page(counter, "");
        if (method != "POST" || target != "/visit") return Page(counter, "Unsupported route.");
        int delta = 1;
        if (body != "")
        {
            Int32ParseResult parsed = IntegerText.ParseInt32(body);
            if (!parsed.Success || parsed.Value < -10 || parsed.Value > 10) return Page(counter, "Delta must be a decimal integer from -10 to 10.");
            delta = parsed.Value;
        }
        if (Website.Visits >= 64) return Page(counter, "Visit limit reached.");
        ObjectMapResult updated = State.Set("last-body", body);
        if (!updated.Success) return Page(counter, "Object map is full or invalid.");
        counter.Add(delta);
        Website.Handle(id, method, target, "");
        return Page(counter, "Visit recorded.");
    }
}
