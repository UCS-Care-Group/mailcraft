// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A fake ManageSieve server (RFC 5804) for the tests: plain TCP on a loopback port, keeping the
//! scripts it is given and which of them is active.

use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use crate::fake::{Fake, pretend_tls, read_line};

/// How the fake server begins a connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Opening {
    /// With a ManageSieve greeting, as RFC 5804 says.
    #[default]
    Greets,
    /// With nothing, waiting for the client to go away (a listener behind a proxy that waits for
    /// a header of its own).
    Silent,
    /// With nothing, as a listener that expects TLS from the first byte: a TLS hello is answered
    /// with a TLS alert, anything else goes unanswered.
    ImplicitTls,
    /// With these bytes, and then nothing (another protocol's banner).
    Says(Vec<u8>),
    /// By closing it at once.
    Closes,
}

/// The fake server's state.
pub(crate) struct SieveWorld {
    /// How it begins a connection.
    pub(crate) opening: Opening,
    pub(crate) user: String,
    pub(crate) password: String,
    /// The access token that signs `user` in (OAUTHBEARER); empty for a server that takes none.
    pub(crate) token: String,
    /// Say STARTTLS is on offer (it isn't: what follows the go-ahead is no handshake).
    pub(crate) offer_starttls: bool,
    /// The Sieve extensions it says its scripts may use.
    pub(crate) extensions: String,
    /// The scripts by name, with their text.
    pub(crate) scripts: Vec<(String, String)>,
    pub(crate) active: Option<String>,
    /// A script with this in it is refused as having an error.
    pub(crate) refuse_scripts_with: Option<String>,
    /// What a refusal of a script says (several lines go as a literal).
    pub(crate) refusal: String,
    /// A script with the first in it is taken with the second as its warnings.
    pub(crate) warn_scripts_with: Option<(String, String)>,
    /// Say the VERSION of RFC 5804; a server from before it says none, and has no CHECKSCRIPT.
    pub(crate) version: bool,
    /// Send the client to another server instead of signing it in.
    pub(crate) referral: bool,
    /// Every line received; a literal is written `{n bytes}`.
    pub(crate) log: Vec<String>,
    pub(crate) connections: usize,
    pub(crate) sign_ins: usize,
}

impl Default for SieveWorld {
    fn default() -> Self {
        SieveWorld {
            opening: Opening::Greets,
            user: String::new(),
            password: String::new(),
            token: String::new(),
            offer_starttls: false,
            extensions: "fileinto reject envelope vacation imap4flags copy date relational include".into(),
            scripts: Vec::new(),
            active: None,
            refuse_scripts_with: None,
            refusal: "line 1: error: unknown command".into(),
            warn_scripts_with: None,
            version: true,
            referral: false,
            log: Vec::new(),
            connections: 0,
            sign_ins: 0,
        }
    }
}

impl SieveWorld {
    /// The text of the script `name`.
    pub(crate) fn script(&self, name: &str) -> Option<&str> {
        self.scripts.iter().find(|(n, _)| n == name).map(|(_, text)| text.as_str())
    }

    /// Whether a line starting with `command` was received.
    pub(crate) fn heard(&self, command: &str) -> bool {
        self.log.iter().any(|line| line.starts_with(command))
    }
}

pub(crate) type FakeSieve = Fake<SieveWorld>;

impl Fake<SieveWorld> {
    pub(crate) fn sieve(world: SieveWorld) -> FakeSieve {
        Fake::start(world, serve_sieve)
    }
}

/// A string as the server writes it: in quotes.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The server's words: in quotes, or as a literal when they run over several lines.
fn said(text: &str) -> String {
    if text.contains(['\r', '\n']) { format!("{{{}}}\r\n{text}", text.len()) } else { quoted(text) }
}

