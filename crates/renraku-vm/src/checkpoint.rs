//! Atomic universe checkpoints. The binary format is independent of SQLite.
mod validate;

use crate::{
    cil::Reader,
    effects::{EffectRecord, EffectState},
    error::{Error, Result, require},
    execution::{
        CallResult, Continuation, ExecutionContext, ExecutionStatus, Frame, MethodVersion,
    },
    heap::{AllocationDomain, Heap},
    identity::{
        AssemblyId, CodeVersionId, EffectId, ExecutionId, FieldId, MethodId, ObjectId, TypeId,
        UniverseId,
    },
    program::{AssemblyBindings, Program},
    runtime::{RunOutcome, Vm},
    value::Value,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{cell::Cell, collections::BTreeMap, path::Path, time::Duration};

const MAGIC: &[u8; 8] = b"RENRAKU\0";
const FORMAT: u32 = 3;
const APPLICATION_ID: i64 = 0x52454e52;
const MAX_BYTES: usize = 128 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;

#[derive(Clone, Debug)]
struct State {
    generation: u64,
    universe: UniverseId,
    heap: Heap,
    statics: BTreeMap<FieldId, Value>,
    interned_strings: BTreeMap<Vec<u16>, ObjectId>,
    effects: BTreeMap<EffectId, EffectRecord>,
    context: ExecutionContext,
    entry: Option<MethodVersion>,
    instructions: u64,
    max_frames: usize,
}

#[derive(Debug)]
pub struct Snapshot {
    pub program: Program,
    state: State,
}
impl Snapshot {
    pub fn restore(&self) -> Result<Vm<'_>> {
        let state = self.state.clone();
        let effects = state
            .effects
            .into_iter()
            .map(|(id, mut effect)| {
                if effect.state == EffectState::InFlight {
                    effect.state = EffectState::OutcomeUnknown {
                        reason: "recovered without a committed response".into(),
                    };
                }
                (id, effect)
            })
            .collect();
        let vm = Vm {
            program: &self.program,
            heap: state.heap,
            statics: state.statics,
            interned_strings: state.interned_strings,
            effects,
            context: state.context,
            universe: state.universe,
            entry: state.entry,
            instructions_executed: state.instructions,
            max_frames: state.max_frames,
            checkpoint_generation: Cell::new(state.generation),
        };
        validate::validate(&vm)?;
        Ok(vm)
    }
}
#[derive(Debug)]
pub struct StoredCheckpoint {
    pub generation: u64,
    pub snapshot: Snapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitStage {
    Prepared,
    Written,
    Committed,
}

pub struct CheckpointStore {
    connection: Connection,
    expected_generation: u64,
}
struct StoredRow {
    generation: i64,
    universe: Vec<u8>,
    format: u32,
    digest: Vec<u8>,
    payload: Vec<u8>,
}
impl CheckpointStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if application == 0 && version == 0 {
            let tables: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            require(tables == 0, "database is not an empty Renraku store")?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(
                "CREATE TABLE checkpoint (
                    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                    generation INTEGER NOT NULL CHECK(generation>0),
                    universe BLOB NOT NULL CHECK(length(universe)=16),
                    format INTEGER NOT NULL,
                    digest BLOB NOT NULL CHECK(length(digest)=32),
                    payload BLOB NOT NULL
                );
                PRAGMA application_id=1380273746;
                PRAGMA user_version=1;",
            )?;
            transaction.commit()?;
        } else {
            require(
                application == APPLICATION_ID && version == 1,
                "unsupported checkpoint database identity or schema",
            )?;
        }
        let mode: String = connection.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            let mode: String = connection.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
            require(
                mode.eq_ignore_ascii_case("wal"),
                "checkpoint store requires SQLite WAL mode",
            )?;
        }
        connection.pragma_update(None, "synchronous", "FULL")?;
        // Keep dirty pages spillable so fault tests can terminate a transaction
        // with uncommitted WAL pages rather than only an in-memory page cache.
        connection.pragma_update(None, "cache_size", 32)?;
        let generation: Option<i64> = connection
            .query_row(
                "SELECT generation FROM checkpoint WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let expected_generation = generation.map_or(Ok(0), |g| {
            u64::try_from(g).map_err(|_| Error("invalid checkpoint generation".into()))
        })?;
        Ok(Self {
            connection,
            expected_generation,
        })
    }
    pub fn generation(&self) -> u64 {
        self.expected_generation
    }
    pub fn save(&mut self, vm: &Vm<'_>) -> Result<u64> {
        self.save_with_observer(vm, |_| Ok(()))
    }
    /// Observers expose deterministic crash windows for testing. An error before
    /// commit rolls back; an error from Committed cannot undo the completed commit.
    pub fn save_with_observer(
        &mut self,
        vm: &Vm<'_>,
        mut observer: impl FnMut(CommitStage) -> Result<()>,
    ) -> Result<u64> {
        let payload = encode(vm)?;
        let digest = Sha256::digest(&payload);
        observer(CommitStage::Prepared)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let head: Option<(i64, Vec<u8>)> = transaction
            .query_row(
                "SELECT generation,universe FROM checkpoint WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let actual = head.as_ref().map_or(0, |head| head.0);
        require(
            actual >= 0 && actual as u64 == self.expected_generation,
            "checkpoint conflict: store advanced since it was read",
        )?;
        if let Some((_, universe)) = head {
            require(
                universe.as_slice() == vm.universe.as_bytes(),
                "database belongs to a different universe",
            )?;
        }
        require(
            vm.checkpoint_generation.get() == actual as u64,
            "checkpoint conflict: VM was restored from an older generation",
        )?;
        let generation = actual
            .checked_add(1)
            .ok_or_else(|| Error("checkpoint generation exhausted".into()))?;
        transaction.execute(
            "INSERT INTO checkpoint(singleton,generation,universe,format,digest,payload) VALUES(1,?1,?2,?3,?4,?5)
             ON CONFLICT(singleton) DO UPDATE SET generation=excluded.generation,universe=excluded.universe,
                format=excluded.format,digest=excluded.digest,payload=excluded.payload",
            params![generation,vm.universe.as_bytes().as_slice(),FORMAT,digest.as_slice(),payload])?;
        transaction.cache_flush()?;
        observer(CommitStage::Written)?;
        transaction.commit()?;
        self.expected_generation = generation as u64;
        vm.checkpoint_generation.set(generation as u64);
        observer(CommitStage::Committed)?;
        Ok(generation as u64)
    }
    pub fn load(&mut self) -> Result<StoredCheckpoint> {
        let row:Option<StoredRow> = self.connection.query_row(
            "SELECT generation,universe,format,digest,payload FROM checkpoint WHERE singleton=1",
            [],|r|Ok(StoredRow {generation:r.get(0)?,universe:r.get(1)?,format:r.get(2)?,digest:r.get(3)?,payload:r.get(4)?})).optional()?;
        let StoredRow {
            generation,
            universe,
            format,
            digest,
            payload,
        } = row.ok_or_else(|| Error("no committed checkpoint".into()))?;
        require(
            generation > 0 && matches!(format, 1..=3),
            "unsupported checkpoint format or generation",
        )?;
        require(
            payload.len() <= MAX_BYTES,
            "checkpoint exceeds format size limit",
        )?;
        require(
            Sha256::digest(&payload).as_slice() == digest,
            "checkpoint checksum mismatch",
        )?;
        require(
            payload
                .get(8..12)
                .is_some_and(|v| v == format.to_le_bytes()),
            "checkpoint row and payload format disagree",
        )?;
        let mut snapshot = decode(&payload)?;
        snapshot.state.generation = generation as u64;
        require(
            snapshot.state.universe.as_bytes().as_slice() == universe,
            "checkpoint universe identity mismatch",
        )?;
        self.expected_generation = generation as u64;
        Ok(StoredCheckpoint {
            generation: generation as u64,
            snapshot,
        })
    }
}

