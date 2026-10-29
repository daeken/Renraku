//! Bounded, read-only projections. Logical reference IDs are observations, not
//! new retention handles; the caller owns any roots needed across operations.
use crate::{
    error::{Error, Result, require},
    heap::AllocationDomain,
    identity::{FieldId, ObjectId, TypeId},
    program::{FieldDefinition, Program, SlotType},
    runtime::Vm,
    value::Value,
};
use serde_json::{Value as Json, json};

pub const MAX_FIELDS: usize = 32;
pub const MAX_STRING_UNITS: usize = 256;
pub const MAX_OUTPUT_BYTES: usize = 12 * 1024;
#[derive(Clone, Copy, Debug)]
pub struct InspectionLimits {
    pub max_fields: usize,
    pub string_units: usize,
}
impl Default for InspectionLimits {
    fn default() -> Self {
        Self {
            max_fields: 16,
            string_units: 64,
        }
    }
}
impl InspectionLimits {
    pub fn validate(self) -> Result<()> {
        require(
            (1..=MAX_FIELDS).contains(&self.max_fields) && self.string_units <= MAX_STRING_UNITS,
            "inspection limits require 1..32 fields and 0..256 string units",
        )
    }
}
fn label(name: &str) -> (&str, bool) {
    let mut end = name.len().min(128);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    (&name[..end], end != name.len())
}
fn byte_len(value: &Json) -> Result<usize> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|e| Error(e.to_string()))
}
fn bounded(value: Json) -> Result<Json> {
    require(
        byte_len(&value)? <= MAX_OUTPUT_BYTES,
        "inspection metadata exceeds 12288 bytes",
    )?;
    Ok(value)
}
impl Program {
    /// Exact qualified declaration or logical ID; no inherited-name guessing.
    pub fn resolve_static_field(&self, selector: &str) -> Result<FieldId> {
        require(
            selector.len() <= 1024,
            "static selector exceeds 1024 bytes; use its logical ID",
        )?;
        let matches: Vec<_> = if let Ok(id) = uuid::Uuid::parse_str(selector) {
            self.fields
                .get(&FieldId::from_bytes(*id.as_bytes()))
                .into_iter()
                .collect()
        } else {
            self.fields
                .values()
                .filter(|field| {
                    self.types.get(&field.owner).is_some_and(|owner| {
                        selector
                            .strip_prefix(&owner.name)
                            .and_then(|suffix| suffix.strip_prefix("::"))
                            == Some(field.name.as_str())
                    })
                })
                .collect()
        };
        require(matches.len() == 1, "field selector is missing or ambiguous")?;
        let field = matches[0];
        require(
            field.is_static && !field.literal,
            "selector is not a stored static field",
        )?;
        Ok(field.id)
    }
}
impl Vm<'_> {
    pub fn resolve_object_id(&self, selector: &str) -> Result<ObjectId> {
        require(selector.len() <= 64, "object selector exceeds its ID bound")?;
        let parsed = uuid::Uuid::parse_str(selector)
            .map_err(|_| Error("invalid logical object ID".into()))?;
        let id = ObjectId::from_bytes(*parsed.as_bytes());
        let object = self.heap.object(id)?;
        require(
            object.domain() == AllocationDomain::Managed,
            "transient objects are unsupported by durable inspection",
        )?;
        require(
            self.program.types.contains_key(&object.type_id()),
            "inspection object has an unknown type",
        )?;
        Ok(id)
    }
    fn inspection_type(&self, id: TypeId) -> Result<Json> {
        let ty = self
            .program
            .types
            .get(&id)
            .ok_or_else(|| Error("inspection type is missing".into()))?;
        let (name, truncated) = label(&ty.name);
        Ok(json!({"id":id.to_string(),"name":name,"nameTruncated":truncated}))
    }
    fn inspection_slot(&self, slot: SlotType) -> Result<Json> {
        Ok(match slot {
            SlotType::I32 => json!({"kind":"i32"}),
            SlotType::I64 => json!({"kind":"i64"}),
            SlotType::Boolean => json!({"kind":"boolean"}),
            SlotType::Char => json!({"kind":"char"}),
            SlotType::Reference(id) => json!({"kind":"reference","type":self.inspection_type(id)?}),
        })
    }
    fn inspection_string(units: &[u16], limit: usize) -> Json {
        let prefix = &units[..units.len().min(limit)];
        let text = String::from_utf16(prefix).ok();
        json!({"unitCount":units.len(),"units":prefix,"previewWellFormed":text.is_some(),
            "text":text,"truncated":prefix.len()!=units.len()})
    }
    fn inspection_value(&self, value: Value, units: usize) -> Result<Json> {
        Ok(match value {
            Value::Null => json!({"kind":"null"}),
            Value::I32(value) => json!({"kind":"i32","value":value}),
            Value::I64(value) => json!({"kind":"i64","value":value.to_string()}),
            Value::Reference(id) => {
                let object = self.heap.object(id)?;
                require(
                    object.domain() == AllocationDomain::Managed,
                    "transient references are unsupported by durable inspection",
                )?;
                let mut value = json!({"kind":"object","id":id.to_string(),"type":self.inspection_type(object.type_id())?});
                if let Some(string) = object.string_units() {
                    value["stringPreview"] = Self::inspection_string(string, units);
                }
                value
            }
        })
    }
    fn inspection_field(
        &self,
        field: &FieldDefinition,
        value: Value,
        units: usize,
    ) -> Result<Json> {
        let (name, truncated) = label(&field.name);
        Ok(
            json!({"id":field.id.to_string(),"name":name,"nameTruncated":truncated,
            "owner":self.inspection_type(field.owner)?,"declaredSlot":self.inspection_slot(field.value_type)?,
            "value":self.inspection_value(value,units)?}),
        )
    }
    /// No traversal of referenced objects' fields and no heap or checkpoint writes.
    /// `checkpoint` is the VM's base generation. CLI/agent callers use a fresh
    /// restore; embedders inspecting unsaved changes must retain that distinction.
    pub fn inspect_object(&self, selector: &str, limits: InspectionLimits) -> Result<Json> {
        limits.validate()?;
        let id = self.resolve_object_id(selector)?;
        let object = self.heap.object(id)?;
        let count = object.fields().len();
        let mut view = json!({"format":1,"kind":"object","universe":self.universe_id().to_string(),
            "checkpoint":self.checkpoint_generation.get(),"objectId":id.to_string(),
            "type":self.inspection_type(object.type_id())?,"schemaVersion":object.schema_version(),
            "domain":"managed","fieldCount":count,"fieldsReturned":0,"fieldsTruncated":count>0,
            "fields":[],"limits":{"maxFields":limits.max_fields,"stringUnits":limits.string_units,"maxOutputBytes":MAX_OUTPUT_BYTES}});
        if let Some(units) = object.string_units() {
            view["stringPreview"] = Self::inspection_string(units, limits.string_units);
        }
        for (id, value) in object.fields().iter().take(limits.max_fields) {
            let field = self
                .program
                .fields
                .get(id)
                .ok_or_else(|| Error("inspection field declaration is missing".into()))?;
            require(
                !field.is_static && self.program.is_assignable(object.type_id(), field.owner),
                "invalid inspection field ownership",
            )?;
            let row = self.inspection_field(field, *value, limits.string_units)?;
            view["fields"].as_array_mut().unwrap().push(row);
            let length = view["fields"].as_array().unwrap().len();
            view["fieldsReturned"] = json!(length);
            view["fieldsTruncated"] = json!(length < count);
            if byte_len(&view)? > MAX_OUTPUT_BYTES {
                view["fields"].as_array_mut().unwrap().pop();
                view["fieldsReturned"] = json!(length - 1);
                view["fieldsTruncated"] = json!(true);
                break;
            }
        }
        bounded(view)
    }
    /// Observe this VM's storage. Resolving a selector does not freeze its value.
    /// CLI/agent callers restore a checkpoint immediately before observing it.
    pub fn inspect_static(&self, selector: &str, string_units: usize) -> Result<Json> {
        InspectionLimits {
            string_units,
            ..InspectionLimits::default()
        }
        .validate()?;
        let id = self.program.resolve_static_field(selector)?;
        let field = &self.program.fields[&id];
        let value = *self
            .statics
            .get(&id)
            .ok_or_else(|| Error("static field has no storage".into()))?;
        bounded(
            json!({"format":1,"kind":"static","universe":self.universe_id().to_string(),
            "checkpoint":self.checkpoint_generation.get(),"field":self.inspection_field(field,value,string_units)?,
            "limits":{"stringUnits":string_units,"maxOutputBytes":MAX_OUTPUT_BYTES}}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{identity::ArenaId, program::TypeDefinition};
    fn registry() -> Program {
        let mut program = Program::default();
        for (n, name, base) in [
            (1, "System.Object", None),
            (2, "Example.Parent", Some(1)),
            (3, "Example.Child", Some(2)),
            (4, "System.String", Some(1)),
        ] {
            let id = TypeId::from_bytes([n; 16]);
            program.types.insert(
                id,
                TypeDefinition {
                    id,
                    name: name.into(),
                    base: base.map(|n| TypeId::from_bytes([n; 16])),
                    fields: vec![],
                    flags: 1,
                },
            );
        }
        for (n, owner, name, slot, static_) in [
            (1, 2, "Value", SlotType::I32, false),
            (2, 3, "Value", SlotType::I32, false),
            (
                3,
                3,
                "Next",
                SlotType::Reference(TypeId::from_bytes([3; 16])),
                false,
            ),
            (
                4,
                3,
                "Root",
                SlotType::Reference(TypeId::from_bytes([3; 16])),
                true,
            ),
        ] {
            let id = FieldId::from_bytes([n; 16]);
            let owner = TypeId::from_bytes([owner; 16]);
            program.fields.insert(
                id,
                FieldDefinition {
                    id,
                    owner,
                    name: name.into(),
                    value_type: slot,
                    is_static: static_,
                    literal: false,
                },
            );
            program.types.get_mut(&owner).unwrap().fields.push(id);
        }
        program
    }
    #[test]
    fn shallow_cycle_views_preserve_hidden_field_owners_and_all_runtime_state() {
        let program = registry();
        let mut vm = Vm::new(&program);
        let id = ObjectId::fresh();
        vm.heap
            .allocate_initialized(
                id,
                TypeId::from_bytes([3; 16]),
                std::collections::BTreeMap::from([
                    (FieldId::from_bytes([1; 16]), Value::I32(7)),
                    (FieldId::from_bytes([2; 16]), Value::I32(11)),
                    (FieldId::from_bytes([3; 16]), Value::Null),
                ]),
            )
            .unwrap();
        vm.heap
            .set_field(id, FieldId::from_bytes([3; 16]), Value::Reference(id))
            .unwrap();
        vm.statics
            .insert(FieldId::from_bytes([4; 16]), Value::Reference(id));
        vm.checkpoint_generation.set(42);
        vm.set_model_calls_enabled(true);
        let heap = vm.heap.clone();
        let statics = vm.statics.clone();
        let instructions = vm.instructions_executed();
        let view = vm
            .inspect_object(&id.to_string(), InspectionLimits::default())
            .unwrap();
        assert_eq!(view["checkpoint"], 42);
        assert_eq!(view["fieldCount"], 3);
        assert_eq!(view["fieldsReturned"], 3);
        assert_eq!(view["fields"][0]["name"], "Value");
        assert_eq!(view["fields"][1]["name"], "Value");
        assert_eq!(view["fields"][0]["owner"]["name"], "Example.Parent");
        assert_eq!(view["fields"][1]["owner"]["name"], "Example.Child");
        assert_eq!(view["fields"][2]["value"]["id"], id.to_string());
        assert!(view["fields"][2]["value"].get("fields").is_none());
        let static_ = vm.inspect_static("Example.Child::Root", 64).unwrap();
        assert_eq!(static_["field"]["value"]["id"], id.to_string());
        assert_eq!(vm.heap, heap);
        assert_eq!(vm.statics, statics);
        assert_eq!(vm.instructions_executed(), instructions);
        assert!(vm.model_calls_enabled);
        assert_eq!(vm.checkpoint_generation.get(), 42);
    }
    #[test]
    fn output_and_label_bounds_preserve_exact_utf16_and_signal_split_surrogates() {
        let mut program = registry();
        let child = TypeId::from_bytes([3; 16]);
        program.types.get_mut(&child).unwrap().name = "\0".repeat(200);
        let mut fields = std::collections::BTreeMap::new();
        for n in 20..52 {
            let id = FieldId::from_bytes([n; 16]);
            program.fields.insert(
                id,
                FieldDefinition {
                    id,
                    owner: child,
                    name: "\n".repeat(200),
                    value_type: SlotType::Reference(TypeId::from_bytes([4; 16])),
                    is_static: false,
                    literal: false,
                },
            );
            program.types.get_mut(&child).unwrap().fields.push(id);
            fields.insert(id, Value::Null);
        }
        let mut vm = Vm::new(&program);
        let text = vm.new_string(vec![0; 256]).unwrap();
        for value in fields.values_mut() {
            *value = Value::Reference(text);
        }
        fields.insert(FieldId::from_bytes([1; 16]), Value::I32(7));
        fields.insert(FieldId::from_bytes([2; 16]), Value::I32(11));
        fields.insert(FieldId::from_bytes([3; 16]), Value::Null);
        let id = ObjectId::fresh();
        vm.heap.allocate_initialized(id, child, fields).unwrap();
        let view = vm
            .inspect_object(
                &id.to_string(),
                InspectionLimits {
                    max_fields: 32,
                    string_units: 256,
                },
            )
            .unwrap();
        assert!(byte_len(&view).unwrap() <= MAX_OUTPUT_BYTES);
        assert_eq!(view["fieldsTruncated"], true);
        assert!(
            view["fieldsReturned"].as_u64().unwrap() > 0
                && view["fieldsReturned"].as_u64().unwrap() < 32
        );
        assert_eq!(view["type"]["nameTruncated"], true);
        assert!(
            view["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field["nameTruncated"] == true)
        );
        let units = vec![0xd800, 0, 0xd83e, 0xdd80];
        let lone = vm.new_string(units.clone()).unwrap();
        let view = vm
            .inspect_object(&lone.to_string(), InspectionLimits::default())
            .unwrap();
        assert_eq!(view["stringPreview"]["units"], json!(units));
        assert!(view["stringPreview"]["text"].is_null());
        assert_eq!(view["stringPreview"]["previewWellFormed"], false);
        assert_eq!(vm.heap.object(lone).unwrap().string_units().unwrap(), units);
        let pair = vm.new_string(vec![0xd83e, 0xdd80]).unwrap();
        let one = vm
            .inspect_object(
                &pair.to_string(),
                InspectionLimits {
                    string_units: 1,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(one["stringPreview"]["truncated"], true);
        assert_eq!(one["stringPreview"]["previewWellFormed"], false);
        let two = vm
            .inspect_object(
                &pair.to_string(),
                InspectionLimits {
                    string_units: 2,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(two["stringPreview"]["text"], "🦀");
    }
    #[test]
    fn selectors_bounds_transients_and_large_integer_projection_are_explicit() {
        let mut program = registry();
        let duplicate = TypeId::from_bytes([7; 16]);
        program.types.insert(
            duplicate,
            TypeDefinition {
                id: duplicate,
                name: "Example.Child".into(),
                base: None,
                fields: vec![],
                flags: 1,
            },
        );
        for (n, name, ty, literal) in [
            (6, "Root", SlotType::I64, false),
            (7, "Constant", SlotType::I32, true),
        ] {
            let id = FieldId::from_bytes([n; 16]);
            program.fields.insert(
                id,
                FieldDefinition {
                    id,
                    owner: duplicate,
                    name: name.into(),
                    value_type: ty,
                    is_static: true,
                    literal,
                },
            );
        }
        let mut vm = Vm::new(&program);
        let wide = FieldId::from_bytes([6; 16]);
        vm.statics.insert(wide, Value::I64(9_007_199_254_740_993));
        assert_eq!(
            vm.inspect_static(&wide.to_string(), 64).unwrap()["field"]["value"]["value"],
            "9007199254740993"
        );
        for selector in [
            "Example.Child::Root",
            "Example.Child::Constant",
            "/private/file",
            &FieldId::from_bytes([1; 16]).to_string(),
        ] {
            assert!(vm.inspect_static(selector, 64).is_err());
        }
        assert!(vm.inspect_static(&wide.to_string(), 257).is_err());
        assert!(vm.inspect_object("no-id", Default::default()).is_err());
        assert!(
            vm.inspect_object(&ObjectId::fresh().to_string(), Default::default())
                .is_err()
        );
        let transient = ObjectId::fresh();
        vm.heap
            .allocate(
                transient,
                TypeId::from_bytes([3; 16]),
                1,
                AllocationDomain::Transient(ArenaId::fresh()),
            )
            .unwrap();
        assert!(
            vm.inspect_object(&transient.to_string(), Default::default())
                .unwrap_err()
                .0
                .contains("transient")
        );
        for fields in [0, 33] {
            assert!(
                InspectionLimits {
                    max_fields: fields,
                    string_units: 0
                }
                .validate()
                .is_err()
            );
        }
    }
}
