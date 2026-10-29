# Live persistent-state inspection, October 29, 2026

GPT-6.1 Sol used the new read-only tools to observe the actual saved Counter,
invoke one existing mutation, and verify the changed field on the same logical
object. It used the user's authorized subscription with a missing compiler-driver
path. No source compilation, installation, getter, or replacement object was used.

The goal asked for actual stored values before and after a single call to the
existing Simulation.Advance helper. Across fresh foreground processes with
three-model-operation budgets, the model selected:

| Turn | Action | Result |
| --- | --- | --- |
| 1 | Inspect Simulation.Saved | Obtained the persistent Counter reference |
| 2 | Inspect that object | Read Value **19** at checkpoint **18** |
| 3 | Search Simulation.Advance | Found its existing logical method |
| 4 | Read the method's CIL | Inspected its four instructions |
| 5 | Invoke Advance on the observed object with 7 | Returned 26 under one durable invocation |
| 6 | Inspect the same object | Read stored Value **26** at checkpoint **61** |
| 7 | Finish | Reported both observations and the unchanged object identity |

The value checks came from object field views, in addition to the invocation
result. The logical identities were:

```text
universe: 2c9c59ccb2cd4e368b6f7261c6ad2dfd
counter: 200aea2024c243ebb998a750cb70e90b
Value field: 96e2fd6282674c08b39432c3862773c4
Advance method: 73a70744c14e4988a8763be75512a9b4
Advance code: 6cb00a03e03d46abbd75fa2015027413
invocation: agent/2c9c59ccb2cd4e368b6f7261c6ad2dfd/5/invoke
execution: 43b92368f4e14f948cd2ff90ed56e2be
```

The run used **seven model operations with seven terminal local receipts**, seven
tool actions, and 1,021 coordinator/invocation instructions. Budget stops after
turns 3 and 6 resumed in new processes. The managed transcript contains seven
turns and 16 messages. The compiler ledger was never created; publication authority
was not enabled.

After completion, native inspect-static/inspect-object commands ran with no tools
on PATH. They returned the same root identity and stored value 26, leaving the
checkpoint generation and digest unchanged. Native invoke-once replay returned
the original execution/result without advancing the counter again. Finished agent
recovery used zero instructions, provider operations, and tool actions.

Reports, exact managed history, native views, receipt replay, and the evidence
summary are retained under `target/live-inspection-20261029-01/`. The private model
completion ledger is outside Git in the user's Renraku data directory. No local
databases or credentials are committed, and all prior demo universes/hosts were
left unchanged.

The [inspection contract](../inspection.md) describes runtime slots, bounded
UTF-16 previews, declaring owners, target retention, and historical read semantics.
Local tests additionally cover GC after a static root is replaced, re-observation
after a lost unappended read, cycles and hidden fields, exact lone surrogates,
output caps, invalid selectors, and transient rejection. An eight-turn local HTTP
fixture verifies the same path through the subscription adapter and exact managed
history prefixes. Those fixtures are scripted regression evidence; this report
records a separate real model run.
