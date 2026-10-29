use crate::{
    cil::{self, Instruction, Op, Reader},
    error::{Error, Result, require},
    execution::MethodVersion,
    identity::{AssemblyId, CodeVersionId, FieldId, MethodId, TypeId},
    image::Image,
    value::Value,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotType {
    I32,
    I64,
    Boolean,
    Char,
    Reference(TypeId),
}
impl SlotType {
    pub fn default_value(self) -> Value {
        match self {
            Self::I32 | Self::Boolean | Self::Char => Value::I32(0),
            Self::I64 => Value::I64(0),
            Self::Reference(_) => Value::Null,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StringIntrinsic {
    Length,
    CharAt,
    Concat,
    Equal,
    NotEqual,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intrinsic {
    String(StringIntrinsic),
    ModelComplete,
    ModelChat,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Signature {
    pub instance: bool,
    pub parameters: Vec<SlotType>,
    pub result: Option<SlotType>,
}

#[derive(Debug)]
pub struct TypeDefinition {
    pub id: TypeId,
    pub name: String,
    pub base: Option<TypeId>,
    pub fields: Vec<FieldId>,
    pub(crate) flags: u32,
}
#[derive(Debug)]
pub struct FieldDefinition {
    pub id: FieldId,
    pub owner: TypeId,
    pub name: String,
    pub value_type: SlotType,
    pub is_static: bool,
    pub(crate) literal: bool,
}
#[derive(Debug)]
pub struct Method {
    pub(crate) intrinsic: Option<Intrinsic>,
    pub binding: MethodVersion,
    pub owner: TypeId,
    pub name: String,
    pub signature: Signature,
    pub locals: Vec<SlotType>,
    pub initialize_locals: bool,
    pub max_stack: usize,
    pub instructions: BTreeMap<u32, Instruction>,
    pub(crate) assembly: AssemblyId,
}

/// Owned code and logical bindings. Raw assembly bytes are retained for future
/// checkpoint storage; parser views never appear in frames or managed values.
#[derive(Debug, Default)]
pub struct Program {
    pub(crate) string_literals: BTreeMap<(AssemblyId, u32), Vec<u16>>,
    pub(crate) type_tokens: BTreeMap<(AssemblyId, u32), TypeId>,
    pub(crate) types: BTreeMap<TypeId, TypeDefinition>,
    pub(crate) fields: BTreeMap<FieldId, FieldDefinition>,
    pub(crate) methods: BTreeMap<CodeVersionId, Method>,
    pub(crate) dispatch: BTreeMap<MethodId, CodeVersionId>,
    pub(crate) method_tokens: BTreeMap<(AssemblyId, u32), MethodId>,
    pub(crate) field_tokens: BTreeMap<(AssemblyId, u32), FieldId>,
    pub(crate) images: BTreeMap<AssemblyId, Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AssemblyBindings {
    pub id: AssemblyId,
    pub bytes: Vec<u8>,
    pub types: BTreeMap<u32, TypeId>,
    pub fields: BTreeMap<u32, FieldId>,
    pub methods: BTreeMap<u32, MethodVersion>,
}

type Names = BTreeMap<(String, String), TypeId>;
struct Resolver<'a, 'b> {
    image: &'a Image<'b>,
    assembly: &'a str,
    names: &'a Names,
    own_types: &'a BTreeMap<u32, TypeId>,
}
impl Resolver<'_, '_> {
    fn builtin(&self, name: &str) -> Result<TypeId> {
        let matches: Vec<_> = self
            .names
            .iter()
            .filter(|((_, n), _)| n == name)
            .map(|(_, id)| *id)
            .collect();
        require(
            matches.len() == 1,
            &format!("{name} binding is ambiguous or missing"),
        )?;
        Ok(matches[0])
    }
    fn named(&self, assembly: &str, name: &str) -> Result<TypeId> {
        self.names
            .get(&(assembly.into(), name.into()))
            .copied()
            .ok_or_else(|| Error(format!("unresolved type {assembly}::{name}")))
    }
    fn type_token(&self, token: u32) -> Result<TypeId> {
        match token >> 24 {
            2 => self
                .own_types
                .get(&(token & 0xffffff))
                .copied()
                .ok_or_else(|| Error("invalid TypeDef token".into())),
            1 => {
                let row = self.image.row(1, token & 0xffffff)?;
                let scope = row[0];
                let assembly = match scope & 3 {
                    0 => self.assembly,
                    2 => self.image.string(self.image.row(35, scope >> 2)?[6])?,
                    _ => {
                        return Err(Error(
                            "nested and module-scoped TypeRefs are unsupported".into(),
                        ));
                    }
                };
                self.named(
                    assembly,
                    &qualified(self.image.string(row[2])?, self.image.string(row[1])?),
                )
            }
            _ => Err(Error(format!("unsupported type token {token:08x}"))),
        }
    }
    fn coded_type(&self, coded: u32) -> Result<TypeId> {
        let table = match coded & 3 {
            0 => 2,
            1 => 1,
            _ => return Err(Error("TypeSpec is unsupported".into())),
        };
        self.type_token((table << 24) | (coded >> 2))
    }
    fn slot(
        &self,
        reader: &mut Reader<'_>,
        types: &BTreeMap<TypeId, TypeDefinition>,
    ) -> Result<SlotType> {
        Ok(match reader.u8()? {
            0x02 => SlotType::Boolean,
            0x03 => SlotType::Char,
            0x0e => SlotType::Reference(self.builtin("System.String")?),
            0x08 | 0x09 => SlotType::I32,
            0x0a | 0x0b => SlotType::I64,
            0x12 => SlotType::Reference(self.coded_type(reader.compressed()?)?),
            0x11 => {
                let id = self.coded_type(reader.compressed()?)?;
                let ty = &types[&id];
                let base = ty.base.and_then(|base| types.get(&base));
                require(
                    base.is_some_and(|base| base.name == "System.Enum"),
                    "only Int32 enum value types are supported",
                )?;
                SlotType::I32
            }
            0x1c => SlotType::Reference(self.builtin("System.Object")?),
            tag => return Err(Error(format!("unsupported signature element {tag:02x}"))),
        })
    }
    fn signature(
        &self,
        blob: &[u8],
        types: &BTreeMap<TypeId, TypeDefinition>,
    ) -> Result<Signature> {
        let mut reader = Reader::new(blob);
        let convention = reader.u8()?;
        require(
            convention == 0 || convention == 0x20,
            "only nongeneric default managed signatures are supported",
        )?;
        let count = reader.compressed()? as usize;
        let result = if reader.data.get(reader.position) == Some(&1) {
            reader.u8()?;
            None
        } else {
            Some(self.slot(&mut reader, types)?)
        };
        let mut parameters = Vec::new();
        for _ in 0..count {
            parameters.push(self.slot(&mut reader, types)?);
        }
        require(
            reader.position == blob.len(),
            "trailing method signature data",
        )?;
        Ok(Signature {
            instance: convention & 0x20 != 0,
            parameters,
            result,
        })
    }
}

fn qualified(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.into()
    } else {
        format!("{namespace}.{name}")
    }
}
fn member_range(start: u32, end: u32, count: usize) -> Result<std::ops::Range<u32>> {
    require(
        start >= 1 && start <= end && end as usize <= count + 1,
        "invalid member list range",
    )?;
    Ok(start..end)
}

impl Program {
    /// Append implementation modules to a preview registry. Existing code and
    /// logical bindings remain immutable; reference facades are not inputs here.
    pub fn link_modules(
        &self,
        additions: &[&[u8]],
        views: &[crate::workbench::ReferenceView],
    ) -> Result<Self> {
        require(
            !additions.is_empty() && additions.len() <= 16,
            "link between 1 and 16 implementation modules",
        )?;
        crate::workbench::validate_reference_views(views, self)?;
        let mut names = BTreeSet::new();
        for bytes in additions {
            require(
                bytes.len() <= 16 * 1024 * 1024,
                "implementation module exceeds its size limit",
            )?;
            let image = Image::parse(bytes)?;
            require(
                names.insert(image.string(image.row(32, 1)?[7])?.to_owned()),
                "duplicate added module",
            )?;
        }
        let mut old = self.bindings();
        let mut bytes: Vec<_> = old.iter().map(|b| b.bytes.as_slice()).collect();
        bytes.extend_from_slice(additions);
        let fresh = Self::load(&bytes)?;
        let mut added = Vec::new();
        for binding in fresh.bindings() {
            let image = Image::parse(&binding.bytes)?;
            if names.contains(image.string(image.row(32, 1)?[7])?) {
                added.push(binding);
            }
        }
        old.extend(added);
        Self::from_bindings(&old)
    }
    pub(crate) fn extends(&self, prior: &Self) -> Result<()> {
        let current: BTreeMap<_, _> = self.bindings().into_iter().map(|b| (b.id, b)).collect();
        require(
            prior
                .bindings()
                .iter()
                .all(|b| current.get(&b.id) == Some(b)),
            "installation must retain all existing implementation images and logical bindings",
        )
    }
    pub fn load(assembly_bytes: &[&[u8]]) -> Result<Self> {
        Self::load_inner(assembly_bytes, None)
    }
    pub(crate) fn from_bindings(bindings: &[AssemblyBindings]) -> Result<Self> {
        let bytes: Vec<_> = bindings.iter().map(|b| b.bytes.as_slice()).collect();
        let program = Self::load_inner(&bytes, Some(bindings))?;
        let actual: BTreeMap<_, _> = program.bindings().into_iter().map(|b| (b.id, b)).collect();
        let expected: BTreeMap<_, _> = bindings.iter().cloned().map(|b| (b.id, b)).collect();
        require(
            expected.len() == bindings.len() && actual == expected,
            "saved identity bindings do not match the assemblies",
        )?;
        Ok(program)
    }
    pub(crate) fn bindings(&self) -> Vec<AssemblyBindings> {
        self.images
            .iter()
            .map(|(&id, bytes)| AssemblyBindings {
                id,
                bytes: bytes.clone(),
                types: self
                    .type_tokens
                    .iter()
                    .filter(|((assembly, token), _)| *assembly == id && token >> 24 == 2)
                    .map(|((_, token), id)| (*token, *id))
                    .collect(),
                fields: self
                    .field_tokens
                    .iter()
                    .filter(|((assembly, token), _)| *assembly == id && token >> 24 == 4)
                    .map(|((_, token), id)| (*token, *id))
                    .collect(),
                methods: self
                    .method_tokens
                    .iter()
                    .filter(|((assembly, token), _)| *assembly == id && token >> 24 == 6)
                    .map(|((_, token), id)| (*token, self.methods[&self.dispatch[id]].binding))
                    .collect(),
            })
            .collect()
    }
    fn load_inner(assembly_bytes: &[&[u8]], bindings: Option<&[AssemblyBindings]>) -> Result<Self> {
        let images: Vec<_> = assembly_bytes
            .iter()
            .map(|bytes| Image::parse(bytes))
            .collect::<Result<_>>()?;
        let mut program = Self::default();
        let mut names = Names::new();
        let mut assembly_names = BTreeSet::new();
        let mut modules = Vec::new();
        for (image_index, image) in images.iter().enumerate() {
            require(
                !image.is_reference_assembly()?,
                "reference assemblies are compiler inputs, not executable implementations",
            )?;
            let saved = bindings.map(|bindings| &bindings[image_index]);
            require(
                image.rows(32).len() == 1,
                "exactly one Assembly definition is required",
            )?;
            let assembly_name = image.string(image.row(32, 1)?[7])?.to_owned();
            require(
                assembly_names.insert(assembly_name.clone()),
                "duplicate assembly identity",
            )?;
            for table in [
                3, 5, 7, 9, 13, 14, 16, 19, 22, 25, 27, 28, 29, 30, 31, 41, 42, 43, 44,
            ] {
                require(
                    image.rows(table).is_empty(),
                    &format!("unsupported metadata table {table} in {assembly_name}"),
                )?;
            }
            let assembly_id = saved.map_or_else(AssemblyId::fresh, |b| b.id);
            require(
                !program.images.contains_key(&assembly_id),
                "duplicate assembly ID",
            )?;
            let mut own_types = BTreeMap::new();
            for (index, row) in image.rows(2).iter().enumerate() {
                let name = qualified(image.string(row[2])?, image.string(row[1])?);
                if name == "<Module>" {
                    continue;
                }
                require(row[0] & 0x20 == 0, "interfaces are unsupported")?;
                let token = 0x02000000 | (index as u32 + 1);
                let id = match saved {
                    Some(b) => *b
                        .types
                        .get(&token)
                        .ok_or_else(|| Error("missing saved type ID".into()))?,
                    None => TypeId::fresh(),
                };
                require(
                    !program.types.contains_key(&id),
                    "duplicate logical type ID",
                )?;
                program.type_tokens.insert((assembly_id, token), id);
                require(
                    names
                        .insert((assembly_name.clone(), name.clone()), id)
                        .is_none(),
                    "duplicate type definition",
                )?;
                own_types.insert(index as u32 + 1, id);
                program.types.insert(
                    id,
                    TypeDefinition {
                        id,
                        name,
                        base: None,
                        fields: Vec::new(),
                        flags: row[0],
                    },
                );
            }
            program.images.insert(assembly_id, image.bytes.to_vec());
            modules.push((assembly_id, assembly_name, own_types));
        }
        // Resolve inheritance only after every assembly's logical types exist.
        for (image, (_, assembly, own_types)) in images.iter().zip(&modules) {
            let resolver = Resolver {
                image,
                assembly,
                names: &names,
                own_types,
            };
            for (&rid, &id) in own_types {
                let extends = image.row(2, rid)?[3];
                let base = if extends == 0 {
                    None
                } else {
                    Some(resolver.coded_type(extends)?)
                };
                program.types.get_mut(&id).unwrap().base = base;
            }
        }
        for id in program.types.keys() {
            let mut seen = BTreeSet::new();
            let mut current = Some(*id);
            while let Some(id) = current {
                require(seen.insert(id), "cyclic type inheritance")?;
                current = program.types[&id].base;
            }
        }
        for (image, (_, _, own_types)) in images.iter().zip(&modules) {
            for row in image.rows(15) {
                let id = *own_types
                    .get(&row[2])
                    .ok_or_else(|| Error("invalid ClassLayout parent".into()))?;
                let value_type = program
                    .types
                    .values()
                    .find(|ty| ty.name == "System.ValueType")
                    .map(|ty| ty.id);
                require(
                    value_type.is_some_and(|base| program.is_assignable(id, base)),
                    "explicit reference class layouts are unsupported",
                )?;
            }
        }
        for (image, (assembly_id, assembly, own_types)) in images.iter().zip(&modules) {
            let resolver = Resolver {
                image,
                assembly,
                names: &names,
                own_types,
            };
            for (index, row) in image.rows(2).iter().enumerate() {
                let next = image.rows(2).get(index + 1);
                let fields = member_range(
                    row[4],
                    next.map_or(image.rows(4).len() as u32 + 1, |r| r[4]),
                    image.rows(4).len(),
                )?;
                let methods = member_range(
                    row[5],
                    next.map_or(image.rows(6).len() as u32 + 1, |r| r[5]),
                    image.rows(6).len(),
                )?;
                let Some(&owner) = own_types.get(&(index as u32 + 1)) else {
                    require(
                        fields.is_empty() && methods.is_empty(),
                        "global fields and methods are unsupported",
                    )?;
                    continue;
                };
                for rid in fields {
                    let field = image.row(4, rid)?;
                    let mut reader = Reader::new(image.blob(field[2])?);
                    require(reader.u8()? == 6, "invalid field signature")?;
                    let value_type = resolver.slot(&mut reader, &program.types)?;
                    require(
                        reader.position == reader.data.len(),
                        "trailing field signature data",
                    )?;
                    let id = match bindings {
                        Some(bindings) => *bindings
                            .iter()
                            .find(|b| b.id == *assembly_id)
                            .unwrap()
                            .fields
                            .get(&(0x04000000 | rid))
                            .ok_or_else(|| Error("missing saved field ID".into()))?,
                        None => FieldId::fresh(),
                    };
                    require(
                        !program.fields.contains_key(&id),
                        "duplicate logical field ID",
                    )?;
                    let literal = field[0] & 0x40 != 0;
                    let is_static = field[0] & 0x10 != 0;
                    let name = image.string(field[1])?.to_owned();
                    if program.types[&owner]
                        .base
                        .is_some_and(|base| program.types[&base].name == "System.Enum")
                        && name == "value__"
                    {
                        require(
                            value_type == SlotType::I32,
                            "only Int32 enum storage is supported",
                        )?;
                    }
                    require(
                        field[0] & (0x100 | 0x1000) == 0,
                        "RVA-backed and marshaled fields are unsupported",
                    )?;
                    program.fields.insert(
                        id,
                        FieldDefinition {
                            id,
                            owner,
                            name,
                            value_type,
                            is_static,
                            literal,
                        },
                    );
                    program.types.get_mut(&owner).unwrap().fields.push(id);
                    require(
                        program
                            .field_tokens
                            .insert((*assembly_id, 0x04000000 | rid), id)
                            .is_none(),
                        "overlapping field lists",
                    )?;
                }
                for rid in methods {
                    let method = image.row(6, rid)?;
                    require(
                        matches!(method[1], 0 | 0x1000) && method[2] & (0x40 | 0x400 | 0x2000) == 0,
                        "virtual, abstract, native, and special implementation methods are unsupported",
                    )?;
                    let name = image.string(method[3])?.to_owned();
                    require(name != ".cctor", "type initializers are not supported yet")?;
                    let signature = resolver.signature(image.blob(method[4])?, &program.types)?;
                    require(
                        signature.instance == (method[2] & 0x10 == 0),
                        "method flags and signature disagree",
                    )?;
                    let intrinsic = if method[1] == 0x1000 {
                        require(
                            assembly == "Renraku.CoreLib" && method[0] == 0,
                            "unrecognized InternalCall method",
                        )?;
                        Some(match program.types[&owner].name.as_str() {
                            "System.String" => {
                                Intrinsic::String(string_intrinsic(&name, &signature, owner)?)
                            }
                            "Renraku.Model" => {
                                let string = SlotType::Reference(program.string_type()?);
                                let (intrinsic, parameters) =
                                    match name.as_str() {
                                        "Complete" => (Intrinsic::ModelComplete, vec![string]),
                                        "Chat" => (
                                            Intrinsic::ModelChat,
                                            vec![
                                                SlotType::Reference(resolver.named(
                                                    "Renraku.CoreLib",
                                                    "Renraku.ModelBinding",
                                                )?),
                                                SlotType::Reference(resolver.named(
                                                    "Renraku.CoreLib",
                                                    "Renraku.ChatMessage",
                                                )?),
                                            ],
                                        ),
                                        _ => {
                                            return Err(Error(
                                                "invalid model intrinsic signature".into(),
                                            ));
                                        }
                                    };
                                require(
                                    signature
                                        == Signature {
                                            instance: false,
                                            parameters,
                                            result: Some(string),
                                        },
                                    "invalid model intrinsic signature",
                                )?;
                                intrinsic
                            }
                            _ => return Err(Error("unrecognized InternalCall method".into())),
                        })
                    } else {
                        None
                    };
                    let (instructions, max_stack, locals, initialize_locals) = if intrinsic
                        .is_some()
                    {
                        (BTreeMap::new(), 0, Vec::new(), false)
                    } else {
                        read_body(image, method[0], &resolver, &program.types).map_err(|error| {
                            Error(format!("{}::{name}: {error}", program.types[&owner].name))
                        })?
                    };
                    let binding = match bindings {
                        Some(bindings) => *bindings
                            .iter()
                            .find(|b| b.id == *assembly_id)
                            .unwrap()
                            .methods
                            .get(&(0x06000000 | rid))
                            .ok_or_else(|| Error("missing saved method version".into()))?,
                        None => MethodVersion {
                            method: MethodId::fresh(),
                            code: CodeVersionId::fresh(),
                        },
                    };
                    require(
                        !program.dispatch.contains_key(&binding.method)
                            && !program.methods.contains_key(&binding.code),
                        "duplicate logical method or code version ID",
                    )?;
                    program.dispatch.insert(binding.method, binding.code);
                    require(
                        program
                            .method_tokens
                            .insert((*assembly_id, 0x06000000 | rid), binding.method)
                            .is_none(),
                        "overlapping method lists",
                    )?;
                    program.methods.insert(
                        binding.code,
                        Method {
                            intrinsic,
                            binding,
                            owner,
                            name,
                            signature,
                            locals,
                            initialize_locals,
                            max_stack,
                            instructions,
                            assembly: *assembly_id,
                        },
                    );
                }
            }
        }
        // Link MemberRefs by declaring type and complete supported signature.
        for (image, (assembly_id, assembly, own_types)) in images.iter().zip(&modules) {
            let resolver = Resolver {
                image,
                assembly,
                names: &names,
                own_types,
            };
            for (index, row) in image.rows(10).iter().enumerate() {
                let parent_table = match row[0] & 7 {
                    0 => 2,
                    1 => 1,
                    _ => return Err(Error("unsupported MemberRef parent".into())),
                };
                let owner = resolver.type_token((parent_table << 24) | (row[0] >> 3))?;
                let name = image.string(row[1])?;
                let blob = image.blob(row[2])?;
                let token = 0x0a000000 | (index as u32 + 1);
                if blob.first() == Some(&6) {
                    let mut reader = Reader::new(&blob[1..]);
                    let value_type = resolver.slot(&mut reader, &program.types)?;
                    require(
                        reader.position == reader.data.len(),
                        "trailing field reference signature",
                    )?;
                    let fields: Vec<_> = program
                        .fields
                        .values()
                        .filter(|f| {
                            f.owner == owner && f.name == name && f.value_type == value_type
                        })
                        .map(|f| f.id)
                        .collect();
                    require(fields.len() == 1, "unresolved or ambiguous field reference")?;
                    program
                        .field_tokens
                        .insert((*assembly_id, token), fields[0]);
                } else {
                    let signature = resolver.signature(blob, &program.types)?;
                    let methods: Vec<_> = program
                        .methods
                        .values()
                        .filter(|m| m.owner == owner && m.name == name && m.signature == signature)
                        .map(|m| m.binding.method)
                        .collect();
                    require(
                        methods.len() == 1,
                        &format!(
                            "unresolved or ambiguous method reference {}::{name}",
                            program.types[&owner].name
                        ),
                    )?;
                    program
                        .method_tokens
                        .insert((*assembly_id, token), methods[0]);
                }
            }
        }
        for (image, (assembly, assembly_name, own_types)) in images.iter().zip(&modules) {
            let resolver = Resolver {
                image,
                assembly: assembly_name,
                names: &names,
                own_types,
            };
            for method in program.methods.values().filter(|m| m.assembly == *assembly) {
                for instruction in method.instructions.values() {
                    if let Op::ReferenceCast { token, .. } = instruction.op {
                        let id = resolver.type_token(token)?;
                        program.type_tokens.insert((*assembly, token), id);
                    }
                    if let Op::LoadString(token) = instruction.op {
                        require(token >> 24 == 0x70, "invalid ldstr token")?;
                        program
                            .string_literals
                            .insert((*assembly, token), image.user_string(token & 0xffffff)?);
                    }
                }
            }
        }
        program.validate_instructions()?;
        Ok(program)
    }

    pub fn types(&self) -> impl Iterator<Item = &TypeDefinition> {
        self.types.values()
    }
    pub fn methods(&self) -> impl Iterator<Item = &Method> {
        self.methods.values()
    }
    pub fn fields(&self) -> impl Iterator<Item = &FieldDefinition> {
        self.fields.values()
    }
    pub(crate) fn string_type(&self) -> Result<TypeId> {
        let matches: Vec<_> = self
            .types
            .values()
            .filter(|ty| ty.name == "System.String")
            .collect();
        require(
            matches.len() == 1,
            "System.String binding is ambiguous or missing",
        )?;
        let id = matches[0].id;
        let mut current = Some(id);
        while let Some(owner) = current {
            let ty = &self.types[&owner];
            require(
                ty.fields.iter().all(|field| self.fields[field].is_static),
                "string layout cannot contain instance fields",
            )?;
            current = ty.base;
        }
        Ok(id)
    }
    pub fn method(&self, id: MethodId) -> Result<&Method> {
        let code = self
            .dispatch
            .get(&id)
            .ok_or_else(|| Error("unknown logical method".into()))?;
        self.methods
            .get(code)
            .ok_or_else(|| Error("missing method implementation".into()))
    }
    pub fn entry(&self, name: &str) -> Result<MethodId> {
        let matches: Vec<_> = self
            .methods
            .values()
            .filter(|method| {
                method.binding.method.to_string() == name
                    || format!("{}::{}", self.types[&method.owner].name, method.name) == name
            })
            .collect();
        require(matches.len() == 1, "entry method is missing or overloaded")?;
        Ok(matches[0].binding.method)
    }
    pub fn is_assignable(&self, actual: TypeId, expected: TypeId) -> bool {
        let mut current = Some(actual);
        while let Some(id) = current {
            if id == expected {
                return true;
            }
            current = self.types.get(&id).and_then(|ty| ty.base);
        }
        false
    }
    pub(crate) fn reference_cast_target(&self, assembly: AssemblyId, token: u32) -> Result<TypeId> {
        require(
            matches!(token >> 24, 1 | 2),
            "reference casts require TypeDef/TypeRef targets",
        )?;
        let id = *self
            .type_tokens
            .get(&(assembly, token))
            .ok_or_else(|| Error("unresolved reference cast target".into()))?;
        let mut current = Some(id);
        while let Some(owner) = current {
            let ty = self
                .types
                .get(&owner)
                .ok_or_else(|| Error("missing reference cast type".into()))?;
            require(
                ty.flags & 0x20 == 0
                    && ty.flags & 0x18 == 0
                    && !matches!(
                        ty.name.as_str(),
                        "System.ValueType" | "System.Enum" | "System.Array"
                    ),
                "reference cast target must be a supported reference class; value/array/interface targets are unsupported",
            )?;
            current = ty.base;
        }
        Ok(id)
    }
    pub(crate) fn allocatable(&self, id: TypeId) -> Result<()> {
        let ty = &self.types[&id];
        require(ty.flags & 0x80 == 0, "cannot instantiate an abstract type")?;
        require(
            ty.flags & 0x18 == 0,
            "explicit and sequential reference class layouts are unsupported",
        )?;
        let value_base = self
            .types
            .values()
            .find(|ty| ty.name == "System.ValueType")
            .map(|ty| ty.id);
        require(
            !value_base.is_some_and(|base| self.is_assignable(id, base)),
            "value type allocation is unsupported",
        )?;
        require(
            ty.name != "System.String" && ty.name != "System.Array",
            "string and array allocation are unsupported",
        )
    }
    fn validate_instructions(&self) -> Result<()> {
        for method in self.methods.values() {
            if method.intrinsic.is_some() {
                self.string_type()?;
            }
            for instruction in method.instructions.values() {
                match instruction.op {
                    Op::ReferenceCast { token, .. } => {
                        self.reference_cast_target(method.assembly, token)?;
                    }
                    Op::LoadString(token) => {
                        self.string_type()?;
                        require(
                            self.string_literals.contains_key(&(method.assembly, token)),
                            "missing string literal",
                        )?;
                    }
                    Op::Call { token, .. } | Op::NewObject(token) => {
                        let id = self
                            .method_tokens
                            .get(&(method.assembly, token))
                            .ok_or_else(|| Error("unresolved call token".into()))?;
                        let callee = self.method(*id)?;
                        if matches!(
                            instruction.op,
                            Op::Call {
                                null_check: true,
                                ..
                            }
                        ) {
                            require(
                                callee.signature.instance,
                                "callvirt requires an instance method",
                            )?;
                        }
                        if matches!(instruction.op, Op::NewObject(_)) {
                            require(
                                callee.name == ".ctor"
                                    && callee.signature.instance
                                    && callee.signature.result.is_none(),
                                "newobj requires a void instance constructor",
                            )?;
                            self.allocatable(callee.owner)?;
                        }
                    }
                    Op::LoadField { token, is_static } | Op::StoreField { token, is_static } => {
                        let id = self
                            .field_tokens
                            .get(&(method.assembly, token))
                            .ok_or_else(|| Error("unresolved field token".into()))?;
                        let field = &self.fields[id];
                        require(
                            field.is_static == is_static && !field.literal,
                            "invalid static/instance/literal field access",
                        )?;
                    }
                    Op::LoadArgument(index) | Op::StoreArgument(index) => {
                        require(
                            index
                                < method.signature.parameters.len()
                                    + usize::from(method.signature.instance),
                            "argument index out of range",
                        )?;
                    }
                    Op::LoadLocal(index) | Op::StoreLocal(index) => {
                        require(index < method.locals.len(), "local index out of range")?
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

fn string_intrinsic(name: &str, signature: &Signature, owner: TypeId) -> Result<StringIntrinsic> {
    let string = SlotType::Reference(owner);
    let (intrinsic, instance, parameters, result) = match name {
        "get_Length" => (StringIntrinsic::Length, true, vec![], SlotType::I32),
        "get_Chars" => (
            StringIntrinsic::CharAt,
            true,
            vec![SlotType::I32],
            SlotType::Char,
        ),
        "Concat" => (StringIntrinsic::Concat, false, vec![string, string], string),
        "op_Equality" => (
            StringIntrinsic::Equal,
            false,
            vec![string, string],
            SlotType::Boolean,
        ),
        "op_Inequality" => (
            StringIntrinsic::NotEqual,
            false,
            vec![string, string],
            SlotType::Boolean,
        ),
        _ => return Err(Error("unrecognized string InternalCall".into())),
    };
    require(
        *signature
            == Signature {
                instance,
                parameters,
                result: Some(result),
            },
        "invalid string intrinsic signature",
    )?;
    Ok(intrinsic)
}

type Body = (BTreeMap<u32, Instruction>, usize, Vec<SlotType>, bool);
fn read_body(
    image: &Image<'_>,
    rva: u32,
    resolver: &Resolver<'_, '_>,
    types: &BTreeMap<TypeId, TypeDefinition>,
) -> Result<Body> {
    let first = image.rva(rva, 1)?[0];
    let (header_size, code_size, max_stack, local_token, initialize) = if first & 3 == 2 {
        (1, (first >> 2) as usize, 8, 0, false)
    } else {
        let mut header = Reader::new(image.rva(rva, 12)?);
        let flags = header.u16()?;
        require(
            flags & 3 == 3 && flags >> 12 == 3,
            "unsupported method header",
        )?;
        require(
            flags & 8 == 0,
            "exception handling sections are unsupported",
        )?;
        let stack = header.u16()? as usize;
        let size = header.u32()? as usize;
        (12, size, stack, header.u32()?, flags & 0x10 != 0)
    };
    let mut locals = Vec::new();
    if local_token != 0 {
        require(local_token >> 24 == 0x11, "invalid local signature token")?;
        let mut reader = Reader::new(image.blob(image.row(17, local_token & 0xffffff)?[0])?);
        require(reader.u8()? == 7, "invalid local signature")?;
        let count = reader.compressed()?;
        for _ in 0..count {
            locals.push(resolver.slot(&mut reader, types)?);
        }
        require(
            reader.position == reader.data.len(),
            "trailing local signature data",
        )?;
    }
    let code_rva = rva
        .checked_add(header_size)
        .ok_or_else(|| Error("method RVA overflow".into()))?;
    Ok((
        cil::decode(image.rva(code_rva, code_size)?)?,
        max_stack,
        locals,
        initialize,
    ))
}
