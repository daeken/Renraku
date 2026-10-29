namespace Renraku.Guest
{
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
