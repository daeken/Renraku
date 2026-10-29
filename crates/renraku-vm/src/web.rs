//! A deliberately small localhost HTTP edge. Request ownership is the ordinary
//! invocation inbox: pinned CIL, immutable arguments, frames, and terminal result.
use crate::{
    checkpoint::CheckpointStore,
    error::{Error, Result, require},
    execution::ExecutionStatus,
    identity::{MethodId, UniverseId},
    invocation::{InvocationRecord, InvocationState},
    program::{Program, SlotType},
    runtime::Vm,
    value::Value,
};
use std::{
    collections::BTreeMap,
    fmt,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_HEADERS: usize = 8192;
const MAX_BODY: usize = 8192;
const MAX_RESPONSE: usize = 32768;

#[derive(Clone, Debug)]
pub struct WebRequest {
    pub id: String,
    pub method: String,
    pub target: String,
    pub body: String,
}
#[derive(Debug)]
pub struct WebError {
    pub status: u16,
    pub message: String,
}
impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.status, self.message)
    }
}
impl std::error::Error for WebError {}
impl From<Error> for WebError {
    fn from(e: Error) -> Self {
        fail(503, &e.0)
    }
}
type WebResult<T> = std::result::Result<T, WebError>;
fn fail(status: u16, message: &str) -> WebError {
    WebError {
        status,
        message: message.into(),
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebStage {
    Accepted,
    Paused,
    Completed,
}
#[derive(Clone, Debug)]
pub struct WebReply {
    pub status: u16,
    pub body: String,
    pub state: &'static str,
    pub execution: String,
    pub request_id: String,
    pub instructions: u64,
}
#[derive(Clone, Debug)]
pub struct WebHost {
    database: PathBuf,
    handler: String,
    universe: UniverseId,
}
fn validate_id(id: &str) -> WebResult<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(fail(
            400,
            "request ID must contain 1..64 ASCII letters, digits, dots, underscores, or hyphens",
        ));
    }
    Ok(())
}
fn validate_target(target: &str) -> WebResult<()> {
    if !target.starts_with('/')
        || target.len() > 1024
        || !target
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && b != b'#')
    {
        return Err(fail(
            400,
            "only bounded ASCII origin-form targets are supported",
        ));
    }
    Ok(())
}
fn handler(program: &Program, name: &str) -> Result<MethodId> {
    let id = program.entry(name)?;
    let method = program.method(id)?;
    let string = SlotType::Reference(program.find_type("System.String")?);
    require(
        !method.signature.instance
            && method.intrinsic.is_none()
            && method.signature.parameters == vec![string; 4]
            && method.signature.result == Some(string),
        "HTTP handler must be static string Handle(string id, string method, string target, string body)",
    )?;
    Ok(id)
}
fn string(vm: &Vm<'_>, value: Value) -> Result<String> {
    let Value::Reference(id) = value else {
        return Err(Error("HTTP value is not a string".into()));
    };
    let units = vm
        .heap()
        .object(id)?
        .string_units()
        .ok_or_else(|| Error("HTTP value is not a string".into()))?;
    String::from_utf16(units).map_err(|_| Error("HTTP text contains an unpaired surrogate".into()))
}
impl WebHost {
    pub fn open(database: impl AsRef<Path>, selector: &str) -> Result<Self> {
        require(
            database.as_ref().is_file(),
            "website checkpoint does not exist",
        )?;
        let mut store = CheckpointStore::open(database.as_ref())?;
        let saved = store.load()?;
        handler(&saved.snapshot.program, selector)?;
        let vm = saved.snapshot.restore()?;
        Ok(Self {
            database: database.as_ref().to_path_buf(),
            handler: selector.into(),
            universe: vm.universe_id(),
        })
    }
    pub fn universe(&self) -> UniverseId {
        self.universe
    }
    pub fn process(&self, request: &WebRequest, budget: u64) -> WebResult<WebReply> {
        self.process_with_observer(request, budget, |_| Ok(()))
    }
    pub fn process_with_observer(
        &self,
        request: &WebRequest,
        budget: u64,
        mut observer: impl FnMut(WebStage) -> Result<()>,
    ) -> WebResult<WebReply> {
        validate_id(&request.id)?;
        validate_target(&request.target)?;
        if !matches!(request.method.as_str(), "GET" | "POST") {
            return Err(fail(405, "only GET and POST are supported"));
        }
        if request.body.len() > MAX_BODY {
            return Err(fail(413, "request body exceeds 8192 bytes"));
        }
        if budget > 1_000_000 {
            return Err(fail(400, "guest instruction budget exceeds policy"));
        }
        let mut store = CheckpointStore::open(&self.database)?;
        let saved = store.load()?;
        let mut vm = saved.snapshot.restore()?;
        require(
            vm.universe_id() == self.universe,
            "website universe changed; reopen the host explicitly",
        )?;
        let request_id = format!("http-v1/{}/{}", self.universe, request.id);
        let texts = [&request.id, &request.method, &request.target, &request.body];
        let (entry, arguments) = if let Some(record) = vm.invocation(&request_id) {
            let entry = handler(vm.program, &record.method.method.to_string())?;
            require(
                record.method == vm.program.method(entry)?.binding && record.arguments.len() == 4,
                "invalid HTTP receipt binding",
            )?;
            for (value, expected) in record.arguments.iter().zip(texts) {
                if string(&vm, *value)? != *expected {
                    return Err(fail(
                        409,
                        "request ID is already bound to different method, target, or body",
                    ));
                }
            }
            (entry, record.arguments.clone())
        } else {
            if !matches!(
                vm.context().status,
                ExecutionStatus::Completed { .. } | ExecutionStatus::Faulted { .. }
            ) {
                return Err(fail(
                    503,
                    "another execution is active; finish its accepted request first",
                ));
            }
            let entry = handler(vm.program, &self.handler)?;
            let arguments = texts
                .into_iter()
                .map(|s| {
                    vm.new_string(s.encode_utf16().collect())
                        .map(Value::Reference)
                })
                .collect::<Result<Vec<_>>>()?;
            (entry, arguments)
        };
        let before = vm.instructions_executed();
        // Durable intent precedes every handler instruction, even at budget 0.
        let mut receipt =
            vm.invoke_once(&mut store, &request_id, entry, arguments.clone(), 0, 100)?;
        observer(WebStage::Accepted)?;
        if receipt.state == InvocationState::Running && budget > 0 {
            receipt = vm.invoke_once(&mut store, &request_id, entry, arguments, budget, 100)?;
        }
        let (status, body, state) = response(&vm, &receipt);
        observer(if receipt.state == InvocationState::Running {
            WebStage::Paused
        } else {
            WebStage::Completed
        })?;
        Ok(WebReply {
            status,
            body,
            state,
            execution: receipt.execution.to_string(),
            request_id,
            instructions: vm.instructions_executed() - before,
        })
    }
}
fn response(vm: &Vm<'_>, receipt: &InvocationRecord) -> (u16, String, &'static str) {
    match receipt.state {
        InvocationState::Running => (202,"Request is pending. Retry the identical request ID.".into(),"pending"),
        InvocationState::Faulted {..} => (500,"Guest handler faulted. Its saved receipt prevents repeating the call; inspect the checkpoint.".into(),"faulted"),
        InvocationState::Completed {result:Some(value)} => match string(vm,value) {
            Ok(body) if body.len()<=MAX_RESPONSE => (200,body,"completed"),
            _ => (500,"Guest response must be non-null UTF-8 text of at most 32768 bytes.".into(),"invalid_response"),
        },
        _ => (500,"Guest response must be a string.".into(),"invalid_response"),
    }
}

