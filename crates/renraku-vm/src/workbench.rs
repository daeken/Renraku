//! Read-only runtime tools and frozen compiler views. Facades are never code authority.
use crate::{
    cil::{Op, Reader},
    error::{Error, Result, require},
    identity::{AssemblyId, TypeId},
    image::Image,
    program::Program,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReferenceView {
    format: u32,
    pub schema_hash: String,
    pub core_sha256: String,
    pub schema_json: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Schema {
    module_id: String,
    module_name: String,
    roots: Vec<String>,
    declaration: String,
    bindings: Vec<Binding>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    kind: String,
    id: String,
    name: String,
    signature: String,
}
impl ReferenceView {
    pub fn to_json(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| Error(e.to_string()))
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        require(
            bytes.len() <= 4 * 1024 * 1024,
            "reference view exceeds its size limit",
        )?;
        let view: Self =
            serde_json::from_slice(bytes).map_err(|_| Error("invalid reference view".into()))?;
        require(
            view.format == 1 && hash(view.schema_json.as_bytes()) == view.schema_hash,
            "reference schema hash or format mismatch",
        )?;
        let _: Schema = serde_json::from_str(&view.schema_json)
            .map_err(|_| Error("invalid reference schema".into()))?;
        Ok(view)
    }
    pub fn validate(&self, program: &Program) -> Result<()> {
        let verified = Self::from_json(&self.to_json()?)?;
        let data: Schema = serde_json::from_str(&verified.schema_json).unwrap();
        let roots = data
            .roots
            .iter()
            .map(|id| program.find_type(id))
            .collect::<Result<Vec<_>>>()?;
        let actual = program.reference_view(&roots)?;
        require(
            actual.schema_hash == self.schema_hash && actual.core_sha256 == self.core_sha256,
            "reference view does not match the runtime schema",
        )
    }
}
impl Program {
    pub fn find_type(&self, name: &str) -> Result<TypeId> {
        let matches: Vec<_> = self
            .types
            .values()
            .filter(|t| t.name == name || t.id.to_string() == name)
            .collect();
        require(matches.len() == 1, "type name/ID is missing or ambiguous")?;
        Ok(matches[0].id)
    }
    pub fn search_symbols(&self, query: &str, limit: usize) -> Result<Value> {
        require(
            query.len() <= 256 && (1..=100).contains(&limit),
            "invalid symbol search query or limit",
        )?;
        let terms: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
        require(terms.len() <= 16, "too many symbol search terms")?;
        let mut symbols = Vec::new();
        for ty in self.types.values() {
            symbols.push((
                ty.name.clone(),
                "type",
                json!({"id":ty.id.to_string(),"kind":"type","name":ty.name}),
            ));
        }
        for field in self.fields.values() {
            let name = format!("{}::{}", self.types[&field.owner].name, field.name);
            symbols.push((name.clone(), "field", json!({"id":field.id.to_string(),"kind":"field","name":name,"owner":field.owner.to_string(),"slot":format!("{:?}",field.value_type),"static":field.is_static})));
        }
        for method in self.methods.values() {
            let name = format!("{}::{}", self.types[&method.owner].name, method.name);
            symbols.push((name.clone(), "method", json!({"id":method.binding.method.to_string(),"codeVersion":method.binding.code.to_string(),"kind":"method","name":name,"owner":method.owner.to_string(),"signature":format!("{:?}",method.signature)})));
        }
        symbols.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        let found: Vec<_> = symbols
            .into_iter()
            .filter(|(name, kind, _)| {
                let text = format!("{kind} {}", name.to_lowercase());
                terms.iter().all(|term| text.contains(term))
            })
            .collect();
        Ok(
            json!({"format":1,"total":found.len(),"truncated":found.len()>limit,"symbols":found.into_iter().take(limit).map(|(_,_,s)|s).collect::<Vec<_>>()}),
        )
    }
    pub fn read_code(&self, name_or_id: &str) -> Result<Value> {
        let matches: Vec<_> = self
            .methods
            .values()
            .filter(|m| {
                m.binding.code.to_string() == name_or_id
                    || m.binding.method.to_string() == name_or_id
                    || format!("{}::{}", self.types[&m.owner].name, m.name) == name_or_id
            })
            .collect();
        require(
            matches.len() == 1,
            "method/code ID is missing or ambiguous; select a codeVersion from symbol search",
        )?;
        let method = matches[0];
        let instructions: Vec<_> = method.instructions.values().map(|instruction| {
            let target = match instruction.op {
                Op::Call { token, .. } | Op::NewObject(token) => self.method_tokens.get(&(method.assembly,token)).map(|id| {
                    let target = &self.methods[&self.dispatch[id]]; json!({"kind":"method","id":id.to_string(),"name":format!("{}::{}",self.types[&target.owner].name,target.name)})
                }),
                Op::LoadField { token, .. } | Op::StoreField { token, .. } => self.field_tokens.get(&(method.assembly,token)).map(|id| {
                    let field = &self.fields[id]; json!({"kind":"field","id":id.to_string(),"name":format!("{}::{}",self.types[&field.owner].name,field.name)})
                }),
                Op::LoadString(token) => self.string_literals.get(&(method.assembly,token)).map(|units| json!({"kind":"string","utf16":units})),
                _ => None,
            };
            json!({"offset":instruction.offset,"next":instruction.next,"operation":format!("{:?}",instruction.op),"target":target})
        }).collect();
        Ok(
            json!({"format":1,"representation":"cil","methodId":method.binding.method.to_string(),"codeVersion":method.binding.code.to_string(),
            "owner":self.types[&method.owner].name,"name":method.name,"signature":format!("{:?}",method.signature),"intrinsic":method.intrinsic.is_some(),
            "sourceAvailable":false,"instructions":instructions}),
        )
    }
    pub fn reference_view(&self, roots: &[TypeId]) -> Result<ReferenceView> {
        require(
            !roots.is_empty() && roots.len() <= 32,
            "select between 1 and 32 reference roots",
        )?;
        let module = self
            .type_tokens
            .iter()
            .find(|((_, token), id)| *id == &roots[0] && token >> 24 == 2)
            .map(|((module, _), _)| *module)
            .ok_or_else(|| Error("unknown reference root".into()))?;
        require(
            roots.iter().all(|id| {
                self.type_tokens.iter().any(|((owner, token), actual)| {
                    *owner == module && token >> 24 == 2 && id == actual
                })
            }),
            "reference roots must belong to one logical module",
        )?;
        let image = Image::parse(&self.images[&module])?;
        let module_name = image.string(image.row(32, 1)?[7])?.to_owned();
        require(
            module_name != "Renraku.CoreLib",
            "use the bootstrap core image as a compiler reference",
        )?;
        require(
            image.row(32, 1)?[1..5].iter().all(|v| *v == 0)
                && image.row(32, 1)?[5] == 0
                && image.blob(image.row(32, 1)?[6])?.is_empty(),
            "reference export currently requires an unsigned version-zero module",
        )?;
        let core = self
            .images
            .values()
            .find(|bytes| {
                Image::parse(bytes).ok().is_some_and(|i| {
                    i.string(i.row(32, 1).unwrap()[7]).unwrap() == "Renraku.CoreLib"
                })
            })
            .ok_or_else(|| Error("bootstrap core is missing".into()))?;
        let mut builder = ViewBuilder {
            program: self,
            image,
            module,
            module_name: module_name.clone(),
            pending: roots.to_vec(),
            seen: BTreeSet::new(),
            declarations: BTreeMap::new(),
            bindings: Vec::new(),
        };
        while let Some(id) = builder.pending.pop() {
            if builder.seen.insert(id) {
                require(
                    builder.seen.len() <= 256,
                    "reference closure exceeds its type limit",
                )?;
                builder.render(id)?;
            }
        }
        builder.bindings.sort_by(|a, b| {
            (&a.kind, &a.name, &a.signature, &a.id).cmp(&(&b.kind, &b.name, &b.signature, &b.id))
        });
        let declaration = builder
            .declarations
            .into_values()
            .collect::<Vec<_>>()
            .join("\n");
        require(
            declaration.len() <= 1024 * 1024 && builder.bindings.len() <= 8192,
            "reference declaration exceeds its limits",
        )?;
        let mut root_ids: Vec<_> = roots.iter().map(ToString::to_string).collect();
        root_ids.sort();
        root_ids.dedup();
        let schema = Schema {
            module_id: module.to_string(),
            module_name,
            roots: root_ids,
            declaration,
            bindings: builder.bindings,
        };
        let schema_json = serde_json::to_string(&schema).map_err(|e| Error(e.to_string()))?;
        Ok(ReferenceView {
            format: 1,
            schema_hash: hash(schema_json.as_bytes()),
            core_sha256: hash(core),
            schema_json,
        })
    }
}
struct ViewBuilder<'a> {
    program: &'a Program,
    image: Image<'a>,
    module: AssemblyId,
    module_name: String,
    pending: Vec<TypeId>,
    seen: BTreeSet<TypeId>,
    declarations: BTreeMap<String, String>,
    bindings: Vec<Binding>,
}
fn identifier(name: &str) -> Result<String> {
    require(
        !name.is_empty() && name.as_bytes()[0].is_ascii_alphabetic() || name.starts_with('_'),
        "reference export requires C#-representable ASCII identifiers",
    )?;
    require(
        name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "reference export requires C#-representable ASCII identifiers",
    )?;
    Ok(format!("@{name}"))
}
fn qualified(name: &str) -> Result<String> {
    Ok(name
        .split('.')
        .map(identifier)
        .collect::<Result<Vec<_>>>()?
        .join("."))
}
fn access(flags: u32) -> Result<&'static str> {
    match flags & 7 {
        1 => Ok("private"),
        2 => Ok("private protected"),
        3 => Ok("internal"),
        4 => Ok("protected"),
        5 => Ok("protected internal"),
        6 => Ok("public"),
        _ => Err(Error("unsupported reference member visibility".into())),
    }
}
impl ViewBuilder<'_> {
    fn type_name(&mut self, id: TypeId) -> Result<String> {
        let ty = self
            .program
            .types
            .get(&id)
            .ok_or_else(|| Error("unknown reference type".into()))?;
        let owner = self
            .program
            .type_tokens
            .iter()
            .find(|((_, token), actual)| token >> 24 == 2 && **actual == id)
            .map(|((a, _), _)| *a)
            .unwrap();
        if owner == self.module {
            self.pending.push(id);
        } else {
            let image = Image::parse(&self.program.images[&owner])?;
            require(
                image.string(image.row(32, 1)?[7])? == "Renraku.CoreLib",
                "reference closure across non-core modules is not supported yet",
            )?;
        }
        Ok(format!("global::{}", qualified(&ty.name)?))
    }
    fn coded_type(&self, coded: u32) -> Result<TypeId> {
        if coded & 3 == 0 {
            return self
                .program
                .type_tokens
                .get(&(self.module, 0x02000000 | (coded >> 2)))
                .copied()
                .ok_or_else(|| Error("unknown TypeDef in reference signature".into()));
        }
        require(coded & 3 == 1, "TypeSpec reference export is unsupported")?;
        let row = self.image.row(1, coded >> 2)?;
        let assembly = match row[0] & 3 {
            0 => self.module_name.as_str(),
            2 => self.image.string(self.image.row(35, row[0] >> 2)?[6])?,
            _ => return Err(Error("unsupported reference scope".into())),
        };
        let name = if self.image.string(row[2])?.is_empty() {
            self.image.string(row[1])?.to_owned()
        } else {
            format!(
                "{}.{}",
                self.image.string(row[2])?,
                self.image.string(row[1])?
            )
        };
        let mut found = None;
        for ((owner, token), id) in &self.program.type_tokens {
            if token >> 24 != 2 || self.program.types[id].name != name {
                continue;
            }
            let image = Image::parse(&self.program.images[owner])?;
            if image.string(image.row(32, 1)?[7])? == assembly {
                found = Some(*id);
                break;
            }
        }
        found.ok_or_else(|| Error("unresolved reference signature type".into()))
    }
    fn slot(&mut self, reader: &mut Reader<'_>) -> Result<String> {
        Ok(match reader.u8()? {
            2 => "bool".into(),
            3 => "char".into(),
            8 => "int".into(),
            9 => "uint".into(),
            10 => "long".into(),
            11 => "ulong".into(),
            14 => "string".into(),
            28 => "object".into(),
            17 | 18 => {
                let id = self.coded_type(reader.compressed()?)?;
                self.type_name(id)?
            }
            tag => {
                return Err(Error(format!(
                    "unsupported reference signature element {tag:02x}"
                )));
            }
        })
    }
    fn signature(
        &mut self,
        blob: &[u8],
        parameter_names: &[String],
    ) -> Result<(String, Vec<String>)> {
        let mut reader = Reader::new(blob);
        let convention = reader.u8()?;
        require(
            convention == 0 || convention == 0x20,
            "unsupported reference calling convention",
        )?;
        let count = reader.compressed()?;
        require(
            count <= 128 && count as usize == parameter_names.len(),
            "missing/invalid reference parameters",
        )?;
        let result = if reader.data.get(reader.position) == Some(&1) {
            reader.u8()?;
            "void".into()
        } else {
            self.slot(&mut reader)?
        };
        let mut parameters = Vec::new();
        for name in parameter_names {
            parameters.push(format!("{} {}", self.slot(&mut reader)?, identifier(name)?));
        }
        require(
            reader.position == blob.len(),
            "trailing reference signature",
        )?;
        Ok((result, parameters))
    }
    fn render(&mut self, id: TypeId) -> Result<()> {
        let ty = &self.program.types[&id];
        require(
            ty.flags & !0x00100187 == 0 && ty.flags & 7 <= 1,
            "unsupported reference type flags",
        )?;
        require(
            ty.base.is_none_or(|base| {
                !matches!(
                    self.program.types[&base].name.as_str(),
                    "System.ValueType" | "System.Enum"
                )
            }),
            "reference export currently supports reference classes only",
        )?;
        let rid = self
            .program
            .type_tokens
            .iter()
            .find(|((a, t), actual)| *a == self.module && t >> 24 == 2 && **actual == id)
            .map(|((_, t), _)| t & 0xffffff)
            .unwrap();
        require(
            !self.image.rows(21).iter().any(|r| r[0] == rid)
                && !self.image.rows(18).iter().any(|r| r[0] == rid),
            "properties and events are not supported by reference export yet",
        )?;
        let row = self.image.row(2, rid)?.to_vec();
        let next = self.image.rows(2).get(rid as usize);
        let field_end = next.map_or(self.image.rows(4).len() as u32 + 1, |r| r[4]);
        let method_end = next.map_or(self.image.rows(6).len() as u32 + 1, |r| r[5]);
        let parameter_start = self
            .image
            .rows(6)
            .get(row[5] as usize - 1)
            .map_or(1, |r| r[5]);
        let parameter_end = self
            .image
            .rows(6)
            .get(method_end as usize - 1)
            .map_or(self.image.rows(8).len() as u32 + 1, |r| r[5]);
        require(
            !self.image.rows(12).iter().any(|attribute| {
                let parent = attribute[0] >> 5;
                match attribute[0] & 31 {
                    3 => parent == rid,
                    0 => (row[5]..method_end).contains(&parent),
                    1 => (row[4]..field_end).contains(&parent),
                    4 => (parameter_start..parameter_end).contains(&parent),
                    _ => false,
                }
            }),
            "custom attributes on exported declarations are not supported yet",
        )?;
        let name = ty.name.clone();
        let (namespace, short) = name.rsplit_once('.').unwrap_or(("", &name));
        let mut body = Vec::new();
        for field_rid in row[4]..field_end {
            let field = self.image.row(4, field_rid)?.to_vec();
            require(
                field[0] & !0x37 == 0,
                "literal/default/attributed fields are not supported by reference export",
            )?;
            let field_name = self.image.string(field[1])?.to_owned();
            let blob = self.image.blob(field[2])?.to_vec();
            let mut reader = Reader::new(&blob);
            require(reader.u8()? == 6, "invalid field signature")?;
            let slot = self.slot(&mut reader)?;
            require(reader.position == blob.len(), "trailing field signature")?;
            let declaration = format!(
                "{} {}{}{} {};",
                access(field[0])?,
                if field[0] & 0x10 != 0 { "static " } else { "" },
                if field[0] & 0x20 != 0 {
                    "readonly "
                } else {
                    ""
                },
                slot,
                identifier(&field_name)?
            );
            body.push(declaration.clone());
            self.bindings.push(Binding {
                kind: "field".into(),
                id: self.program.field_tokens[&(self.module, 0x04000000 | field_rid)].to_string(),
                name: format!("{name}::{field_name}"),
                signature: declaration,
            });
        }
        for method_rid in row[5]..method_end {
            let method = self.image.row(6, method_rid)?.to_vec();
            let method_name = self.image.string(method[3])?.to_owned();
            require(
                method_name == ".ctor" || method[2] & 0x1800 == 0,
                "special-name methods require a richer reference schema",
            )?;
            let blob = self.image.blob(method[4])?.to_vec();
            let end = self
                .image
                .rows(6)
                .get(method_rid as usize)
                .map_or(self.image.rows(8).len() as u32 + 1, |row| row[5]);
            let mut parameter_names = Vec::new();
            for rid in method[5]..end {
                let parameter = self.image.row(8, rid)?;
                require(
                    parameter[0] == 0,
                    "parameter defaults/modifiers are not supported by reference export",
                )?;
                if parameter[1] == 0 {
                    continue;
                }
                require(
                    parameter[1] as usize == parameter_names.len() + 1,
                    "unordered reference parameters are not supported",
                )?;
                parameter_names.push(self.image.string(parameter[2])?.to_owned());
            }
            let (result, parameters) = self.signature(&blob, &parameter_names)?;
            let declaration = if method_name == ".ctor" {
                format!(
                    "{} extern {}({});",
                    access(method[2])?,
                    identifier(short)?,
                    parameters.join(", ")
                )
            } else {
                format!(
                    "{} {}extern {} {}({});",
                    access(method[2])?,
                    if method[2] & 0x10 != 0 { "static " } else { "" },
                    result,
                    identifier(&method_name)?,
                    parameters.join(", ")
                )
            };
            body.push(declaration.clone());
            self.bindings.push(Binding {
                kind: "method".into(),
                id: self.program.method_tokens[&(self.module, 0x06000000 | method_rid)].to_string(),
                name: format!("{name}::{method_name}"),
                signature: declaration,
            });
        }
        body.sort();
        let modifier = match ty.flags & 0x180 {
            0x180 => "static ",
            0x100 => "sealed ",
            0x80 => "abstract ",
            _ => "",
        };
        let base = if modifier == "static " {
            String::new()
        } else if let Some(base) = ty.base {
            format!(" : {}", self.type_name(base)?)
        } else {
            String::new()
        };
        let declaration = format!(
            "{}{} {modifier}class {}{base} {{\n{}\n}}{}",
            if namespace.is_empty() {
                String::new()
            } else {
                format!("namespace {} {{\n", qualified(namespace)?)
            },
            if ty.flags & 7 == 1 {
                "public"
            } else {
                "internal"
            },
            identifier(short)?,
            body.join("\n"),
            if namespace.is_empty() { "" } else { "\n}" }
        );
        self.bindings.push(Binding {
            kind: "type".into(),
            id: id.to_string(),
            name: name.clone(),
            signature: format!("flags={:x};base={:?}", ty.flags, ty.base),
        });
        self.declarations.insert(name, declaration);
        Ok(())
    }
}