impl Vm<'_> {
    pub fn run_checkpointed(
        &mut self,
        store: &mut CheckpointStore,
        budget: u64,
        interval: u64,
    ) -> Result<RunOutcome> {
        require(interval > 0, "checkpoint interval must be positive")?;
        let mut remaining = budget;
        loop {
            let chunk = remaining.min(interval);
            let outcome = self.run(chunk)?;
            remaining -= chunk;
            if outcome == RunOutcome::Paused && remaining > 0 {
                self.context.status = ExecutionStatus::Runnable;
            }
            store.save(self)?;
            if outcome != RunOutcome::Paused || remaining == 0 {
                return Ok(outcome);
            }
        }
    }
}

struct Writer(Vec<u8>);
impl Writer {
    fn byte(&mut self, v: u8) {
        self.0.push(v);
    }
    fn word(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    fn wide(&mut self, v: u64) {
        self.0.extend(v.to_le_bytes());
    }
    fn id(&mut self, v: &[u8; 16]) {
        self.0.extend(v);
    }
    fn count(&mut self, v: usize) -> Result<()> {
        require(v <= MAX_ITEMS, "checkpoint collection exceeds format limit")?;
        self.word(v as u32);
        Ok(())
    }
    fn bytes(&mut self, v: &[u8]) -> Result<()> {
        require(v.len() <= MAX_BYTES, "checkpoint data exceeds format limit")?;
        self.word(v.len() as u32);
        self.0.extend(v);
        Ok(())
    }
    fn value(&mut self, v: Value) {
        self.0.extend(v.to_bytes());
    }
    fn values(&mut self, values: &[Value]) -> Result<()> {
        self.count(values.len())?;
        for v in values {
            self.value(*v);
        }
        Ok(())
    }
    fn method(&mut self, v: MethodVersion) {
        self.id(v.method.as_bytes());
        self.id(v.code.as_bytes());
    }
    fn fields(&mut self, fields: &BTreeMap<FieldId, Value>) -> Result<()> {
        self.count(fields.len())?;
        for (id, value) in fields {
            self.id(id.as_bytes());
            self.value(*value);
        }
        Ok(())
    }
}
fn encode(vm: &Vm<'_>) -> Result<Vec<u8>> {
    validate::validate(vm)?;
    let retained = vm.retained_objects()?;
    let mut w = Writer(MAGIC.to_vec());
    w.word(FORMAT);
    w.id(vm.universe.as_bytes());
    let bindings = vm.program.bindings();
    require(
        !bindings.is_empty(),
        "checkpoint requires original assembly images",
    )?;
    w.count(bindings.len())?;
    for b in bindings {
        w.id(b.id.as_bytes());
        w.bytes(&b.bytes)?;
        w.count(b.types.len())?;
        for (token, id) in b.types {
            w.word(token);
            w.id(id.as_bytes());
        }
        w.count(b.fields.len())?;
        for (token, id) in b.fields {
            w.word(token);
            w.id(id.as_bytes());
        }
        w.count(b.methods.len())?;
        for (token, binding) in b.methods {
            w.word(token);
            w.method(binding);
        }
    }
    w.count(retained.len())?;
    for id in retained {
        let object = vm.heap.object(id)?;
        w.id(id.as_bytes());
        w.id(object.type_id().as_bytes());
        w.word(object.schema_version());
        if let Some(units) = object.string_units() {
            w.byte(1);
            w.count(units.len())?;
            for unit in units {
                w.0.extend(unit.to_le_bytes());
            }
        } else {
            w.byte(0);
            w.fields(object.fields())?;
        }
    }
    w.count(vm.heap.roots().len())?;
    for (name, id) in vm.heap.roots() {
        w.bytes(name.as_bytes())?;
        w.id(id.as_bytes());
    }
    w.fields(&vm.statics)?;
    w.count(vm.interned_strings.len())?;
    for id in vm.interned_strings.values() {
        w.id(id.as_bytes());
    }
    w.count(vm.effects.len())?;
    for effect in vm.effects.values() {
        w.id(effect.id.as_bytes());
        w.id(effect.request.as_bytes());
        w.bytes(effect.capability.as_bytes())?;
        match &effect.state {
            EffectState::Prepared => w.byte(0),
            EffectState::InFlight => w.byte(1),
            EffectState::Completed { response } => {
                w.byte(2);
                w.id(response.as_bytes());
            }
            EffectState::OutcomeUnknown { reason } => {
                w.byte(3);
                w.bytes(reason.as_bytes())?;
            }
        }
    }
    w.id(vm.context.id.as_bytes());
    match &vm.context.status {
        ExecutionStatus::Runnable => w.byte(0),
        ExecutionStatus::Paused => w.byte(1),
        ExecutionStatus::Completed { result } => {
            w.byte(2);
            w.byte(u8::from(result.is_some()));
            if let Some(value) = result {
                w.value(*value);
            }
        }
        ExecutionStatus::Faulted { message } => {
            w.byte(3);
            w.bytes(message.as_bytes())?;
        }
    }
    w.byte(u8::from(vm.entry.is_some()));
    if let Some(entry) = vm.entry {
        w.method(entry);
    }
    w.wide(vm.instructions_executed);
    w.count(vm.max_frames)?;
    w.count(vm.context.frames.len())?;
    for frame in &vm.context.frames {
        w.method(frame.method);
        match frame.continuation {
            Continuation::Execute { next } => {
                w.byte(0);
                w.word(next);
            }
            Continuation::AwaitReturn {
                call_site,
                next,
                result,
            } => {
                w.byte(1);
                w.word(call_site);
                w.word(next);
                match result {
                    CallResult::CalleeReturn => w.byte(0),
                    CallResult::ConstructedObject(id) => {
                        w.byte(1);
                        w.id(id.as_bytes());
                    }
                }
            }
        }
        w.values(&frame.arguments)?;
        w.count(frame.locals.len())?;
        for local in &frame.locals {
            w.byte(u8::from(local.is_some()));
            if let Some(value) = local {
                w.value(*value);
            }
        }
        w.values(&frame.evaluation_stack)?;
    }
    require(
        w.0.len() <= MAX_BYTES,
        "checkpoint exceeds format size limit",
    )?;
    Ok(w.0)
}

fn id(r: &mut Reader<'_>) -> Result<[u8; 16]> {
    Ok(r.take(16)?.try_into().unwrap())
}
fn wide(r: &mut Reader<'_>) -> Result<u64> {
    Ok(u64::from_le_bytes(r.take(8)?.try_into().unwrap()))
}
fn count(r: &mut Reader<'_>, minimum: usize) -> Result<usize> {
    let count = r.u32()? as usize;
    require(
        count <= MAX_ITEMS && count <= (r.data.len() - r.position) / minimum.max(1),
        "invalid checkpoint collection length",
    )?;
    Ok(count)
}
fn bytes<'a>(r: &mut Reader<'a>) -> Result<&'a [u8]> {
    let length = r.u32()? as usize;
    require(length <= MAX_BYTES, "invalid checkpoint data length")?;
    r.take(length)
}
fn value(r: &mut Reader<'_>) -> Result<Value> {
    let start = r.position;
    let version = r.u8()?;
    let tag = r.u8()?;
    require(version == 1, "unknown checkpoint value version")?;
    let length = match tag {
        0 => 0,
        1 => 4,
        2 => 8,
        3 => 16,
        _ => return Err(Error("unknown checkpoint value tag".into())),
    };
    r.take(length)?;
    Value::from_bytes(&r.data[start..r.position]).map_err(|e| Error(e.to_string()))
}
fn flag(r: &mut Reader<'_>) -> Result<bool> {
    match r.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error("invalid checkpoint flag".into())),
    }
}
fn method(r: &mut Reader<'_>) -> Result<MethodVersion> {
    Ok(MethodVersion {
        method: MethodId::from_bytes(id(r)?),
        code: CodeVersionId::from_bytes(id(r)?),
    })
}
fn fields(r: &mut Reader<'_>) -> Result<BTreeMap<FieldId, Value>> {
    let mut fields = BTreeMap::new();
    for _ in 0..count(r, 18)? {
        let key = FieldId::from_bytes(id(r)?);
        require(
            fields.insert(key, value(r)?).is_none(),
            "duplicate checkpoint field",
        )?;
    }
    Ok(fields)
}
fn values(r: &mut Reader<'_>) -> Result<Vec<Value>> {
    (0..count(r, 2)?).map(|_| value(r)).collect()
}
fn decode(payload: &[u8]) -> Result<Snapshot> {
    require(
        payload.len() <= MAX_BYTES,
        "checkpoint exceeds format size limit",
    )?;
    let mut r = Reader::new(payload);
    require(r.take(8)? == MAGIC, "unsupported checkpoint payload format")?;
    let format = r.u32()?;
    require(
        matches!(format, 1..=3),
        "unsupported checkpoint payload format",
    )?;
    let universe = UniverseId::from_bytes(id(&mut r)?);
    let mut assemblies = Vec::new();
    for _ in 0..count(&mut r, 32)? {
        let assembly = AssemblyId::from_bytes(id(&mut r)?);
        let data = bytes(&mut r)?.to_vec();
        let mut types = BTreeMap::new();
        for _ in 0..count(&mut r, 20)? {
            let token = r.u32()?;
            require(
                types
                    .insert(token, TypeId::from_bytes(id(&mut r)?))
                    .is_none(),
                "duplicate type binding",
            )?;
        }
        let mut field_ids = BTreeMap::new();
        for _ in 0..count(&mut r, 20)? {
            let token = r.u32()?;
            require(
                field_ids
                    .insert(token, FieldId::from_bytes(id(&mut r)?))
                    .is_none(),
                "duplicate field binding",
            )?;
        }
        let mut methods = BTreeMap::new();
        for _ in 0..count(&mut r, 36)? {
            let token = r.u32()?;
            require(
                methods.insert(token, method(&mut r)?).is_none(),
                "duplicate method binding",
            )?;
        }
        assemblies.push(AssemblyBindings {
            id: assembly,
            bytes: data,
            types,
            fields: field_ids,
            methods,
        });
    }
    let program = Program::from_bindings(&assemblies)?;
    let mut heap = Heap::default();
    let mut object_fields = Vec::new();
    for _ in 0..count(&mut r, 40)? {
        let object = ObjectId::from_bytes(id(&mut r)?);
        let type_id = TypeId::from_bytes(id(&mut r)?);
        let schema = r.u32()?;
        require(schema == 1, "unsupported object schema version")?;
        let kind = if format >= 2 { r.u8()? } else { 0 };
        match kind {
            0 => {
                heap.allocate(object, type_id, schema, AllocationDomain::Managed)?;
                object_fields.push((object, fields(&mut r)?));
            }
            1 => {
                let mut units = Vec::new();
                for _ in 0..count(&mut r, 2)? {
                    units.push(r.u16()?);
                }
                heap.allocate_string(object, type_id, units)?;
            }
            _ => return Err(Error("unsupported object payload kind".into())),
        }
    }
    for (owner, fields) in object_fields {
        for (field, value) in fields {
            heap.set_field(owner, field, value)?;
        }
    }
    for _ in 0..count(&mut r, 20)? {
        let name = std::str::from_utf8(bytes(&mut r)?)
            .map_err(|_| Error("invalid root name UTF-8".into()))?
            .to_owned();
        let object = ObjectId::from_bytes(id(&mut r)?);
        require(!heap.roots().contains_key(&name), "duplicate root name")?;
        heap.set_root(name, object)?;
    }
    let statics = fields(&mut r)?;
    let mut interned_strings = BTreeMap::new();
    if format >= 2 {
        for _ in 0..count(&mut r, 16)? {
            let id = ObjectId::from_bytes(id(&mut r)?);
            let units = heap
                .object(id)?
                .string_units()
                .ok_or_else(|| Error("interned object is not a string".into()))?
                .to_vec();
            require(
                interned_strings.insert(units, id).is_none(),
                "duplicate interned string",
            )?;
        }
    }
    let mut effects = BTreeMap::new();
    if format >= 3 {
        for _ in 0..count(&mut r, 37)? {
            let effect = EffectId::from_bytes(id(&mut r)?);
            let request = ObjectId::from_bytes(id(&mut r)?);
            let capability = std::str::from_utf8(bytes(&mut r)?)
                .map_err(|_| Error("invalid effect capability UTF-8".into()))?
                .into();
            let state = match r.u8()? {
                0 => EffectState::Prepared,
                1 => EffectState::InFlight,
                2 => EffectState::Completed {
                    response: ObjectId::from_bytes(id(&mut r)?),
                },
                3 => EffectState::OutcomeUnknown {
                    reason: std::str::from_utf8(bytes(&mut r)?)
                        .map_err(|_| Error("invalid effect diagnostic UTF-8".into()))?
                        .into(),
                },
                _ => return Err(Error("unknown effect state".into())),
            };
            require(
                effects
                    .insert(
                        effect,
                        EffectRecord {
                            id: effect,
                            capability,
                            request,
                            state,
                        },
                    )
                    .is_none(),
                "duplicate effect identity",
            )?;
        }
    }
    let execution = ExecutionId::from_bytes(id(&mut r)?);
    let status = match r.u8()? {
        0 => ExecutionStatus::Runnable,
        1 => ExecutionStatus::Paused,
        2 => ExecutionStatus::Completed {
            result: if flag(&mut r)? {
                Some(value(&mut r)?)
            } else {
                None
            },
        },
        3 => ExecutionStatus::Faulted {
            message: std::str::from_utf8(bytes(&mut r)?)
                .map_err(|_| Error("invalid fault text".into()))?
                .into(),
        },
        _ => return Err(Error("unknown execution status".into())),
    };
    let entry = if flag(&mut r)? {
        Some(method(&mut r)?)
    } else {
        None
    };
    let instructions = wide(&mut r)?;
    let max_frames = r.u32()? as usize;
    require(max_frames <= MAX_ITEMS, "invalid checkpoint frame limit")?;
    let mut frames = Vec::new();
    for _ in 0..count(&mut r, 49)? {
        let method = method(&mut r)?;
        let continuation = match r.u8()? {
            0 => Continuation::Execute { next: r.u32()? },
            1 => {
                let call_site = r.u32()?;
                let next = r.u32()?;
                let result = match r.u8()? {
                    0 => CallResult::CalleeReturn,
                    1 => CallResult::ConstructedObject(ObjectId::from_bytes(id(&mut r)?)),
                    _ => return Err(Error("unknown call result continuation".into())),
                };
                Continuation::AwaitReturn {
                    call_site,
                    next,
                    result,
                }
            }
            _ => return Err(Error("unknown frame continuation".into())),
        };
        let arguments = values(&mut r)?;
        let mut locals = Vec::new();
        for _ in 0..count(&mut r, 1)? {
            locals.push(if flag(&mut r)? {
                Some(value(&mut r)?)
            } else {
                None
            });
        }
        let evaluation_stack = values(&mut r)?;
        frames.push(Frame {
            method,
            continuation,
            arguments,
            locals,
            evaluation_stack,
        });
    }
    require(r.position == payload.len(), "trailing checkpoint data")?;
    let snapshot = Snapshot {
        program,
        state: State {
            generation: 0,
            universe,
            heap,
            statics,
            interned_strings,
            effects,
            context: ExecutionContext {
                id: execution,
                status,
                frames,
            },
            entry,
            instructions,
            max_frames,
        },
    };
    let vm = snapshot.restore()?;
    require(
        vm.retained_objects()?.len() == vm.heap.objects().count(),
        "checkpoint contains unreachable objects",
    )?;
    Ok(snapshot)
}