/// Serial, dedicated blocking host. No socket or host future enters a checkpoint.
pub struct WebServer {
    host: WebHost,
    listener: TcpListener,
    budget: u64,
}
impl WebServer {
    pub fn bind(host: WebHost, address: SocketAddr, budget: u64) -> Result<Self> {
        require(
            address.ip().is_loopback(),
            "website listener must bind to a loopback address",
        )?;
        require(
            budget <= 1_000_000,
            "guest instruction budget exceeds policy",
        )?;
        Ok(Self {
            host,
            listener: TcpListener::bind(address)?,
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
        observer: impl FnMut(WebStage) -> Result<()>,
    ) -> Result<()> {
        let (mut stream, _) = self.listener.accept()?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let parsed = read_request(&mut stream);
        match parsed {
            Ok((None, request))
                if request.method == "GET" && request.target == "/" && request.body.is_empty() =>
            {
                write_response(
                    &mut stream,
                    200,
                    &bootstrap(self.host.universe),
                    "text/html; charset=utf-8",
                    None,
                )?;
            }
            Ok((Some(id), mut request)) => {
                request.id = id;
                match self
                    .host
                    .process_with_observer(&request, self.budget, observer)
                {
                    Ok(reply) => write_response(
                        &mut stream,
                        reply.status,
                        &reply.body,
                        if reply.status == 200 {
                            "text/html; charset=utf-8"
                        } else {
                            "text/plain; charset=utf-8"
                        },
                        Some(&reply),
                    )?,
                    Err(error) => {
                        eprintln!("website: {error}");
                        write_response(
                            &mut stream,
                            error.status,
                            &error.message,
                            "text/plain; charset=utf-8",
                            None,
                        )?;
                    }
                }
            }
            Ok(_) => write_response(
                &mut stream,
                400,
                "X-Renraku-Request-Id is required for managed requests.",
                "text/plain; charset=utf-8",
                None,
            )?,
            Err(error) => write_response(
                &mut stream,
                error.status,
                &error.message,
                "text/plain; charset=utf-8",
                None,
            )?,
        }
        Ok(())
    }
}
fn read_request(stream: &mut TcpStream) -> WebResult<(Option<String>, WebRequest)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut read_more = |bytes: &mut Vec<u8>| -> WebResult<()> {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| fail(408, "request read deadline exceeded"))?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|_| fail(400, "cannot read request"))?;
        let count = stream
            .read(&mut chunk)
            .map_err(|_| fail(408, "request read interrupted or timed out"))?;
        if count == 0 {
            return Err(fail(400, "incomplete HTTP request"));
        }
        bytes.extend_from_slice(&chunk[..count]);
        Ok(())
    };
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
            break end + 4;
        }
        if bytes.len() > MAX_HEADERS {
            return Err(fail(431, "HTTP headers exceed 8192 bytes"));
        }
        read_more(&mut bytes)?;
    };
    if end > MAX_HEADERS {
        return Err(fail(431, "HTTP headers exceed 8192 bytes"));
    }
    if !bytes[..end].is_ascii() {
        return Err(fail(400, "HTTP headers must be ASCII"));
    }
    let head = std::str::from_utf8(&bytes[..end - 4]).unwrap();
    let mut lines = head.split("\r\n");
    let line = lines.next().unwrap_or_default();
    if line.len() > 2048 {
        return Err(fail(414, "request line exceeds limit"));
    }
    let parts: Vec<_> = line.split(' ').collect();
    if parts.len() != 3 || parts[2] != "HTTP/1.1" {
        return Err(fail(400, "expected a strict HTTP/1.1 request line"));
    }
    let method = parts[0].to_owned();
    let target = parts[1].to_owned();
    if !matches!(method.as_str(), "GET" | "POST") {
        return Err(fail(405, "only GET and POST are supported"));
    }
    validate_target(&target)?;
    let mut headers = BTreeMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| fail(400, "malformed HTTP header"))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || !value
                .bytes()
                .all(|b| b == b'\t' || (0x20..=0x7e).contains(&b))
        {
            return Err(fail(400, "invalid HTTP header syntax"));
        }
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err(fail(400, "duplicate HTTP headers are unsupported"));
        }
        if headers.len() > 32 {
            return Err(fail(431, "too many HTTP headers"));
        }
    }
    if headers.get("host").is_none_or(String::is_empty) {
        return Err(fail(400, "one Host header is required"));
    }
    if headers.contains_key("transfer-encoding") || headers.contains_key("content-encoding") {
        return Err(fail(
            400,
            "transfer/content codings are unsupported; use Content-Length and UTF-8",
        ));
    }
    if headers.contains_key("expect") {
        return Err(fail(417, "Expect is unsupported"));
    }
    let length = if let Some(length) = headers.get("content-length") {
        if length.is_empty() || !length.bytes().all(|b| b.is_ascii_digit()) {
            return Err(fail(400, "invalid Content-Length"));
        }
        length
            .parse::<usize>()
            .map_err(|_| fail(413, "request body exceeds limit"))?
    } else {
        0
    };
    if length > MAX_BODY {
        return Err(fail(413, "request body exceeds 8192 bytes"));
    }
    let id = headers.get("x-renraku-request-id").cloned();
    if let Some(id) = &id {
        validate_id(id)?;
    }
    while bytes.len() < end + length {
        read_more(&mut bytes)?;
    }
    if bytes.len() != end + length {
        return Err(fail(
            400,
            "pipelined or excess request bytes are unsupported",
        ));
    }
    let body = String::from_utf8(bytes[end..].to_vec())
        .map_err(|_| fail(400, "request body must be UTF-8"))?;
    Ok((
        id,
        WebRequest {
            id: String::new(),
            method,
            target,
            body,
        },
    ))
}
fn write_response(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
    content_type: &str,
    receipt: Option<&WebReply>,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Content Too Large",
        414 => "URI Too Long",
        417 => "Expectation Failed",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "Service Unavailable",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        body.len()
    )?;
    if let Some(receipt) = receipt {
        write!(
            stream,
            "X-Renraku-State: {}\r\nX-Renraku-Execution: {}\r\n\r\n",
            receipt.state, receipt.execution
        )?;
    } else {
        write!(stream, "\r\n")?;
    }
    stream.write_all(body.as_bytes())?;
    stream.flush()?;
    Ok(())
}
fn bootstrap(universe: UniverseId) -> String {
    include_str!("web_client.html").replace("@UNIVERSE@", &universe.to_string())
}