/// The arguments of a command line: quoted strings, bare words, and a literal at the end of the
/// line, read from `reader` (and the rest of the line after it).
fn arguments(mut rest: String, reader: &mut BufReader<TcpStream>, log: &mut Vec<String>) -> Option<Vec<String>> {
    let mut out = Vec::new();
    loop {
        let mut chars = rest.chars().peekable();
        let mut literal = None;
        while let Some(c) = chars.next() {
            match c {
                ' ' => {}
                '"' => {
                    let mut text = String::new();
                    while let Some(c) = chars.next() {
                        match c {
                            '\\' => text.extend(chars.next()),
                            '"' => break,
                            c => text.push(c),
                        }
                    }
                    out.push(text);
                }
                '{' => {
                    let count: String = chars.by_ref().take_while(|c| *c != '}').collect();
                    literal = Some(count.trim_end_matches('+').parse::<usize>().ok()?);
                }
                c => {
                    let mut word = String::from(c);
                    while let Some(c) = chars.next_if(|c| *c != ' ') {
                        word.push(c);
                    }
                    out.push(word);
                }
            }
        }
        let Some(n) = literal else { return Some(out) };
        let mut bytes = vec![0u8; n];
        reader.read_exact(&mut bytes).ok()?;
        log.push(format!("{{{n} bytes}}"));
        out.push(String::from_utf8_lossy(&bytes).into_owned());
        rest = read_line(reader)?;
    }
}

/// The user and token of an OAUTHBEARER response (`n,a=<user>,^Aauth=Bearer <token>^A^A`).
fn oauthbearer(payload: &str) -> (String, String) {
    let decoded = mailcraft_charset::base64::STANDARD.decode(payload).unwrap_or_default();
    let decoded = String::from_utf8_lossy(&decoded).into_owned();
    let parts: Vec<&str> = decoded.split('\u{1}').collect();
    match parts.as_slice() {
        [header, auth, "", ""] => {
            let user = header.strip_prefix("n,a=").and_then(|u| u.strip_suffix(',')).unwrap_or("");
            (user.replace("=2C", ",").replace("=3D", "="), auth.strip_prefix("auth=Bearer ").unwrap_or("").into())
        }
        _ => Default::default(),
    }
}

/// A connection that isn't begun with a greeting: as `opening` says, until the client goes away.
fn serve_otherwise(mut stream: TcpStream, opening: Opening) {
    if let Opening::Says(bytes) = &opening {
        let _ = stream.write_all(bytes);
    }
    if opening == Opening::Closes {
        return;
    }
    let mut buf = [0u8; 1024];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            return;
        }
        // A TLS record (a hello): refused with a TLS alert (fatal, handshake_failure), as an
        // encrypted listener that doesn't take it would.
        if opening == Opening::ImplicitTls && matches!(buf.get(..2), Some([0x16, 0x03])) {
            let _ = stream.write_all(&[0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x28]);
            return;
        }
    }
}

