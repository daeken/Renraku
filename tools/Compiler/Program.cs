using System.Globalization;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;

// This process runs on the host CLR. Emitted guest code is never loaded here.
return Compiler.Run(args);

static partial class Compiler
{
    const int MaxSource = 1024 * 1024;
    const int MaxSources = 4 * MaxSource;
    const int MaxAssembly = 16 * MaxSource;
    static readonly JsonSerializerOptions JsonOptions = new() { WriteIndented = true };

    public static int Run(string[] args)
    {
        if (args is ["--identity"]) { Report(Identity()); return 0; }
        if (args.Length == 0 || args is ["--help"] or ["-h"])
        {
            Console.WriteLine("Usage: Renraku.Compiler --core <Renraku.CoreLib.dll> --output <new-directory> --name <assembly-name> [--reference-view <view.json> ...] <source.cs> [source.cs ...]");
            return args.Length == 0 ? 2 : 0;
        }
        string? staging = null;
        try
        {
            var request = Parse(args);
            if (Exists(request.Output))
                throw new ArgumentException("output already exists; choose a new directory");
            byte[] core = ReadBounded(request.Core, MaxAssembly);
            using (var pe = new PEReader(new MemoryStream(core, writable: false)))
            {
                if (!pe.HasMetadata) throw new ArgumentException("core reference has no CLI metadata");
                var metadata = pe.GetMetadataReader();
                if (!metadata.IsAssembly || metadata.GetString(metadata.GetAssemblyDefinition().Name) != "Renraku.CoreLib")
                    throw new ArgumentException("core reference must be a Renraku.CoreLib assembly");
            }
            var names = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
            int totalSourceBytes = 0;
            var inputs = request.Sources.Select(path =>
            {
                string name = Path.GetFileName(path);
                if (!names.Add(name)) throw new ArgumentException("source filenames must be distinct, including case");
                byte[] bytes = ReadBounded(path, MaxSource);
                totalSourceBytes = checked(totalSourceBytes + bytes.Length);
                if (totalSourceBytes > MaxSources)
                    throw new ArgumentException("source inputs exceed the 4 MiB aggregate limit");
                string text = new UTF8Encoding(false, true).GetString(bytes);
                if (text.StartsWith('\uFEFF')) text = text[1..];
                return new Input(name, bytes, text);
            }).ToArray();
            var parse = new CSharpParseOptions(LanguageVersion.CSharp12);
            var options = new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary,
                optimizationLevel: OptimizationLevel.Release, deterministic: true,
                concurrentBuild: false, checkOverflow: false, allowUnsafe: false);
            var views = BuildReferences(request, core, parse, options);
            var compilation = CSharpCompilation.Create(request.Name,
                inputs.Select(input => CSharpSyntaxTree.ParseText(input.Text, parse, input.Name, Encoding.UTF8)),
                (views.Any(v=>v.Name=="Renraku.CoreLib") ? Array.Empty<MetadataReference>() : new[] { MetadataReference.CreateFromImage(core) }).Concat(views.OrderBy(view=>view.Name,StringComparer.Ordinal).Select(view => MetadataReference.CreateFromImage(view.Image))), options);
            using var image = new BoundedImage(MaxAssembly);
            var result = compilation.Emit(image, options:
                new Microsoft.CodeAnalysis.Emit.EmitOptions(runtimeMetadataVersion: "v4.0.30319"));
            var diagnostics = result.Diagnostics.Where(d => d.Severity is DiagnosticSeverity.Error or DiagnosticSeverity.Warning)
                .OrderBy(d => d.Location.SourceTree?.FilePath, StringComparer.Ordinal)
                .ThenBy(d => d.Location.SourceSpan.Start).ThenBy(d => d.Id, StringComparer.Ordinal).ToArray();
            var report = diagnostics.Take(128).Select(d =>
            {
                var span = d.Location.GetLineSpan();
                return new { code = d.Id, severity = d.Severity.ToString().ToLowerInvariant(),
                    message = Limit(d.GetMessage(CultureInfo.InvariantCulture), 2048),
                    file = d.Location.IsInSource ? span.Path : null,
                    line = d.Location.IsInSource ? span.StartLinePosition.Line + 1 : (int?)null,
                    column = d.Location.IsInSource ? span.StartLinePosition.Character + 1 : (int?)null };
            }).ToArray();
            if (!result.Success)
            {
                Report(new { success = false, diagnostics = report, diagnosticsTruncated = diagnostics.Length > 128, identity = Identity() });
                return 1;
            }
            byte[] assembly = image.ToArray();
            var manifest = new
            {
                format = views.Length == 0 ? 1 : 2,
                compiler = new { csharp = CompilerRecord(typeof(CSharpCompilation).Assembly),
                    common = CompilerRecord(typeof(Compilation).Assembly),
                    driver = CompilerRecord(typeof(Compiler).Assembly) },
                languageVersion = "CSharp12",
                options = new { outputKind = "DynamicallyLinkedLibrary", optimization = "release",
                    deterministic = true, concurrentBuild = false, overflowChecks = false,
                    allowUnsafe = false, runtimeMetadataVersion = "v4.0.30319" },
                core = FileRecord("Renraku.CoreLib.dll", core),
                assembly = FileRecord(request.Name + ".dll", assembly),
                sources = inputs.Select(input => FileRecord("sources/" + input.Name, input.Bytes)).ToArray(),
                references = views.Select(view => new { schemaHash = view.SchemaHash,
                    image = FileRecord("references/" + view.Name + ".dll", view.Image),
                    view = FileRecord("references/" + view.Name + ".view.json", view.View) }).ToArray()
            };
            // Capture all inputs before publishing. The caller loads the exact
            // copied core reference, never a later edit to the original path.
            string parent = Path.GetDirectoryName(request.Output)!;
            Directory.CreateDirectory(parent);
            staging = Path.Combine(parent, ".renraku-compile-" + Guid.NewGuid());
            if (OperatingSystem.IsWindows()) Directory.CreateDirectory(staging);
            else Directory.CreateDirectory(staging, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute);
            Directory.CreateDirectory(Path.Combine(staging, "sources"));
            Save(Path.Combine(staging, "Renraku.CoreLib.dll"), core);
            Save(Path.Combine(staging, request.Name + ".dll"), assembly);
            foreach (var input in inputs) Save(Path.Combine(staging, "sources", input.Name), input.Bytes);
            if (views.Length != 0) Directory.CreateDirectory(Path.Combine(staging, "references"));
            foreach (var view in views)
            {
                Save(Path.Combine(staging, "references", view.Name + ".dll"), view.Image);
                Save(Path.Combine(staging, "references", view.Name + ".view.json"), view.View);
            }
            Save(Path.Combine(staging, "renraku-build.json"), JsonSerializer.SerializeToUtf8Bytes(manifest, JsonOptions));
            Directory.Move(staging, request.Output); // Never replaces an existing bundle.
            staging = null;
            Report(new { success = true, outputDirectory = request.Output, assemblyFile = request.Name + ".dll",
                diagnostics = report, diagnosticsTruncated = diagnostics.Length > 128, identity = Identity() });
            return 0;
        }
        catch (Exception error) when (error is ArgumentException or IOException or UnauthorizedAccessException or BadImageFormatException or InvalidOperationException or JsonException or KeyNotFoundException or FormatException)
        {
            Report(new { success = false, error = Limit(error.Message, 2048),
                failureKind = error is IOException or UnauthorizedAccessException ? "infrastructure" : "input",
                identity = Identity() });
            return 2;
        }
        finally
        {
            if (staging is not null)
            {
                try { Directory.Delete(staging, recursive: true); }
                catch (IOException) { }
                catch (UnauthorizedAccessException) { }
            }
        }
    }
    static Request Parse(string[] args)
    {
        var options = new Dictionary<string, string>(StringComparer.Ordinal);
        var views = new List<string>();
        int index = 0;
        while (index < args.Length && args[index].StartsWith("--", StringComparison.Ordinal))
        {
            string option = args[index++];
            if (option == "--reference-view" && index < args.Length && views.Count < 16)
            { views.Add(Path.GetFullPath(args[index++])); continue; }
            if (option is not ("--core" or "--output" or "--name") || index == args.Length || !options.TryAdd(option, args[index++]))
                throw new ArgumentException("expected each of --core, --output, and --name once, followed by source files");
        }
        if (options.Count != 3 || args.Length - index is < 1 or > 32)
            throw new ArgumentException("provide --core, --output, --name, and between 1 and 32 source files");
        string name = options["--name"];
        ValidateName(name);
        if (name.Equals("Renraku.CoreLib", StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException("assembly name cannot be Renraku.CoreLib");
        string output = Path.TrimEndingDirectorySeparator(Path.GetFullPath(options["--output"]));
        if (Path.GetDirectoryName(output) is null) throw new ArgumentException("output cannot be a filesystem root");
        return new Request(Path.GetFullPath(options["--core"]), output, name, args[index..], views.ToArray());
    }
    static void ValidateName(string name)
    {
        if (name.Length is < 1 or > 80
            || name.Split('.').Any(part => part.Length == 0 || !(char.IsAsciiLetter(part[0]) || part[0] == '_')
                || part.Any(c => !char.IsAsciiLetterOrDigit(c) && c != '_')))
            throw new ArgumentException("assembly name must contain dot-separated ASCII identifiers");
    }
    static byte[] ReadBounded(string path, int limit)
    {
        using var file = File.OpenRead(path);
        if (file.Length > limit) throw new ArgumentException($"input exceeds its {limit}-byte limit");
        byte[] bytes = new byte[checked((int)file.Length)];
        file.ReadExactly(bytes);
        if (file.ReadByte() != -1) throw new IOException("input grew while being read; try a stable input");
        return bytes;
    }
    static bool Exists(string path)
    {
        try { _ = File.GetAttributes(path); return true; }
        catch (FileNotFoundException) { return false; }
        catch (DirectoryNotFoundException) { return false; }
    }
    static object FileRecord(string file, byte[] bytes) => new { file, bytes = bytes.Length,
        sha256 = Hash(bytes) };
    static string Hash(byte[] bytes) => Convert.ToHexStringLower(SHA256.HashData(bytes));
    static object CompilerRecord(System.Reflection.Assembly assembly)
    {
        using var file = File.OpenRead(assembly.Location);
        return new { name = assembly.GetName().FullName, sha256 = Convert.ToHexStringLower(SHA256.HashData(file)) };
    }
    static object Identity() => new { format = 1,
        compiler = new { csharp = CompilerRecord(typeof(CSharpCompilation).Assembly),
            common = CompilerRecord(typeof(Compilation).Assembly), driver = CompilerRecord(typeof(Compiler).Assembly) },
        runtime = new { framework = System.Runtime.InteropServices.RuntimeInformation.FrameworkDescription,
            architecture = System.Runtime.InteropServices.RuntimeInformation.ProcessArchitecture.ToString(),
            core = CompilerRecord(typeof(object).Assembly) } };
    static void Save(string path, byte[] bytes)
    {
        using var file = new FileStream(path, FileMode.CreateNew, FileAccess.Write, FileShare.None);
        file.Write(bytes);
        file.Flush(flushToDisk: true);
    }
    static string Limit(string value, int limit) => value.Length <= limit ? value : value[..limit];
    static void Report(object value) => Console.WriteLine(JsonSerializer.Serialize(value, JsonOptions));
    sealed record Request(string Core, string Output, string Name, string[] Sources, string[] Views);
    sealed record ReferenceInput(string Name, byte[] View, byte[] Image, string SchemaHash);
    sealed record Input(string Name, byte[] Bytes, string Text);
    sealed class BoundedImage(int limit) : MemoryStream
    {
        void Check(long length) { if (length > limit) throw new IOException("compiled assembly exceeds the 16 MiB limit"); }
        public override void SetLength(long value) { Check(value); base.SetLength(value); }
        public override void Write(byte[] buffer, int offset, int count) { Check(Position + count); base.Write(buffer, offset, count); }
        public override void Write(ReadOnlySpan<byte> buffer) { Check(Position + buffer.Length); base.Write(buffer); }
        public override void WriteByte(byte value) { Check(Position + 1); base.WriteByte(value); }
    }
}
