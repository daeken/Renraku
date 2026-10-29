//! Prototype text/JSON agent protocol. Durable ownership lives in linked CIL
//! AgentSession/AgentTurn/ChatMessage objects; native workers are disposable.
use crate::{
    artifact::install_once,
    checkpoint::CheckpointStore,
    compiler::{CompilationRequest, CompilationStore, CompilerToolchain},
    driver::{DriverLimits, DriverStop},
    effects::AsyncModelProvider,
    error::{Error, Result, require},
    execution::ExecutionStatus,
    identity::{ObjectId, TypeId},
    invocation::InvocationState,
    program::Program,
    runtime::Vm,
    value::Value,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeSet,
    future::{Future, poll_fn},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    task::{Poll, Waker},
};

const HARNESS: &str = "Renraku.Guest.AgentHarness";
const SESSION: &str = "Renraku.Guest.AgentSession";
const TURN: &str = "Renraku.Guest.AgentTurn";
const INPUT: &str = "Renraku.Guest.AgentInput";
const MAX_TURNS: u32 = 32;
const MAX_ACTION: usize = 64 * 1024;
const MAX_OUTPUT: usize = 16 * 1024;
pub const SYSTEM_PROMPT: &str = r#"You control a persistent CIL runtime. Reply with exactly one JSON object, no Markdown or prose. This is a text protocol, not native function calling. Tool results arrive as user JSON messages. Supported actions:
{"op":"search","query":"counter","limit":20} searches live type/field/method names (limit 1..50).
{"op":"read_code","method":"logical method/code ID or Type::Method"} reads exact saved CIL and retained assembly-scoped source. Optional sourceOnly:true returns retained assembly-scoped source provenance and pinned method/code identity without CIL instructions or the build manifest; use it when a full listing exceeds the output limit. Source is provenance, not executable authority.
{"op":"describe","type":"type name or logical ID"} describes a compiler contract; unsupported shapes return an error.
{"op":"inspect_static","field":"logical field ID or Declaring.Type::Field","stringUnits":64} observes one stored static field without executing guest code. Returns output.field with owner/declaredSlot/value; object values include a logical id. Scalar values show the runtime's signed i32/i64 storage bits, not source-language unsigned or enum formatting; describe the compiler contract for source declarations. Constants and ambiguous selectors reject. stringUnits defaults 64 and permits 0..256 exact UTF-16 units.
{"op":"inspect_object","id":"logical object ID","maxFields":16,"stringUnits":64} observes one managed object's fields without recursively expanding reference graphs. Freeze retains this object through GC. maxFields defaults 16 and permits 1..32; stringUnits defaults 64 and permits 0..256. Views include stable identities, owners/types, scalar values or object IDs, observed checkpoint generation, and explicit truncation. Int64 values are decimal strings. String previews retain exact UTF-16 unit arrays; text is null if the displayed prefix contains an unpaired surrogate. Label previews may truncate. Reference IDs in old results are observations, not permanent retention promises. A lost unappended read may observe a later checkpoint on resume; committed tool results remain in managed history. No host paths or mutation/capability grants.
{"op":"compile","assembly":"Generated.Example","source":"C# source","referenceTypes":["Renraku.Guest.Counter"]} compiles one Generated.cs against the core and selected live types. Roots may span modules; acyclic declaration dependencies are frozen automatically. Source <=32768 UTF-8 bytes. Compilation alone does not install code.
{"op":"install","compilationTurn":3} installs the artifact compiled in that earlier numbered turn. Assembly names must be new; this does not replace existing bodies.
{"op":"invoke","method":"logical method ID or Type::Method","arguments":[{"kind":"static","field":"Renraku.Guest.Simulation::Saved"}]} invokes a static entry with <=8 arguments. Other arguments: {"kind":"i32","value":7}, {"kind":"i64","value":"9007199254740993"}, {"kind":"object","id":"logical object ID"}, {"kind":"null"}. Calls have bounded instructions and cannot submit model requests. Static selectors freeze to their current values before execution.
{"op":"web_status"} inspects the saved default website publication, current host publication authority, and supported handler ABI. It performs no publication or HTTP request.
{"op":"preview_web","method":"GET","target":"/page","body":""} executes the current universe's published handler locally. GET/POST only, ASCII origin-form target <=1024 bytes, UTF-8 body <=8192 bytes (default empty). The selected publication and exact request are frozen; a route change before acceptance returns an error, while an accepted call resumes its original code. This can mutate managed state, once per saved turn-derived request ID, and runs under guest instruction budgets with model calls disabled. It opens no socket and does not test HTTP transport. Results include status/state/body, execution ID, original body byte length, and explicit truncation (at most 8192 JSON-encoded body bytes). Do not include a request ID, publication, URL, host path, or compiler path.
{"op":"publish_web","method":"logical method ID or Type::Method"} publishes an already installed static string handler with four string arguments (id, method, target, body). Optional codeVersion pins an exact installed code ID. Requires separate host publication opt-in, disabled by default. New HTTP request IDs select the publication; accepted requests keep their pinned code/response. Compilation, installation, and publication are separate operations. Handler HTML is bounded to 32768 UTF-8 bytes, with fixed HTTP-v1 status 200/content type. The handler owns application routing and uses ordinary persistent objects/statics. No host paths, sockets, or extra capabilities are granted. Only one guest context can execute; HTTP work must wait while the agent is active. Do not invent formatting/collection APIs: inspect supported CIL and installed methods first. If separately installed, Renraku.Text.IntegerText offers FormatInt32/FormatInt64 and ParseInt32/ParseInt64 (Success/Value/Error/Position), with optional ASCII sign/digits and a 64-UTF-16-unit parse limit. Search/describe it and select its frozen referenceTypes before use; existing universes do not automatically contain this module. If separately installed, Renraku.Text.UnsignedIntegerText offers FormatUInt32/FormatUInt64 and ParseUInt32/ParseUInt64 (Success/Value/Error/Position), with optional ASCII plus/digits within 64 UTF-16 units; minus signs including -0 reject. Unsigned values use the same raw I32/I64 storage bits exposed by inspection/invoke arguments. Search/describe its own frozen view before use. If separately installed, Renraku.Text.HexIntegerText offers FormatUInt32/FormatUInt64 (lowercase, no prefix) and ParseUInt32/ParseUInt64 (ASCII hex digits with optional 0x/0X prefix, no signs/whitespace, <=64 UTF-16 units), returning the existing unsigned parse-result classes. Select its frozen root; foreign result dependencies are included automatically. If separately installed, Renraku.Text.OrdinalText offers IndexOf(text, needle, startIndex), StartsWith and EndsWith returning StringMatchResult (Success/Found/Index/Error), with text/needle limits 256/32 exact UTF-16 units. Empty needles match startIndex/0/text.Length respectively. Null/length/start errors are data; valid no-match has Success true, Found false and Index -1. Comparisons are ordinal including NUL/surrogates. Worst-case bounded IndexOf is tested below 150000 guest instructions; budget pauses must resume the same invocation. Select its frozen root before use. If separately installed, Renraku.Collections.StringMap offers instance Get/Set/Remove/Count, with 64 entries, 128-unit keys, 4096-unit nullable values, ordinal UTF-16 equality, and Success/Found/Changed/Value/Error/Count results. Search/describe its own frozen view before use. Mutations are ordinary multi-instruction CIL; preserve the original invocation ID on recovery.
{"op":"finish","message":"short result"} ends the session.
Search and inspect before coding. The initial test object is Renraku.Guest.Simulation::Saved, a persistent Counter (created with Value=19 only if absent). Use normal managed fields/methods and supported CIL: integers, references, strings, constructors, calls, branches. Int32/UInt32/Int64/UInt64 division and remainder are supported: signed division truncates toward zero; zero divisors and signed minimum / or % -1 cause terminal VM faults. Fixed-width conv.i4/conv.u4 narrow to low 32 bits; conv.i8 sign-extends and conv.u8 zero-extends I32 to I64. Unchecked integer unary negation is supported: neg preserves I32/I64 width and signed minima remain unchanged; C# uint unary minus widens to I64 first. This does not use the division/remainder overflow-fault policy. Unchecked conv.i1/conv.i2 keep low 8/16 bits and sign-extend to I32; conv.u1/conv.u2 zero-extend to I32. Char return/store slots support exact UTF-16 units, including surrogates. Byte/SByte/Int16/UInt16 storage signatures/fields/locals and checked/native/floating conversions remain unsupported. Int32/UInt32/Int64/UInt64 bitwise &, |, ^ and ~ are supported with exact width-preserving bits. Fixed-width <<, >> and >>> are supported with I32 counts: raw shl/shr/shr.un mask counts to low 5 bits for I32 values and low 6 for I64, including negative/high counts; signed >> preserves sign and >>> shifts in zero. Dense integer/Int32-enum switch dispatch is supported with I32 indices and at most 4096 jump-table entries; negative/out-of-range indices fall through. String-switch lowering and general pattern matching are not verified. Native/checked operations remain unsupported. Arrays, generics, exceptions, floating point, native integers, .NET host BCL, native I/O and C# async/await are unsupported. Source/compiler/host paths are not accepted. Do not redefine live types: compile callers using referenceTypes. Examine errors and use a new turn to correct code. Tool outcomes and request identities are durable; no hidden retries or remote inference permissions are granted. Maximum 32 assistant turns, bounded transcript/tool output. Return finish when the goal is satisfied. A later user message may provide a follow-up goal in the same universe; retain the prior history and existing objects/code, and use new turns rather than replaying earlier mutations."#;

