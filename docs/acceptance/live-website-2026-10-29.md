# Live website authoring and publication, October 29, 2026

GPT-6.1 Sol authored, compiled, installed, and published two successive website
handlers using the user's authorized ChatGPT subscription. The second goal kept
the first goal's persistent visit state. This is a real model-selected run,
separate from the deterministic local Responses fixtures.

The first goal requested discovery and inspection of Website, then a new handler
that added a heading and called the existing Website.Handle. Across fresh processes
with four-model-operation budgets, the model selected:

| Turn | Action | Result |
| --- | --- | --- |
| 1 | Search Website | Discovered 11 symbols |
| 2 | Read Website.Handle | Inspected 58 saved CIL instructions |
| 3 | Describe Website | Received its frozen compiler contract |
| 4 | Inspect website status | Saw explicit publication authority and no current publication |
| 5 | Describe System.String | Received the bootstrap-core-reference boundary error |
| 6 | Compile Generated.ModelWebsite | Captured a successful artifact |
| 7 | Install turn 6 | Added a new assembly without replacing Website |
| 8 | Publish Generated.ModelWebsite.Handle | Committed publication 1 |
| 9 | Finish | Reported the published handler |

The model wrote this source, retained by the installation:

```csharp
namespace Generated {
    public static class ModelWebsite {
        public static string Handle(string id, string method, string target, string body) {
            return string.Concat("<h1>Model-authored website</h1>",
                Renraku.Guest.Website.Handle(id, method, target, body));
        }
    }
}
```

A native localhost HTTP host then accepted POST `/visit` with client ID
`live-goal-one-visit`. Its page included the model's heading and that visit.
Repeating the request returned the same status, headers, body, and execution ID.
The native host exited before the second goal.

`agent-input` accepted ID `website-v2` at a zero instruction budget. Retrying the
same input returned its original pending execution. The new goal requested a
fresh handler/assembly with a v2 heading, keeping existing Website state and
inspecting the earlier publication's retained source. Fresh agent-drive processes
selected:

| Turn | Action | Result |
| --- | --- | --- |
| 10 | Inspect website status | Found publication 1 |
| 11 | Read its saved method | Received exact original source and CIL |
| 12 | Compile Generated.ModelWebsiteV2 | Captured a new artifact |
| 13 | Install turn 12 | Added the second assembly |
| 14 | Publish Generated.ModelWebsiteV2.Handle | Committed publication 2 |
| 15 | Finish | Reported preserved Website state and v2 publication |

The second generated body prepends `<h1>Model-authored website v2</h1>` and calls
the same original Website.Handle. It does not invoke Initialize or reset statics.
New HTTP requests returned the new heading and the first visit. A second visit,
`live-goal-two-visit`, retained both IDs. Replaying the first POST after the second
publication and second visit still returned its original v1 page, headers, and
execution ID. Replaying the second POST likewise returned its original v2 page.
The verification host ran with no tools on PATH.

Both goals used **15 model operations, 15 tool actions, and 2,039 coordinator
instructions**, with budget stops after turns 4, 8, and 13. The managed journal
contains 15 turns, 33 messages, and one follow-up input after turn 9. All 15 model
requests have terminal local completion receipts. A finished recovery omitted
publication authority and used zero guest instructions, provider operations, and
tool actions. The original guest type and code were retained; publication changed
the handler selected for new request IDs.

```text
universe: 8adf432fbbe54dbe9efc8e261cdd8c36
publication 1: agent.8adf432fbbe54dbe9efc8e261cdd8c36.8.publish
method 1: 888f51489ef6404384c41fb58bea0326
code 1: ac8f674093e44ec293b0e87525ac532f
publication 2: agent.8adf432fbbe54dbe9efc8e261cdd8c36.14.publish
method 2: 58f8e9c4b85b434fb9c0d0e7905598c0
code 2: f2d96aae5355481590b66b8a66280feb
first POST execution: 6f64d25b971d4a7fb93c563e1edebe5b
second POST execution: a78cf7e8254c457495e7dbe58b565a45
```

Artifacts, exact managed transcript, captured sources, reports, HTTP evidence, and
the verification script are under `target/live-website-20261029-01/`. The private
model completion ledger is outside Git under the user's Renraku data directory.
No credentials or local database files are committed. A separate demo host is
available at `http://127.0.0.1:59177/`; its port may need rebinding after restart.
Browser verification showed the v2 heading and both retained visit IDs. Screenshot:
`target/live-website-20261029-01/website-v2.png`. The original demo universe at
`target/web-demo-20261029.db` was left unchanged.

The connection used account-specific model discovery and terminal streamed
Responses through the existing OAuth adapter, following the
[official subscription inference contract](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference).
The [agent guide](../agent.md#author-and-publish-a-website) documents the explicit
publication opt-in, exact code binding, and recovery policy. Local fixtures cover
lost publication results and revoked authority; this live run did not deliberately
interrupt a submitted model request.
