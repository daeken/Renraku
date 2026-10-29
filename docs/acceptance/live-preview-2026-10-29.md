# Live page preview and correction, October 29, 2026

GPT-6.1 Sol used the new local preview tool to observe an existing page, author
a correction, publish it, and verify the corrected response and retained visit.
This used the user's authorized subscription and real model-selected code,
separately from the deterministic twelve-turn HTTP regression fixture.

A fresh universe began with the existing Website.Handle published as `baseline`.
The goal asked the model to start with a POST `/visit` preview, then make the
published page contain the heading `Renraku preview verified` while preserving
that visit. No generated source or fixed action sequence was supplied beyond the
requested initial preview. The model selected:

| Turn | Action | Evidence |
| --- | --- | --- |
| 1 | Preview POST `/visit` | Observed the original 231-byte page and recorded one visit |
| 2 | Search Website | Discovered its live symbols |
| 3 | Describe Website | Read the frozen compiler contract |
| 4 | Read Website.Handle | Inspected 58 saved CIL instructions |
| 5 | Inspect website status | Found the baseline and explicit publication authority |
| 6 | Compile Generated.VerifiedPreview | Captured its own 292-byte source and artifact |
| 7 | Install turn 6 | Added one new assembly |
| 8 | Publish the new handler | Committed publication 2 |
| 9 | Preview GET `/page` | Observed the required heading and original visit in a 264-byte response |
| 10 | Finish | Reported the observed correction and local-preview boundary |

The model's generated body first calls the original handler, then prepends the
requested heading:

```csharp
namespace Generated {
    public static class VerifiedPreview {
        public static string Handle(string id, string method, string target, string body) {
            string original = Renraku.Guest.Website.Handle(id, method, target, body);
            return string.Concat("<h1>Renraku preview verified</h1>", original);
        }
    }
}
```

The first assistant action was saved before tools ran. Subsequent fresh native
processes used seven-instruction, zero-provider-operation budgets to finish Freeze
and accept the preview. Its running invocation receipt then resumed seven guest
instructions in another process under the same execution ID. A SQLite backup
retains that paused checkpoint. The remaining foreground processes recovered its
frames, appended its actual response, and continued with model-operation budget
stops after turns 5 and 9. The original preview used no second inference request
or duplicate mutation during this recovery.

After turn 10 finished, an independent bounded localhost TCP host served the
corrected page. Its GET body exactly matched turn 9's managed preview, including
the single original visit. A POST using turn 1's exact client ID/method/target/body
returned the old baseline page and original execution ID after the new publication.
Repeating both HTTP requests returned their original status, headers, and body.
This separately verifies transport; the preview tool itself opened no socket and
reported `socketTested: false`.

The run used **10 model operations with 10 terminal completion receipts**, 1,423
coordinator/preview instructions, and 12 tool attempts across the small-budget
resumes. The linked managed journal has 10 turns and 22 messages. Its frozen turn
1 selects baseline publication/method/code and retains the exact request; turn 9
selects the newly published method/code. The full response strings and immutable
arguments remain rooted by their invocation receipts. A finished recovery omitted
publication authority and used zero instructions, provider operations, and tools.

```text
universe: b5d7534679a0420da25be5fdcbe37af0
first client ID: agent.b5d7534679a0420da25be5fdcbe37af0.1.preview
first execution: 8527b39208fe42f0bfed3c37e6ad15cd
original method: df9696dce35c4e56991e354cc279a576
original code: d79ae103c44a4352875f8a84302eca2c
corrected publication: agent.b5d7534679a0420da25be5fdcbe37af0.8.publish
corrected method: 10a85b66134f4011b36ebd5c76911af8
corrected code: 4701cb5e732141409303922980004cbd
```

Reports, commands, partial-process logs, paused checkpoint backup, exact managed
history, source, and independent socket evidence are retained under
`target/live-preview-20261029-01/`. The private completion ledger remains outside
Git in the user's Renraku data directory. No credentials or database files are
committed. Both earlier demo universes and their hosts were left unchanged; this
verification host exited after its four connections.

The existing subscription adapter follows the
[official completed-inference contract](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference).
The [preview guide](../agent.md#preview-and-correct-a-published-page) describes
strict inputs, frozen publication selection, output truncation, and recovery.
Local fixtures additionally cover lost results, pre-acceptance publication
conflicts, post-acceptance code pinning, failed checkpoints, GC, and terminal
fault/invalid-text responses. This run did not deliberately interrupt a submitted
model request.
