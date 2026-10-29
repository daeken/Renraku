# Live multi-module managed-library acceptance, October 29, 2026

GPT-6.1 Sol discovered an existing grouped website map, inspected its state and
retained source, compiled its own C# against four frozen module facades, updated
the original map once, and published/previewed a read-only page. Native HTTP
verification and fresh-process receipt replay preserved the same map and older
responses. This is an actual model-selected subscription run, separate from the
18-turn scripted local Responses fixture.

The operator created a fresh universe/session. One bootstrap model operation
returned `finish: ready` so existing foreground commands could install the
managed decimal/map modules and GroupedWebsite, record two native HTTP visits
for group `red`, and append a durable follow-up goal at a zero instruction
budget. This reused the existing CLI/input boundary; no new setup capability,
provider path, or initialization shortcut was added. Both older demo universes
and hosts were left alone.

The main goal asked the model to observe `red = 2`, use the existing logical map
and managed libraries, advance red to 3 exactly once without changing total
visits, then publish a read-only four-string handler and verify its page.
Seven fresh `agent-drive` processes used three-model-operation budgets:

| Saved turn | Model-selected action | Observed result |
| --- | --- | --- |
| 2 | Search GroupedWebsite | Found its handler and saved map field |
| 3 | Inspect Counts static | Original StringMap reference |
| 4 | Inspect map | Original head and one-entry count |
| 5 | Inspect head node | Stored red value 2 |
| 6 | Read Handle with sourceOnly | Exact retained page source without oversized CIL |
| 7 | Describe GroupedWebsite | Primary schema plus foreign map dependency |
| 8 | Describe IntegerText | Decimal parser/formatter contracts |
| 9 | Describe Website | Original global visit contract |
| 10 | Inspect Visits | Stored total 2 |
| 11 | Read MaxParseUnits | Saved CIL and retained complete text-library source |
| 12 | Inspect web status | Existing publication and explicit authority |
| 13 | Compile its own assembly | First attempt compiled successfully against four facades |
| 14 | Install turn 13 | Append-only code/source/reference provenance |
| 15 | Invoke AdvanceRed | Original map updated to 3; terminal invocation receipt |
| 16 | Inspect original node | Stored value 3 on the same node |
| 17 | Publish its Handle | New default publication, original code preserved |
| 18 | Preview GET /page | 200, model heading, red=3 and total=2 |
| 19 | Inspect Counts static | Same original map identity |
| 20 | Inspect Visits | Still 2 |
| 21 | Finish | Reported identities and before/after observations |

The main goal used **20 model operations, 20 tool actions, and 3,126 coordinator,
invocation, and preview instructions**. Together with the bootstrap, the managed
journal has 21 turns, 45 messages, and one follow-up input. All 21 model requests
have terminal private completion receipts. Budget stops after turns 4, 7, 10,
13, 16, and 19 resumed the same managed session. No submitted model request was
deliberately interrupted or resubmitted.

The model authored this 1,212-byte source, retained by installation:

```csharp
using Renraku.Collections;
using Renraku.Text;
public static class MultiModuleVerified {
 public static string AdvanceRed() {
  if (GroupedWebsite.Counts == null) return "missing map";
  if (Renraku.Guest.Website.Visits != 2) return "unexpected total";
  StringMapResult old = GroupedWebsite.Counts.Get("red");
  if (!old.Success || old.Value != "2") return "unexpected red";
  Int32ParseResult parsed = IntegerText.ParseInt32(old.Value);
  if (!parsed.Success || parsed.Value != 2) return "invalid red";
  StringMapResult updated = GroupedWebsite.Counts.Set("red", IntegerText.FormatInt32(parsed.Value + 1));
  if (!updated.Success || !updated.Changed) return "update failed";
  return updated.Value;
 }
 public static string Handle(string id, string method, string target, string body) {
  if (method != "GET" || target != "/page" || body != "") return "<h1>Unsupported route</h1>";
  StringMapResult red = GroupedWebsite.Counts.Get("red");
  string page = string.Concat("<h1>Multi-module model verified</h1><p>red=", red.Success ? red.Value : "missing");
  page = string.Concat(page, string.Concat("; total=", IntegerText.FormatInt32(Renraku.Guest.Website.Visits)));
  return string.Concat(page, "</p>");
 }
}
```

The independent native edge served exactly:

```html
<h1>Multi-module model verified</h1><p>red=3; total=2</p>
```

After the model finished, read-only native storage commands preserved checkpoint
generation/digest and confirmed the original map, owner/type/field IDs and node,
with exactly one map object. The map's stored red value was 3 and Visits remained
2. Native invoke-once replay kept the original execution and result reference;
the guarded method did not execute again. Replaying the two pre-agent POST IDs
returned their original status, headers and bodies. A new GET and retry returned
the same model page. The bounded verification host exited afterward.

A finished recovery with zero budgets, no publication authority, a missing
compiler path and no tools on PATH used zero guest instructions, model operations
and tool actions. Private receipts remained outside Git. No new object schema,
checkpoint format, host formatter/map, native function calling, or runtime body
replacement was needed.

```text
universe: 7bd0d66bf3de45bca69378c87d8f140e
map: 24d6acaeacc74415a2efd4a562563d05
map type: fe6af1488dc6445b95ddcdfa01c2ba08
Counts field: 60b97c86a96d447f92aafb33ba802965
red node: 45748c7a837a4a6aa79f1fc7acd0551f
invoke: agent/7bd0d66bf3de45bca69378c87d8f140e/15/invoke
invoke execution: 167cd7448c1f491bb98db48e3c4556cd
invoke method/code: 94564bf181354bd191c483ecbc17a843 / 32eac4ab12614609a076e05a668d2d97
publication: agent.7bd0d66bf3de45bca69378c87d8f140e.17.publish
published method/code: 06aeef59dadd45d695869a4582dcce7f / 728cd00211354a81a87edcddad2a6ef8
```

Evidence and verification scripts are under `target/live-multimodule-20261029-01/`,
including the actual managed transcript, native storage views, retained source,
all foreground reports, HTTP responses and acceptance-summary.json. Sources and
machine-local databases are not committed. The user-authorized connection used
the existing sign-in and account-specific model catalog, following the
[official subscription inference contract](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference).
Local fixtures separately cover corrective errors and lost results; this live
run compiled its generated code on the first attempt and does not claim those
deliberate fault injections occurred during real inference.