#[derive(Clone, Debug, Default)]
pub struct AgentHost {
    pub universe: PathBuf,
    pub compiler_ledger: PathBuf,
    pub dotnet: PathBuf,
    pub compiler_driver: PathBuf,
    /// Transient host authority, never restored from guest state or installed code.
    pub allow_web_publication: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct AgentLimits {
    pub instructions: u64,
    pub provider_operations: u64,
    pub tool_actions: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentStop {
    Finished(Json),
    InstructionBudget,
    ProviderBudget,
    ToolBudget,
    TurnLimit,
    TranscriptLimit,
    OutcomeUnknown(String),
}
#[derive(Clone, Debug)]
pub struct AgentReport {
    pub stop: AgentStop,
    pub instructions: u64,
    pub provider_operations: u64,
    pub tool_actions: u32,
}
impl AgentReport {
    pub fn summary(&self) -> Json {
        let (stop, result, effect) = match &self.stop {
            AgentStop::Finished(result) => ("finished", Some(result.clone()), None),
            AgentStop::InstructionBudget => ("instruction_budget", None, None),
            AgentStop::ProviderBudget => ("provider_budget", None, None),
            AgentStop::ToolBudget => ("tool_budget", None, None),
            AgentStop::TurnLimit => ("turn_limit", None, None),
            AgentStop::TranscriptLimit => ("transcript_limit", None, None),
            AgentStop::OutcomeUnknown(effect) => ("outcome_unknown", None, Some(effect)),
        };
        json!({"format":1,"stop":stop,"result":result,"uncertainEffect":effect,"instructions":self.instructions,"providerOperations":self.provider_operations,"toolActions":self.tool_actions})
    }
}
/// A bounded projection of a saved managed turn, never a host action queue.
/// IDs may be replayed on resume; callers can deduplicate presentation by ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentEvent {
    pub id: String,
    pub session_id: String,
    pub turn: u32,
    pub checkpoint: u64,
    pub kind: AgentEventKind,
    pub action: &'static str,
    pub ok: Option<bool>,
    pub summary: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentEventKind {
    ActionSaved,
    ResultSaved,
}
impl AgentEvent {
    pub fn json(&self) -> Json {
        json!({"format":1,"id":self.id,"sessionId":self.session_id,"turn":self.turn,
            "checkpoint":self.checkpoint,"kind":match self.kind {
                AgentEventKind::ActionSaved=>"action_saved", AgentEventKind::ResultSaved=>"result_saved"},
            "action":self.action,"ok":self.ok,"summary":self.summary})
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentStage {
    ToolStarted(u32),
    ToolCompleted(u32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Search {
        query: String,
        #[serde(default = "default_limit")]
        limit: usize,
    },
    ReadCode {
        method: String,
        #[serde(rename = "sourceOnly", default, skip_serializing_if = "is_false")]
        source_only: bool,
    },
    Describe {
        #[serde(rename = "type")]
        type_name: String,
    },
    InspectObject {
        id: String,
        #[serde(rename = "maxFields", default = "default_inspection_fields")]
        max_fields: usize,
        #[serde(rename = "stringUnits", default = "default_string_units")]
        string_units: usize,
    },
    InspectStatic {
        field: String,
        #[serde(rename = "stringUnits", default = "default_string_units")]
        string_units: usize,
    },
    Compile {
        assembly: String,
        source: String,
        #[serde(rename = "referenceTypes", default)]
        reference_types: Vec<String>,
    },
    Install {
        #[serde(rename = "compilationTurn")]
        compilation_turn: u32,
    },
    Invoke {
        method: String,
        arguments: Vec<Argument>,
    },
    WebStatus {},
    PreviewWeb {
        method: String,
        target: String,
        #[serde(default)]
        body: String,
        // Host-only frozen state, rejected in actual model action text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        publication: Option<crate::web::PublicationRecord>,
    },
    PublishWeb {
        method: String,
        #[serde(
            rename = "codeVersion",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        code_version: Option<String>,
    },
    Finish {
        message: String,
    },
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn default_limit() -> usize {
    20
}
fn default_inspection_fields() -> usize {
    16
}
fn default_string_units() -> usize {
    64
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Argument {
    I32 { value: i32 },
    I64 { value: String },
    Object { id: String },
    Static { field: String },
    Null,
}

struct SessionView {
    id: String,
    count: u32,
    phase: i32,
    current: Option<ObjectId>,
    input: Option<String>,
    action: Option<Vec<u16>>,
    history_units: usize,
    result: Option<String>,
    input_support: bool,
    followups: Vec<InputView>,
}
struct InputView {
    id: String,
    content: String,
    after_turn: u32,
    message: ObjectId,
}
fn reference(value: Value) -> Result<ObjectId> {
    if let Value::Reference(id) = value {
        Ok(id)
    } else {
        Err(Error("agent state is missing an object reference".into()))
    }
}
fn owner(program: &Program, name: &str) -> Result<TypeId> {
    program.find_type(name)
}
fn field(vm: &Vm<'_>, id: ObjectId, ty: &str, name: &str) -> Result<Value> {
    optional_field(vm, id, ty, name)?
        .ok_or_else(|| Error(format!("agent field {ty}::{name} is missing")))
}
fn optional_field(vm: &Vm<'_>, id: ObjectId, ty: &str, name: &str) -> Result<Option<Value>> {
    let owner = owner(vm.program, ty)?;
    let object = vm.heap().object(id)?;
    require(
        object.type_id() == owner,
        "agent object has the wrong schema",
    )?;
    let Some(definition) = vm
        .program
        .fields()
        .find(|f| f.owner == owner && f.name == name && !f.is_static)
    else {
        return Ok(None);
    };
    object
        .fields()
        .get(&definition.id)
        .copied()
        .map(Some)
        .ok_or_else(|| Error("agent field has no storage".into()))
}
fn validate_input_id(id: &str) -> Result<()> {
    require(
        !id.is_empty()
            && id.len() <= 48
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "input ID must contain 1..48 ASCII letters, digits, dots, underscores, or hyphens",
    )
}
fn input_request_id(session: &str, id: &str) -> String {
    format!("agent/{session}/input/{id}")
}
fn followup_inputs(vm: &Vm<'_>, saved: ObjectId, count: u32) -> Result<(bool, Vec<InputView>)> {
    let first = optional_field(vm, saved, SESSION, "FirstInput")?;
    let last = optional_field(vm, saved, SESSION, "LastInput")?;
    if first.is_none() && last.is_none() {
        return Ok((false, vec![]));
    }
    require(
        first.is_some() && last.is_some(),
        "incomplete follow-up input schema",
    )?;
    let entry = vm.program.entry(&format!("{HARNESS}::AcceptInput"))?;
    let binding = vm.program.method(entry)?.binding;
    let mut cursor = first.unwrap();
    let mut tail = Value::Null;
    let mut seen = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut inputs = Vec::new();
    let mut previous_turn = 0;
    while cursor != Value::Null {
        let node = reference(cursor)?;
        require(
            seen.insert(node) && seen.len() <= MAX_TURNS as usize,
            "cyclic/oversized follow-up input list",
        )?;
        let id_ref = field(vm, node, INPUT, "Id")?;
        let content_ref = field(vm, node, INPUT, "Content")?;
        let id = text(vm, id_ref)?;
        validate_input_id(&id)?;
        let content = text(vm, content_ref)?;
        let after_turn = integer(field(vm, node, INPUT, "AfterTurn")?)?;
        require(
            ids.insert(id.clone())
                && !content.trim().is_empty()
                && content.len() <= 4096
                && after_turn > previous_turn
                && after_turn <= count as i32,
            "invalid follow-up input identity/content/turn",
        )?;
        let message = reference(field(vm, node, INPUT, "Message")?)?;
        require(
            field(vm, message, "Renraku.ChatMessage", "Content")? == content_ref,
            "follow-up input/message content mismatch",
        )?;
        let receipt = vm
            .invocation(&input_request_id(&vm.universe_id().to_string(), &id))
            .ok_or_else(|| Error("follow-up input has no invocation receipt".into()))?;
        require(
            receipt.method == binding
                && receipt.arguments == vec![id_ref, content_ref, Value::I32(after_turn)]
                && receipt.state == (InvocationState::Completed { result: None }),
            "follow-up input/receipt mismatch",
        )?;
        inputs.push(InputView {
            id,
            content,
            after_turn: after_turn as u32,
            message,
        });
        previous_turn = after_turn;
        tail = cursor;
        cursor = field(vm, node, INPUT, "Next")?;
    }
    require(last == Some(tail), "follow-up input tail mismatch")?;
    Ok((true, inputs))
}
fn units(vm: &Vm<'_>, value: Value) -> Result<Vec<u16>> {
    Ok(vm
        .heap()
        .object(reference(value)?)?
        .string_units()
        .ok_or_else(|| Error("agent field is not a string".into()))?
        .to_vec())
}
fn text(vm: &Vm<'_>, value: Value) -> Result<String> {
    String::from_utf16(&units(vm, value)?)
        .map_err(|_| Error("agent metadata is not well-formed Unicode".into()))
}
fn optional_text(vm: &Vm<'_>, value: Value) -> Result<Option<String>> {
    if value == Value::Null {
        Ok(None)
    } else {
        Ok(Some(text(vm, value)?))
    }
}
fn integer(value: Value) -> Result<i32> {
    if let Value::I32(n) = value {
        Ok(n)
    } else {
        Err(Error("agent field is not Int32".into()))
    }
}
fn saved_session(vm: &Vm<'_>) -> Result<Value> {
    let owner = owner(vm.program, HARNESS)?;
    let field = vm
        .program
        .fields()
        .find(|f| f.owner == owner && f.name == "Saved" && f.is_static)
        .ok_or_else(|| Error("agent harness has no Saved field".into()))?;
    vm.static_fields()
        .get(&field.id)
        .copied()
        .ok_or_else(|| Error("agent session has no storage".into()))
}
fn session(vm: &Vm<'_>) -> Result<SessionView> {
    let saved = reference(saved_session(vm)?)?;
    let id = text(vm, field(vm, saved, SESSION, "Id")?)?;
    require(
        id == vm.universe_id().to_string(),
        "agent session identity does not match its universe",
    )?;
    let count = integer(field(vm, saved, SESSION, "Count")?)?;
    let phase = integer(field(vm, saved, SESSION, "Phase")?)?;
    require(
        (0..=MAX_TURNS as i32).contains(&count) && (0..=3).contains(&phase),
        "invalid agent turn/phase",
    )?;
    let mut cursor = field(vm, saved, SESSION, "FirstTurn")?;
    let mut seen = BTreeSet::new();
    let mut last = None;
    let mut turns = Vec::new();
    while cursor != Value::Null {
        let turn = reference(cursor)?;
        require(
            seen.insert(turn) && seen.len() <= MAX_TURNS as usize,
            "cyclic/oversized agent turn list",
        )?;
        require(
            integer(field(vm, turn, TURN, "Number")?)? == seen.len() as i32,
            "agent turns are out of order",
        )?;
        last = Some(turn);
        turns.push(turn);
        cursor = field(vm, turn, TURN, "Next")?;
    }
    let current = field(vm, saved, SESSION, "Current")?;
    require(
        last.map(Value::Reference).unwrap_or(Value::Null) == current
            && seen.len() == count as usize,
        "agent turn list/current mismatch",
    )?;
    let (input_support, followups) = followup_inputs(vm, saved, count as u32)?;
    let mut message = field(vm, saved, SESSION, "First")?;
    let mut messages = BTreeSet::new();
    let mut history_units = 0;
    let mut chat = Vec::new();
    while message != Value::Null {
        let id = reference(message)?;
        require(
            messages.insert(id) && messages.len() <= 128,
            "cyclic/oversized agent transcript",
        )?;
        history_units += units(vm, field(vm, id, "Renraku.ChatMessage", "Content")?)?.len();
        chat.push(id);
        message = field(vm, id, "Renraku.ChatMessage", "Next")?;
    }
    require(
        history_units <= 1_000_000,
        "agent transcript exceeds model limits",
    )?;
    require(
        chat.len() >= 2
            && field(vm, saved, SESSION, "Last")? == Value::Reference(*chat.last().unwrap()),
        "agent transcript tail mismatch",
    )?;
    require(
        integer(field(vm, chat[0], "Renraku.ChatMessage", "Role")?)? == 0
            && integer(field(vm, chat[1], "Renraku.ChatMessage", "Role")?)? == 1,
        "agent transcript has no system/goal prefix",
    )?;
    let mut expected = 2;
    let mut input_index = 0;
    for (index, turn) in turns.into_iter().enumerate() {
        require(
            expected < chat.len(),
            "agent transcript has no matching assistant action",
        )?;
        require(
            integer(field(vm, chat[expected], "Renraku.ChatMessage", "Role")?)? == 2,
            "agent transcript has no matching assistant action",
        )?;
        require(
            field(vm, chat[expected], "Renraku.ChatMessage", "Content")?
                == field(vm, turn, TURN, "Action")?,
            "agent transcript/action mismatch",
        )?;
        expected += 1;
        let result = field(vm, turn, TURN, "Result")?;
        if result != Value::Null {
            require(
                expected < chat.len()
                    && integer(field(vm, chat[expected], "Renraku.ChatMessage", "Role")?)? == 1
                    && field(vm, chat[expected], "Renraku.ChatMessage", "Content")? == result,
                "agent transcript/tool result mismatch",
            )?;
            expected += 1;
        }
        if let Some(input) = followups.get(input_index)
            && input.after_turn == index as u32 + 1
        {
            require(
                result != Value::Null
                    && matches!(
                        parse_action(&units(vm, field(vm, turn, TURN, "Action")?)?),
                        Ok(Action::Finish { .. })
                    ),
                "follow-up input does not follow a finished goal",
            )?;
            require(
                expected < chat.len()
                    && chat[expected] == input.message
                    && integer(field(vm, input.message, "Renraku.ChatMessage", "Role")?)? == 1,
                "agent transcript/follow-up input mismatch",
            )?;
            expected += 1;
            input_index += 1;
        }
    }
    require(
        expected == chat.len() && input_index == followups.len(),
        "agent transcript contains unowned messages",
    )?;
    let (input, action, result) = if let Some(turn) = last {
        (
            optional_text(vm, field(vm, turn, TURN, "Input")?)?,
            Some(units(vm, field(vm, turn, TURN, "Action")?)?),
            optional_text(vm, field(vm, turn, TURN, "Result")?)?,
        )
    } else {
        (None, None, None)
    };
    Ok(SessionView {
        id,
        count: count as u32,
        phase,
        current: last,
        input,
        action,
        history_units,
        result,
        input_support,
        followups,
    })
}
fn active_owned(vm: &Vm<'_>) -> Result<Option<bool>> {
    if matches!(
        vm.context().status,
        ExecutionStatus::Completed { .. } | ExecutionStatus::Faulted { .. }
    ) {
        return Ok(None);
    }
    let Some(entry) = vm.entry else {
        return Ok(None);
    };
    let method = vm.program.method(entry.method)?;
    if method.owner != owner(vm.program, HARNESS)? {
        return Ok(None);
    }
    match method.name.as_str() {
        "Initialize" | "Ask" => Ok(Some(true)),
        "Freeze" | "Apply" | "AcceptInput" => Ok(Some(false)),
        _ => Ok(None),
    }
}
fn idle(vm: &Vm<'_>) -> bool {
    matches!(
        vm.context().status,
        ExecutionStatus::Completed { .. } | ExecutionStatus::Faulted { .. }
    )
}
fn action_id(session: &str, turn: u32, operation: &str) -> String {
    format!("agent/{session}/{turn}/{operation}")
}
fn parse_action(units: &[u16]) -> Result<Action> {
    let text = String::from_utf16(units)
        .map_err(|_| Error("action must be well-formed Unicode JSON".into()))?;
    require(text.len() <= MAX_ACTION, "action exceeds 64 KiB")?;
    let value: Json =
        serde_json::from_str(&text).map_err(|_| Error("invalid action JSON".into()))?;
    require(
        !(value["op"] == "preview_web" && value.get("publication").is_some()),
        "preview publication is host-owned frozen state; omit it from the action",
    )?;
    serde_json::from_value(value).map_err(|_| {
        Error("invalid action schema; return one supported JSON object with no extra fields".into())
    })
}
fn event_line(text: &str) -> String {
    let mut line = String::new();
    for c in text.chars() {
        let escaped = if c.is_control() {
            c.escape_default().to_string()
        } else {
            c.to_string()
        };
        if line.len() + escaped.len() > 509 {
            line.push('…');
            break;
        }
        line.push_str(&escaped);
    }
    line
}
fn action_summary(action: Option<&Action>) -> (&'static str, String) {
    match action {
        Some(Action::Search { query, .. }) => ("search", format!("search {query}")),
        Some(Action::ReadCode { method, .. }) => ("read_code", format!("read code {method}")),
        Some(Action::Describe { type_name }) => ("describe", format!("describe {type_name}")),
        Some(Action::InspectObject { id, .. }) => {
            ("inspect_object", format!("inspect object {id}"))
        }
        Some(Action::InspectStatic { field, .. }) => {
            ("inspect_static", format!("inspect static field {field}"))
        }
        Some(Action::Compile {
            assembly, source, ..
        }) => (
            "compile",
            format!("compile {assembly} ({} source bytes)", source.len()),
        ),
        Some(Action::Install { compilation_turn }) => (
            "install",
            format!("install compilation from turn {compilation_turn}"),
        ),
        Some(Action::Invoke { method, arguments }) => (
            "invoke",
            format!("invoke {method} ({} arguments)", arguments.len()),
        ),
        Some(Action::WebStatus {}) => (
            "web_status",
            "inspect website publication and authority".into(),
        ),
        Some(Action::PreviewWeb { method, target, .. }) => {
            ("preview_web", format!("preview {method} {target}"))
        }
        Some(Action::PublishWeb { method, .. }) => {
            ("publish_web", format!("publish website handler {method}"))
        }
        Some(Action::Finish { message }) => ("finish", message.clone()),
        None => ("invalid", "invalid or unsupported action".into()),
    }
}
fn result_summary(result: &Json) -> String {
    if let Some(error) = result["error"].as_str() {
        return error.into();
    }
    if result["output"]["kind"] == "object" {
        return format!(
            "{} fields observed at checkpoint {}{}",
            result["output"]["fieldsReturned"],
            result["output"]["checkpoint"],
            if result["output"]["fieldsTruncated"] == true {
                " (field view truncated)"
            } else {
                ""
            }
        );
    }
    if result["output"]["kind"] == "static" {
        return format!(
            "static storage observed at checkpoint {}",
            result["output"]["checkpoint"]
        );
    }
    if let Some(status) = result["preview"]["status"].as_u64() {
        return format!(
            "managed preview {status} {}; {} body bytes{}",
            result["preview"]["state"].as_str().unwrap_or("unknown"),
            result["preview"]["bodyBytes"],
            if result["preview"]["truncated"] == true {
                " (view truncated)"
            } else {
                ""
            }
        );
    }
    if result["output"]["state"] == "rejected" {
        if let Some(diagnostic) = result["output"]["report"]["diagnostics"]
            .as_array()
            .and_then(|ds| {
                ds.iter()
                    .find(|d| d["severity"] == "error")
                    .or_else(|| ds.first())
            })
        {
            return format!(
                "{} {}:{}: {}",
                diagnostic["code"].as_str().unwrap_or("compiler error"),
                diagnostic["file"].as_str().unwrap_or("source"),
                diagnostic["line"],
                diagnostic["message"]
                    .as_str()
                    .unwrap_or("compilation rejected")
            );
        }
        return "compilation rejected; inspect the saved diagnostics".into();
    }
    if result["output"]["state"] == "compiled" {
        return "compiled artifact saved".into();
    }
    if let Some(assembly) = result["assemblyId"].as_str() {
        return format!("installed assembly {assembly}");
    }
    if let Some(method) = result["publication"]["method"].as_str() {
        return format!(
            "website publication {} saved for {method}",
            result["publication"]["sequence"]
        );
    }
    if let Some(value) = result.get("result") {
        return format!("returned {value}");
    }
    if let Some(message) = result["message"].as_str() {
        return message.into();
    }
    if let Some(symbols) = result["output"]["symbols"].as_array() {
        return format!("{} symbols returned", symbols.len());
    }
    if let Some(instructions) = result["output"]["instructions"].as_array() {
        return format!(
            "{} CIL instructions; retained source {}",
            instructions.len(),
            if result["output"]["sourceAvailable"] == true {
                "available"
            } else {
                "unavailable"
            }
        );
    }
    if result.get("schemaHash").is_some() {
        return "compiler contract returned".into();
    }
    "tool result saved".into()
}
fn publish_progress(
    state: &SessionView,
    checkpoint: u64,
    delivered: &mut BTreeSet<String>,
    progress: &mut impl FnMut(AgentEvent) -> Result<()>,
) -> Result<()> {
    let Some(raw) = state.action.as_deref() else {
        return Ok(());
    };
    let action = parse_action(raw).ok();
    let (op, summary) = action_summary(action.as_ref());
    let mut publish = |kind, ok, summary: String| -> Result<()> {
        let suffix = if kind == AgentEventKind::ActionSaved {
            "action"
        } else {
            "result"
        };
        let id = format!("agent/{}/{}/{suffix}", state.id, state.count);
        if delivered.insert(id.clone()) {
            progress(AgentEvent {
                id,
                session_id: state.id.clone(),
                turn: state.count,
                checkpoint,
                kind,
                action: op,
                ok,
                summary: event_line(&summary),
            })?;
        }
        Ok(())
    };
    publish(AgentEventKind::ActionSaved, None, summary)?;
    // Apply can checkpoint halfway through updating the linked transcript.
    // The engine calls this only after owned helper frames have completed.
    if let Some(result) = state.result.as_deref() {
        let result: Json =
            serde_json::from_str(result).map_err(|_| Error("invalid saved agent result".into()))?;
        publish(
            AgentEventKind::ResultSaved,
            result["ok"].as_bool(),
            result_summary(&result),
        )?;
    }
    Ok(())
}
fn argument(vm: &Vm<'_>, arg: &Argument) -> Result<Value> {
    match arg {
        Argument::Null => Ok(Value::Null),
        Argument::I32 { value } => Ok(Value::I32(*value)),
        Argument::I64 { value } => Ok(Value::I64(
            value
                .parse()
                .map_err(|_| Error("invalid Int64 argument".into()))?,
        )),
        Argument::Object { id } => {
            let uuid = uuid::Uuid::parse_str(id).map_err(|_| Error("invalid object ID".into()))?;
            let id = ObjectId::from_bytes(*uuid.as_bytes());
            vm.heap().object(id)?;
            Ok(Value::Reference(id))
        }
        Argument::Static { field: name } => {
            let matches: Vec<_> = vm
                .program
                .fields()
                .filter(|f| {
                    f.is_static
                        && (f.id.to_string() == *name
                            || format!("{}::{}", vm.program.types[&f.owner].name, f.name) == *name)
                })
                .collect();
            require(
                matches.len() == 1,
                "static selector is missing or ambiguous",
            )?;
            vm.static_fields()
                .get(&matches[0].id)
                .copied()
                .ok_or_else(|| Error("static selector has no storage".into()))
        }
    }
}
fn literal(value: Value) -> Argument {
    match value {
        Value::Null => Argument::Null,
        Value::I32(value) => Argument::I32 { value },
        Value::I64(value) => Argument::I64 {
            value: value.to_string(),
        },
        Value::Reference(id) => Argument::Object { id: id.to_string() },
    }
}
fn freeze(vm: &Vm<'_>, action: Action, count: u32) -> Result<Action> {
    Ok(match action {
        Action::Search { query, limit } => {
            require(
                query.len() <= 256 && (1..=50).contains(&limit),
                "invalid search query/limit",
            )?;
            Action::Search { query, limit }
        }
        Action::ReadCode {
            method,
            source_only,
        } => {
            let code = vm.read_code(&method)?;
            Action::ReadCode {
                method: code["codeVersion"].as_str().unwrap().into(),
                source_only,
            }
        }
        Action::Describe { type_name } => Action::Describe {
            type_name: vm.program.find_type(&type_name)?.to_string(),
        },
        Action::InspectObject {
            id,
            max_fields,
            string_units,
        } => {
            crate::inspection::InspectionLimits {
                max_fields,
                string_units,
            }
            .validate()?;
            Action::InspectObject {
                id: vm.resolve_object_id(&id)?.to_string(),
                max_fields,
                string_units,
            }
        }
        Action::InspectStatic {
            field,
            string_units,
        } => {
            crate::inspection::InspectionLimits {
                string_units,
                ..Default::default()
            }
            .validate()?;
            Action::InspectStatic {
                field: vm.program.resolve_static_field(&field)?.to_string(),
                string_units,
            }
        }
        Action::Compile {
            assembly,
            source,
            reference_types,
        } => {
            require(
                source.len() <= 32768 && reference_types.len() <= 8,
                "compile action exceeds source/reference limits",
            )?;
            let roots = reference_types
                .iter()
                .map(|name| vm.program.find_type(name))
                .collect::<Result<Vec<_>>>()?;
            if !roots.is_empty() {
                vm.program.reference_set(&roots)?;
            }
            Action::Compile {
                assembly,
                source,
                reference_types: roots.iter().map(ToString::to_string).collect(),
            }
        }
        Action::Install { compilation_turn } => {
            require(
                compilation_turn > 0 && compilation_turn < count,
                "installation must refer to an earlier compilation turn",
            )?;
            Action::Install { compilation_turn }
        }
        Action::Invoke { method, arguments } => {
            require(arguments.len() <= 8, "invoke action has too many arguments")?;
            let id = vm.program.entry(&method)?;
            let definition = vm.program.method(id)?;
            let values = arguments
                .iter()
                .map(|a| argument(vm, a))
                .collect::<Result<Vec<_>>>()?;
            require(
                !definition.signature.instance
                    && definition.intrinsic.is_none()
                    && definition.owner != owner(vm.program, HARNESS)?
                    && values.len() == definition.signature.parameters.len(),
                "invoke entry/signature is unsupported",
            )?;
            for (value, slot) in values.iter().zip(&definition.signature.parameters) {
                vm.check_value(*value, *slot)?;
            }
            Action::Invoke {
                method: id.to_string(),
                arguments: values.into_iter().map(literal).collect(),
            }
        }
        Action::WebStatus {} => Action::WebStatus {},
        Action::PreviewWeb {
            method,
            target,
            body,
            publication,
        } => {
            crate::web::validate_request(
                &crate::web::WebRequest {
                    id: "preview".into(),
                    method: method.clone(),
                    target: target.clone(),
                    body: body.clone(),
                },
                0,
            )
            .map_err(|e| Error(e.to_string()))?;
            let publication = match publication {
                Some(record) => {
                    crate::web::validate_publication(vm, &record)?;
                    record
                }
                None => crate::web::current_publication(vm)?.ok_or_else(|| {
                    Error("no published website handler; publish one before previewing".into())
                })?,
            };
            Action::PreviewWeb {
                method,
                target,
                body,
                publication: Some(publication),
            }
        }
        Action::PublishWeb {
            method,
            code_version,
        } => {
            let method = crate::web::handler(vm.program, &method)?;
            let binding = vm.program.method(method)?.binding;
            require(
                code_version
                    .as_ref()
                    .is_none_or(|code| *code == binding.code.to_string()),
                "website publication code version differs from the installed binding",
            )?;
            Action::PublishWeb {
                method: method.to_string(),
                code_version: Some(binding.code.to_string()),
            }
        }
        Action::Finish { message } => {
            require(message.len() <= 4096, "finish message exceeds limit")?;
            Action::Finish { message }
        }
    })
}
fn output(value: Json) -> Result<String> {
    let text = serde_json::to_string(&value).map_err(|e| Error(e.to_string()))?;
    if text.len() > MAX_OUTPUT {
        Ok(json!({"ok":false,"truncated":true,"error":"tool output exceeds 16384 bytes; select a narrower query, smaller method, or read_code sourceOnly:true"}).to_string())
    } else {
        Ok(text)
    }
}
fn failed(error: Error) -> Json {
    json!({"ok":false,"error":error.0})
}
fn start_apply(
    vm: &mut Vm<'_>,
    store: &mut CheckpointStore,
    result: String,
    finish: bool,
) -> Result<()> {
    let text = vm.new_string(result.encode_utf16().collect())?;
    vm.start(
        vm.program.entry(&format!("{HARNESS}::Apply"))?,
        vec![Value::Reference(text), Value::I32(i32::from(finish))],
    )?;
    store.save(vm)?;
    Ok(())
}

/// Queue one follow-up without inference. Existing IDs bind the original string
/// references and pinned helper, so retries resume or return their receipt.
pub fn submit_input(
    store: &mut CheckpointStore,
    id: &str,
    goal: &str,
    budget: u64,
) -> Result<Json> {
    validate_input_id(id)?;
    require(
        !goal.trim().is_empty() && goal.len() <= 4096,
        "follow-up goal must contain 1..4096 UTF-8 bytes",
    )?;
    require(
        budget <= 1_000_000,
        "follow-up instruction budget exceeds policy",
    )?;
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    let method = vm.program.entry(&format!("{HARNESS}::AcceptInput"))
        .map_err(|_| Error("saved agent schema does not support follow-up inputs; its original session remains readable/resumable".into()))?;
    let request_id = input_request_id(&vm.universe_id().to_string(), id);
    let (arguments, after_turn) = if let Some(record) = vm.invocation(&request_id) {
        require(
            record.method == vm.program.method(method)?.binding && record.arguments.len() == 3,
            "input ID is bound to a different call",
        )?;
        require(
            text(&vm, record.arguments[0])? == id && text(&vm, record.arguments[1])? == goal,
            "input ID is already bound to different content",
        )?;
        (record.arguments.clone(), integer(record.arguments[2])?)
    } else {
        require(
            idle(&vm),
            "a new follow-up requires a finished goal and idle execution",
        )?;
        let state = session(&vm)?;
        require(
            state.input_support,
            "saved agent schema does not support follow-up inputs",
        )?;
        require(
            idle(&vm) && state.phase == 3 && state.count > 0,
            "a new follow-up requires a finished goal and idle execution",
        )?;
        require(
            state.count < MAX_TURNS,
            "session turn limit reached; input was not accepted",
        )?;
        require(
            state.history_units + goal.encode_utf16().count() < 500_000,
            "session transcript limit reached; input was not accepted",
        )?;
        require(
            matches!(
                parse_action(state.action.as_deref().unwrap())?,
                Action::Finish { .. }
            ),
            "saved goal is not finished",
        )?;
        let id = vm.new_string(id.encode_utf16().collect())?;
        let goal = vm.new_string(goal.encode_utf16().collect())?;
        (
            vec![
                Value::Reference(id),
                Value::Reference(goal),
                Value::I32(state.count as i32),
            ],
            state.count as i32,
        )
    };
    let before = vm.instructions_executed();
    let receipt = vm.invoke_once(store, &request_id, method, arguments, budget, 100)?;
    let (state, diagnostic) = match &receipt.state {
        InvocationState::Running => ("pending", None),
        InvocationState::Completed { result: None } => ("accepted", None),
        InvocationState::Completed { .. } => {
            return Err(Error("invalid follow-up helper result".into()));
        }
        InvocationState::Faulted { message } => ("faulted", Some(message)),
    };
    Ok(
        json!({"format":1,"inputId":id,"requestId":request_id,"afterTurn":after_turn,
        "execution":receipt.execution.to_string(),"state":state,"diagnostic":diagnostic,
        "instructions":vm.instructions_executed()-before}),
    )
}

impl AgentHost {
    pub fn initialize(
        vm: &mut Vm<'_>,
        store: &mut CheckpointStore,
        capability: &str,
        model: &str,
        goal: &str,
    ) -> Result<()> {
        require(
            saved_session(vm)? == Value::Null,
            "agent session already exists",
        )?;
        require(goal.len() <= 4096, "agent goal exceeds 4096 bytes")?;
        crate::model::validate_identifier(capability)?;
        crate::model::validate_identifier(model)?;
        let args = [
            vm.universe_id().to_string(),
            capability.into(),
            model.into(),
            SYSTEM_PROMPT.into(),
            goal.into(),
        ]
        .into_iter()
        .map(|text| {
            vm.new_string(text.encode_utf16().collect())
                .map(Value::Reference)
        })
        .collect::<Result<Vec<_>>>()?;
        vm.start(vm.program.entry(&format!("{HARNESS}::Initialize"))?, args)?;
        store.save(vm)?;
        Ok(())
    }
    pub async fn drive<P: AsyncModelProvider>(
        &self,
        provider: &mut P,
        limits: AgentLimits,
    ) -> Result<AgentReport> {
        self.drive_with_observer(provider, limits, |_| Ok(())).await
    }
    pub async fn drive_with_observer<P: AsyncModelProvider>(
        &self,
        provider: &mut P,
        limits: AgentLimits,
        observer: impl FnMut(AgentStage) -> Result<()>,
    ) -> Result<AgentReport> {
        self.drive_observed(provider, limits, observer, |_| Ok(()))
            .await
    }
    /// Progress is disposable presentation of the latest saved turn. It can
    /// repeat IDs after restart and is never emitted from a partial CIL helper.
    pub async fn drive_with_progress<P: AsyncModelProvider>(
        &self,
        provider: &mut P,
        limits: AgentLimits,
        progress: impl FnMut(AgentEvent) -> Result<()>,
    ) -> Result<AgentReport> {
        self.drive_observed(provider, limits, |_| Ok(()), progress)
            .await
    }
    async fn drive_observed<P: AsyncModelProvider>(
        &self,
        provider: &mut P,
        limits: AgentLimits,
        mut observer: impl FnMut(AgentStage) -> Result<()>,
        mut progress: impl FnMut(AgentEvent) -> Result<()>,
    ) -> Result<AgentReport> {
        require(
            limits.tool_actions <= 32 && limits.instructions <= 1_000_000,
            "agent invocation limits exceed policy",
        )?;
        let mut instructions = 0;
        let mut operations = 0;
        let mut actions = 0;
        let mut delivered = BTreeSet::new();
        macro_rules! stop {
            ($stop:expr) => {
                return Ok(AgentReport {
                    stop: $stop,
                    instructions,
                    provider_operations: operations,
                    tool_actions: actions,
                })
            };
        }
        loop {
            let mut store = CheckpointStore::open(&self.universe)?;
            let saved = store.load()?;
            let mut vm = saved.snapshot.restore()?;
            if let Some(model) = active_owned(&vm)? {
                if model {
                    // A waiting Chat call has not begun appending its answer.
                    // Its previous turn is stable even while the native HTTP
                    // future/receipt lookup is pending on this resumed drive.
                    if matches!(vm.context().status, ExecutionStatus::Waiting { .. }) {
                        publish_progress(
                            &session(&vm)?,
                            store.generation(),
                            &mut delivered,
                            &mut progress,
                        )?;
                    }
                    vm.set_model_calls_enabled(true);
                    let report = vm
                        .drive_model_async(
                            &mut store,
                            provider,
                            DriverLimits {
                                instructions: limits.instructions - instructions,
                                provider_operations: limits.provider_operations - operations,
                                checkpoint_interval: 100,
                            },
                        )
                        .await?;
                    instructions += report.instructions;
                    operations += report.provider_operations;
                    match report.stop {
                        DriverStop::Completed(_) => {}
                        DriverStop::InstructionBudget => stop!(AgentStop::InstructionBudget),
                        DriverStop::ProviderBudget { .. } => stop!(AgentStop::ProviderBudget),
                        DriverStop::OutcomeUnknown { effect } => {
                            stop!(AgentStop::OutcomeUnknown(effect.to_string()))
                        }
                    }
                } else {
                    if instructions == limits.instructions {
                        stop!(AgentStop::InstructionBudget);
                    }
                    let before = vm.instructions_executed();
                    vm.run_checkpointed(
                        &mut store,
                        (limits.instructions - instructions).min(100),
                        100,
                    )?;
                    instructions += vm.instructions_executed() - before;
                }
                continue;
            }
            let state = session(&vm)?;
            publish_progress(&state, store.generation(), &mut delivered, &mut progress)?;
            if state.phase == 3 {
                let result: Json = serde_json::from_str(
                    state
                        .result
                        .as_deref()
                        .ok_or_else(|| Error("finished agent has no result".into()))?,
                )
                .map_err(|_| Error("invalid saved agent result".into()))?;
                stop!(AgentStop::Finished(result));
            }
            if state.phase == 2 {
                if state.count >= MAX_TURNS {
                    stop!(AgentStop::TurnLimit);
                }
                if state.history_units >= 500_000 {
                    stop!(AgentStop::TranscriptLimit);
                }
                require(
                    idle(&vm),
                    "unrelated execution is active during agent continuation",
                )?;
                vm.start(vm.program.entry(&format!("{HARNESS}::Ask"))?, vec![])?;
                store.save(&vm)?;
                continue;
            }
            require(
                state.phase == 1 && state.current.is_some(),
                "agent has no owned continuation or pending action",
            )?;
            if actions == limits.tool_actions {
                stop!(AgentStop::ToolBudget);
            }
            if state.input.is_none() {
                require(
                    idle(&vm),
                    "unrelated execution is active during action preparation",
                )?;
                let prepared = parse_action(state.action.as_deref().unwrap())
                    .and_then(|action| freeze(&vm, action, state.count));
                match prepared {
                    Ok(action) => {
                        let mut retained = if let Action::InspectObject { id, .. } = &action {
                            vec![Value::Reference(vm.resolve_object_id(id)?)]
                        } else if let Action::Invoke { arguments, .. } = &action {
                            arguments
                                .iter()
                                .filter(|arg| matches!(arg, Argument::Object { .. }))
                                .map(|arg| argument(&vm, arg))
                                .collect::<Result<Vec<_>>>()?
                        } else {
                            vec![]
                        };
                        let input =
                            serde_json::to_string(&action).map_err(|e| Error(e.to_string()))?;
                        let input = vm.new_string(input.encode_utf16().collect())?;
                        let mut arguments = vec![Value::Reference(input)];
                        arguments.append(&mut retained);
                        arguments.resize(9, Value::Null);
                        vm.start(vm.program.entry(&format!("{HARNESS}::Freeze"))?, arguments)?;
                        store.save(&vm)?;
                    }
                    Err(error) => {
                        actions += 1;
                        start_apply(&mut vm, &mut store, output(failed(error))?, false)?;
                    }
                }
                continue;
            }
            let input = state.input.unwrap();
            let action: Action = serde_json::from_str(&input)
                .map_err(|_| Error("saved frozen agent input is invalid".into()))?;
            let finish = matches!(action, Action::Finish { .. });
            let expected_session = state.id;
            let expected_turn = state.count;
            drop(vm);
            drop(saved);
            drop(store);
            actions += 1;
            let host = self.clone();
            let session_id = expected_session.clone();
            let remaining = (limits.instructions - instructions).min(1024);
            observer(AgentStage::ToolStarted(expected_turn))?;
            let result =
                on_worker(move || execute(&host, &session_id, expected_turn, action, remaining))
                    .await?;
            instructions += result.instructions;
            if result.pending {
                if instructions == limits.instructions {
                    stop!(AgentStop::InstructionBudget);
                }
                continue;
            }
            observer(AgentStage::ToolCompleted(expected_turn))?;
            let mut store = CheckpointStore::open(&self.universe)?;
            let saved = store.load()?;
            let mut vm = saved.snapshot.restore()?;
            if active_owned(&vm)?.is_some() {
                continue;
            }
            let current = session(&vm)?;
            if current.id != expected_session
                || current.count != expected_turn
                || current.phase != 1
                || current.input.as_deref() != Some(input.as_str())
            {
                continue;
            }
            require(
                idle(&vm),
                "unrelated execution is active before applying a tool result",
            )?;
            start_apply(&mut vm, &mut store, output(result.value)?, finish)?;
        }
    }
}

struct ToolResult {
    value: Json,
    instructions: u64,
    pending: bool,
}
fn preview_body(text: &str) -> (String, bool) {
    // Bound the JSON representation as well as UTF-8. Control characters can
    // expand sixfold; cutting encoded JSON itself could split an escape or scalar.
    let mut body = String::new();
    let mut encoded = 0;
    for c in text.chars() {
        let bytes = match c {
            '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
            '\0'..='\u{1f}' => 6,
            _ => c.len_utf8(),
        };
        if encoded + bytes > 8192 {
            break;
        }
        encoded += bytes;
        body.push(c);
    }
    let truncated = body.len() != text.len();
    (body, truncated)
}
fn execute(
    host: &AgentHost,
    session_id: &str,
    turn: u32,
    action: Action,
    budget: u64,
) -> Result<ToolResult> {
    let mut store = CheckpointStore::open(&host.universe)?;
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    let state = session(&vm)?;
    require(
        state.id == session_id && state.count == turn && state.phase == 1,
        "agent action ownership changed before execution",
    )?;
    require(
        state.input.as_deref()
            == Some(
                serde_json::to_string(&action)
                    .map_err(|e| Error(e.to_string()))?
                    .as_str(),
            ),
        "frozen agent input changed before execution",
    )?;
    let checked = freeze(&vm, action.clone(), turn)?;
    require(
        serde_json::to_string(&checked).map_err(|e| Error(e.to_string()))? == state.input.unwrap(),
        "saved agent input is not a canonical frozen action",
    )?;
    let mut instructions = 0;
    let mut pending = false;
    let value = match action {
        Action::Search { query, limit } => match vm.program.search_symbols(&query, limit) {
            Ok(value) => json!({"ok":true,"output":value}),
            Err(error) => failed(error),
        },
        Action::ReadCode {
            method,
            source_only,
        } => match vm.read_code(&method) {
            Ok(mut value) => {
                if source_only {
                    value.as_object_mut().unwrap().remove("instructions");
                    value.as_object_mut().unwrap().remove("buildManifest");
                    value["representation"] = json!("source_provenance");
                }
                json!({"ok":true,"output":value})
            }
            Err(error) => failed(error),
        },
        Action::InspectObject {
            id,
            max_fields,
            string_units,
        } => match vm.inspect_object(
            &id,
            crate::inspection::InspectionLimits {
                max_fields,
                string_units,
            },
        ) {
            Ok(view) => json!({"ok":true,"output":view}),
            Err(error) => failed(error),
        },
        Action::InspectStatic {
            field,
            string_units,
        } => match vm.inspect_static(&field, string_units) {
            Ok(view) => json!({"ok":true,"output":view}),
            Err(error) => failed(error),
        },
        Action::Describe { type_name } => match vm
            .program
            .find_type(&type_name)
            .and_then(|id| vm.program.reference_set(&[id]).map(|set| (id, set)))
        {
            Ok((id, set)) => {
                let root = set
                    .views
                    .iter()
                    .position(|v| {
                        serde_json::from_str::<Json>(&v.schema_json).is_ok_and(|s| {
                            s["roots"]
                                .as_array()
                                .is_some_and(|roots| roots.contains(&json!(id.to_string())))
                        })
                    })
                    .ok_or_else(|| Error("missing description root".into()))?;
                let view = &set.views[root];
                let mut result = json!({"ok":true,"schemaHash":view.schema_hash,"output":serde_json::from_str::<Json>(&view.schema_json).map_err(|e|Error(e.to_string()))?});
                if set.views.len() > 1 {
                    result["referenceSetHash"] = json!(set.set_hash);
                    result["dependencies"] = json!(
                        set.views
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| *i != root)
                            .map(|(_, v)| serde_json::from_str::<Json>(&v.schema_json))
                            .collect::<std::result::Result<Vec<_>, _>>()
                            .map_err(|e| Error(e.to_string()))?
                    );
                }
                result
            }
            Err(error) => failed(error),
        },
        Action::Compile {
            assembly,
            source,
            reference_types,
        } => {
            let roots = reference_types
                .iter()
                .map(|id| vm.program.find_type(id))
                .collect::<Result<Vec<_>>>()?;
            let views = if roots.is_empty() {
                vec![]
            } else {
                vm.program.reference_set(&roots)?.views
            };
            let sources = vec![("Generated.cs".into(), source.into_bytes())];
            let id = action_id(session_id, turn, "compile");
            let mut compiler = CompilationStore::open(&host.compiler_ledger)?;
            let prior = compiler.find(vm.universe_id(), &id)?;
            let compiled = if let Some(prior) = prior {
                require(
                    prior.request.matches_inputs(&assembly, &sources, &views)?,
                    "saved compiler input differs from agent turn",
                )?;
                if prior.report.is_some() {
                    prior
                } else {
                    let toolchain = CompilerToolchain::open(
                        resolve_dotnet(&host.dotnet)?,
                        &host.compiler_driver,
                    )?;
                    compiler.drive(vm.universe_id(), &id, &toolchain)?
                }
            } else {
                let toolchain =
                    CompilerToolchain::open(resolve_dotnet(&host.dotnet)?, &host.compiler_driver)?;
                let request = match CompilationRequest::capture(
                    vm.program,
                    vm.universe_id(),
                    &toolchain,
                    assembly,
                    sources,
                    views,
                ) {
                    Ok(request) => request,
                    Err(error) => {
                        return Ok(ToolResult {
                            value: failed(error),
                            instructions: 0,
                            pending: false,
                        });
                    }
                };
                compiler.prepare(&id, &request)?;
                compiler.drive(vm.universe_id(), &id, &toolchain)?
            };
            let mut summary = compiled.summary()?;
            let mut truncated = false;
            if let Some(diagnostics) = summary["report"]["diagnostics"].as_array_mut() {
                if diagnostics.len() > 8 {
                    diagnostics.truncate(8);
                    truncated = true;
                }
                for diagnostic in diagnostics {
                    if let Some(message) = diagnostic["message"].as_str()
                        && message.len() > 512
                    {
                        let mut end = 512;
                        while !message.is_char_boundary(end) {
                            end -= 1;
                        }
                        let shortened = message[..end].to_owned();
                        diagnostic["message"] = json!(shortened);
                        truncated = true;
                    }
                }
            }
            if truncated {
                summary["report"]["diagnosticsTruncated"] = json!(true);
            }
            json!({"ok":compiled.artifact.is_some(),"requestId":id,"output":summary})
        }
        Action::Install { compilation_turn } => {
            let mut cursor = reference(saved_session(&vm)?)?;
            cursor = reference(field(&vm, cursor, SESSION, "FirstTurn")?)?;
            for _ in 1..compilation_turn {
                cursor = reference(field(&vm, cursor, TURN, "Next")?)?;
            }
            let previous = optional_text(&vm, field(&vm, cursor, TURN, "Input")?)?;
            if !previous
                .as_deref()
                .and_then(|text| serde_json::from_str::<Action>(text).ok())
                .is_some_and(|action| matches!(action, Action::Compile { .. }))
            {
                failed(Error("selected turn is not a compilation".into()))
            } else {
                let compiler = CompilationStore::open(&host.compiler_ledger)?;
                let compiled = compiler.find(
                    vm.universe_id(),
                    &action_id(session_id, compilation_turn, "compile"),
                )?;
                if let Some(artifact) = compiled.and_then(|c| c.artifact) {
                    let id = action_id(session_id, turn, "install");
                    let preflight = if vm.installation(&id).is_some() {
                        Ok(())
                    } else {
                        vm.program
                            .link_modules(&[artifact.implementation()?], &artifact.views()?)
                            .map(|_| ())
                    };
                    match preflight {
                        Err(error) => failed(error),
                        Ok(()) => {
                            let record = install_once(&mut store, &id, &artifact)?;
                            json!({"ok":true,"requestId":id,"artifactId":record.artifact.id(),"assemblyId":record.assembly.to_string()})
                        }
                    }
                } else {
                    failed(Error(
                        "selected compilation has no published artifact".into(),
                    ))
                }
            }
        }
        Action::Invoke { method, arguments } => {
            let method = vm.program.entry(&method)?;
            let values = arguments
                .iter()
                .map(|arg| argument(&vm, arg))
                .collect::<Result<Vec<_>>>()?;
            require(
                !arguments
                    .iter()
                    .any(|arg| matches!(arg, Argument::Static { .. })),
                "invocation input was not frozen",
            )?;
            let before = vm.instructions_executed();
            let id = action_id(session_id, turn, "invoke");
            let record = vm.invoke_once(&mut store, &id, method, values, budget, 100)?;
            instructions = vm.instructions_executed() - before;
            match record.state {
                InvocationState::Running => {
                    pending = true;
                    json!({"ok":true,"state":"running","requestId":id})
                }
                InvocationState::Completed { result } => {
                    json!({"ok":true,"requestId":id,"execution":record.execution.to_string(),"result":result.map(value_json)})
                }
                InvocationState::Faulted { message } => {
                    json!({"ok":false,"requestId":id,"execution":record.execution.to_string(),"error":message})
                }
            }
        }
        Action::WebStatus {} => {
            let status = crate::web::inspect_publications(&mut store)?;
            json!({"ok":true,"output":{"format":1,"route":"default",
                "current":status["current"],"publicationCount":status["publications"].as_array().unwrap().len(),
                "publicationLimit":crate::web::MAX_PUBLICATIONS,"publicationEnabled":host.allow_web_publication,
                "handlerAbi":"static string Handle(string id, string method, string target, string body)",
                "httpProfile":"http-v1","oneContext":true}})
        }
        Action::PreviewWeb {
            method,
            target,
            body,
            publication,
        } => {
            let selected =
                publication.ok_or_else(|| Error("preview input was not frozen".into()))?;
            let request = crate::web::WebRequest {
                id: format!("agent.{session_id}.{turn}.preview"),
                method,
                target,
                body,
            };
            match crate::web::preview_handler(&mut vm, &mut store, &request, &selected, budget) {
                Ok(reply) => {
                    instructions = reply.instructions;
                    pending = reply.state == "pending";
                    let (body, truncated) = preview_body(&reply.body);
                    json!({"ok":reply.status==200,"requestId":reply.request_id,"preview":{
                        "transport":"managed_handler","socketTested":false,"status":reply.status,
                        "state":reply.state,"body":body,"bodyBytes":reply.body.len(),"truncated":truncated,
                        "execution":reply.execution,"publicationId":selected.id,"method":selected.method,"codeVersion":selected.code}})
                }
                Err(error) if error.status == 503 => return Err(Error(error.to_string())),
                Err(error) => failed(Error(error.to_string())),
            }
        }
        Action::PublishWeb {
            method,
            code_version,
        } => {
            // Publication IDs use the HTTP journal's bounded ASCII alphabet.
            let id = format!("agent.{session_id}.{turn}.publish");
            let existing = crate::web::find_publication(&mut store, &id)?;
            if let Some(record) = existing {
                require(
                    record.method == method && Some(&record.code) == code_version.as_ref(),
                    "saved website publication differs from its frozen agent input",
                )?;
                json!({"ok":true,"requestId":id,"publication":record})
            } else if !host.allow_web_publication {
                failed(Error(
                    "website publication requires explicit host opt-in (--allow-web-publication)"
                        .into(),
                ))
            } else if crate::web::inspect_publications(&mut store)?["publications"]
                .as_array()
                .unwrap()
                .len()
                >= crate::web::MAX_PUBLICATIONS
            {
                failed(Error("HTTP publication limit exceeded".into()))
            } else {
                // Save/storage failures propagate: retain the pending managed
                // action so recovery reconciles the same receipt before Apply.
                let record = crate::web::publish_handler(&mut store, &id, &method)?;
                json!({"ok":true,"requestId":id,"publication":record})
            }
        }
        Action::Finish { message } => json!({"ok":true,"message":message}),
    };
    Ok(ToolResult {
        value,
        instructions,
        pending,
    })
}
fn value_json(value: Value) -> Json {
    match value {
        Value::Null => json!({"kind":"null"}),
        Value::I32(v) => json!({"kind":"i32","value":v}),
        Value::I64(v) => json!({"kind":"i64","value":v.to_string()}),
        Value::Reference(id) => json!({"kind":"object","id":id.to_string()}),
    }
}
fn resolve_dotnet(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    require(
        path.as_os_str() == "dotnet",
        "configured dotnet host does not exist",
    )?;
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|root| {
            root.join(if cfg!(windows) {
                "dotnet.exe"
            } else {
                "dotnet"
            })
        })
        .find(|path| path.is_file())
        .ok_or_else(|| Error("dotnet host is not on PATH".into()))
}

struct Job<T> {
    result: Option<Result<T>>,
    waker: Option<Waker>,
}
fn on_worker<T: Send + 'static>(
    task: impl FnOnce() -> Result<T> + Send + 'static,
) -> impl Future<Output = Result<T>> {
    let state = Arc::new(Mutex::new(Job {
        result: None,
        waker: None,
    }));
    let worker = state.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
            .unwrap_or_else(|_| Err(Error("agent tool worker panicked".into())));
        let mut state = worker.lock().unwrap();
        state.result = Some(result);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    });
    poll_fn(move |context| {
        let mut state = state.lock().unwrap();
        if let Some(result) = state.result.take() {
            Poll::Ready(result)
        } else {
            state.waker = Some(context.waker().clone());
            Poll::Pending
        }
    })
}

impl Vm<'_> {
    pub fn agent_status(&self) -> Result<Json> {
        let state = session(self)?;
        let mut cursor = field(self, reference(saved_session(self)?)?, SESSION, "First")?;
        let mut transcript = Vec::new();
        while cursor != Value::Null {
            let id = reference(cursor)?;
            let content = units(self, field(self, id, "Renraku.ChatMessage", "Content")?)?;
            transcript.push(json!({"role":integer(field(self,id,"Renraku.ChatMessage","Role")?)?,"content":String::from_utf16(&content).ok(),"utf16Units":content.len()}));
            cursor = field(self, id, "Renraku.ChatMessage", "Next")?;
        }
        Ok(
            json!({"format":1,"sessionId":state.id,"turn":state.count,"phase":state.phase,"transcript":transcript,"pendingInput":state.input,"result":state.result,
                "followupSupported":state.input_support,"inputs":state.followups.iter().map(|i|json!({"id":i.id,"content":i.content,"afterTurn":i.after_turn})).collect::<Vec<_>>()}),
        )
    }
}
