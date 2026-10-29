using System.Reflection;
using System.Runtime.Loader;
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
    byte[] core = Compile("Renraku.CoreLib", File.ReadAllText(Path.Combine(root, "guest/CoreLib.cs")),
        Array.Empty<MetadataReference>());
    string source = File.ReadAllText(Path.Combine(root, "guest/Simulation.cs"));
    byte[] guest = Compile("Renraku.Simulation", source,
        new[] { MetadataReference.CreateFromImage(core) });
    File.WriteAllBytes(Path.Combine(output, "Renraku.CoreLib.dll"), core);
    File.WriteAllBytes(Path.Combine(output, "Renraku.Simulation.dll"), guest);

    // Compile the identical source against the real BCL for differential tests.
    var framework = ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!)
        .Split(Path.PathSeparator).Select(path => MetadataReference.CreateFromFile(path));
    byte[] reference = Compile("Renraku.Reference", source + Environment.NewLine +
        File.ReadAllText(Path.Combine(root, "guest/ReferenceModel.cs")), framework);
    using var referenceStream = new MemoryStream(reference);
    var assembly = AssemblyLoadContext.Default.LoadFromStream(referenceStream);
    var type = assembly.GetType("Renraku.Guest.Simulation", throwOnError: true)!;
    string[] cases = { "Run", "Sharing", "Arithmetic", "WideArithmetic", "Wrap", "Branches",
        "UnsignedComparisons", "SignedComparisons", "ArgumentStore", "Recursion", "SaveObject", "NullComparisons", "DurableRun", "Inheritance",
        "Strings", "StringEquality", "StringNullConcat", "StringInterning", "ModelHarness", "ChatHarnessScore" };
    var results = cases.Select(name => $"{name}={type.GetMethod(name, BindingFlags.Public | BindingFlags.Static)!.Invoke(null, null)}");
    File.WriteAllLines(Path.Combine(output, "reference-results.txt"), results);
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
