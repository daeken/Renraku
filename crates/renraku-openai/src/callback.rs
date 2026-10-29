use crate::{Result, fail, require};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use url::Url;

pub(crate) fn equal(left: &str, right: &str) -> bool {
    left.as_bytes().ct_eq(right.as_bytes()).into()
}
pub(crate) struct Callback {
    pub code: String,
    pub client_id: Option<String>,
}
pub(crate) struct Listener {
    socket: TcpListener,
    port: u16,
}
impl Listener {
    pub fn bind() -> Result<Self> {
        let socket = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| fail("could not start the loopback sign-in listener"))?;
        socket
            .set_nonblocking(true)
            .map_err(|_| fail("could not configure the sign-in listener"))?;
        let port = socket
            .local_addr()
            .map_err(|_| fail("could not read the sign-in port"))?
            .port();
        Ok(Self { socket, port })
    }
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/auth/callback", self.port)
    }
    pub fn wait(self, state: &str, timeout: Duration) -> Result<Callback> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.socket.accept() {
                Ok((mut stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    if stream.set_nonblocking(false).is_err() {
                        continue;
                    }
                    let remaining = deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(2));
                    if remaining.is_zero() {
                        break;
                    }
                    let parsed = request(&mut stream, self.port, state, remaining);
                    match parsed {
                        Ok(parameters) => {
                            if parameters.contains_key("error") {
                                respond(&mut stream, false);
                                return Err(fail("ChatGPT sign-in was declined or failed"));
                            }
                            let code = parameters
                                .get("code")
                                .filter(|c| !c.is_empty() && c.len() <= 4096)
                                .cloned();
                            if let Some(code) = code {
                                respond(&mut stream, true);
                                return Ok(Callback {
                                    code,
                                    client_id: parameters.get("client_id").cloned(),
                                });
                            }
                            respond(&mut stream, false);
                        }
                        Err(_) => respond(&mut stream, false),
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return Err(fail("sign-in listener failed")),
            }
        }
        Err(fail("ChatGPT sign-in timed out"))
    }
}
fn request(
    stream: &mut TcpStream,
    port: u16,
    state: &str,
    timeout: Duration,
) -> Result<BTreeMap<String, String>> {
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|_| fail("callback read timeout failed"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|_| fail("callback write timeout failed"))?;
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    while !bytes.ends_with(b"\r\n\r\n") {
        require(
            Instant::now() < deadline && bytes.len() < 8192,
            "callback request exceeded its bounds",
        )?;
        let count = stream
            .read(&mut buffer)
            .map_err(|_| fail("could not read callback"))?;
        require(count > 0, "incomplete callback request")?;
        bytes.extend_from_slice(&buffer[..count]);
    }
    require(bytes.len() <= 8192, "callback request is too large")?;
    let request = std::str::from_utf8(&bytes).map_err(|_| fail("callback request is not UTF-8"))?;
    let mut lines = request.split("\r\n");
    let parts: Vec<_> = lines.next().unwrap_or("").split(' ').collect();
    require(
        parts.len() == 3
            && parts[0] == "GET"
            && matches!(parts[2], "HTTP/1.0" | "HTTP/1.1")
            && parts[1].starts_with("/auth/callback?"),
        "invalid callback method or path",
    )?;
    let hosts: Vec<_> = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim())
        .collect();
    require(
        hosts == [format!("127.0.0.1:{port}")],
        "callback host did not match",
    )?;
    let url = Url::parse(&format!("http://127.0.0.1:{port}{}", parts[1]))
        .map_err(|_| fail("invalid callback URL"))?;
    require(
        url.path() == "/auth/callback" && url.fragment().is_none(),
        "invalid callback path",
    )?;
    let mut parameters = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        require(
            parameters
                .insert(key.into_owned(), value.into_owned())
                .is_none(),
            "duplicate callback parameter",
        )?;
    }
    require(
        parameters
            .get("state")
            .is_some_and(|returned| equal(returned, state)),
        "callback state did not match",
    )?;
    require(
        !(parameters.contains_key("error") && parameters.contains_key("code")),
        "callback contains both code and error",
    )?;
    Ok(parameters)
}
fn respond(stream: &mut TcpStream, accepted: bool) {
    let (status, text) = if accepted {
        (
            "200 OK",
            "Return to Renraku to finish verifying your sign-in.",
        )
    } else {
        (
            "400 Bad Request",
            "Sign-in callback was not accepted. Return to Renraku.",
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{text}",
        text.len()
    );
    let _ = stream.write_all(response.as_bytes());
}
