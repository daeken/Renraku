# Live follow-up goals, October 29, 2026

GPT-6.1 Sol completed two goals in the same persistent universe using the user's
authorized ChatGPT subscription. The second goal reused the first goal's installed
code and original counter, with its conversation and input recorded in CIL objects.

The first goal asked for a reusable caller taking a Counter and an integer delta.
The model searched symbols, described the contract, inspected Counter.Add, then
compiled and installed this caller:

```csharp
public static class CounterCaller { public static int Add(Renraku.Guest.Counter counter, int delta) { return counter.Add(delta); } }
```

It invoked the caller with delta 7, observed **26**, and finished at turn 7. This
used 993 guest instructions, seven model operations, and seven tool actions.

In a fresh process, `agent-input` queued input ID `goal-2` with a zero instruction
budget. The new goal requested the same installed caller with delta -3, without
compiling or installing anything else. Retrying that input command returned the
same pending execution and zero instructions. No append or inference had run yet;
the intent, pinned helper, and arguments were already checkpointed.

Another fresh `agent-drive` process resumed the saved input helper, appended the
user goal, and sent the full retained history. Its compiler path was deliberately
`/missing/compiler.dll`. The model selected:

| Turn | Action | Result |
| --- | --- | --- |
| 8 | Invoke `CounterCaller::Add` on `Simulation::Saved` with -3 | **23** |
| 9 | Finish | Reported 23 and reuse of the existing caller |

The follow-up used 338 guest instructions, two model operations, and two tool
actions. It needed no compilation or installation. The saved heap contains the
same counter at 23; both calls pin the same method and code version:

```text
universe: 9d5ff177510046e8a48db3235e668077
counter object: e143ff5ce214447cb89ee1dd2001243f
method: d8a5dc94e1ba48619eca1705d8d4d580
code: 1ff6bf2d5aeb4b39a8035a966ae69619
first invocation: agent/9d5ff177510046e8a48db3235e668077/6/invoke
first execution: 856a3ea9ecb2457cbe4583ce507bc3a0 (result 26)
follow-up invocation: agent/9d5ff177510046e8a48db3235e668077/8/invoke
follow-up execution: cfb3a73a135845a0869e496ddde35f4d (result 23)
input receipt: agent/9d5ff177510046e8a48db3235e668077/input/goal-2
input execution: 02cfcfb9923943de8a111ee4d2015e50
artifact: de71b3350ba8fc6a5e6ed75ddea6c94f6a9cb8118b7f69baf6f3418b223cdf23
```

After turn 9 finished, retrying `goal-2` with the original text returned the same
accepted input execution with zero instructions. It did not append another goal,
reset the phase, or repeat a model call. Inspection shows one follow-up node
(`afterTurn: 7`), 21 managed messages, one installation, and three invocation
receipts: the two counter calls and the input append. All nine inference requests
have terminal local completion receipts.

Artifacts and logs are under `target/live-followup-20261029-01/`; the private
completion ledger is outside Git under the user's Renraku data directory. No
credentials or local database files are committed. This run used a fresh universe
with the new input-capable guest; the earlier live acceptance universe retains
its original schema and explicitly does not support new follow-up input.

The [agent guide](../agent.md#add-a-follow-up-goal) documents commands and bounds.
Local tests separately cover every append instruction with GC/recovery, input
conflicts, lifetime limits, old-schema recovery, and the full HTTP history.
