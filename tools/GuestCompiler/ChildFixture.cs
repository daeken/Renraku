using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
internal static class ChildFixture
{
    public static void WriteRaw(byte[] original,string path)
    {
        byte[] bytes=original.ToArray();using var pe=new PEReader(new MemoryStream(original));var md=pe.GetMetadataReader();
        int method(string type,string name) { var t=md.TypeDefinitions.Single(h=>md.GetString(md.GetTypeDefinition(h).Name)==type);return MetadataTokens.GetToken(md.GetTypeDefinition(t).GetMethods().Single(h=>md.GetString(md.GetMethodDefinition(h).Name)==name)); }
        int field(string name) { var t=md.TypeDefinitions.Single(h=>md.GetString(md.GetTypeDefinition(h).Name)=="PrivateParent");return MetadataTokens.GetToken(md.GetTypeDefinition(t).GetFields().Single(h=>md.GetString(md.GetFieldDefinition(h).Name)==name)); }
        int secretType=MetadataTokens.GetToken(md.TypeDefinitions.Single(h=>md.GetString(md.GetTypeDefinition(h).Name)=="SecretThing"));int patched=0;
        foreach(var h in md.MethodDefinitions) {var m=md.GetMethodDefinition(h);string name=md.GetString(m.Name);if(!name.StartsWith("Raw",StringComparison.Ordinal))continue;
            int rva=m.RelativeVirtualAddress;var section=pe.PEHeaders.SectionHeaders.Single(s=>rva>=s.VirtualAddress&&rva<s.VirtualAddress+s.VirtualSize);int offset=section.PointerToRawData+rva-section.VirtualAddress;int header=(bytes[offset]&3)==2?1:(bytes[offset+1]>>4)*4;int length=pe.GetMethodBody(rva).GetILBytes()!.Length;
            var body=new List<byte>();int token;
            switch(name){case "RawCall":body.Add(0x28);token=method("PrivateParent","HiddenRead");break;case "RawField":body.Add(0x7e);token=field("Number");break;case "RawCast":body.Add(2);body.Add(0x75);token=secretType;break;case "RawNew":body.Add(0x73);token=method("SecretThing",".ctor");break;default:throw new InvalidOperationException(name);}
            body.AddRange(BitConverter.GetBytes(token));body.Add(0x2a);if(body.Count>length)throw new InvalidOperationException("child fixture reserve too small");Array.Clear(bytes,offset+header,length);body.ToArray().CopyTo(bytes,offset+header);bytes[offset+header+length-1]=0x2a;patched++;
        }
        if(patched!=4)throw new InvalidOperationException("missing raw child attacks");File.WriteAllBytes(path,bytes);
    }
    public static void WriteLargeStack(byte[] original,string path)
    {
        byte[] bytes=original.ToArray();using var pe=new PEReader(new MemoryStream(original));var md=pe.GetMetadataReader();
        var h=md.MethodDefinitions.Single(h=>md.GetString(md.GetMethodDefinition(h).Name)=="RawCast");int rva=md.GetMethodDefinition(h).RelativeVirtualAddress;
        var section=pe.PEHeaders.SectionHeaders.Single(s=>rva>=s.VirtualAddress&&rva<s.VirtualAddress+s.VirtualSize);int offset=section.PointerToRawData+rva-section.VirtualAddress;
        if((bytes[offset]&3)!=3)throw new InvalidOperationException("child large-stack fixture needs fat header");
        bytes[offset+2]=1;bytes[offset+3]=1;File.WriteAllBytes(path,bytes);
    }

}
