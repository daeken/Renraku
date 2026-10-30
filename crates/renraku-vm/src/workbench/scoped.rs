//! Versioned declaration filters. A filter describes a view; only the trusted
//! bootstrap record can authorize its installation or eventual child admission.
use super::*;
use crate::{
    endowment::Endowment,
    identity::{CodeVersionId, FieldId},
};
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ApiFilter {
    pub types: BTreeSet<String>,
    pub methods: BTreeSet<String>,
    pub construct: BTreeSet<String>,
    pub read: BTreeSet<String>,
    pub write: BTreeSet<String>,
}
impl ApiFilter {
    pub fn from_endowment(e: &Endowment) -> Self {
        Self {
            types: e.types.iter().map(ToString::to_string).collect(),
            methods: e.methods.iter().map(ToString::to_string).collect(),
            construct: e.construct.iter().map(ToString::to_string).collect(),
            read: e.read.iter().map(ToString::to_string).collect(),
            write: e.write.iter().map(ToString::to_string).collect(),
        }
    }
    pub fn field(&self, id: FieldId) -> bool {
        self.read.contains(&id.to_string()) || self.write.contains(&id.to_string())
    }
    pub fn method(&self, id: CodeVersionId) -> bool {
        self.methods.contains(&id.to_string())
    }
    pub fn allow_type(&self, id: TypeId) -> Result<()> {
        require(
            self.types.contains(&id.to_string()),
            "reference type is outside bootstrap scope",
        )
    }
    pub fn validate_against(&self, p: &Program) -> Result<()> {
        self.validate()?;
        let slot = |s: crate::program::SlotType| -> Result<()> {
            if let crate::program::SlotType::Reference(t) = s {
                self.allow_type(t)?;
            }
            Ok(())
        };
        for id in &self.types {
            let t = p.find_type(id)?;
            if let Some(base) = p.types[&t].base {
                self.allow_type(base)?;
            }
        }
        for id in &self.construct {
            p.allocatable(p.find_type(id)?)?;
        }
        for id in self.read.iter().chain(&self.write) {
            let id = FieldId::from_bytes(crate::placement::id(id)?);
            let f = p
                .fields
                .get(&id)
                .ok_or_else(|| Error("unknown scoped field".into()))?;
            require(!f.literal, "literal is not a stored field")?;
            self.allow_type(f.owner)?;
            slot(f.value_type)?;
        }
        for id in &self.methods {
            let id = CodeVersionId::from_bytes(crate::placement::id(id)?);
            let m = p
                .methods
                .get(&id)
                .ok_or_else(|| Error("unknown scoped method".into()))?;
            self.allow_type(m.owner)?;
            for s in m
                .signature
                .parameters
                .iter()
                .chain(m.signature.result.iter())
            {
                slot(*s)?;
            }
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<()> {
        for (s, max) in [
            (&self.types, 128),
            (&self.methods, 256),
            (&self.construct, 128),
            (&self.read, 256),
            (&self.write, 256),
        ] {
            require(s.len() <= max, "bootstrap API list exceeds bound")?;
            for id in s {
                crate::placement::id(id)?;
            }
        }
        require(
            self.construct.is_subset(&self.types),
            "construction type outside scope",
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ScopeBinding {
    pub format: u32,
    pub universe: String,
    pub approval: String,
    pub filter: ApiFilter,
}
impl ScopeBinding {
    pub fn validate(&self) -> Result<()> {
        require(self.format == 1, "unsupported bootstrap scope version")?;
        crate::placement::id(&self.universe)?;
        require(
            self.approval.len() == 64
                && self
                    .approval
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "invalid bootstrap approval hash",
        )?;
        self.filter.validate()
    }
}
// Fixed compiler scaffolding is deliberately not a general CoreLib API export.
// These declarations support C#'s predefined types and reference marker only;
// their presence cannot authorize their tokens at child admission/runtime.
pub(super) fn core_schema(
    p: &Program,
    roots: &[TypeId],
    scope: &ScopeBinding,
    module: AssemblyId,
    image: &Image<'_>,
    core: String,
) -> Result<(Schema, String, BTreeMap<AssemblyId, BTreeSet<TypeId>>)> {
    require(
        roots == [p.find_type("System.Object")?],
        "scoped core root must be Object",
    )?;
    scope.filter.allow_type(roots[0])?;
    let ctor = p
        .methods
        .values()
        .find(|m| m.owner == roots[0] && m.name == ".ctor")
        .ok_or_else(|| Error("core Object constructor missing".into()))?;
    let token = p
        .method_tokens
        .iter()
        .find(|((m, t), id)| *m == module && *t >> 24 == 6 && **id == ctor.binding.method)
        .map(|((_, t), _)| *t & 0xffffff)
        .ok_or_else(|| Error("core constructor binding missing".into()))?;
    require(
        ctor.signature.instance
            && ctor.signature.parameters.is_empty()
            && ctor.signature.result.is_none()
            && image.row(6, token)?[2] & 7 == 6,
        "unsupported core Object constructor profile",
    )?;
    let expose = scope.filter.method(ctor.binding.code)
        && scope.filter.construct.contains(&roots[0].to_string());
    for m in p
        .methods
        .values()
        .filter(|m| m.assembly == module && scope.filter.method(m.binding.code))
    {
        require(
            m.binding == ctor.binding,
            "scoped core currently exports only Object construction",
        )?;
    }
    require(
        !p.fields
            .values()
            .any(|f| type_module(p, f.owner) == module && scope.filter.field(f.id)),
        "scoped core fields are unsupported",
    )?;
    let access = if expose { "public" } else { "private" };
    let mut declaration = format!(
        "namespace System {{ public class Object {{ {access} extern Object(); }} public abstract class ValueType {{ protected extern ValueType(); }} public abstract class Enum : ValueType {{ protected extern Enum(); }} public class Attribute {{ protected extern Attribute(); }} public abstract class Array {{ protected extern Array(); }} public abstract class Delegate {{ protected extern Delegate(); }} public sealed class String {{ private extern String(); }}"
    );
    for name in [
        "Void", "Boolean", "Char", "SByte", "Byte", "Int16", "UInt16", "Int32", "UInt32", "Int64",
        "UInt64", "IntPtr", "UIntPtr", "Single", "Double",
    ] {
        declaration.push_str(&format!(" public struct {name} {{ }}"));
    }
    declaration.push_str(" public enum AttributeTargets { Assembly=1,Module=2,Class=4,Struct=8,Enum=16,Constructor=32,Method=64,Property=128,Field=256,Event=512,Interface=1024,Parameter=2048,Delegate=4096,ReturnValue=8192,GenericParameter=16384,All=32767 } public sealed class AttributeUsageAttribute : Attribute { public extern AttributeUsageAttribute(AttributeTargets validOn); public extern bool AllowMultiple {get;set;} public extern bool Inherited {get;set;} } } namespace System.Runtime.CompilerServices { public sealed class ReferenceAssemblyAttribute : System.Attribute { public extern ReferenceAssemblyAttribute(); } }");
    let mut bindings = vec![Binding {
        kind: "type".into(),
        id: roots[0].to_string(),
        name: "System.Object".into(),
        signature: "compiler scaffold; no implicit authority".into(),
    }];
    if expose {
        bindings.push(Binding {
            kind: "method".into(),
            id: ctor.binding.method.to_string(),
            name: "System.Object::.ctor".into(),
            signature: "public extern Object();".into(),
        });
    }
    Ok((
        Schema {
            module_id: module.to_string(),
            module_name: "Renraku.CoreLib".into(),
            roots: vec![roots[0].to_string()],
            declaration,
            bindings,
            dependencies: vec![],
            scope: Some(scope.clone()),
        },
        core,
        BTreeMap::new(),
    ))
}
fn type_module(p: &Program, id: TypeId) -> AssemblyId {
    p.type_tokens
        .iter()
        .find(|((_, t), v)| t >> 24 == 2 && **v == id)
        .map(|((m, _), _)| *m)
        .unwrap()
}
impl Program {
    pub(crate) fn scoped_references(&self, scope: &ScopeBinding) -> Result<ReferenceSet> {
        scope.validate()?;
        scope.filter.validate_against(self)?;
        let core = self.find_type("System.Object")?;
        let mut selected: BTreeMap<AssemblyId, BTreeSet<TypeId>> = BTreeMap::new();
        for id in &scope.filter.types {
            let id = self.find_type(id)?;
            let module = self
                .type_tokens
                .iter()
                .find(|((_, token), actual)| token >> 24 == 2 && **actual == id)
                .map(|((m, _), _)| *m)
                .ok_or_else(|| Error("unknown bootstrap type".into()))?;
            // Core language types use the fixed scaffold. Other CoreLib APIs are
            // explicitly outside this first scoped compiler profile.
            if type_module(self, id) == type_module(self, core) {
                require(
                    id == core || id == self.string_type()?,
                    "scoped core class API unsupported",
                )?;
                continue;
            }
            selected.entry(module).or_default().insert(id);
        }
        selected.insert(type_module(self, core), BTreeSet::from([core]));
        let mut schemas = Vec::new();
        let mut cores = Vec::new();
        for ids in selected.values() {
            let (mut s, c, foreign) =
                self.filtered_schema(&ids.iter().copied().collect::<Vec<_>>(), true, Some(scope))?;
            for (m, types) in foreign {
                require(
                    selected.get(&m).is_some_and(|ids| types.is_subset(ids)),
                    "missing endowed dependency type",
                )?;
                let i = Image::parse(&self.images[&m])?;
                s.dependencies.push(references::Dependency {
                    module_id: m.to_string(),
                    module_name: i.string(i.row(32, 1)?[7])?.into(),
                    schema_hash: String::new(),
                    types: types.iter().map(ToString::to_string).collect(),
                });
            }
            schemas.push(s);
            cores.push(c);
        }
        let mut views: Vec<ReferenceView> = Vec::new();
        for i in references::ordering(&schemas)? {
            let mut s = schemas[i].clone();
            for d in &mut s.dependencies {
                d.schema_hash = views
                    .iter()
                    .find(|v| {
                        serde_json::from_str::<Schema>(&v.schema_json)
                            .is_ok_and(|s| s.module_id == d.module_id)
                    })
                    .unwrap()
                    .schema_hash
                    .clone();
            }
            views.push(references::view(s, cores[i].clone())?);
        }
        ReferenceSet::from_views(views)
    }
}
