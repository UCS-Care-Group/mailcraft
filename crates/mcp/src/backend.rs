// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where MCP tool calls end up: a control-channel method call.

use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

/// Something that answers control-channel methods (`engine.execute`, `mailbox.inspect`,
/// `ui.click`, `ui.screenshot`, …). See `mailcraft_ui_egui::control` for the full list.
pub trait Backend {
    /// Call one method. `Ok` carries the `result`, `Err` the error message.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String>;
    /// True when a real UI is attached (`ui.screenshot`, `ui.click`, dialogs… work).
    fn has_ui(&self) -> bool;
    /// Short human description ("headless", "connected to 127.0.0.1:7981").
    fn describe(&self) -> String;
}

/// Looks up the control key for an address (`HOST:PORT`).
type KeyFinder = Box<dyn Fn(&str) -> Result<String, String> + Send>;

/// A running MailCraft app, reached through its loopback control port.
pub struct Remote {
    addr: String,
    conn: Option<(BufReader<TcpStream>, TcpStream)>,
    next_id: u64,
    /// The connection leads to a loopback address: requests carry the control key.
    send_key: bool,
    /// The control key, looked up once per connection (the app picks a new one at every start).
    key: Option<String>,
    /// The app at the other end has proved it holds the key (`auth.challenge`): until it has,
    /// neither the key nor a request is sent.
    verified: bool,
    find_key: KeyFinder,
    /// What the owner allowed the agent in advance (`--allow`). Every request carries it in its
    /// `agent` field, set here and never from a tool's arguments: the app knows the request is
    /// an agent's, and asks the owner about the rest.
    allow: Vec<mailcraft_engine::agent::Class>,
}

/// The control key: `env` (`MAILCRAFT_CONTROL_KEY`) when set, else the app's key file for
/// `port` in `settings`. The error says where it looked.
fn lookup_key(env: Option<String>, settings: Option<&Path>, port: Option<u16>) -> Result<String, String> {
    if let Some(k) = env.map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
        return Ok(k);
    }
    let (Some(settings), Some(port)) = (settings, port) else {
        return Err("no control key: set MAILCRAFT_CONTROL_KEY (no settings folder or port to find the key file)".to_string());
    };
    let path = mailcraft_control_key::key_path(settings, port);
    match std::fs::read_to_string(&path).map(|k| k.trim().to_string()) {
        Ok(k) if !k.is_empty() => Ok(k),
        _ => Err(format!("no control key: set MAILCRAFT_CONTROL_KEY, or start the app with --control {port} (it writes {})", path.display())),
    }
}

/// The key for the app at `addr`, from the environment or the app's key file.
fn find_key(addr: &str) -> Result<String, String> {
    let port = addr.rsplit_once(':').and_then(|(_, p)| p.parse().ok());
    lookup_key(std::env::var("MAILCRAFT_CONTROL_KEY").ok(), mailcraft_control_key::settings_dir().as_deref(), port)
}

/// Whether `addr` (`HOST:PORT`, as given to `--connect`) names a loopback address: 127.0.0.0/8,
/// ::1 or `localhost`. Other host names are not resolved: they never count.
fn is_loopback_addr(addr: &str) -> bool {
    let loopback = |ip: IpAddr| ip.to_canonical().is_loopback();
    if let Ok(sa) = addr.parse::<SocketAddr>() {
        return loopback(sa.ip());
    }
    if let Ok(ip) = addr.parse::<IpAddr>() {
        return loopback(ip);
    }
    let host = addr.rsplit_once(':').map_or(addr, |(h, _)| h);
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    host.parse::<IpAddr>().map_or_else(|_| host.eq_ignore_ascii_case("localhost"), loopback)
}

/// The key goes only to a loopback `--connect` address, and only when the connection really
/// leads to one (`peer`, so a `localhost` that resolves elsewhere gets no key either).
fn may_send_key(addr: &str, peer: Option<SocketAddr>) -> bool {
    is_loopback_addr(addr) && peer.is_some_and(|p| p.ip().to_canonical().is_loopback())
}

/// Why the app could not be made to prove itself.
enum Auth {
    /// The connection failed: tried again on a new one.
    Io(std::io::Error),
    /// It answered, wrongly, or the key could not be read: nothing was sent.
    Refused(String),
}

impl Remote {
    /// Connect to `addr` (`127.0.0.1:7981`), failing fast when nothing is listening.
    pub fn connect(addr: &str) -> std::io::Result<Self> {
        Self::connect_with(addr, Box::new(find_key))
    }

    fn connect_with(addr: &str, find_key: KeyFinder) -> std::io::Result<Self> {
        let mut r = Self { addr: addr.to_string(), conn: None, next_id: 1, send_key: false, key: None, verified: false, find_key, allow: Vec::new() };
        r.reconnect()?;
        if let Some(w) = r.key_warning() {
            log::warn!("{w}");
        }
        Ok(r)
    }

    /// As [`Remote::connect`], with the control key given instead of looked up (for tests of the
    /// bridge from outside the crate).
    #[doc(hidden)]
    pub fn connect_with_key(addr: &str, key: &str) -> std::io::Result<Self> {
        let key = key.to_string();
        Self::connect_with(addr, Box::new(move |_| Ok(key.clone())))
    }