fn serve_sieve(stream: TcpStream, shared: Arc<Mutex<SieveWorld>>) {
    let opening = {
        let mut world = shared.lock().unwrap();
        world.connections += 1;
        world.opening.clone()
    };
    if opening != Opening::Greets {
        serve_otherwise(stream, opening);
        return;
    }
    let mut out = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let greeting = {
        let world = shared.lock().unwrap();
        let sasl = if world.token.is_empty() { "PLAIN" } else { "PLAIN OAUTHBEARER" };
        let starttls = if world.offer_starttls { "\"STARTTLS\"\r\n" } else { "" };
        let version = if world.version { "\"VERSION\" \"1.0\"\r\n" } else { "" };
        format!(
            "\"IMPLEMENTATION\" \"Fake Sieve\"\r\n\"SASL\" \"{sasl}\"\r\n\"SIEVE\" {}\r\n{starttls}{version}OK \"Ready.\"\r\n",
            quoted(&world.extensions)
        )
    };
    let _ = out.write_all(greeting.as_bytes());
    let mut signed_in = false;
    while let Some(line) = read_line(&mut reader) {
        let (command, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        let command = command.to_ascii_uppercase();
        let mut log = vec![line.clone()];
        let Some(args) = arguments(rest.to_string(), &mut reader, &mut log) else { return };
        let mut world = shared.lock().unwrap();
        world.log.append(&mut log);
        let arg = |n: usize| args.get(n).cloned().unwrap_or_default();
        let reply = match command.as_str() {
            "STARTTLS" if world.offer_starttls && !signed_in => {
                drop(world);
                pretend_tls(&mut out, &mut reader, "OK \"Begin TLS negotiation now.\"\r\n");
                return;
            }
            "AUTHENTICATE" if world.referral => {
                let _ = out.write_all(b"BYE (REFERRAL \"sieve://elsewhere.example.org\") \"Try Remote.\"\r\n");
                return;
            }
            "AUTHENTICATE" => {
                let mechanism = arg(0).to_ascii_uppercase();
                let payload = arg(1);
                let accepted = match mechanism.as_str() {
                    "PLAIN" => {
                        let decoded = mailcraft_charset::base64::STANDARD.decode(&payload).unwrap_or_default();
                        let decoded = String::from_utf8_lossy(&decoded).into_owned();
                        let parts: Vec<&str> = decoded.split('\0').collect();
                        if parts == ["", world.user.as_str(), world.password.as_str()] {
                            Ok(())
                        } else {
                            // A careless server: it repeats the password it was sent.
                            Err(format!("NO \"Authentication failed for password {}\"", parts.get(2).copied().unwrap_or("")))
                        }
                    }
                    "OAUTHBEARER" if !world.token.is_empty() => {
                        if oauthbearer(&payload) == (world.user.clone(), world.token.clone()) {
                            Ok(())
                        } else {
                            // The reasons, then the verdict after the client's ^A.
                            drop(world);
                            let _ = out.write_all(b"\"eyJzdGF0dXMiOiJpbnZhbGlkX3Rva2VuIn0=\"\r\n");
                            let Some(answer) = read_line(&mut reader) else { return };
                            shared.lock().unwrap().log.push(answer);
                            let _ = out.write_all(b"NO \"Authentication failed\"\r\n");
                            continue;
                        }
                    }
                    _ => Err("NO \"Unsupported mechanism\"".to_string()),
                };
                match accepted {
                    Ok(()) => {
                        signed_in = true;
                        world.sign_ins += 1;
                        "OK \"Logged in.\"".to_string()
                    }
                    Err(refusal) => refusal,
                }
            }
            "CAPABILITY" => format!("\"IMPLEMENTATION\" \"Fake Sieve\"\r\n\"SIEVE\" {}\r\n\"VERSION\" \"1.0\"\r\nOK", quoted(&world.extensions)),
            "LOGOUT" => {
                let _ = out.write_all(b"OK \"Logout completed.\"\r\n");
                return;
            }
            _ if !signed_in => "NO \"Sign in first\"".to_string(),
            "LISTSCRIPTS" => {
                let mut listing = String::new();
                for (name, _) in &world.scripts {
                    let active = if world.active.as_deref() == Some(name.as_str()) { " ACTIVE" } else { "" };
                    listing.push_str(&format!("{}{active}\r\n", quoted(name)));
                }
                format!("{listing}OK \"Listscripts completed.\"")
            }
            "GETSCRIPT" => match world.script(&arg(0)) {
                Some(text) => format!("{{{}}}\r\n{text}\r\nOK \"Getscript completed.\"", text.len()),
                None => "NO (NONEXISTENT) \"There is no script by that name\"".to_string(),
            },
            "CHECKSCRIPT" if !world.version => "NO \"Unknown command\"".to_string(),
            "PUTSCRIPT" | "CHECKSCRIPT" => {
                let (name, text) = if command == "PUTSCRIPT" { (arg(0), arg(1)) } else { (String::new(), arg(0)) };
                let warned = world.warn_scripts_with.as_ref().filter(|(with, _)| text.contains(with.as_str())).map(|(_, said)| said.clone());
                if world.refuse_scripts_with.as_ref().is_some_and(|bad| text.contains(bad.as_str())) {
                    format!("NO {}", said(&world.refusal))
                } else if let Some(warnings) = warned.filter(|_| command == "CHECKSCRIPT") {
                    format!("OK (WARNINGS) {}", said(&warnings))
                } else {
                    if command == "PUTSCRIPT" {
                        match world.scripts.iter_mut().find(|(n, _)| *n == name) {
                            Some((_, kept)) => *kept = text,
                            None => world.scripts.push((name, text)),
                        }
                    }
                    "OK".to_string()
                }
            }
            "SETACTIVE" => {
                let name = arg(0);
                if name.is_empty() {
                    world.active = None;
                    "OK".to_string()
                } else if world.script(&name).is_some() {
                    world.active = Some(name);
                    "OK".to_string()
                } else {
                    "NO (NONEXISTENT) \"There is no script by that name\"".to_string()
                }
            }
            "DELETESCRIPT" => {
                let name = arg(0);
                if world.active.as_deref() == Some(name.as_str()) {
                    "NO (ACTIVE) \"You may not delete an active script\"".to_string()
                } else {
                    world.scripts.retain(|(n, _)| *n != name);
                    "OK".to_string()
                }
            }
            "HAVESPACE" => "OK".to_string(),
            _ => "NO \"Unknown command\"".to_string(),
        };
        drop(world);
        let _ = out.write_all(format!("{reply}\r\n").as_bytes());
        let _ = out.flush();
    }
}
