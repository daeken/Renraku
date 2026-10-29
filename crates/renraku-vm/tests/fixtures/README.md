# Legacy checkpoint fixtures

`checkpoint-v1.bin` was captured from the format 1 runtime before managed strings
were added. It contains the original CoreLib and simulation assemblies, saved
identity bindings, one Counter object, and three active frames after instruction
30 of Simulation.Run. Resuming it must return 48 after 155 total instructions.

`checkpoint-v2.bin` was captured from the format 2 runtime before effect records
were added. It contains the earlier CoreLib and simulation assemblies and a
paused Simulation.Strings execution after 80 instructions. Resuming it must
return 167861, preserving literal identities and exact UTF-16 string payloads.

Both fixtures must stay unchanged. They test reading earlier formats and
upgrading on the next successful commit; regenerating them with the current
writer would lose that coverage.
