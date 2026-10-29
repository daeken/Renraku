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
const MAX_TURNS: u32 = 32;
const MAX_ACTION: usize = 64 * 1024;
const MAX_OUTPUT: usize = 16 * 1024;
pub const SYSTEM_PROMPT: &str = r#"You control a persistent CIL runtime. Reply with exactly one JSON object, no Markdown or prose. This is a text protocol, not native function calling. Tool results arrive as user JSON messages. Supported actions:
{"op":"search","query":"counter","limit":20} searches live type/field/method names (limit 1..50).
{"op":"read_code","method":"logical method/code ID or Type::Method"} reads exact saved CIL and retained assembly-scoped source.
{"op":"describe","type":"type name or logical ID"} describes a compiler contract; unsupported shapes return an error.
{"op":"compile","assembly":"Generated.Example","source":"C# source","referenceTypes":["Renraku.Guest.Counter"]} compiles one Generated.cs against the core and selected live types. Roots must share one module. Source <=32768 UTF-8 bytes. Compilation alone does not install code.
{"op":"install","compilationTurn":3} installs the artifact compiled in that earlier numbered turn. Assembly names must be new; this does not replace existing bodies.
{"op":"invoke","method":"logical method ID or Type::Method","arguments":[{"kind":"static","field":"Renraku.Guest.Simulation::Saved"}]} invokes a static entry with <=8 arguments. Other arguments: {"kind":"i32","value":7}, {"kind":"i64","value":"9007199254740993"}, {"kind":"object","id":"logical object ID"}, {"kind":"null"}. Calls have bounded instructions and cannot submit model requests. Static selectors freeze to their current values before execution.
{"op":"finish","message":"short result"} ends the session.
Search and inspect before coding. The initial test object is Renraku.Guest.Simulation::Saved, a persistent Counter (created with Value=19 only if absent). Use normal managed fields/methods and supported CIL: integers, references, strings, constructors, calls, branches. Arrays, generics, exceptions, division, .NET host BCL, native I/O and C# async/await are unsupported. Source/compiler/host paths are not accepted. Do not redefine live types: compile callers using referenceTypes. Examine errors and use a new turn to correct code. Tool outcomes and request identities are durable; no hidden retries or remote inference permissions are granted. Maximum 32 assistant turns, bounded transcript/tool output. Return finish when the goal is satisfied."#;

#[derive(Clone, Debug)]
pub struct AgentHost {
    pub universe: PathBuf,
    pub compiler_ledger: PathBuf,
    pub dotnet: PathBuf,
    pub compiler_driver: PathBuf,
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
    },
    Describe {
        #[serde(rename = "type")]
        type_name: String,
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
    Finish {
        message: String,
    },
}
fn default_limit() -> usize {
    20
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
    let owner = owner(vm.program, ty)?;
    let object = vm.heap().object(id)?;
    require(
        object.type_id() == owner,
        "agent object has the wrong schema",
    )?;
    let definition = vm
        .program
        .fields()
        .find(|f| f.owner == owner && f.name == name && !f.is_static)
        .ok_or_else(|| Error(format!("agent field {ty}::{name} is missing")))?;
    object
        .fields()
        .get(&definition.id)
        .copied()
        .ok_or_else(|| Error("agent field has no storage".into()))
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
    for turn in turns {
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
    }
    require(
        expected == chat.len(),
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
        "Freeze" | "Apply" => Ok(Some(false)),
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
    serde_json::from_str(&text).map_err(|_| {
        Error("invalid action schema; return one supported JSON object with no extra fields".into())
    })
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
        Action::ReadCode { method } => {
            let code = vm.read_code(&method)?;
            Action::ReadCode {
                method: code["codeVersion"].as_str().unwrap().into(),
            }
        }
        Action::Describe { type_name } => Action::Describe {
            type_name: vm.program.find_type(&type_name)?.to_string(),
        },
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
                vm.program.reference_view(&roots)?;
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
        Action::Finish { message } => {
            require(message.len() <= 4096, "finish message exceeds limit")?;
            Action::Finish { message }
        }
    })
}
fn output(value: Json) -> Result<String> {
    let text = serde_json::to_string(&value).map_err(|e| Error(e.to_string()))?;
    if text.len() > MAX_OUTPUT {
        Ok(json!({"ok":false,"truncated":true,"error":"tool output exceeds 16384 bytes; select a narrower query or smaller method"}).to_string())
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
        mut observer: impl FnMut(AgentStage) -> Result<()>,
    ) -> Result<AgentReport> {
        require(
            limits.tool_actions <= 32 && limits.instructions <= 1_000_000,
            "agent invocation limits exceed policy",
        )?;
        let mut instructions = 0;
        let mut operations = 0;
        let mut actions = 0;
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
                        let mut retained = if let Action::Invoke { arguments, .. } = &action {
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
        Action::ReadCode { method } => match vm.read_code(&method) {
            Ok(value) => json!({"ok":true,"output":value}),
            Err(error) => failed(error),
        },
        Action::Describe { type_name } => match vm
            .program
            .find_type(&type_name)
            .and_then(|id| vm.program.reference_view(&[id]))
        {
            Ok(view) => {
                json!({"ok":true,"schemaHash":view.schema_hash,"output":serde_json::from_str::<Json>(&view.schema_json).map_err(|e|Error(e.to_string()))?})
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
                vec![vm.program.reference_view(&roots)?]
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
            json!({"format":1,"sessionId":state.id,"turn":state.count,"phase":state.phase,"transcript":transcript,"pendingInput":state.input,"result":state.result}),
        )
    }
}
