using System.Globalization;
using System.Numerics;
using System.Reflection;
using System.Reflection.Emit;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text.Json;

internal static class SmallCheckedConversionReference
{
    private static byte Opcode(string name)
    {
        bool un=name.EndsWith("Un",StringComparison.Ordinal);
        string target=name.Split("To")[1].Replace("Un","");
        return(target,un) switch{
            ("I8",false)=>0xb3,("U8",false)=>0xb4,("I16",false)=>0xb5,("U16",false)=>0xb6,
            ("I8",true)=>0x82,("U8",true)=>0x86,("I16",true)=>0x83,("U16",true)=>0x87,
            _=>throw new InvalidOperationException(name)};
    }
    public static void WriteRawFixture(byte[] original,string path)
    {
        byte[] bytes=original.ToArray();using var pe=new PEReader(new MemoryStream(original));var metadata=pe.GetMetadataReader();int patched=0;
        foreach(var handle in metadata.MethodDefinitions){
            var method=metadata.GetMethodDefinition(handle);string name=metadata.GetString(method.Name);
            if(!name.StartsWith("Raw",StringComparison.Ordinal))continue;
            int rva=method.RelativeVirtualAddress;var section=pe.PEHeaders.SectionHeaders.Single(s=>rva>=s.VirtualAddress&&rva<s.VirtualAddress+s.VirtualSize);
            int offset=section.PointerToRawData+rva-section.VirtualAddress;int header=(bytes[offset]&3)==2?1:(bytes[offset+1]>>4)*4;
            int length=pe.GetMethodBody(rva).GetILBytes()!.Length;if(length<3)throw new InvalidOperationException("raw conversion exceeds reserved body");
            Array.Clear(bytes,offset+header,length);bytes[offset+header+length-1]=0x2a;
            bytes[offset+header]=2;bytes[offset+header+1]=Opcode(name);bytes[offset+header+2]=0x2a;patched++;
        }
        if(patched!=16)throw new InvalidOperationException("missing raw small checked conversions");File.WriteAllBytes(path,bytes);
    }
    private static DynamicMethod Raw(string name,int width)
    {
        var method=new DynamicMethod(name,typeof(int),new[]{width==32?typeof(int):typeof(long)});var il=method.GetILGenerator();il.Emit(OpCodes.Ldarg_0);
        il.Emit(Opcode(name) switch{0xb3=>OpCodes.Conv_Ovf_I1,0xb4=>OpCodes.Conv_Ovf_U1,0xb5=>OpCodes.Conv_Ovf_I2,0xb6=>OpCodes.Conv_Ovf_U2,
            0x82=>OpCodes.Conv_Ovf_I1_Un,0x86=>OpCodes.Conv_Ovf_U1_Un,0x83=>OpCodes.Conv_Ovf_I2_Un,0x87=>OpCodes.Conv_Ovf_U2_Un,_=>throw new InvalidOperationException(name)});
        il.Emit(OpCodes.Ret);return method;
    }
    public static void Write(Assembly assembly,string output)
    {
        var type=assembly.GetType("Renraku.Guest.SmallCheckedIntegerConversions",throwOnError:true)!;var cases=new List<object>();var random=new Random(0x534d43);
        foreach(bool raw in new[]{false,true})
        foreach(string source in raw?new[]{"I32","I64"}:new[]{"I32","U32","I64","U64"})
        foreach(string target in raw?new[]{"I8","U8","I16","U16"}:new[]{"I8","U8","I16","U16","Char"})
        foreach(bool un in raw?new[]{false,true}:new[]{false}){
            int width=source.EndsWith("32",StringComparison.Ordinal)?32:64;int targetWidth=target.EndsWith("8",StringComparison.Ordinal)?8:16;
            bool sourceUnsigned=raw?un:source.StartsWith('U');bool targetUnsigned=target.StartsWith('U')||target=="Char";
            string name=raw?"Raw"+width+"To"+target+(un?"Un":""):source+"To"+target;
            MethodInfo method=raw?Raw(name,width):type.GetMethod(name)!;
            long[] boundaries={0,1,-1,2,-2,-129,-128,-127,126,127,128,254,255,256,
                -32769,-32768,-32767,32766,32767,32768,65534,65535,65536,
                0xd7ff,0xd800,0xdbff,0xdc00,0xdfff,0xe000,
                int.MinValue-1L,int.MinValue,int.MinValue+1L,int.MaxValue-1L,int.MaxValue,int.MaxValue+1L,
                4294967294,4294967295,4294967296,-4294967296,-4294967297,
                long.MinValue,long.MinValue+1,long.MaxValue-1,long.MaxValue};
            var values=boundaries.ToList();for(int i=0;i<128;i++){byte[] bytes=new byte[8];random.NextBytes(bytes);values.Add(BitConverter.ToInt64(bytes));}
            foreach(long value in values){
                long input=width==32?unchecked((int)value):value;
                object argument=method.GetParameters()[0].ParameterType switch{
                    Type t when t==typeof(uint)=>(object)unchecked((uint)input),Type t when t==typeof(ulong)=>unchecked((ulong)input),
                    Type t when t==typeof(int)=>unchecked((int)input),_=>input};
                BigInteger number=sourceUnsigned?width==32?new BigInteger(unchecked((uint)input)):new BigInteger(unchecked((ulong)input)):new BigInteger(input);
                BigInteger low=targetUnsigned?BigInteger.Zero:-(BigInteger.One<<(targetWidth-1));BigInteger high=(BigInteger.One<<(targetUnsigned?targetWidth:targetWidth-1))-1;
                bool overflow=number<low||number>high;string? result=null,fault=null;
                try{
                    object actual=method.Invoke(null,new[]{argument})!;int bits=actual switch{int n=>n,char n=>n,_=>throw new InvalidOperationException("unexpected result")};
                    if(overflow||new BigInteger(bits)!=number)throw new InvalidOperationException("CoreCLR/BigInteger small checked conversion mismatch");result=bits.ToString(CultureInfo.InvariantCulture);
                }catch(TargetInvocationException error){fault=error.InnerException!.GetType().FullName;if(!overflow||fault!="System.OverflowException")throw;}
                cases.Add(new{name,raw,width,targetWidth,targetUnsigned,sourceUnsigned,input=input.ToString(CultureInfo.InvariantCulture),value=result,fault});
            }
        }
        File.WriteAllText(Path.Combine(output,"small-checked-conversion-reference.json"),JsonSerializer.Serialize(cases));
    }
}
