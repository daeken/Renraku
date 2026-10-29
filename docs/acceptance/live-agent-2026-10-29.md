# Live coding-agent acceptance, October 29, 2026

GPT-6.1 Sol completed the user's coding-agent acceptance target using the signed-in
ChatGPT subscription through Renraku's Responses adapter. The model chose its own
actions and source from the real saved tool history. No scripted provider or
prewritten source response was used for this run.

The goal was to discover the Counter API, inspect existing code, write a new caller
that advances the saved counter by 7, compile and install it, invoke it once, then
inspect its installed code and retained source. The saved counter started at 19.

## Observed actions

| Turn | Model-selected action | Observed result |
| --- | --- | --- |
| 1 | Search `Counter` | Live type, field, and method symbols |
| 2 | Describe `Renraku.Guest.Counter` | Frozen compiler contract and logical bindings |
| 3 | Read `Counter::Add` | Exact saved CIL |
| 4 | Read `Counter::.ctor` | Constructor CIL |
| 5 | Search `Saved` | Located the persistent Simulation field |
| 6 | Compile `Generated.AdvanceCounterSeven` | Success on the first attempt, no diagnostics |
| 7 | Install turn 6 | Captured artifact and installation receipt committed |
| 8 | Invoke `AdvanceCounterSeven::Run` on `Simulation::Saved` | Returned **26** |
| 9 | Read `AdvanceCounterSeven::Run` | Saved caller CIL and its exact retained source |
| 10 | Finish | Reported the observed result and inspected implementation |

The exact model-generated source was:

```csharp
public static class AdvanceCounterSeven
{
    public static int Run(Renraku.Guest.Counter counter)
    {
        return counter.Add(7);
    }
}
```

The controller stopped at provider budgets after turns 3, 6, and 9. Each
continuation used `agent-drive` in a fresh process against the same universe,
compiler ledger, and private completion ledger. The complete run used 1,374 guest
instructions, 10 provider operations, and 10 tool actions. All 10 inference
requests have terminal local receipts; the managed transcript has 22 messages.

## Recovery evidence

The saved heap contains the original counter with `Value = 26`. Its invocation
receipt pins these identities:

```text
universe: 6f788cad3c9049efa56b42ff84ec7d93
request: agent/6f788cad3c9049efa56b42ff84ec7d93/8/invoke
method: 99de596ba88c417e91fe66f7fc74aac7
code: 138e8ba46c14481aa22a935202cc7fb3
execution: 8245418953564ed597489e04212bba40
counter object: b1fe17de18cd447eb649b472ef3d6efb
artifact: 9f32848e2dd4f31fe98274b3354b5730a7e4ca0931720106a92f93ba689176af
```

Reopening the finished agent with zero instruction/provider/tool budgets returned
the same saved finish result with **0 instructions, 0 provider operations, and
0 tool actions**. Replaying the invocation ID with a zero instruction budget
returned 26 with the same method/code/execution identities. It did not advance
the counter a second time.

Local development artifacts are under `target/live-agent-20261029-01/`, including
the universe, compiler ledger, generated source, transcript inspection, and run
logs. The completion ledger is outside the Git checkout in the user's private
Renraku data directory. These artifacts and credentials are not checked in.
The initial attempt used the guide's incorrect in-checkout receipt path and
failed before inference; the same saved initialization resumed with the corrected
private path. The guide now uses an outside-Git receipt directory.

This live run establishes the bounded discovery/inspection/compile/install/execute
workflow. Deterministic HTTP fixtures separately cover corrective compiler errors,
uncertain streams, lost replies, and actual SIGKILL boundaries; this live run used
planned process stops and successful terminal streams. See the
[coding-agent guide](../agent.md) for commands and current runtime limits.
