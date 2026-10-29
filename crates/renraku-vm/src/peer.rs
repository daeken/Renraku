//! Authenticated loopback owner transport and a durable host caller journal.
//! The remote module supplies imported contract handles and portable guest waits.
use crate::{
    checkpoint::CheckpointStore,
    error::{Error, Result, require},
    execution::{ExecutionStatus, MethodVersion},
    identity::{CodeVersionId, ExecutionId, GrantId, MethodId, NodeId, ObjectId, UniverseId},
    owner::{self, OwnerCall, OwnerReply, ReplyState},
    placement::{self, Argument, OwnerContext},
    runtime::Vm,
};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
const MAGIC: &[u8; 8] = b"RNKOWN1\0";
const REQUEST: &[u8] = b"renraku.owner-wire.request.v1\0";
const RESPONSE: &[u8] = b"renraku.owner-wire.response.v1\0";
const NS: &str = "renraku.peer-v1/";
pub const MAX_FRAME: usize = 65_536;
pub const MAX_CALLS: usize = 128;
pub const MAX_PEERS: usize = 32;
const TIMEOUT: Duration = Duration::from_secs(5);
#[derive(Clone)]
pub struct PeerKey(Zeroizing<[u8; 32]>);
impl fmt::Debug for PeerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PeerKey([REDACTED])")
    }
}
impl PeerKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
    pub fn generate() -> Result<Self> {
        let mut key = Zeroizing::new([0; 32]);
        getrandom::fill(key.as_mut())
            .map_err(|_| Error("peer random source unavailable".into()))?;
        Ok(Self(key))
    }
    #[cfg(unix)]
    pub fn load_private(path: impl AsRef<Path>) -> Result<Self> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)?;
        let meta = file.metadata()?;
        require(
            meta.is_file() && meta.permissions().mode() & 0o077 == 0,
            "peer key must be a private regular file",
        )?;
        let mut key = Zeroizing::new([0; 32]);
        file.read_exact(key.as_mut())?;
        let mut extra = [0];
        require(
            file.read(&mut extra)? == 0,
            "peer key must contain exactly 32 bytes",
        )?;
        Ok(Self(key))
    }
    #[cfg(not(unix))]
    pub fn load_private(_path: impl AsRef<Path>) -> Result<Self> {
        Err(Error(
            "private peer key files require Unix; use an embedding key provider".into(),
        ))
    }
    #[cfg(not(unix))]
    pub fn write_new(&self, _path: impl AsRef<Path>) -> Result<()> {
        Err(Error(
            "private peer key files require Unix; use an embedding key provider".into(),
        ))
    }
    #[cfg(unix)]
    pub fn write_new(&self, path: impl AsRef<Path>) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(self.0.as_ref())?;
        file.sync_all()?;
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub owner: UniverseId,
    pub node: NodeId,
    pub epoch: u64,
    pub address: SocketAddr,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum WireValue {
    I32(i32),
    I64(String),
    Text(Vec<u16>),
    Null,
}
impl WireValue {
    fn from_arg(arg: &Argument) -> Self {
        match arg {
            Argument::I32(n) => Self::I32(*n),
            Argument::I64(n) => Self::I64(n.to_string()),
            Argument::Text(s) => Self::Text(s.clone()),
            Argument::Null => Self::Null,
        }
    }
    fn arg(&self) -> Result<Argument> {
        Ok(match self {
            Self::I32(n) => Argument::I32(*n),
            Self::I64(s) => {
                let n: i64 = s.parse().map_err(|_| Error("invalid wire I64".into()))?;
                require(n.to_string() == *s, "noncanonical wire I64")?;
                Argument::I64(n)
            }
            Self::Text(s) => {
                require(
                    s.len() <= placement::MAX_TEXT_UNITS,
                    "wire text exceeds 4096 units",
                )?;
                Argument::Text(s.clone())
            }
            Self::Null => Argument::Null,
        })
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireCall {
    format: u32,
    owner: String,
    epoch: u64,
    operation: String,
    grant: String,
    object: String,
    method: String,
    code: String,
    arguments: Vec<WireValue>,
}
fn op(id: &str) -> Result<()> {
    require(
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "invalid peer operation ID",
    )
}
impl WireCall {
    fn from_call(call: &OwnerCall) -> Result<Self> {
        let w = Self {
            format: 1,
            owner: call.owner.to_string(),
            epoch: call.epoch,
            operation: call.operation.clone(),
            grant: call.grant.to_string(),
            object: call.object.to_string(),
            method: call.method.method.to_string(),
            code: call.method.code.to_string(),
            arguments: call.arguments.iter().map(WireValue::from_arg).collect(),
        };
        w.call()?;
        Ok(w)
    }
    fn call(&self) -> Result<OwnerCall> {
        require(
            self.format == 1 && self.epoch > 0 && self.arguments.len() <= placement::MAX_ARGUMENTS,
            "unsupported peer call format/epoch/count",
        )?;
        op(&self.operation)?;
        let arguments = self
            .arguments
            .iter()
            .map(WireValue::arg)
            .collect::<Result<Vec<_>>>()?;
        let total: usize = arguments
            .iter()
            .map(|a| {
                if let Argument::Text(s) = a {
                    s.len()
                } else {
                    0
                }
            })
            .sum();
        require(
            total <= placement::MAX_CALL_TEXT_UNITS,
            "peer call text exceeds 8192 units",
        )?;
        Ok(OwnerCall {
            operation: self.operation.clone(),
            owner: UniverseId::from_bytes(placement::id(&self.owner)?),
            epoch: self.epoch,
            grant: GrantId::from_bytes(placement::id(&self.grant)?),
            object: ObjectId::from_bytes(placement::id(&self.object)?),
            method: MethodVersion {
                method: MethodId::from_bytes(placement::id(&self.method)?),
                code: CodeVersionId::from_bytes(placement::id(&self.code)?),
            },
            arguments,
        })
    }
}
pub fn encode_call(call: &OwnerCall) -> Result<Vec<u8>> {
    json(&WireCall::from_call(call)?)
}
pub fn decode_call(bytes: &[u8]) -> Result<OwnerCall> {
    parse::<WireCall>(bytes)?.call()
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum State {
    Pending,
    Completed { value: Option<WireValue> },
    Faulted { diagnostic: String, truncated: bool },
    InvalidResult { diagnostic: String },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireReply {
    execution: String,
    method: String,
    code: String,
    object: String,
    reply: State,
}
impl WireReply {
    fn from_reply(reply: &OwnerReply) -> Result<Self> {
        let state = match &reply.state {
            ReplyState::Pending => State::Pending,
            ReplyState::Completed(value) => State::Completed {
                value: value.as_ref().map(WireValue::from_arg),
            },
            ReplyState::Faulted {
                diagnostic,
                truncated,
            } => State::Faulted {
                diagnostic: diagnostic.clone(),
                truncated: *truncated,
            },
            ReplyState::InvalidResult { diagnostic } => State::InvalidResult {
                diagnostic: diagnostic.clone(),
            },
        };
        let w = Self {
            execution: reply.execution.to_string(),
            method: reply.method.method.to_string(),
            code: reply.method.code.to_string(),
            object: reply.object.to_string(),
            reply: state,
        };
        w.reply()?;
        Ok(w)
    }
    fn reply(&self) -> Result<OwnerReply> {
        let state = match &self.reply {
            State::Pending => ReplyState::Pending,
            State::Completed { value } => {
                ReplyState::Completed(value.as_ref().map(WireValue::arg).transpose()?)
            }
            State::Faulted {
                diagnostic,
                truncated,
            } => {
                require(
                    diagnostic.len() <= 4096,
                    "peer diagnostic exceeds its bound",
                )?;
                ReplyState::Faulted {
                    diagnostic: diagnostic.clone(),
                    truncated: *truncated,
                }
            }
            State::InvalidResult { diagnostic } => {
                require(
                    diagnostic == "owner result exceeds 4096 UTF-16 units",
                    "unknown version-1 result diagnostic",
                )?;
                ReplyState::InvalidResult {
                    diagnostic: diagnostic.clone(),
                }
            }
        };
        Ok(OwnerReply {
            execution: ExecutionId::from_bytes(placement::id(&self.execution)?),
            method: MethodVersion {
                method: MethodId::from_bytes(placement::id(&self.method)?),
                code: CodeVersionId::from_bytes(placement::id(&self.code)?),
            },
            object: ObjectId::from_bytes(placement::id(&self.object)?),
            state,
        })
    }
    fn matches(&self, call: &OwnerCall) -> Result<()> {
        let reply = self.reply()?;
        require(
            reply.method == call.method && reply.object == call.object,
            "peer reply target/code mismatch",
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    format: u32,
    node: String,
    call: WireCall,
    resume: bool,
    expected_execution: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Response {
    format: u32,
    owner: String,
    node: String,
    epoch: u64,
    request_hash: String,
    reply: Option<WireReply>,
}
fn json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value).map_err(|e| Error(e.to_string()))?;
    require(bytes.len() <= MAX_FRAME, "peer document exceeds 64 KiB")?;
    Ok(bytes)
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    require(bytes.len() <= MAX_FRAME, "peer document exceeds 64 KiB")?;
    serde_json::from_slice(bytes).map_err(|_| Error("invalid peer document".into()))
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn nonce() -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| Error("peer random source unavailable".into()))?;
    Ok(bytes)
}
fn mac(key: &PeerKey, domain: &[u8], header: &[u8], body: &[u8]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.0.as_ref()).expect("32-byte HMAC key");
    mac.update(domain);
    mac.update(header);
    mac.update(body);
    mac
}
fn header(caller: UniverseId, nonce: &[u8; 32], len: usize) -> Result<[u8; 60]> {
    require(len <= MAX_FRAME, "peer frame exceeds 64 KiB")?;
    let mut h = [0; 60];
    h[..8].copy_from_slice(MAGIC);
    h[8..12].copy_from_slice(&(len as u32).to_le_bytes());
    h[12..28].copy_from_slice(caller.as_bytes());
    h[28..].copy_from_slice(nonce);
    Ok(h)
}
fn read_exact(stream: &mut TcpStream, bytes: &mut [u8], deadline: Instant) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        require(!remaining.is_zero(), "peer read deadline exceeded")?;
        stream
            .set_read_timeout(Some(Duration::from_millis(
                remaining.as_millis().max(1) as u64
            )))
            .map_err(|e| Error(format!("peer set read timeout: {e}")))?;
        match stream.read(&mut bytes[offset..]) {
            Ok(0) => return Err(Error("truncated peer frame".into())),
            Ok(n) => offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn response_header(h: &[u8; 60], caller: UniverseId, nonce: &[u8; 32]) -> Result<()> {
    require(
        h[12..28] == *caller.as_bytes() && h[28..] == *nonce,
        "peer reply caller/nonce mismatch",
    )
}
fn read_header(stream: &mut TcpStream, deadline: Instant) -> Result<([u8; 60], UniverseId, usize)> {
    let mut h = [0; 60];
    read_exact(stream, &mut h, deadline)?;
    require(&h[..8] == MAGIC, "unsupported peer frame")?;
    let len = u32::from_le_bytes(h[8..12].try_into().unwrap()) as usize;
    require(len <= MAX_FRAME, "peer frame exceeds 64 KiB")?;
    let caller = UniverseId::from_bytes(h[12..28].try_into().unwrap());
    Ok((h, caller, len))
}
fn read_body(
    stream: &mut TcpStream,
    key: &PeerKey,
    domain: &[u8],
    h: &[u8; 60],
    len: usize,
    deadline: Instant,
) -> Result<Vec<u8>> {
    let mut body = vec![0; len];
    read_exact(stream, &mut body, deadline)?;
    let mut tag = [0; 32];
    read_exact(stream, &mut tag, deadline)?;
    mac(key, domain, h, &body)
        .verify_slice(&tag)
        .map_err(|_| Error("peer authentication failed".into()))?;
    Ok(body)
}
fn write_frame(
    stream: &mut TcpStream,
    key: &PeerKey,
    domain: &[u8],
    caller: UniverseId,
    nonce: &[u8; 32],
    body: &[u8],
) -> Result<()> {
    let h = header(caller, nonce, body.len())?;
    let tag = mac(key, domain, &h, body).finalize().into_bytes();
    let mut bytes = h.to_vec();
    bytes.extend(body);
    bytes.extend_from_slice(&tag);
    let deadline = Instant::now() + TIMEOUT;
    let mut offset = 0;
    while offset < bytes.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        require(!remaining.is_zero(), "peer write deadline exceeded")?;
        stream
            .set_write_timeout(Some(Duration::from_millis(
                remaining.as_millis().max(1) as u64
            )))
            .map_err(|e| Error(format!("peer set write timeout: {e}")))?;
        let n = stream
            .write(&bytes[offset..])
            .map_err(|e| Error(format!("peer write: {e}")))?;
        require(n > 0, "peer socket stopped writing")?;
        offset += n;
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerStage {
    Authenticated,
    BeforeWrite,
}
pub struct OwnerServer {
    listener: TcpListener,
    database: PathBuf,
    context: OwnerContext,
    owner: UniverseId,
    peers: BTreeMap<UniverseId, PeerKey>,
    budget: u64,
}
impl OwnerServer {
    pub fn bind(
        database: impl AsRef<Path>,
        context: OwnerContext,
        address: SocketAddr,
        peers: Vec<(UniverseId, PeerKey)>,
        budget: u64,
    ) -> Result<Self> {
        require(
            address.ip().is_loopback(),
            "owner transport is loopback-only",
        )?;
        require(
            database.as_ref().is_file(),
            "owner checkpoint does not exist",
        )?;
        require(
            budget <= 1_000_000,
            "peer instruction budget exceeds 1000000",
        )?;
        require(
            !peers.is_empty() && peers.len() <= MAX_PEERS,
            "owner server requires 1..32 peers",
        )?;
        let mut fingerprints = BTreeSet::new();
        let mut map = BTreeMap::new();
        for (caller, key) in peers {
            require(
                fingerprints.insert(Sha256::digest(key.0.as_ref()).to_vec()),
                "peer keys must be unique per caller",
            )?;
            require(
                map.insert(caller, key).is_none(),
                "duplicate peer principal",
            )?;
        }
        let mut store = CheckpointStore::open(database.as_ref())?;
        let saved = store.load()?;
        let vm = saved.snapshot.restore()?;
        let registry = placement::read(&vm)?
            .ok_or_else(|| Error("owner placement registry is missing".into()))?;
        require(
            registry.owner().node == context.node && registry.owner().epoch == context.epoch,
            "server owner node/epoch mismatch",
        )?;
        require(
            !map.contains_key(&vm.universe_id()),
            "peer principal must be a different universe",
        )?;
        Ok(Self {
            listener: TcpListener::bind(address)?,
            database: database.as_ref().into(),
            context,
            owner: vm.universe_id(),
            peers: map,
            budget,
        })
    }
    pub fn address(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }
    pub fn serve_one(&self) -> Result<()> {
        self.serve_one_with_observer(|_| Ok(()))
    }
    pub fn serve_one_with_observer(
        &self,
        mut observe: impl FnMut(ServerStage) -> Result<()>,
    ) -> Result<()> {
        let (mut stream, address) = self.listener.accept()?;
        require(address.ip().is_loopback(), "non-loopback peer rejected")?;
        let deadline = Instant::now() + TIMEOUT;
        let (h, caller, len) = read_header(&mut stream, deadline)?;
        let key = self
            .peers
            .get(&caller)
            .ok_or_else(|| Error("unknown peer principal".into()))?;
        let bytes = read_body(&mut stream, key, REQUEST, &h, len, deadline)?;
        observe(ServerStage::Authenticated)?;
        let result = (|| -> Result<WireReply> {
            let request: Request = parse(&bytes)?;
            require(
                json(&request)? == bytes
                    && request.format == 1
                    && NodeId::from_bytes(placement::id(&request.node)?) == self.context.node,
                "invalid peer request binding",
            )?;
            let call = request.call.call()?;
            require(
                call.owner == self.owner && call.epoch == self.context.epoch,
                "request owner/epoch mismatch",
            )?;
            let expected = request
                .expected_execution
                .as_deref()
                .map(|id| placement::id(id).map(ExecutionId::from_bytes))
                .transpose()?;
            let mut store = CheckpointStore::open(&self.database)?;
            let reply = if request.resume {
                owner::resume(
                    &mut store,
                    &self.context,
                    caller,
                    &call,
                    self.budget,
                    expected,
                )?
            } else {
                require(expected.is_none(), "new call cannot specify execution")?;
                owner::accept(&mut store, &self.context, caller, &call, self.budget)?
            };
            WireReply::from_reply(&reply)
        })();
        let response = Response {
            format: 1,
            owner: self.owner.to_string(),
            node: self.context.node.to_string(),
            epoch: self.context.epoch,
            request_hash: hash(&bytes),
            reply: result.ok(),
        };
        let response = json(&response)?;
        observe(ServerStage::BeforeWrite)?;
        let n: [u8; 32] = h[28..].try_into().unwrap();
        write_frame(&mut stream, key, RESPONSE, caller, &n, &response)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CallReport {
    Unresolved,
    Owner(OwnerReply),
}
pub fn report_json(report: &CallReport) -> Result<serde_json::Value> {
    Ok(match report {
        CallReport::Unresolved => serde_json::json!({"format":1,"state":"unresolved"}),
        CallReport::Owner(r) => {
            let mut value = serde_json::to_value(WireReply::from_reply(r)?)
                .map_err(|e| Error(e.to_string()))?;
            value["format"] = 1.into();
            value
        }
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    format: u32,
    caller: String,
    node: String,
    address: String,
    call: WireCall,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ack {
    format: u32,
    intent_hash: String,
    execution: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedReply {
    format: u32,
    intent_hash: String,
    reply: WireReply,
}
fn intent_key(operation: &str) -> Result<String> {
    op(operation)?;
    Ok(format!("{NS}call/{operation}"))
}
fn named(kind: &str, operation: &str) -> String {
    format!("{NS}{kind}/{operation}")
}
fn put(vm: &mut Vm<'_>, name: String, bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error("invalid peer journal UTF8".into()))?;
    let object = vm.new_string(text.encode_utf16().collect())?;
    vm.set_root(name, object)
}
fn root_bytes(vm: &Vm<'_>, name: &str) -> Result<Option<Vec<u8>>> {
    vm.heap()
        .roots()
        .get(name)
        .map(|id| {
            let units = vm
                .heap()
                .object(*id)?
                .string_units()
                .ok_or_else(|| Error("peer journal root must be a string".into()))?;
            require(units.len() <= MAX_FRAME, "peer journal exceeds unit bound")?;
            let text =
                String::from_utf16(units).map_err(|_| Error("invalid peer journal text".into()))?;
            require(text.len() <= MAX_FRAME, "peer journal exceeds byte bound")?;
            Ok(text.into_bytes())
        })
        .transpose()
}
fn endpoint(intent: &Intent) -> Result<Endpoint> {
    let call = intent.call.call()?;
    let address: SocketAddr = intent
        .address
        .parse()
        .map_err(|_| Error("invalid peer endpoint".into()))?;
    require(
        intent.format == 1
            && address.ip().is_loopback()
            && address.port() != 0
            && address.to_string() == intent.address,
        "invalid peer intent version/address",
    )?;
    Ok(Endpoint {
        owner: call.owner,
        node: NodeId::from_bytes(placement::id(&intent.node)?),
        epoch: call.epoch,
        address,
    })
}
fn journal(vm: &Vm<'_>) -> Result<BTreeMap<String, Intent>> {
    let mut records = BTreeMap::new();
    let mut expected = BTreeSet::new();
    for name in vm
        .heap()
        .roots()
        .keys()
        .filter(|n| n.starts_with(&format!("{NS}call/")))
    {
        require(records.len() < MAX_CALLS, "peer call limit exceeded")?;
        let bytes = root_bytes(vm, name)?.unwrap();
        let intent: Intent = parse(&bytes)?;
        endpoint(&intent)?;
        require(
            UniverseId::from_bytes(placement::id(&intent.caller)?) == vm.universe_id()
                && intent_key(&intent.call.operation)? == *name
                && json(&intent)? == bytes,
            "peer intent scope/canonical binding mismatch",
        )?;
        let operation = &intent.call.operation;
        expected.insert(name.clone());
        let sent = named("sent", operation);
        let attempted = if let Some(object) = vm.heap().roots().get(&sent) {
            require(
                *object == vm.heap().roots()[name],
                "peer attempt marker must alias its intent",
            )?;
            expected.insert(sent);
            true
        } else {
            false
        };
        let ackname = named("ack", operation);
        let ack = if let Some(bytes) = root_bytes(vm, &ackname)? {
            let ack: Ack = parse(&bytes)?;
            require(
                attempted
                    && ack.format == 1
                    && ack.intent_hash == hash(&json(&intent)?)
                    && json(&ack)? == bytes,
                "invalid peer acknowledgement",
            )?;
            placement::id(&ack.execution)?;
            expected.insert(ackname);
            Some(ack)
        } else {
            None
        };
        let replyname = named("reply", operation);
        if let Some(bytes) = root_bytes(vm, &replyname)? {
            let reply: SavedReply = parse(&bytes)?;
            reply.reply.matches(&intent.call.call()?)?;
            require(
                reply.format == 1
                    && reply.intent_hash == hash(&json(&intent)?)
                    && ack
                        .as_ref()
                        .is_some_and(|a| a.execution == reply.reply.execution)
                    && reply.reply.reply != State::Pending
                    && json(&reply)? == bytes,
                "invalid peer terminal reply",
            )?;
            expected.insert(replyname);
        }
        records.insert(operation.clone(), intent);
    }
    require(
        vm.heap()
            .roots()
            .keys()
            .filter(|n| n.starts_with(NS))
            .cloned()
            .collect::<BTreeSet<_>>()
            == expected,
        "unknown/orphan peer journal root",
    )?;
    Ok(records)
}
pub(crate) fn validate(vm: &Vm<'_>) -> Result<()> {
    journal(vm).map(|_| ())
}
pub fn inspect(store: &mut CheckpointStore, operation: &str) -> Result<Option<CallReport>> {
    op(operation)?;
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    let records = journal(&vm)?;
    if !records.contains_key(operation) {
        return Ok(None);
    };
    if let Some(bytes) = root_bytes(&vm, &named("reply", operation))? {
        let reply: SavedReply = parse(&bytes)?;
        return Ok(Some(CallReport::Owner(reply.reply.reply()?)));
    }
    Ok(Some(CallReport::Unresolved))
}
pub(crate) struct GuestIntent {
    pub id: ObjectId,
    intent: Intent,
    bytes: Vec<u8>,
}
pub(crate) fn prepare_guest(
    vm: &Vm<'_>,
    endpoint: &Endpoint,
    call: &OwnerCall,
) -> Result<GuestIntent> {
    let records = journal(vm)?;
    require(
        records.len() < MAX_CALLS && !records.contains_key(&call.operation),
        "guest peer operation limit/collision",
    )?;
    let intent = Intent {
        format: 1,
        caller: vm.universe_id().to_string(),
        node: endpoint.node.to_string(),
        address: endpoint.address.to_string(),
        call: WireCall::from_call(call)?,
    };
    require(
        self::endpoint(&intent)? == *endpoint,
        "guest endpoint mismatch",
    )?;
    let bytes = json(&intent)?;
    let mut id = ObjectId::fresh();
    while vm.heap.object(id).is_ok() {
        id = ObjectId::fresh();
    }
    vm.program.string_type()?;
    Ok(GuestIntent { id, intent, bytes })
}
pub(crate) fn publish_guest(vm: &mut Vm<'_>, prepared: GuestIntent) -> Result<()> {
    let units = String::from_utf8(prepared.bytes)
        .unwrap()
        .encode_utf16()
        .collect();
    vm.heap
        .allocate_string(prepared.id, vm.program.string_type()?, units)?;
    vm.set_root(intent_key(&prepared.intent.call.operation)?, prepared.id)
}
pub(crate) fn guest_call(vm: &Vm<'_>, request: ObjectId) -> Result<(Endpoint, OwnerCall)> {
    let records = journal(vm)?;
    let intent = records
        .values()
        .find(|i| {
            vm.heap
                .roots()
                .get(&format!("{NS}call/{}", i.call.operation))
                == Some(&request)
        })
        .ok_or_else(|| Error("guest remote intent missing".into()))?;
    let call = intent.call.call()?;
    require(
        call.operation
            == format!(
                "g.{}.{}",
                vm.context.id,
                vm.instructions_executed
                    .checked_sub(1)
                    .ok_or_else(|| Error("remote instruction counter underflow".into()))?
            ),
        "guest remote operation/execution mismatch",
    )?;
    Ok((endpoint(intent)?, call))
}
pub(crate) fn guest_reply(vm: &Vm<'_>, operation: &str) -> Result<Option<OwnerReply>> {
    root_bytes(vm, &named("reply", operation))?
        .map(|b| parse::<SavedReply>(&b)?.reply.reply())
        .transpose()
}
pub(crate) fn call_guest(
    store: &mut CheckpointStore,
    endpoint: &Endpoint,
    call: &OwnerCall,
    key: &PeerKey,
) -> Result<CallReport> {
    call_inner(store, endpoint, call, key, true)
}
/// Commit caller intent before any socket write. Explicit retries after a sent
/// marker reconcile only existing owner receipts; a missing ledger never starts.
pub fn call_once(
    store: &mut CheckpointStore,
    peer: &Endpoint,
    call: &OwnerCall,
    key: &PeerKey,
) -> Result<CallReport> {
    call_inner(store, peer, call, key, false)
}
fn call_inner(
    store: &mut CheckpointStore,
    peer: &Endpoint,
    call: &OwnerCall,
    key: &PeerKey,
    guest: bool,
) -> Result<CallReport> {
    require(
        peer.address.ip().is_loopback()
            && peer.address.port() != 0
            && peer.owner == call.owner
            && peer.epoch == call.epoch,
        "peer endpoint/call scope mismatch",
    )?;
    let saved = store.load()?;
    let mut vm = saved.snapshot.restore()?;
    let records = journal(&vm)?;
    require(
        !guest
            || (records.contains_key(&call.operation)
                && crate::remote::owns(&vm, &call.operation)?),
        "guest remote call has no active owned intent",
    )?;
    let intent = Intent {
        format: 1,
        caller: vm.universe_id().to_string(),
        node: peer.node.to_string(),
        address: peer.address.to_string(),
        call: WireCall::from_call(call)?,
    };
    let bytes = json(&intent)?;
    let name = intent_key(&call.operation)?;
    if let Some(old) = records.get(&call.operation) {
        require(
            json(old)? == bytes,
            "peer operation ID is already bound to another call/endpoint",
        )?;
        if let Some(bytes) = root_bytes(&vm, &named("reply", &call.operation))? {
            let r: SavedReply = parse(&bytes)?;
            return Ok(CallReport::Owner(r.reply.reply()?));
        }
    } else {
        require(records.len() < MAX_CALLS, "peer call limit reached")?;
        require(
            matches!(
                vm.context().status,
                ExecutionStatus::Completed { .. } | ExecutionStatus::Faulted { .. }
            ),
            "caller universe is busy",
        )?;
        put(&mut vm, name.clone(), &bytes)?;
        store.save(&vm)?;
    }
    require(
        guest
            || matches!(
                vm.context().status,
                ExecutionStatus::Completed { .. } | ExecutionStatus::Faulted { .. }
            ),
        "caller universe is busy",
    )?;
    let sent = named("sent", &call.operation);
    let resume = vm.heap().roots().contains_key(&sent);
    let ackname = named("ack", &call.operation);
    let ack = root_bytes(&vm, &ackname)?
        .map(|b| parse::<Ack>(&b))
        .transpose()?;
    let request = Request {
        format: 1,
        node: peer.node.to_string(),
        call: intent.call.clone(),
        resume,
        expected_execution: ack.as_ref().map(|a| a.execution.clone()),
    };
    let payload = json(&request)?;
    let attempt = nonce()?;
    let mut stream = match TcpStream::connect_timeout(&peer.address, TIMEOUT) {
        Ok(s) => s,
        Err(_) => return Ok(CallReport::Unresolved),
    };
    if !resume {
        let object = vm.heap().roots()[&name];
        vm.set_root(sent, object)?;
        store.save(&vm)?;
    }
    let response = (|| -> Result<Response> {
        write_frame(
            &mut stream,
            key,
            REQUEST,
            vm.universe_id(),
            &attempt,
            &payload,
        )?;
        let deadline = Instant::now() + TIMEOUT;
        let (h, caller, len) = read_header(&mut stream, deadline)?;
        require(caller == vm.universe_id(), "peer reply principal mismatch")?;
        response_header(&h, vm.universe_id(), &attempt)?;
        let bytes = read_body(&mut stream, key, RESPONSE, &h, len, deadline)?;
        let response: Response = parse(&bytes)?;
        require(
            json(&response)? == bytes
                && response.format == 1
                && response.owner == peer.owner.to_string()
                && response.node == peer.node.to_string()
                && response.epoch == peer.epoch
                && response.request_hash == hash(&payload),
            "peer response binding mismatch",
        )?;
        Ok(response)
    })();
    let Some(reply) = response.ok().and_then(|r| r.reply) else {
        return Ok(CallReport::Unresolved);
    };
    reply.matches(call)?;
    if let Some(ack) = &ack {
        require(
            ack.execution == reply.execution,
            "peer response execution changed",
        )?;
    }
    let digest = hash(&bytes);
    if ack.is_none() {
        put(
            &mut vm,
            ackname,
            &json(&Ack {
                format: 1,
                intent_hash: digest.clone(),
                execution: reply.execution.clone(),
            })?,
        )?;
    }
    if reply.reply != State::Pending {
        put(
            &mut vm,
            named("reply", &call.operation),
            &json(&SavedReply {
                format: 1,
                intent_hash: digest,
                reply: reply.clone(),
            })?,
        )?;
    }
    let projected = reply.reply()?;
    if guest {
        crate::remote::complete(&mut vm, &projected)?;
    }
    store.save(&vm)?;
    Ok(CallReport::Owner(projected))
}
pub fn resume(
    store: &mut CheckpointStore,
    operation: &str,
    key: Option<&PeerKey>,
) -> Result<CallReport> {
    if let Some(CallReport::Owner(reply)) = inspect(store, operation)? {
        return Ok(CallReport::Owner(reply));
    };
    let saved = store.load()?;
    let vm = saved.snapshot.restore()?;
    let records = journal(&vm)?;
    let intent = records
        .get(operation)
        .ok_or_else(|| Error("peer call is not recorded".into()))?;
    let peer = endpoint(intent)?;
    let call = intent.call.call()?;
    call_once(
        store,
        &peer,
        &call,
        key.ok_or_else(|| Error("pending peer call requires host credentials".into()))?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_binds_direction_caller_nonce_length_and_exact_payload() {
        let key = PeerKey::from_bytes([7; 32]);
        let caller = UniverseId::fresh();
        let n = [3; 32];
        let h = header(caller, &n, 3).unwrap();
        let body = b"abc";
        let tag = mac(&key, REQUEST, &h, body).finalize().into_bytes();
        mac(&key, REQUEST, &h, body).verify_slice(&tag).unwrap();
        assert!(mac(&key, RESPONSE, &h, body).verify_slice(&tag).is_err());
        assert!(
            mac(&PeerKey::from_bytes([8; 32]), REQUEST, &h, body)
                .verify_slice(&tag)
                .is_err()
        );
        for offset in [8, 12, 28, 59] {
            let mut altered = h;
            altered[offset] ^= 1;
            assert!(
                mac(&key, REQUEST, &altered, body)
                    .verify_slice(&tag)
                    .is_err()
            );
        }
        assert!(mac(&key, REQUEST, &h, b"abd").verify_slice(&tag).is_err());
        response_header(&h, caller, &n).unwrap();
        assert!(response_header(&h, caller, &[4; 32]).is_err());
        assert!(response_header(&h, UniverseId::fresh(), &n).is_err());
    }
    #[test]
    fn header_length_is_bounded_before_body_allocation_and_truncation_rejects() {
        for variant in 0..3 {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                read_header(&mut stream, Instant::now() + Duration::from_millis(100))
            });
            let mut stream = TcpStream::connect(address).unwrap();
            let mut h = header(UniverseId::fresh(), &[0; 32], 0).unwrap();
            match variant {
                0 => h[8..12].copy_from_slice(&((MAX_FRAME + 1) as u32).to_le_bytes()),
                1 => h[0] = b'X',
                _ => {
                    stream.write_all(&h[..10]).unwrap();
                    drop(stream);
                    assert!(worker.join().unwrap().is_err());
                    continue;
                }
            }
            stream.write_all(&h).unwrap();
            assert!(worker.join().unwrap().is_err());
        }
    }
    #[test]
    fn authenticated_wire_still_rejects_unknown_shapes_and_noncanonical_i64() {
        let valid = OwnerCall {
            operation: "valid".into(),
            owner: UniverseId::fresh(),
            epoch: 1,
            grant: GrantId::fresh(),
            object: ObjectId::fresh(),
            method: MethodVersion {
                method: MethodId::fresh(),
                code: CodeVersionId::fresh(),
            },
            arguments: vec![Argument::I32(1)],
        };
        let request = Request {
            format: 1,
            node: NodeId::fresh().to_string(),
            call: WireCall::from_call(&valid).unwrap(),
            resume: false,
            expected_execution: None,
        };
        let mut bad = serde_json::to_value(&request).unwrap();
        bad["hostPath"] = "/tmp".into();
        assert!(parse::<Request>(&serde_json::to_vec(&bad).unwrap()).is_err());
        let value = WireValue::I64("+1".into());
        assert!(value.arg().is_err());
        assert!(WireValue::I64("-0".into()).arg().is_err());
        assert!(WireValue::I64("9223372036854775808".into()).arg().is_err());
        assert!(parse::<Request>(br#"{"format":1,"node":"00000000000000000000000000000000","call":{},"resume":true,"expectedExecution":null,"hostPath":"/tmp"}"#).is_err());
        let max = Argument::Text(vec![65535; 4096]);
        assert_eq!(WireValue::from_arg(&max).arg().unwrap(), max);
    }
}
