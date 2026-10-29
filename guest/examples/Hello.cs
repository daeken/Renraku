namespace Example
{
    public static class Hello
    {
        public static string Text;
        public static int Visits;

        public static int Run()
        {
            Text = "Hello from externally compiled CIL.";
            Visits = Visits + 1;
            return Text.Length + Visits;
        }
    }
}
