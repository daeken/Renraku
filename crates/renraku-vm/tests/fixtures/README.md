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

All fixtures must stay unchanged. They test reading earlier formats and
upgrading on the next successful commit; regenerating them with the current
writer would lose that coverage.
