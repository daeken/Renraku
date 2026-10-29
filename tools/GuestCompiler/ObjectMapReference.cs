using System.Reflection;
using System.Text.Json;
internal static class ObjectMapReference
{
    public static void Write(Assembly assembly,string output)
    {
        var type=assembly.GetType("Renraku.Tests.ObjectMapHarness",throwOnError:true)!;
        type.GetMethod("Initialize")!.Invoke(null,new object[]{new string(new[]{'s','a','m','e','\ud800'}),new string(new[]{'s','a','m','e','\ud800'})});
        object?[] pool=Enumerable.Range(0,6).Select(id=>type.GetMethod("Pool")!.Invoke(null,new object[]{id})).ToArray();
        if(ReferenceEquals(pool[3],pool[4]))throw new InvalidOperationException("oracle string identities collapsed");
        var dictionary=new Dictionary<string,object?>(StringComparer.Ordinal);var trace=new List<object>();
        int Tag(object? value){if(value==null)return 0;for(int i=1;i<pool.Length;i++)if(ReferenceEquals(value,pool[i]))return i;throw new InvalidOperationException("unknown result identity");}
        void Apply(string operation,string? key,int value=0){
            bool success=false,found=false,changed=false;object? observed=null;string error="none";
            if(key==null)error="null_key";else if(key.Length>128)error="key_too_long";else{
                found=dictionary.TryGetValue(key,out object? previous);
                if(operation=="Get"||operation=="Remove"){
                    if(!found)error="missing";else{success=true;observed=previous;if(operation=="Remove"){dictionary.Remove(key);changed=true;}}
                }else if(!found&&dictionary.Count==64)error="full";
                else{success=true;changed=!found||!ReferenceEquals(previous,pool[value]);if(changed)dictionary[key]=pool[value];observed=dictionary[key];}
            }
            object result=type.GetMethod(operation)!.Invoke(null,operation=="Set"?new object?[]{key,pool[value]}:new object?[]{key})!;var resultType=result.GetType();object? Field(string name)=>resultType.GetField(name)!.GetValue(result);
            if((bool)Field("Success")! !=success||(bool)Field("Found")! !=found||(bool)Field("Changed")! !=changed||!ReferenceEquals(Field("Value"),observed)||(string)Field("Error")! !=error||(int)Field("Count")! !=dictionary.Count||(int)type.GetMethod("Count")!.Invoke(null,null)! !=dictionary.Count)throw new InvalidOperationException("managed object map disagrees with ordinal/reference oracle");
            trace.Add(new{operation,key=key?.Select(c=>(ushort)c).ToArray(),value,success,found,changed,observed=Tag(observed),error,count=dictionary.Count});
        }
        Apply("Get",null);Apply("Set",null,1);Apply("Remove",null);Apply("Get","");Apply("Set","",0);Apply("Get","");Apply("Set","",0);
        foreach(string key in new[]{"a","A","I","\u0131","\u00e9","e\u0301","\ud800","\udc00","\ud83d\ude00","\0","a\0b",new string('k',128)}){Apply("Set",key,1);Apply("Get",key);Apply("Set",key,1);Apply("Set",key,2);}
        Apply("Set","strings",3);Apply("Set","strings",4);Apply("Set","strings",4);Apply("Get","strings");Apply("Set","self",5);Apply("Get","self");
        Apply("Set",new string('k',129),1);Apply("Get",new string('k',129));Apply("Remove",new string('k',129));
        foreach(string key in dictionary.Keys.ToArray())Apply("Remove",key);Apply("Remove","absent");
        for(int i=0;i<64;i++)Apply("Set","k"+i,i%6);Apply("Set","full",1);Apply("Set","k0",2);Apply("Set","k0",2);
        foreach(string key in new[]{"k63","k31","k0"})Apply("Remove",key);foreach(string key in new[]{"new-head","new-middle","new-tail"})Apply("Set",key,1);
        var random=new Random(0x4f424a);for(int i=0;i<256;i++)Apply(new[]{"Get","Set","Remove"}[random.Next(3)],"k"+random.Next(80),random.Next(6));
        foreach(string key in dictionary.Keys.ToArray())Apply("Get",key);
        File.WriteAllText(Path.Combine(output,"object-map-reference.json"),JsonSerializer.Serialize(trace));
    }
}