    /// What the owner allows the agent without being asked (`--allow`).
    pub fn with_allow(mut self, allow: Vec<mailcraft_engine::agent::Class>) -> Self {
        self.allow = allow;
        self
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Why requests go without the control key (the address is not loopback), if they do.
    pub fn key_warning(&self) -> Option<String> {
        (!self.send_key).then(|| {
            format!(
                "{} is not a loopback address, so the control key is not sent (it only goes to 127.0.0.0/8, ::1 and localhost) and the app refuses requests; reach the app through 127.0.0.1, for example with an SSH tunnel",
                self.addr
            )
        })
    }

    fn reconnect(&mut self) -> std::io::Result<()> {
        self.conn = None;
        self.key = None;
        self.verified = false;
        let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, format!("cannot resolve {}", self.addr));
        for sa in self.addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&sa, Duration::from_millis(800)) {
                Ok(s) => {
                    s.set_nodelay(true).ok();
                    // The app answers within 60 s (its own timeout); leave headroom.
                    s.set_read_timeout(Some(Duration::from_secs(90))).ok();
                    self.send_key = may_send_key(&self.addr, s.peer_addr().ok());
                    let read = s.try_clone()?;
                    self.conn = Some((BufReader::new(read), s));
                    return Ok(());
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// Make the program at the other end prove it holds the control key, before the key or any
    /// request is sent to it: it must answer HMAC-SHA256 over a fresh nonce, keyed with the key
    /// read from the environment or the key file (`mailcraft_control_key::proof`). Anything else
    /// that listens on the port, started before the app or instead of it, cannot. A stale key (the
    /// app restarted on this port) is read again once, on a new connection.
    fn authenticate(&mut self) -> Result<(), Auth> {
        for attempt in 0..2 {
            let key = (self.find_key)(&self.addr).map_err(Auth::Refused)?;
            let nonce = mailcraft_control_key::proof::new_nonce().ok_or_else(|| Auth::Refused("no random bytes for the challenge".to_string()))?;
            let line = json!({"id": 0, "method": "auth.challenge", "params": {"nonce": nonce}}).to_string();
            let reply = self.roundtrip(&line).map_err(Auth::Io)?;
            let v: Value = serde_json::from_str(reply.trim()).unwrap_or(Value::Null);
            let proof = v.pointer("/result/proof").and_then(Value::as_str).unwrap_or("");
            if v.get("ok").and_then(Value::as_bool) == Some(true) && mailcraft_control_key::proof::proof_ok(&key, &nonce, proof) {
                self.key = Some(key);
                self.verified = true;
                return Ok(());
            }
            if attempt == 0 {
                self.reconnect().map_err(Auth::Io)?;
            }
        }
        self.conn = None;
        Err(Auth::Refused(format!(
            "the program at {} did not prove it is the MailCraft app that wrote the control key, so neither the key nor any request was sent to it (an older MailCraft that cannot answer the challenge, or another program on that port)",
            self.addr
        )))
    }

    /// The request line; it carries the key when this connection may (the app has proved it holds
    /// it: [`Remote::authenticate`]).
    fn request_line(&mut self, id: u64, method: &str, params: &Value) -> Result<String, String> {
        // Marked as an agent's (`docs/control-protocol.md`, Agents).
        let allow: Vec<&str> = self.allow.iter().map(|c| c.as_str()).collect();
        let mut req = json!({"id": id, "method": method, "params": params, "agent": {"allow": allow}});
        if self.send_key {
            let Some(key) = &self.key else { return Err("the control key was not read".to_string()) };
            req["key"] = json!(key);
        }
        Ok(req.to_string())
    }

    fn roundtrip(&mut self, line: &str) -> std::io::Result<String> {
        let Some((reader, writer)) = self.conn.as_mut() else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "not connected"));
        };
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        let mut reply = String::new();
        if reader.read_line(&mut reply)? == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "control channel closed"));
        }
        Ok(reply)
    }

    /// One request and its reply, with one retry on a new connection (the app may have
    /// restarted; a new connection also reads the key again).
    fn exchange(&mut self, id: u64, method: &str, params: &Value) -> Result<Value, String> {
        let unreachable = |addr: &str, e: std::io::Error| format!("MailCraft app at {addr} is not reachable: {e}");
        let mut first = true;
        let reply = loop {
            let tried = if self.conn.is_some() { Ok(()) } else { self.reconnect() };
            let tried = match tried {
                Ok(()) if self.send_key && !self.verified => match self.authenticate() {
                    Ok(()) => Ok(()),
                    Err(Auth::Io(e)) => Err(e),
                    Err(Auth::Refused(why)) => return Err(why),
                },
                other => other,
            };
            let result = match tried {
                Ok(()) => {
                    let line = self.request_line(id, method, params)?;
                    self.roundtrip(&line)
                }
                Err(e) => Err(e),
            };
            match result {
                Ok(r) => break r,
                Err(e) => {
                    self.conn = None;
                    if !first {
                        return Err(unreachable(&self.addr, e));
                    }
                    first = false;
                }
            }
        };
        serde_json::from_str(reply.trim()).map_err(|e| format!("bad reply from app: {e}"))
    }
}

impl Backend for Remote {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let unauthorized = |v: &Value| v.get("error").and_then(Value::as_str) == Some("unauthorized");
        let mut v = self.exchange(id, method, &params)?;
        if unauthorized(&v) && self.send_key {
            // A stale key (the app restarted on this port): the app closed the connection; a new
            // one reads the key again. Once.
            self.conn = None;
            v = self.exchange(id, method, &params)?;
        }
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else if unauthorized(&v) {
            self.conn = None;
            Err(self.key_warning().map_or_else(
                || {
                    format!(
                        "unauthorized: the MailCraft app at {} refused the control key (from MAILCRAFT_CONTROL_KEY or the app's key file)",
                        self.addr
                    )
                },
                |w| format!("unauthorized: {w}"),
            ))
        } else {
            Err(v.get("error").and_then(Value::as_str).unwrap_or("unknown error").to_string())
        }
    }

    fn has_ui(&self) -> bool {
        true
    }

    fn describe(&self) -> String {
        format!("connected to the MailCraft app at {}", self.addr)
    }
}

#[cfg(test)]
#[path = "backend_tests.rs"]
mod backend_tests;
