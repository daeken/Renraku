using System.Reflection;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text.Json;
internal static class ReferenceCastReference
{
    public static void WriteInvalidFixtures(byte[] original,string output)
    {
        using var pe=new PEReader(new MemoryStream(original));var metadata=pe.GetMetadataReader();
        var method=metadata.MethodDefinitions.Select(h=>metadata.GetMethodDefinition(h)).Single(m=>metadata.GetString(m.Name)=="CastCounter");
        int rva=method.RelativeVirtualAddress;var section=pe.PEHeaders.SectionHeaders.Single(s=>rva>=s.VirtualAddress&&rva<s.VirtualAddress+s.VirtualSize);
        int offset=section.PointerToRawData+rva-section.VirtualAddress;int header=(original[offset]&3)==2?1:(original[offset+1]>>4)*4;
        if(original[offset+header]!=2||original[offset+header+1]!=0x74)throw new InvalidOperationException("unexpected cast fixture body");
        foreach(uint token in new uint[]{0,0x0200ffff,0x0100ffff,0x04000001,0x1b000001}) {
            byte[] bytes=original.ToArray();BitConverter.GetBytes(token).CopyTo(bytes,offset+header+2);
            File.WriteAllBytes(Path.Combine(output,$"BadCast-{token:x8}.dll"),bytes);
        }
    }
    public static void Write(Assembly assembly,string output)
    {
        var type=assembly.GetType("Renraku.Guest.ReferenceCasts",throwOnError:true)!;var cases=new List<object>();
        string[] names={"AsBase","AsDerived","AsLeaf","AsOther","AsCounter","AsString","CastBase","CastDerived","CastLeaf","CastOther","CastCounter","CastString","IsBase","IsDerived","IsCounter","IsString","ReadDerived","ClearThenAs","ClearThenCast"};
        for(int input=0;input<9;input++){
            object? value=type.GetMethod("Create")!.Invoke(null,new object[]{input});
            foreach(string name in names){
                string kind="null";int? scalar=null;string? fault=null;
                try{
                    object? actual=type.GetMethod(name)!.Invoke(null,new[]{value});
                    if(actual is bool flag){kind="i32";scalar=flag?1:0;}
                    else if(actual is int number){kind="i32";scalar=number;}
                    else if(actual!=null){if(!ReferenceEquals(value,actual))throw new InvalidOperationException("cast changed identity");kind="same";}
                }catch(TargetInvocationException error){fault=error.InnerException!.GetType().FullName;if(fault!="System.InvalidCastException")throw;}
                cases.Add(new{input,method=name,kind,scalar,fault});
            }
        }
        File.WriteAllText(Path.Combine(output,"reference-cast-reference.json"),JsonSerializer.Serialize(cases));
    }
}
