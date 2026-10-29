using System.Globalization;
using System.Numerics;
using System.Reflection;
using System.Reflection.Emit;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Text.Json;

internal static class CheckedConversionReference
{
    private static byte Opcode(string name)
    {
        bool un = name.EndsWith("Un", StringComparison.Ordinal);
        string target = name.Split("To")[1].Replace("Un", "");
        return (target, un) switch {
            ("I32",false)=>0xb7,("U32",false)=>0xb8,("I64",false)=>0xb9,("U64",false)=>0xba,
            ("I32",true)=>0x84,("U32",true)=>0x88,("I64",true)=>0x85,("U64",true)=>0x89,
            _ => throw new InvalidOperationException(name) };
    }
    public static void WriteRawFixture(byte[] original, string path)
    {
        byte[] bytes=original.ToArray();using var pe=new PEReader(new MemoryStream(original));
        var metadata=pe.GetMetadataReader();int patched=0;
        foreach (var handle in metadata.MethodDefinitions) {
            var method=metadata.GetMethodDefinition(handle);string name=metadata.GetString(method.Name);
            if (!name.StartsWith("Raw",StringComparison.Ordinal)) continue;
            int rva=method.RelativeVirtualAddress;
            var section=pe.PEHeaders.SectionHeaders.Single(s=>rva>=s.VirtualAddress&&rva<s.VirtualAddress+s.VirtualSize);
            int offset=section.PointerToRawData+rva-section.VirtualAddress;
            int header=(bytes[offset]&3)==2?1:(bytes[offset+1]>>4)*4;
            int length=pe.GetMethodBody(rva).GetILBytes()!.Length;
            if (length<3) throw new InvalidOperationException("raw conversion exceeds reserved method");
            Array.Clear(bytes,offset+header,length);bytes[offset+header+length-1]=0x2a;
            bytes[offset+header]=2;bytes[offset+header+1]=Opcode(name);bytes[offset+header+2]=0x2a;patched++;
        }
        if(patched!=16) throw new InvalidOperationException("missing raw checked conversions");
        File.WriteAllBytes(path,bytes);
    }
    private static DynamicMethod Raw(string name,int width,int targetWidth)
    {
        var method=new DynamicMethod(name,targetWidth==32?typeof(int):typeof(long),new[]{width==32?typeof(int):typeof(long)});
        var il=method.GetILGenerator();il.Emit(OpCodes.Ldarg_0);
        il.Emit(Opcode(name) switch {0xb7=>OpCodes.Conv_Ovf_I4,0xb8=>OpCodes.Conv_Ovf_U4,
            0xb9=>OpCodes.Conv_Ovf_I8,0xba=>OpCodes.Conv_Ovf_U8,0x84=>OpCodes.Conv_Ovf_I4_Un,
            0x88=>OpCodes.Conv_Ovf_U4_Un,0x85=>OpCodes.Conv_Ovf_I8_Un,0x89=>OpCodes.Conv_Ovf_U8_Un,
            _=>throw new InvalidOperationException(name)});il.Emit(OpCodes.Ret);return method;
    }
    public static void Write(Assembly assembly,string output)
    {
        var type=assembly.GetType("Renraku.Guest.CheckedIntegerConversions",throwOnError:true)!;
        var cases=new List<object>();var random=new Random(0x434f4e);
        foreach (bool raw in new[]{false,true})
        foreach (string source in raw?new[]{"I32","I64"}:new[]{"I32","U32","I64","U64"})
        foreach (string target in new[]{"I32","U32","I64","U64"})
        foreach (bool un in raw?new[]{false,true}:new[]{false}) {
            int width=source.EndsWith("32",StringComparison.Ordinal)?32:64;
            int targetWidth=target.EndsWith("32",StringComparison.Ordinal)?32:64;
            bool sourceUnsigned=raw?un:source.StartsWith('U');bool targetUnsigned=target.StartsWith('U');
            string name=raw?"Raw"+width+"To"+target+(un?"Un":""):source+"To"+target;
            MethodInfo method=raw?Raw(name,width,targetWidth):type.GetMethod(name)!;
            long[] boundaries={0,1,-1,2,-2,127,128,255,256,32767,32768,65535,65536,
                int.MinValue-1L,int.MinValue,int.MinValue+1L,int.MaxValue-1L,int.MaxValue,int.MaxValue+1L,
                4294967294,4294967295,4294967296,4294967297,-4294967296,-4294967297,
                long.MinValue,long.MinValue+1,long.MaxValue-1,long.MaxValue};
            var values=boundaries.ToList();
            for(int i=0;i<128;i++){byte[] bytes=new byte[8];random.NextBytes(bytes);values.Add(BitConverter.ToInt64(bytes));}
            foreach(long value in values){
                long input=width==32?unchecked((int)value):value;
                object argument=method.GetParameters()[0].ParameterType switch{
                    Type t when t==typeof(uint)=>(object)unchecked((uint)input),
                    Type t when t==typeof(ulong)=>unchecked((ulong)input),
                    Type t when t==typeof(int)=>unchecked((int)input),_=>input};
                BigInteger number=sourceUnsigned?width==32?new BigInteger(unchecked((uint)input)):new BigInteger(unchecked((ulong)input)):new BigInteger(input);
                BigInteger low=targetUnsigned?BigInteger.Zero:-(BigInteger.One<<(targetWidth-1));
                BigInteger high=(BigInteger.One<<(targetUnsigned?targetWidth:targetWidth-1))-1;
                bool overflow=number<low||number>high;string? result=null,fault=null;
                try{
                    object actual=method.Invoke(null,new[]{argument})!;
                    long bits=actual switch{int n=>n,uint n=>unchecked((int)n),long n=>n,ulong n=>unchecked((long)n),_=>throw new InvalidOperationException("unexpected result")};
                    BigInteger observed=targetUnsigned?targetWidth==32?new BigInteger(unchecked((uint)bits)):new BigInteger(unchecked((ulong)bits)):new BigInteger(bits);
                    if(overflow||observed!=number)throw new InvalidOperationException("CoreCLR/BigInteger conversion mismatch");
                    result=bits.ToString(CultureInfo.InvariantCulture);
                }catch(TargetInvocationException error){fault=error.InnerException!.GetType().FullName;if(!overflow||fault!="System.OverflowException")throw;}
                cases.Add(new{name,raw,width,targetWidth,targetUnsigned,sourceUnsigned,input=input.ToString(CultureInfo.InvariantCulture),value=result,fault});
            }
        }
        File.WriteAllText(Path.Combine(output,"checked-conversion-reference.json"),JsonSerializer.Serialize(cases));
    }
}
