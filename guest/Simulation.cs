namespace Renraku.Guest
{
    // The tool protocol is host interpreted, while transcript, turns, and pending
    // inputs/results are ordinary managed objects in the same durable universe.
    public sealed class AgentArgument
    {
        public object Value;
        public AgentArgument Next;
        public AgentArgument(object value, AgentArgument next) { Value = value; Next = next; }
    }
    public sealed class AgentTurn
    {
        public int Number;
        public string Action;
        public string Input;
        public string Result;
        public AgentArgument Retained;
        public AgentTurn Next;
        public AgentTurn(int number, string action) { Number = number; Action = action; }
    }
    public sealed class AgentInput
    {
        public string Id;
        public string Content;
        public int AfterTurn;
        public Renraku.ChatMessage Message;
        public AgentInput Next;
        public AgentInput(string id, string content, int afterTurn)
        { Id = id; Content = content; AfterTurn = afterTurn; }
    }
    public sealed class AgentSession
    {
        public string Id;
        public Renraku.ModelBinding Binding;
        public Renraku.ChatMessage First;
        public Renraku.ChatMessage Last;
        public AgentTurn FirstTurn;
        public AgentTurn Current;
        public AgentInput FirstInput;
        public AgentInput LastInput;
        public int Count;
        public int Phase; // 0 model, 1 action, 2 next model, 3 finished.
        public AgentSession(string id, string capability, string model, string system, string prompt)
        {
            Id = id; Binding = new Renraku.ModelBinding(capability, model);
            First = new Renraku.ChatMessage(0, system);
            Last = new Renraku.ChatMessage(1, prompt); First.Next = Last;
        }
    }
    public static class AgentHarness
    {
        public static AgentSession Saved;
        public static string Initialize(string id, string capability, string model, string system, string prompt)
        {
            Saved = new AgentSession(id, capability, model, system, prompt);
            if (Simulation.Saved == null) Simulation.Saved = new Counter(19);
            return Ask();
        }
        public static string Ask()
        {
            AgentSession session = Saved; session.Phase = 0;
            string action = Renraku.Model.Chat(session.Binding, session.First);
            Renraku.ChatMessage message = new Renraku.ChatMessage(2, action);
            session.Last.Next = message; session.Last = message;
            AgentTurn turn = new AgentTurn(session.Count + 1, action);
            if (session.Current == null) session.FirstTurn = turn;
            else session.Current.Next = turn;
            session.Current = turn; session.Count = turn.Number; session.Phase = 1;
            return action;
        }
        public static void Freeze(string input, object a0, object a1, object a2, object a3,
            object a4, object a5, object a6, object a7)
        {
            Saved.Current.Input = input;
            AgentArgument retained = null;
            if (a7 != null) retained = new AgentArgument(a7, retained);
            if (a6 != null) retained = new AgentArgument(a6, retained);
            if (a5 != null) retained = new AgentArgument(a5, retained);
            if (a4 != null) retained = new AgentArgument(a4, retained);
            if (a3 != null) retained = new AgentArgument(a3, retained);
            if (a2 != null) retained = new AgentArgument(a2, retained);
            if (a1 != null) retained = new AgentArgument(a1, retained);
            if (a0 != null) retained = new AgentArgument(a0, retained);
            Saved.Current.Retained = retained;
        }
        public static void Apply(string result, bool finish)
        {
            AgentSession session = Saved; session.Current.Result = result;
            Renraku.ChatMessage message = new Renraku.ChatMessage(1, result);
            session.Last.Next = message; session.Last = message;
            session.Phase = finish ? 3 : 2;
        }
        // The host pins this private entry and its arguments in an invocation
        // receipt before any CIL executes. It appends input without inference.
        private static void AcceptInput(string id, string content, int afterTurn)
        {
            AgentSession session = Saved;
            AgentInput input = new AgentInput(id, content, afterTurn);
            Renraku.ChatMessage message = new Renraku.ChatMessage(1, content);
            input.Message = message;
            if (session.LastInput == null) session.FirstInput = input;
            else session.LastInput.Next = input;
            session.LastInput = input;
            session.Last.Next = message; session.Last = message;
            session.Phase = 2;
        }
    }
    public sealed class Counter
    {
        public int Value;
        public Counter Next;
        public Counter(int initial) { Value = initial; }
        public int Add(int delta) { Value = Value + delta; return Value; }
    }

    public static class Simulation
    {
        public static string LastText;
        public static int Total;
        public static Counter Saved;

        // Real transport uses the same ordinary CIL call and linked guest objects.
        public static string OpenAIHarness(string capability, string model, string prompt)
        {
            LastBinding = new Renraku.ModelBinding(capability, model);
            LastChat = new Renraku.ChatMessage(1, prompt);
            LastText = Renraku.Model.Chat(LastBinding, LastChat);
            LastChat.Next = new Renraku.ChatMessage(2, LastText);
            return LastText;
        }

        public static int Run()
        {
            Counter counter = new Counter(3);
            int sum = 0;
            for (int i = 0; i < 5; i++) sum += Advance(counter, i);
            Total = sum;
            return Total + counter.Value;
        }

        private static int Advance(Counter counter, int delta) { return counter.Add(delta); }

        public static int Sharing()
        {
            Counter first = new Counter(5);
            Counter second = new Counter(8);
            first.Next = second;
            second.Next = first;
            Counter alias = first.Next;
            alias.Add(4);
            if (second != alias || second.Next != first || first.Next.Value != 12) return -1;
            Counter empty = new Counter(0);
            if (empty.Next != null) return -2;
            return first.Value + second.Value;
        }

        public static int Arithmetic()
        {
            int result = 1;
            for (int i = 1; i < 7; i++) result *= i;
            return result - 17;
        }

        public static long WideArithmetic()
        {
            long value = 9007199254740993L;
            return (value + 7L) * 3L - 5L;
        }

        public static int Wrap() { int value = 2147483647; return value + 1; }
        public static int Branches()
        {
            int score = 0;
            for (int i = -3; i < 4; i++)
            {
                if (i < 0) score += 2;
                else if (i == 0) score += 5;
                else score -= 1;
            }
            return score;
        }
        public static int Forever() { while (true) { Total = Total + 1; } }
        public static int NullReceiver() { Counter counter = null; return counter.Add(1); }
        public static int UnsignedComparisons() { return CompareUnsigned(4294967295U, 1U); }
        private static int CompareUnsigned(uint a, uint b)
        {
            int score = 0;
            if (a > b) score += 1;
            if (a >= b) score += 2;
            if (a < b) score += 4;
            if (a <= b) score += 8;
            if (a == b) score += 16;
            if (a != b) score += 32;
            return score;
        }
        public static int SignedComparisons() { return CompareSigned(-1, 2) + CompareSigned(2, 2); }
        private static int CompareSigned(int a, int b)
        {
            int score = 0;
            if (a > b) score += 1;
            if (a >= b) score += 2;
            if (a < b) score += 4;
            if (a <= b) score += 8;
            if (a == b) score += 16;
            if (a != b) score += 32;
            return score;
        }
        public static int ArgumentStore() { return AssignArgument(6); }
        private static int AssignArgument(int value) { value = value + 3; return value * value; }
        public static int Recursion() { return Fibonacci(10); }
        private static int Fibonacci(int n)
        {
            if (n < 2) return n;
            return Fibonacci(n - 1) + Fibonacci(n - 2);
        }
        public static int StaticRoundTrip() { Total = Total + 4; return Total; }
        public static int SaveObject() { Saved = new Counter(19); return Saved.Value; }
        public static Counter ReturnObject() { return new Counter(23); }
        public static void Reset() { Total = 0; }
        public static int NullComparisons()
        {
            if (IsPresent(null)) return -1;
            if (!IsPresent(new Counter(0))) return -2;
            return 1;
        }
        private static bool IsPresent(Counter counter) { return counter != null; }
        public static int DurableRun()
        {
            Counter first = new Counter(1);
            Counter second = new Counter(10);
            first.Next = second;
            second.Next = first;
            Saved = first;
            int sum = 0;
            for (int i = 1; i <= 30; i++)
            {
                sum += Advance(first, i);
                first.Next.Add(2);
            }
            Total = sum;
            return Total + first.Value + second.Value;
        }
        public static int Inheritance()
        {
            BaseCounter first = new BaseCounter(7);
            DerivedCounter second = new DerivedCounter(11, 3);
            return Choose(true, first, second).BaseValue + Choose(false, first, second).BaseValue + second.Extra;
        }
        private static BaseCounter Choose(bool flag, BaseCounter first, DerivedCounter second)
        {
            BaseCounter selected = flag ? first : second;
            selected.BaseValue = selected.BaseValue + 1;
            return selected;
        }
        public static int Strings()
        {
            Conversation conversation = new Conversation("hi\0");
            conversation.Append(" \ud83d\ude80");
            conversation.Append("\ud800");
            LastText = conversation.Text;
            int sum = 0;
            for (int i = 0; i < conversation.Text.Length; i++) sum += conversation.Text[i];
            return sum + conversation.Text.Length;
        }
        public static int StringEquality()
        {
            Conversation conversation = new Conversation("alpha");
            conversation.Append("beta");
            if (conversation.Text != "alphabeta") return -1;
            if (conversation.Text == "ALPHABETA") return -2;
            string none = null;
            if (none == "" || none != null || "x" == none) return -3;
            return 1;
        }
        public static int StringNullConcat()
        {
            string empty = System.String.Concat(null, null);
            if (empty.Length != 0) return -1;
            if (System.String.Concat(null, "x") != "x") return -2;
            if (System.String.Concat("x", null) != "x") return -3;
            if (System.String.Concat("", "") != "") return -4;
            return 1;
        }
        private static string Literal() { return "same"; }
        public static int StringInterning()
        {
            if ((object)Literal() != (object)"same") return -1;
            LastText = Literal();
            return 1;
        }
        public static string ReturnString()
        {
            return System.String.Concat("a\0", "\ud800");
        }
        public static int StringNullReceiver() { string value = null; return value.Length; }
        public static int StringBadIndex() { return "abc"[5]; }
        public static int StringNegativeIndex() { return "abc"[-1]; }
        public static int StringTooLarge()
        {
            string value = "x";
            for (int i = 0; i < 21; i++) value = System.String.Concat(value, value);
            return value.Length;
        }
        public static int ConsumeModelReply(string reply)
        {
            LastText = reply;
            return reply.Length;
        }
        public static ModelConversation LastConversation;
        public static int ModelHarness()
        {
            ModelConversation conversation = new ModelConversation();
            LastConversation = conversation;
            string reply = conversation.Ask("hello");
            reply = conversation.Ask(reply);
            return conversation.Count * 100 + reply.Length;
        }
        public static string NullModelPrompt() { return Renraku.Model.Complete(null); }
        public static Renraku.ChatMessage LastChat;
        public static Renraku.ModelBinding LastBinding;
        public static string ChatHarness()
        {
            Renraku.ModelBinding binding = new Renraku.ModelBinding("local.echo", "echo-v1");
            Renraku.ChatMessage system = new Renraku.ChatMessage(0, System.String.Concat("be ", "brief"));
            Renraku.ChatMessage user = new Renraku.ChatMessage(1, "hi\0\ud800");
            system.Next = user;
            LastChat = system;
            LastBinding = binding;
            string first = Renraku.Model.Chat(binding, system);
            // Reuse and extend ordinary linked objects after the first request.
            system.Content = "new instruction";
            Renraku.ChatMessage assistant = new Renraku.ChatMessage(2, first);
            user.Next = assistant;
            assistant.Next = new Renraku.ChatMessage(1, "again");
            string second = Renraku.Model.Chat(binding, system);
            binding.Model = "changed-after-completion";
            return second;
        }
        public static int ChatHarnessScore() { LastText = ChatHarness(); return LastText.Length; }
        public static string BadChat(int kind)
        {
            Renraku.ModelBinding binding = new Renraku.ModelBinding("local.echo", "echo-v1");
            Renraku.ChatMessage message = new Renraku.ChatMessage(1, "prompt");
            if (kind == 0) message.Next = message;
            if (kind == 1) message.Role = 9;
            if (kind == 2) message.Content = null;
            if (kind == 3) binding.Model = "bad model";
            if (kind == 4) message.Role = 2;
            if (kind == 5) binding = null;
            if (kind == 6) message = null;
            if (kind == 7)
            {
                Renraku.ChatMessage head = message;
                for (int i = 0; i < 128; i++) { head.Next = new Renraku.ChatMessage(1, "x"); head = head.Next; }
            }
            return Renraku.Model.Chat(binding, message);
        }
    }
    public sealed class ModelMessage
    {
        public string Text;
        public ModelMessage Next;
        public ModelMessage(string text) { Text = text; }
    }
    public sealed class ModelConversation
    {
        public ModelMessage First;
        public ModelMessage Last;
        public int Count;
        private void Append(string text)
        {
            ModelMessage message = new ModelMessage(text);
            if (Last == null) First = message;
            else Last.Next = message;
            Last = message;
            Count = Count + 1;
        }
        public string Ask(string prompt)
        {
            Append(prompt);
            string reply = Request(prompt);
            Append(reply);
            return reply;
        }
        private static string Request(string prompt) { return Renraku.Model.Complete(prompt); }
    }
    public sealed class Conversation
    {
        public string Text;
        public Conversation(string text) { Text = text; }
        public void Append(string text) { Text = System.String.Concat(Text, text); }
    }
    public class BaseCounter
    {
        public int BaseValue;
        public BaseCounter(int value) { BaseValue = value; }
    }
    public sealed class DerivedCounter : BaseCounter
    {
        public int Extra;
        public DerivedCounter(int value, int extra) : base(value) { Extra = extra; }
    }
}
