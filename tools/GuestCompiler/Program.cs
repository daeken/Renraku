using System.Reflection;
using System.Runtime.Loader;
using System.Globalization;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;

if (args.Length != 2)
{
    Console.Error.WriteLine("Usage: GuestCompiler <repository-root> <output-directory>");
    return 2;
}
string root = Path.GetFullPath(args[0]);
string output = Path.GetFullPath(args[1]);
Directory.CreateDirectory(output);
var parseOptions = new CSharpParseOptions(LanguageVersion.CSharp12);
var options = new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary,
    optimizationLevel: OptimizationLevel.Release, deterministic: true,
    concurrentBuild: false, checkOverflow: false);

byte[] Compile(string name, string source, IEnumerable<MetadataReference> references)
{
    var compilation = CSharpCompilation.Create(name,
        new[] { CSharpSyntaxTree.ParseText(source, parseOptions) }, references, options);
    using var stream = new MemoryStream();
    var result = compilation.Emit(stream, options:
        new Microsoft.CodeAnalysis.Emit.EmitOptions(runtimeMetadataVersion: "v4.0.30319"));
    if (!result.Success)
        throw new InvalidOperationException(string.Join(Environment.NewLine, result.Diagnostics));
    return stream.ToArray();
}

try
{
    string integerText = File.ReadAllText(Path.Combine(root, "guest/IntegerText.cs"));
    byte[] core = Compile("Renraku.CoreLib", File.ReadAllText(Path.Combine(root, "guest/CoreLib.cs")),
        Array.Empty<MetadataReference>());
    byte[] library = Compile("Renraku.Library", integerText, new[] { MetadataReference.CreateFromImage(core) });
    File.WriteAllBytes(Path.Combine(output, "Renraku.Library.dll"), library);
    string collections = File.ReadAllText(Path.Combine(root, "guest/StringMap.cs"));
    byte[] collectionLibrary = Compile("Renraku.Collections", collections, new[] { MetadataReference.CreateFromImage(core) });
    File.WriteAllBytes(Path.Combine(output, "Renraku.Collections.dll"), collectionLibrary);
    byte[] collectionTests = Compile("Renraku.CollectionTests", File.ReadAllText(Path.Combine(root, "guest/StringMapHarness.cs")),
        new[] { MetadataReference.CreateFromImage(core), MetadataReference.CreateFromImage(collectionLibrary) });
    File.WriteAllBytes(Path.Combine(output, "Renraku.CollectionTests.dll"), collectionTests);
    // Real metadata dependency/collision fixtures, never executed by the host.
    var coreReference = MetadataReference.CreateFromImage(core);
    byte[] cycleStub = Compile("Cycle.Left", "namespace Cycle.Left { public class Node { } }", new[] { coreReference });
    byte[] cycleRight = Compile("Cycle.Right", "namespace Cycle.Right { public class Node { public global::Cycle.Left.Node Other; } }",
        new[] { coreReference, MetadataReference.CreateFromImage(cycleStub) });
    byte[] cycleLeft = Compile("Cycle.Left", "namespace Cycle.Left { public class Node { public global::Cycle.Right.Node Other; } }",
        new[] { coreReference, MetadataReference.CreateFromImage(cycleRight) });
    File.WriteAllBytes(Path.Combine(output, "Cycle.Left.dll"), cycleLeft);
    File.WriteAllBytes(Path.Combine(output, "Cycle.Right.dll"), cycleRight);
    foreach (string name in new[] { "Ambiguous.One", "Ambiguous.Two" })
        File.WriteAllBytes(Path.Combine(output, name + ".dll"), Compile(name, "namespace Duplicate { public class Node { } }", new[] { coreReference }));
    File.WriteAllBytes(Path.Combine(output, "Cross.Owner.dll"), Compile("Cross.Owner",
        "public static class CrossOwner { public static Renraku.Collections.StringMap Saved; }",
        new[] { coreReference, MetadataReference.CreateFromImage(collectionLibrary) }));
    string source = File.ReadAllText(Path.Combine(root, "guest/Simulation.cs")) + Environment.NewLine +
        File.ReadAllText(Path.Combine(root, "guest/IntegerArithmetic.cs"));
    byte[] guest = Compile("Renraku.Simulation", source,
        new[] { MetadataReference.CreateFromImage(core) });
    File.WriteAllBytes(Path.Combine(output, "Renraku.CoreLib.dll"), core);
    File.WriteAllBytes(Path.Combine(output, "Renraku.Simulation.dll"), guest);

    // Compile the identical source against the real BCL for differential tests.
    var framework = ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!)
        .Split(Path.PathSeparator).Select(path => MetadataReference.CreateFromFile(path));
    byte[] reference = Compile("Renraku.Reference", collections + Environment.NewLine + integerText + Environment.NewLine + source + Environment.NewLine +
        File.ReadAllText(Path.Combine(root, "guest/ReferenceModel.cs")), framework);
    using var referenceStream = new MemoryStream(reference);
    var assembly = AssemblyLoadContext.Default.LoadFromStream(referenceStream);
    var type = assembly.GetType("Renraku.Guest.Simulation", throwOnError: true)!;
    string[] cases = { "Run", "Sharing", "Arithmetic", "WideArithmetic", "Wrap", "Branches",
        "UnsignedComparisons", "SignedComparisons", "ArgumentStore", "Recursion", "SaveObject", "NullComparisons", "DurableRun", "Inheritance",
        "Strings", "StringEquality", "StringNullConcat", "StringInterning", "ModelHarness", "ChatHarnessScore" };
    var results = cases.Select(name => $"{name}={type.GetMethod(name, BindingFlags.Public | BindingFlags.Static)!.Invoke(null, null)}");
    File.WriteAllLines(Path.Combine(output, "reference-results.txt"), results);
    // Preserve signed storage bits as decimal strings, including UInt64 values.
    // Reflection invokes the actual emitted operators, preventing constant folding.
    var arithmetic = assembly.GetType("Renraku.Guest.IntegerArithmetic", throwOnError: true)!;
    var arithmeticResults = new List<object>();
    foreach (int width in new[] { 32, 64 })
    {
        long min = width == 32 ? int.MinValue : long.MinValue;
        long max = width == 32 ? int.MaxValue : long.MaxValue;
        long[] operands = { 0, 1, -1, 2, -2, 3, -3, 6, -6, 10, -10, 14, -14,
            min, min + 1, max - 1, max };
        foreach (string operation in new[] { "Div", "DivU", "Rem", "RemU" })
        {
            string name = operation + width;
            var method = arithmetic.GetMethod(name)!;
            foreach (long left in operands)
            foreach (long right in operands)
            {
                object Box(long bits) => method.GetParameters()[0].ParameterType switch
                {
                    Type t when t == typeof(int) => (object)(int)bits,
                    Type t when t == typeof(uint) => unchecked((uint)bits),
                    Type t when t == typeof(ulong) => unchecked((ulong)bits),
                    _ => bits
                };
                string? value = null, fault = null;
                try
                {
                    object actual = method.Invoke(null, new[] { Box(left), Box(right) })!;
                    long bits = actual switch
                    {
                        uint v => unchecked((int)v), ulong v => unchecked((long)v),
                        int v => v, long v => v, _ => throw new InvalidOperationException("unexpected result")
                    };
                    value = bits.ToString(CultureInfo.InvariantCulture);
                }
                catch (TargetInvocationException error) { fault = error.InnerException!.GetType().FullName; }
                arithmeticResults.Add(new { method = name, width, left = left.ToString(CultureInfo.InvariantCulture),
                    right = right.ToString(CultureInfo.InvariantCulture), value, fault });
            }
        }
    }
    File.WriteAllText(Path.Combine(output, "arithmetic-reference.json"), JsonSerializer.Serialize(arithmeticResults));
    File.WriteAllText(Path.Combine(output, "arithmetic-workload-reference.txt"),
        arithmetic.GetMethod("PersistentArithmetic")!.Invoke(null, null)!.ToString());
    IntegerTextReference.Write(assembly, output);
    StringMapReference.Write(assembly, output);
    foreach (string path in Directory.GetFiles(Path.Combine(root, "guest/Rejected"), "*.cs"))
        File.WriteAllBytes(Path.Combine(output, Path.GetFileNameWithoutExtension(path) + ".dll"),
            Compile(Path.GetFileNameWithoutExtension(path), File.ReadAllText(path),
                new[] { MetadataReference.CreateFromImage(core) }));
    Console.WriteLine($"Compiled custom CoreLib and guest assembly into {output}");
    return 0;
}
catch (Exception error)
{
    Console.Error.WriteLine(error.Message);
    return 1;
}
