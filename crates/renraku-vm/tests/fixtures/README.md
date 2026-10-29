# Legacy checkpoint fixtures

`checkpoint-v1.bin` was captured from the format 1 runtime before managed strings
were added. It contains the original CoreLib and simulation assemblies, saved
identity bindings, one Counter object, and three active frames after instruction
30 of Simulation.Run. Resuming it must return 48 after 155 total instructions.

`checkpoint-v2.bin` was captured from the format 2 runtime before effect records
were added. It contains the earlier CoreLib and simulation assemblies and a
paused Simulation.Strings execution after 80 instructions. Resuming it must
return 167861, preserving literal identities and exact UTF-16 string payloads.

`checkpoint-v3.bin` was captured from the format 3 runtime before waiting CIL
continuations were added. It has three active Simulation.Run frames at
instruction 30 and a completed host effect for `legacy`, with the managed
response `echo: legacy`. The execution must finish with 48 at instruction 155
and preserve the completed effect and its object IDs.

`checkpoint-v4.bin` contains a waiting ModelHarness call at instruction 44 with
an in-flight effect. `local-provider-v1.db` is the matching original schema 1
receipt ledger, captured after provider acceptance and before the response was
committed to the universe. Upgrading both must reconcile the first response
without another submission, then complete the old guest with result 417.

`checkpoint-v5.bin` was captured from commit 3ef5207's format 5 writer before
invocation receipts were added. It contains ChatHarnessScore waiting at
instruction 54 with a prepared structured model request. Local delivery and
resumption must return 11, retaining its identities and captured history. Its
SHA-256 is `ef1d346a345b2799ea79fa7b016876abdd0aa740815ed61e42f8ac8e6530871f`.

All fixtures must stay unchanged. They test reading earlier formats and
upgrading on the next successful commit; regenerating them with the current
writer would lose that coverage.

`checkpoint-v6.bin` was captured from commit 0c25356's format 6 writer before
installation provenance was added. After Reset, request `legacy-v6` ran four
instructions of StaticRoundTrip, already committing Total=4 while still paused.
Its cumulative instruction count is 7. Recovery must resume that same execution
to result 4 and return 4 on retries. Its SHA-256 is
`61098f09377337a23bd3550c1a4979f3081216bc80f9513095cfbd00679fbf10`.
