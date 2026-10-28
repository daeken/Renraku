# Legacy checkpoint fixture

`checkpoint-v1.bin` was captured from the format 1 runtime before managed strings
were added. It contains the original CoreLib and simulation assemblies, saved
identity bindings, one Counter object, and three active frames after instruction
30 of Simulation.Run. Resuming it must return 48 after 155 total instructions.

This fixture must stay unchanged. It tests reading an earlier format and
upgrading on the next successful commit; regenerating it with the current writer
would lose that coverage.
