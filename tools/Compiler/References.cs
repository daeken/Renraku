using System.Text;
using System.Text.Encodings.Web;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;

static partial class Compiler
{
    sealed record FacadeSpec(string Name, string Id, byte[] Bytes, string Hash, JsonElement Schema);
    static JsonElement[] CanonicalEnvelopes(JsonElement[] views)
    {
        if (views.Length is < 1 or > 16) throw new ArgumentException("reference set must contain 1-16 modules");
        var schemas = views.Select(v => JsonDocument.Parse(v.GetProperty("schemaJson").GetString()!)).ToArray();
        try
        {
            var ids = new Dictionary<string, int>(StringComparer.Ordinal);
            for (int i = 0; i < schemas.Length; i++)
                if (!ids.TryAdd(schemas[i].RootElement.GetProperty("moduleId").GetString()!, i)) throw new ArgumentException("duplicate reference module identity");
            string Name(int i) => schemas[i].RootElement.GetProperty("moduleName").GetString()!;
            var order = new List<int>(); var seen = new HashSet<int>(); var active = new HashSet<int>();
            void Visit(int i)
            {
                if (seen.Contains(i)) return;
                if (!active.Add(i)) throw new ArgumentException("cross-module reference dependency cycle");
                if (schemas[i].RootElement.TryGetProperty("dependencies", out var dependencies))
                {
                    var targets = dependencies.EnumerateArray().Select(d => ids.TryGetValue(d.GetProperty("moduleId").GetString()!, out int target)
                        ? target : throw new ArgumentException("missing reference dependency module"));
                    foreach (int target in targets.OrderBy(Name, StringComparer.Ordinal).ThenBy(t => schemas[t].RootElement.GetProperty("moduleId").GetString(), StringComparer.Ordinal)) Visit(target);
                }
                active.Remove(i); seen.Add(i); order.Add(i);
            }
            foreach (int i in Enumerable.Range(0, views.Length).OrderBy(Name, StringComparer.Ordinal).ThenBy(i => schemas[i].RootElement.GetProperty("moduleId").GetString(), StringComparer.Ordinal)) Visit(i);
            return order.Select(i => views[i]).ToArray();
        }
        finally { foreach (var schema in schemas) schema.Dispose(); }
    }
    static ReferenceInput[] BuildReferences(Request request, byte[] core, CSharpParseOptions parse, CSharpCompilationOptions options)
    {
        var inputs = new List<byte[]>();
        bool strictNames = false;
        foreach (string path in request.Views)
        {
            byte[] bytes = ReadBounded(path, MaxSources);
            using var document = JsonDocument.Parse(bytes);
            var root = document.RootElement;
            if (root.TryGetProperty("kind", out var kind))
            {
                strictNames = true;
                if (kind.GetString() != "referenceSet" || root.GetProperty("format").GetInt32() != 1)
                    throw new ArgumentException("unsupported reference set");
                var views = CanonicalEnvelopes(root.GetProperty("views").EnumerateArray().ToArray());
                var canonical = views.Select(v => new { format = v.GetProperty("format").GetInt32(), schemaHash = v.GetProperty("schemaHash").GetString(),
                    coreSha256 = v.GetProperty("coreSha256").GetString(), schemaJson = v.GetProperty("schemaJson").GetString() }).ToArray();
                var jsonOptions = new JsonSerializerOptions { Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping };
                if (Hash(JsonSerializer.SerializeToUtf8Bytes(canonical, jsonOptions)) != root.GetProperty("setHash").GetString())
                    throw new ArgumentException("reference set hash mismatch");
                foreach (var view in views) inputs.Add(Encoding.UTF8.GetBytes(view.GetRawText()));
            }
            else inputs.Add(bytes);
            if (inputs.Count > 16) throw new ArgumentException("reference set exceeds 16 modules");
        }
        var names = new HashSet<string>(StringComparer.OrdinalIgnoreCase) { request.Name };
        var ids = new HashSet<string>(StringComparer.Ordinal);
        var typeNames = new HashSet<string>(StringComparer.Ordinal);
        var specs = inputs.Select(bytes =>
        {
            using var envelope = JsonDocument.Parse(bytes);
            var root = envelope.RootElement;
            string schema = root.GetProperty("schemaJson").GetString() ?? throw new ArgumentException("missing schemaJson");
            string schemaHash = root.GetProperty("schemaHash").GetString() ?? throw new ArgumentException("missing schemaHash");
            int format = root.GetProperty("format").GetInt32();
            if (format is not (1 or 2 or 3) || Hash(Encoding.UTF8.GetBytes(schema)) != schemaHash || root.GetProperty("coreSha256").GetString() != Hash(core))
                throw new ArgumentException("reference view hash, core, or format mismatch");
            using var metadata = JsonDocument.Parse(schema);
            var data = metadata.RootElement;
            string name = data.GetProperty("moduleName").GetString()!;
            string id = data.GetProperty("moduleId").GetString()!;
            ValidateName(name);
            if ((name.Equals("Renraku.CoreLib", StringComparison.OrdinalIgnoreCase) && (name != "Renraku.CoreLib" || format != 3)) || !names.Add(name) || !ids.Add(id)) throw new ArgumentException("duplicate reference module name/identity");
            bool dependencies = data.TryGetProperty("dependencies", out var deps) && deps.GetArrayLength() != 0;
            if ((format == 3) != data.TryGetProperty("scope",out _) || (format != 3 && (format == 2) != dependencies)) throw new ArgumentException("reference dependency format mismatch");
            return new FacadeSpec(name, id, bytes, schemaHash, data.Clone());
        }).ToArray();
        if (strictNames || specs.Any(s=>s.Schema.TryGetProperty("dependencies",out var deps)&&deps.GetArrayLength()!=0))
            foreach(var binding in specs.SelectMany(s=>s.Schema.GetProperty("bindings").EnumerateArray()).Where(b=>b.GetProperty("kind").GetString()=="type"))
                if (!typeNames.Add(binding.GetProperty("name").GetString()!)) throw new ArgumentException("ambiguous type name across reference modules");
        var scoped = specs.Where(s => s.Schema.TryGetProperty("scope", out _)).ToArray();
        FacadeSpec? filteredCore = null;
        if(scoped.Length != 0) {
            string scope = scoped[0].Schema.GetProperty("scope").GetRawText();
            if(scoped.Length != specs.Length || scoped.Any(s=>s.Schema.GetProperty("scope").GetRawText()!=scope)) throw new ArgumentException("mixed scoped compiler views");
            filteredCore = specs.SingleOrDefault(s=>s.Name=="Renraku.CoreLib") ?? throw new ArgumentException("missing filtered core");
        }
        var built = new Dictionary<string, ReferenceInput>(StringComparer.Ordinal);
        var active = new HashSet<string>(StringComparer.Ordinal);
        ReferenceInput Emit(FacadeSpec spec)
        {
            if (built.TryGetValue(spec.Id, out var prior)) return prior;
            if (!active.Add(spec.Id)) throw new ArgumentException("cross-module reference dependency cycle");
            var references = new Dictionary<string, MetadataReference>(StringComparer.Ordinal);
            void AddDependency(FacadeSpec target)
            {
                var emitted = Emit(target);
                if (!references.TryAdd(target.Name, MetadataReference.CreateFromImage(emitted.Image))) return;
                if (target.Schema.TryGetProperty("dependencies", out var nested))
                    foreach (var dep in nested.EnumerateArray()) AddDependency(specs.Single(s => s.Id == dep.GetProperty("moduleId").GetString()));
            }
            if (spec.Schema.TryGetProperty("dependencies", out var dependencies))
            {
                var seen = new HashSet<string>(StringComparer.Ordinal);
                foreach (var dep in dependencies.EnumerateArray())
                {
                    string id = dep.GetProperty("moduleId").GetString()!;
                    var target = specs.SingleOrDefault(s => s.Id == id) ?? throw new ArgumentException("missing reference dependency module");
                    if (!seen.Add(id) || target.Name != dep.GetProperty("moduleName").GetString() || target.Hash != dep.GetProperty("schemaHash").GetString())
                        throw new ArgumentException("reference dependency identity/hash mismatch");
                    var types = target.Schema.GetProperty("bindings").EnumerateArray().Where(b => b.GetProperty("kind").GetString() == "type").Select(b => b.GetProperty("id").GetString()).ToHashSet(StringComparer.Ordinal);
                    var required = dep.GetProperty("types").EnumerateArray().ToArray();
                    if (required.Length is < 1 or > 256 || required.Any(t => !types.Contains(t.GetString()))) throw new ArgumentException("missing dependency type binding");
                    AddDependency(target);
                }
            }
            if(filteredCore is not null && spec != filteredCore) AddDependency(filteredCore);
            string declaration = "[assembly: global::System.Runtime.CompilerServices.ReferenceAssembly]\n" + spec.Schema.GetProperty("declaration").GetString();
            var facade = CSharpCompilation.Create(spec.Name, new[] { CSharpSyntaxTree.ParseText(declaration, parse, "reference.cs", Encoding.UTF8) },
                (filteredCore is null ? new[] { MetadataReference.CreateFromImage(core) } : Array.Empty<MetadataReference>()).Concat(references.OrderBy(r => r.Key, StringComparer.Ordinal).Select(r => r.Value)), options);
            using var buffer = new BoundedImage(MaxAssembly);
            var emitted = facade.Emit(buffer, options: new Microsoft.CodeAnalysis.Emit.EmitOptions(metadataOnly: true, includePrivateMembers: false, runtimeMetadataVersion: "v4.0.30319"));
            if (!emitted.Success) throw new ArgumentException("invalid reference declaration: " + string.Join("; ", emitted.Diagnostics.Where(d => d.Severity == DiagnosticSeverity.Error).Take(8).Select(d => Limit(d.GetMessage(System.Globalization.CultureInfo.InvariantCulture), 256))));
            var result = new ReferenceInput(spec.Name, spec.Bytes, buffer.ToArray(), spec.Hash);
            active.Remove(spec.Id); built.Add(spec.Id, result); return result;
        }
        // Emission is topological; captured manifest order remains pinned input order.
        return specs.Select(Emit).ToArray();
    }
}
