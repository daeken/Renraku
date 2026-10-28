use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use crate::{
    identity::{ArenaId, FieldId, ObjectId, TypeId},
    value::Value,
};

/// Managed allocations become durable through reachability. Transient objects
/// belong to an arena and cannot be retained by a durable root or managed object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationDomain {
    Managed,
    Transient(ArenaId),
}
pub const MAX_STRING_UNITS: usize = 1_000_000;
static EMPTY_FIELDS: BTreeMap<FieldId, Value> = BTreeMap::new();

#[derive(Clone, Debug, Eq, PartialEq)]
enum ObjectData {
    Fields(BTreeMap<FieldId, Value>),
    String(Vec<u16>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Object {
    id: ObjectId,
    type_id: TypeId,
    schema_version: u32,
    domain: AllocationDomain,
    data: ObjectData,
}

impl Object {
    pub fn id(&self) -> ObjectId {
        self.id
    }
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
    pub fn domain(&self) -> AllocationDomain {
        self.domain
    }
    pub fn fields(&self) -> &BTreeMap<FieldId, Value> {
        match &self.data {
            ObjectData::Fields(fields) => fields,
            ObjectData::String(_) => &EMPTY_FIELDS,
        }
    }
    pub fn string_units(&self) -> Option<&[u16]> {
        match &self.data {
            ObjectData::String(units) => Some(units),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Heap {
    objects: BTreeMap<ObjectId, Object>,
    roots: BTreeMap<String, ObjectId>,
}

impl Heap {
    pub fn objects(&self) -> impl Iterator<Item = &Object> {
        self.objects.values()
    }

    pub(crate) fn retained_subset(&self, retained: &BTreeSet<ObjectId>) -> Self {
        Self {
            objects: self
                .objects
                .iter()
                .filter(|(id, _)| retained.contains(id))
                .map(|(id, object)| (*id, object.clone()))
                .collect(),
            roots: self.roots.clone(),
        }
    }
    pub fn allocate(
        &mut self,
        id: ObjectId,
        type_id: TypeId,
        schema_version: u32,
        domain: AllocationDomain,
    ) -> Result<(), HeapError> {
        if self.objects.contains_key(&id) {
            return Err(HeapError::DuplicateObject(id));
        }
        self.objects.insert(
            id,
            Object {
                id,
                type_id,
                schema_version,
                domain,
                data: ObjectData::Fields(BTreeMap::new()),
            },
        );
        Ok(())
    }

    pub fn object(&self, id: ObjectId) -> Result<&Object, HeapError> {
        self.objects.get(&id).ok_or(HeapError::UnknownObject(id))
    }

    pub(crate) fn allocate_initialized(
        &mut self,
        id: ObjectId,
        type_id: TypeId,
        fields: BTreeMap<FieldId, Value>,
    ) -> Result<(), HeapError> {
        if self.objects.contains_key(&id) {
            return Err(HeapError::DuplicateObject(id));
        }
        for target in fields.values().filter_map(Value::referenced_object) {
            self.require_managed(target)?;
        }
        self.objects.insert(
            id,
            Object {
                id,
                type_id,
                schema_version: 1,
                domain: AllocationDomain::Managed,
                data: ObjectData::Fields(fields),
            },
        );
        Ok(())
    }
    pub(crate) fn allocate_string(
        &mut self,
        id: ObjectId,
        type_id: TypeId,
        units: Vec<u16>,
    ) -> Result<(), HeapError> {
        if self.objects.contains_key(&id) {
            return Err(HeapError::DuplicateObject(id));
        }
        if units.len() > MAX_STRING_UNITS {
            return Err(HeapError::StringTooLarge);
        }
        self.objects.insert(
            id,
            Object {
                id,
                type_id,
                schema_version: 1,
                domain: AllocationDomain::Managed,
                data: ObjectData::String(units),
            },
        );
        Ok(())
    }

    pub fn roots(&self) -> &BTreeMap<String, ObjectId> {
        &self.roots
    }

    /// Validate before mutating, so a rejected reference leaves the field intact.
    /// The interpreter separately validates field declarations and slot types.
    pub fn set_field(
        &mut self,
        owner: ObjectId,
        field: FieldId,
        value: Value,
    ) -> Result<(), HeapError> {
        let owner_domain = self.object(owner)?.domain;
        if let Some(target) = value.referenced_object() {
            let target_domain = self.object(target)?.domain;
            let allowed = match (owner_domain, target_domain) {
                (_, AllocationDomain::Managed) => true,
                (AllocationDomain::Transient(a), AllocationDomain::Transient(b)) => a == b,
                _ => false,
            };
            if !allowed {
                return Err(HeapError::InvalidLifetime { owner, target });
            }
        }
        let ObjectData::Fields(fields) = &mut self.objects.get_mut(&owner).unwrap().data else {
            return Err(HeapError::ImmutableString(owner));
        };
        fields.insert(field, value);
        Ok(())
    }

    pub fn set_root(&mut self, name: impl Into<String>, id: ObjectId) -> Result<(), HeapError> {
        self.require_managed(id)?;
        self.roots.insert(name.into(), id);
        Ok(())
    }

    pub fn remove_root(&mut self, name: &str) -> Option<ObjectId> {
        self.roots.remove(name)
    }

    /// Trace named roots together with retained execution references. This is
    /// the candidate durable graph, not a transaction or a checkpoint operation.
    /// A transient or dangling execution reference makes the entire trace fail.
    pub fn retained_objects(
        &self,
        execution_references: impl IntoIterator<Item = ObjectId>,
    ) -> Result<BTreeSet<ObjectId>, HeapError> {
        let mut pending: Vec<_> = self
            .roots
            .values()
            .copied()
            .chain(execution_references)
            .collect();
        let mut retained = BTreeSet::new();
        while let Some(id) = pending.pop() {
            let object = self.require_managed(id)?;
            if retained.insert(id) {
                pending.extend(
                    object
                        .fields()
                        .values()
                        .filter_map(Value::referenced_object),
                );
            }
        }
        Ok(retained)
    }

    fn require_managed(&self, id: ObjectId) -> Result<&Object, HeapError> {
        let object = self.object(id)?;
        if object.domain != AllocationDomain::Managed {
            return Err(HeapError::TransientRetention(id));
        }
        Ok(object)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeapError {
    StringTooLarge,
    ImmutableString(ObjectId),
    DuplicateObject(ObjectId),
    UnknownObject(ObjectId),
    InvalidLifetime { owner: ObjectId, target: ObjectId },
    TransientRetention(ObjectId),
}

impl fmt::Display for HeapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StringTooLarge => write!(formatter, "string exceeds the UTF-16 unit limit"),
            Self::ImmutableString(id) => write!(formatter, "string object {id} is immutable"),
            Self::DuplicateObject(id) => write!(formatter, "object {id} already exists"),
            Self::UnknownObject(id) => write!(formatter, "object {id} does not exist"),
            Self::InvalidLifetime { owner, target } => {
                write!(
                    formatter,
                    "object {owner} cannot retain object {target} from its allocation domain"
                )
            }
            Self::TransientRetention(id) => write!(
                formatter,
                "transient object {id} cannot be retained durably"
            ),
        }
    }
}

impl Error for HeapError {}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_are_immutable_and_preserve_arbitrary_utf16_units() {
        let mut heap = Heap::default();
        let id = ObjectId::fresh();
        heap.allocate_string(id, TypeId::fresh(), vec![0, 0xd800, 0xffff])
            .unwrap();
        let before = heap.object(id).unwrap().clone();
        assert_eq!(
            heap.set_field(id, FieldId::fresh(), Value::I32(1)),
            Err(HeapError::ImmutableString(id))
        );
        assert_eq!(heap.object(id).unwrap(), &before);
        assert_eq!(
            heap.object(id).unwrap().string_units().unwrap(),
            [0, 0xd800, 0xffff]
        );
    }
}
