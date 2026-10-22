// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fake IMAP, SMTP and HTTP servers for the tests: plain TCP on a loopback port, speaking just
//! enough of each protocol, with state the tests set up and look at.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine as _;

/// A fake web server (a token endpoint, a name server that answers over HTTP): each request is
/// answered with bytes of the test's choosing, and kept for the test to look at.
pub(crate) struct FakeHttp {
    pub(crate) port: u16,
    requests: Arc<Mutex<Vec<String>>>,
}

impl FakeHttp {
    /// Answer request number `n` (from 0) with whatever `answer` makes of it. The request is the
    /// request line, the headers, an empty line and the body, as they came.
    pub(crate) fn start(answer: impl Fn(usize, &str) -> Vec<u8> + Send + 'static) -> FakeHttp {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let kept = requests.clone();
        std::thread::spawn(move || {
            for (n, mut stream) in listener.incoming().flatten().enumerate() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let (mut request, mut length) = (String::new(), 0usize);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    request.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                request.push_str(&String::from_utf8(body).unwrap());
                let answer = answer(n, &request);
                kept.lock().unwrap().push(request);
                let _ = stream.write_all(&answer);
            }
        });
        FakeHttp { port, requests }
    }

    /// Answer every request with the same bytes.
    pub(crate) fn answering(answer: Vec<u8>) -> FakeHttp {
        FakeHttp::start(move |_, _| answer.clone())
    }

    /// `http://127.0.0.1:{port}{path}`.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// How many requests have come.
    pub(crate) fn asked(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// Request number `n`.
    pub(crate) fn request(&self, n: usize) -> String {
        self.requests.lock().unwrap()[n].clone()
    }
}

/// What a [`FakeKeptHttp`] does with a request.
pub(crate) enum Kept {
    /// Write these bytes and wait for the next request on the same connection.
    Answer(Vec<u8>),
    /// Write these bytes and close the connection, saying nothing of it.
    AnswerAndClose(Vec<u8>),
    /// Close the connection without a word of an answer.
    Hang,
}

/// A fake web server that keeps its connections open: every connection has a thread of its
/// own and is read request after request. It counts the connections it took.
pub(crate) struct FakeKeptHttp {
    pub(crate) port: u16,
    connections: Arc<Mutex<usize>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl FakeKeptHttp {
    /// Answer request number `n` (from 0, over all connections) as `answer` says, given the
    /// number of the connection it came on (from 0) and the request as it came.
    pub(crate) fn start(answer: impl Fn(usize, usize, &str) -> Kept + Send + Sync + 'static) -> FakeKeptHttp {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (connections, requests) = (Arc::new(Mutex::new(0usize)), Arc::new(Mutex::new(Vec::new())));
        let (counted, kept) = (connections.clone(), requests.clone());
        let answer = Arc::new(answer);
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let connection = {
                    let mut count = counted.lock().unwrap();
                    *count += 1;
                    *count - 1
                };
                let (kept, answer) = (kept.clone(), answer.clone());
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    loop {
                        let Some(request) = read_one(&mut reader) else { return };
                        let n = {
                            let mut all = kept.lock().unwrap();
                            all.push(request.clone());
                            all.len() - 1
                        };
                        match answer(n, connection, &request) {
                            Kept::Answer(bytes) => {
                                if stream.write_all(&bytes).is_err() {
                                    return;
                                }
                            }
                            Kept::AnswerAndClose(bytes) => {
                                let _ = stream.write_all(&bytes);
                                return;
                            }
                            Kept::Hang => return,
                        }
                    }
                });
            }
        });
        FakeKeptHttp { port, connections, requests }
    }

    /// `http://127.0.0.1:{port}{path}`.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// How many connections it took.
    pub(crate) fn connections(&self) -> usize {
        *self.connections.lock().unwrap()
    }

    /// How many requests have come.
    pub(crate) fn asked(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// Request number `n`.
    pub(crate) fn request(&self, n: usize) -> String {
        self.requests.lock().unwrap()[n].clone()
    }
}

/// One request off a connection: its head, an empty line and its body; `None` once the
/// client has gone.
fn read_one(reader: &mut BufReader<TcpStream>) -> Option<String> {
    let (mut request, mut length) = (String::new(), 0usize);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return None;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
        }
        request.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    request.push_str(&String::from_utf8(body).unwrap());
    Some(request)
}

/// An HTTP answer with a JSON body and its length, that leaves the connection open.
pub(crate) fn kept_answer(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

/// An HTTP answer with a JSON body and its length.
pub(crate) fn json_answer(status: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        .into_bytes()
}

#[derive(Clone)]
pub(crate) struct FakeMessage {
    pub(crate) uid: u32,
    /// As the server writes them: `\Seen \Flagged`.
    pub(crate) flags: String,
    pub(crate) internal_date: String,
    pub(crate) body: Vec<u8>,
    /// The size to report instead of the true one.
    pub(crate) claimed_size: Option<u64>,
}

impl FakeMessage {
    pub(crate) fn has(&self, flag: &str) -> bool {
        self.flags.split_whitespace().any(|f| f.eq_ignore_ascii_case(flag))
    }

    fn set(&mut self, flag: &str, on: bool) {
        let mut flags: Vec<&str> = self.flags.split_whitespace().filter(|f| !f.eq_ignore_ascii_case(flag)).collect();
        if on {
            flags.push(flag);
        }
        self.flags = flags.join(" ");
    }
}

pub(crate) struct FakeMailbox {
    pub(crate) raw: String,
    pub(crate) flags: String,
    pub(crate) uid_validity: u32,
    pub(crate) messages: Vec<FakeMessage>,
    /// Answer NO when it is opened.
    pub(crate) refuse_open: bool,
    /// The UID the next message gets; 0 for one above the highest there is.
    pub(crate) next_uid: u32,
}

impl FakeMailbox {
    /// A UID no message of the mailbox ever had.
    fn take_uid(&mut self) -> u32 {
        self.remember_uids();
        let uid = self.next_uid;
        self.next_uid = uid + 1;
        uid
    }

    /// Note how far the UIDs have got, before messages leave: a UID is never given twice.
    fn remember_uids(&mut self) {
        self.next_uid = self.next_uid.max(self.messages.iter().map(|m| m.uid).max().unwrap_or(0) + 1);
    }

    /// Take out the messages `gone` picks.
    fn remove(&mut self, gone: impl Fn(&FakeMessage) -> bool) {
        self.remember_uids();
        self.messages.retain(|m| !gone(m));
    }

    pub(crate) fn uids(&self) -> Vec<u32> {
        self.messages.iter().map(|m| m.uid).collect()
    }
}

#[derive(Default)]
pub(crate) struct ImapWorld {
    pub(crate) user: String,
    pub(crate) password: String,
    /// The access token that signs `user` in (XOAUTH2); empty for a server that takes none.
    pub(crate) token: String,
    /// Say the token may come with the command (SASL-IR).
    pub(crate) sasl_ir: bool,
    pub(crate) mailboxes: Vec<FakeMailbox>,
    /// Say STARTTLS is on offer (it isn't: what follows the go-ahead is no handshake).
    pub(crate) offer_starttls: bool,
    /// Know UID MOVE (RFC 6851), and say so.
    pub(crate) can_move: bool,
    /// Know UID EXPUNGE and tell where copied and appended messages went (RFC 4315), and say so.
    pub(crate) uidplus: bool,
    /// What separates the levels of a mailbox name; `/` when not set.
    pub(crate) delimiter: Option<char>,
    /// Answer NO to the download of this UID.
    pub(crate) refuse_uid: Option<u32>,
    /// Hang up instead of answering the download of this UID.
    pub(crate) hang_up_at_uid: Option<u32>,
    /// Answer NO to every command, once signed in, whose line has one of these in it.
    pub(crate) refuse: Vec<String>,
    /// Hang up instead of answering a command whose line has this in it.
    pub(crate) hang_up_on: Option<String>,
    /// Hang up half way through the download of this UID.
    pub(crate) cut_at_uid: Option<u32>,
    /// Say nothing more, and keep the connection open, from a command whose line has this in it.
    pub(crate) stall_on: Option<String>,
    /// Answer a command whose line has this in it one byte at a time, with a pause after each.
    pub(crate) drip_on: Option<(String, Duration)>,
    /// What to say, once signed in, in place of the truth to a command whose line has the first
    /// text in it: the second, as it stands, but for `{tag}`, which is the command's tag.
    pub(crate) answers: Vec<(String, String)>,
    /// What to say in place of the truth about where copied messages went (the text of the `COPYUID` code).
    pub(crate) copyuid: Option<String>,
    /// Behave as Gmail does, and say so (`X-GM-EXT-1`): a mailbox is a label, and the `\All`
    /// mailbox has every message that is not in `\Trash`. A message is known by its bytes. A move
    /// to `\All` only takes the label away, and puts no second copy there; a move out of `\All`
    /// adds a label and leaves the message in it; a move to `\Trash` takes the message out of
    /// every other mailbox; and a message expunged from a label is still in `\All`.
    pub(crate) gmail: bool,
    /// The mailboxes subscribed to.
    pub(crate) subscribed: Vec<String>,
    /// Every line received.
    pub(crate) log: Vec<String>,
    pub(crate) connections: usize,
}

impl ImapWorld {
    /// What the server says it can do.
    fn capabilities(&self) -> String {
        let xoauth2 = if self.token.is_empty() { "" } else { " AUTH=XOAUTH2" };
        let sasl_ir = if self.sasl_ir { " SASL-IR" } else { "" };
        let starttls = if self.offer_starttls { " STARTTLS" } else { "" };
        let can_move = if self.can_move { " MOVE" } else { "" };
        let uidplus = if self.uidplus { " UIDPLUS" } else { "" };
        let gmail = if self.gmail { " X-GM-EXT-1" } else { "" };
        format!("IMAP4rev1 SPECIAL-USE{xoauth2}{sasl_ir}{starttls}{can_move}{uidplus}{gmail}")
    }

    pub(crate) fn mailbox(&mut self, raw: &str) -> &mut FakeMailbox {
        self.mailboxes.iter_mut().find(|m| m.raw == raw).unwrap()
    }

    /// How many commands had `text` in them.
    pub(crate) fn sent(&self, text: &str) -> usize {
        self.log.iter().filter(|line| line.contains(text)).count()
    }
}

/// A message the fake SMTP server accepted.
pub(crate) struct Delivery {
    pub(crate) from: String,
    pub(crate) recipients: Vec<String>,
    /// The message as it was before dot-stuffing.
    pub(crate) data: String,
}

#[derive(Default)]
pub(crate) struct SmtpWorld {
    pub(crate) user: String,
    pub(crate) password: String,
    /// Offer AUTH PLAIN and LOGIN, and insist on a sign-in before taking mail.
    pub(crate) auth: bool,
    /// The access token that signs `user` in (XOAUTH2, insisted on likewise); empty for a server that takes none.
    pub(crate) token: String,
    /// Say STARTTLS is on offer (it isn't: what follows the go-ahead is no handshake).
    pub(crate) offer_starttls: bool,
    /// Recipients to answer 550 for.
    pub(crate) refuse: Vec<String>,
    pub(crate) delivered: Vec<Delivery>,
    /// Every line received (not the messages).
    pub(crate) log: Vec<String>,
    pub(crate) connections: usize,
    pub(crate) sign_ins: usize,
}

/// A fake server and its state.
pub(crate) struct Fake<W> {
    pub(crate) port: u16,
    world: Arc<Mutex<W>>,
}

pub(crate) type FakeImap = Fake<ImapWorld>;
pub(crate) type FakeSmtp = Fake<SmtpWorld>;

impl<W: Send + 'static> Fake<W> {
    /// Listen on a free loopback port and serve every connection on its own thread.
    fn start(world: W, serve: fn(TcpStream, Arc<Mutex<W>>)) -> Fake<W> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let world = Arc::new(Mutex::new(world));
        let shared = world.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shared = shared.clone();
                std::thread::spawn(move || serve(stream, shared));
            }
        });
        Fake { port, world }
    }

    /// The server's state, to set up or to look at.
    pub(crate) fn world(&self) -> MutexGuard<'_, W> {
        self.world.lock().unwrap()
    }
}

impl Fake<ImapWorld> {
    pub(crate) fn imap(world: ImapWorld) -> FakeImap {
        Fake::start(world, serve_imap)
    }
}

impl Fake<SmtpWorld> {
    pub(crate) fn smtp(world: SmtpWorld) -> FakeSmtp {
        Fake::start(world, serve_smtp)
    }
}

/// The next line without its line ending; `None` when the client is gone.
fn read_line(reader: &mut BufReader<TcpStream>) -> Option<String> {
    let mut line = Vec::new();
    match reader.read_until(b'\n', &mut line) {
        Ok(n) if n > 0 => Some(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_string()),
        _ => None,
    }
}

/// Agree to STARTTLS, then answer the client's handshake with something that isn't one.
fn pretend_tls(out: &mut TcpStream, reader: &mut BufReader<TcpStream>, go_ahead: &str) {
    let _ = out.write_all(go_ahead.as_bytes());
    let _ = reader.fill_buf();
    let _ = out.write_all(b"this is no handshake\r\n");
}

/// What a server writes in place of its reasons for refusing a token: `{"status":"401"}`.
const REASONS: &str = "eyJzdGF0dXMiOiI0MDEifQ==";

/// The user and the token of an XOAUTH2 response (`user=…^Aauth=Bearer …^A^A` in base64); empty when it isn't one.
fn bearer(payload: &str) -> (String, String) {
    let decoded = base64::engine::general_purpose::STANDARD.decode(payload).unwrap_or_default();
    let decoded = String::from_utf8_lossy(&decoded).into_owned();
    let parts: Vec<&str> = decoded.split('\u{1}').collect();
    match parts.as_slice() {
        [user, auth, "", ""] => (user.strip_prefix("user=").unwrap_or("").into(), auth.strip_prefix("auth=Bearer ").unwrap_or("").into()),
        _ => Default::default(),
    }
}

/// The arguments of a command: quoted strings (with their escapes undone) and bare words.
fn words(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = args.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' => {}
            '"' => {
                let mut word = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => word.extend(chars.next()),
                        c => word.push(c),
                    }
                }
                out.push(word);
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
    out
}

/// Whether `uid` is in a set like `1:*` or `5,9,12:20`.
fn in_set(set: &str, uid: u32) -> bool {
    let number = |s: &str| if s == "*" { u32::MAX } else { s.parse().unwrap_or(0) };
    set.split(',').any(|range| {
        let (a, b) = range.split_once(':').unwrap_or((range, range));
        (number(a).min(number(b))..=number(a).max(number(b))).contains(&uid)
    })
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
}

fn serve_imap(stream: TcpStream, shared: Arc<Mutex<ImapWorld>>) {
    let mut out = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let greeting = {
        let mut world = shared.lock().unwrap();
        world.connections += 1;
        // STARTTLS is only told to those who ask.
        format!("* OK [CAPABILITY {}] fake ready\r\n", world.capabilities().replace(" STARTTLS", ""))
    };
    let _ = out.write_all(greeting.as_bytes());
    let mut signed_in = false;
    let mut open: Option<usize> = None;
    // The open mailbox was opened to be looked at only (EXAMINE).
    let mut read_only = true;
    while let Some(line) = read_line(&mut reader) {
        let mut world = shared.lock().unwrap();
        world.log.push(line.clone());
        let (tag, rest) = line.split_once(' ').unwrap_or((&line, ""));
        let (verb, args) = rest.split_once(' ').unwrap_or((rest, ""));
        let mut reply = Vec::new();
        if world.hang_up_on.as_ref().is_some_and(|text| line.contains(text.as_str())) {
            return;
        }
        if world.stall_on.as_ref().is_some_and(|text| line.contains(text.as_str())) {
            drop(world);
            // Not another word, for as long as the client keeps the connection.
            while read_line(&mut reader).is_some() {}
            return;
        }
        let drip = world.drip_on.as_ref().filter(|(text, _)| line.contains(text.as_str())).map(|(_, pause)| *pause);
        let scripted =
            world.answers.iter().find(|(text, _)| signed_in && line.contains(text.as_str())).map(|(_, answer)| answer.replace("{tag}", tag));
        if let Some(answer) = scripted {
            drop(world);
            if out.write_all(answer.as_bytes()).is_err() {
                return;
            }
            continue;
        }
        if signed_in && world.refuse.iter().any(|text| line.contains(text.as_str())) {
            drop(world);
            if out.write_all(format!("{tag} NO [SERVERBUG] that can't be done\r\n").as_bytes()).is_err() {
                return;
            }
            continue;
        }
        match verb.to_ascii_uppercase().as_str() {
            "CAPABILITY" => reply.extend(format!("* CAPABILITY {}\r\n{tag} OK done\r\n", world.capabilities()).bytes()),
            "STARTTLS" if world.offer_starttls => {
                drop(world);
                pretend_tls(&mut out, &mut reader, &format!("{tag} OK begin TLS now\r\n"));
                return;
            }
            "LOGIN" => {
                let given = words(args);
                if given == [world.user.clone(), world.password.clone()] {
                    signed_in = true;
                    reply.extend(format!("{tag} OK welcome\r\n").bytes());
                } else {
                    // A careless server: it repeats the password it was sent.
                    let password = given.get(1).cloned().unwrap_or_default();
                    reply.extend(format!("{tag} NO [AUTHENTICATIONFAILED] no user with the password {password}\r\n").bytes());
                }
            }
            "AUTHENTICATE" => {
                let (mechanism, with_command) = args.split_once(' ').unwrap_or((args, ""));
                if world.token.is_empty() || !mechanism.eq_ignore_ascii_case("XOAUTH2") {
                    reply.extend(format!("{tag} NO [CANNOT] no such way to sign in\r\n").bytes());
                } else {
                    let (user, token) = (world.user.clone(), world.token.clone());
                    // The rest of the exchange is read without holding the state.
                    drop(world);
                    let payload = if with_command.is_empty() {
                        let _ = out.write_all(b"+ \r\n");
                        let Some(line) = read_line(&mut reader) else { return };
                        shared.lock().unwrap().log.push(line.clone());
                        line
                    } else {
                        with_command.to_string()
                    };
                    let (given_user, given_token) = bearer(&payload);
                    if (given_user, &given_token) == (user, &token) {
                        signed_in = true;
                        let _ = out.write_all(format!("{tag} OK welcome\r\n").as_bytes());
                    } else {
                        // The reasons, and after the client's empty line the verdict. A careless
                        // server: it repeats the token it was sent.
                        let _ = out.write_all(format!("+ {REASONS}\r\n").as_bytes());
                        let Some(line) = read_line(&mut reader) else { return };
                        shared.lock().unwrap().log.push(line);
                        let _ = out.write_all(format!("{tag} NO [AUTHENTICATIONFAILED] no user with the token {given_token}\r\n").as_bytes());
                    }
                    continue;
                }
            }
            "LOGOUT" => {
                let _ = out.write_all(format!("* BYE see you\r\n{tag} OK done\r\n").as_bytes());
                return;
            }
            _ if !signed_in => reply.extend(format!("{tag} BAD sign in first\r\n").bytes()),
            "LIST" => {
                let delimiter = world.delimiter.unwrap_or('/');
                for mailbox in &world.mailboxes {
                    reply.extend(format!("* LIST ({}) \"{delimiter}\" {}\r\n", mailbox.flags, quote(&mailbox.raw)).bytes());
                }
                reply.extend(format!("{tag} OK done\r\n").bytes());
            }
            "EXAMINE" | "SELECT" => {
                let name = words(args).into_iter().next().unwrap_or_default();
                open = world.mailboxes.iter().position(|m| m.raw == name && !m.refuse_open && !m.flags.contains("\\Noselect"));
                read_only = verb.eq_ignore_ascii_case("EXAMINE");
                match open.and_then(|i| world.mailboxes.get(i)) {
                    Some(mailbox) => {
                        let next = mailbox.next_uid.max(mailbox.messages.iter().map(|m| m.uid).max().unwrap_or(0) + 1);
                        reply.extend(
                            format!(
                                "* {} EXISTS\r\n* 0 RECENT\r\n* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)\r\n",
                                mailbox.messages.len()
                            )
                            .bytes(),
                        );
                        reply.extend(format!("* OK [UIDVALIDITY {}] UIDs valid\r\n* OK [UIDNEXT {next}] next\r\n", mailbox.uid_validity).bytes());
                        reply.extend(format!("{tag} OK [{}] done\r\n", if read_only { "READ-ONLY" } else { "READ-WRITE" }).bytes());
                    }
                    None => reply.extend(format!("{tag} NO [NONEXISTENT] no such mailbox\r\n").bytes()),
                }
            }
            "CREATE" => {
                let name = words(args).into_iter().next().unwrap_or_default();
                if name.is_empty() || world.mailboxes.iter().any(|m| m.raw.eq_ignore_ascii_case(&name)) {
                    reply.extend(format!("{tag} NO [ALREADYEXISTS] there is such a mailbox\r\n").bytes());
                } else {
                    let uid_validity = 900 + world.mailboxes.len() as u32;
                    world.mailboxes.push(FakeMailbox {
                        raw: name,
                        flags: "\\HasNoChildren".into(),
                        uid_validity,
                        messages: Vec::new(),
                        refuse_open: false,
                        next_uid: 0,
                    });
                    reply.extend(format!("{tag} OK made\r\n").bytes());
                }
            }
            "SUBSCRIBE" => {
                let name = words(args).into_iter().next().unwrap_or_default();
                world.subscribed.push(name);
                reply.extend(format!("{tag} OK done\r\n").bytes());
            }
            "APPEND" => {
                // `"name" (flags) {size}`: the message follows the go-ahead.
                let name = words(args).into_iter().next().unwrap_or_default();
                let flags = args.split_once('(').and_then(|(_, rest)| rest.split_once(')')).map_or("", |(flags, _)| flags).to_string();
                let size: usize = args.rsplit_once('{').and_then(|(_, n)| n.trim_end_matches('}').parse().ok()).unwrap_or(0);
                if !world.mailboxes.iter().any(|m| m.raw == name) {
                    reply.extend(format!("{tag} NO [TRYCREATE] no such mailbox\r\n").bytes());
                } else {
                    let uidplus = world.uidplus;
                    // The message is read without holding the state.
                    drop(world);
                    let _ = out.write_all(b"+ go ahead\r\n");
                    let mut body = vec![0u8; size];
                    if reader.read_exact(&mut body).is_err() || read_line(&mut reader).is_none() {
                        return;
                    }
                    let mut world = shared.lock().unwrap();
                    let mailbox = world.mailbox(&name);
                    let uid = mailbox.take_uid();
                    let code = if uidplus { format!("[APPENDUID {} {uid}] ", mailbox.uid_validity) } else { String::new() };
                    mailbox.messages.push(FakeMessage { uid, flags, internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body, claimed_size: None });
                    drop(world);
                    let _ = out.write_all(format!("{tag} OK {code}kept\r\n").as_bytes());
                    continue;
                }
            }
            // The one that takes every message marked as deleted, whoever marked it.
            "EXPUNGE" => match open.filter(|_| !read_only).and_then(|i| world.mailboxes.get_mut(i)) {
                Some(mailbox) => {
                    mailbox.remove(|m| m.has("\\Deleted"));
                    reply.extend(format!("{tag} OK gone\r\n").bytes());
                }
                None => reply.extend(format!("{tag} NO [READ-ONLY] nothing is open to be changed\r\n").bytes()),
            },
            "UID" if !args.to_ascii_uppercase().starts_with("FETCH ") => {
                let mut parts = args.splitn(3, ' ');
                let (what, set, rest) = (parts.next().unwrap_or("").to_ascii_uppercase(), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                let (can_move, uidplus, copyuid) = (world.can_move, world.uidplus, world.copyuid.clone());
                let known = matches!(what.as_str(), "STORE" | "COPY") || (what == "MOVE" && can_move) || (what == "EXPUNGE" && uidplus);
                let changes = what != "COPY";
                match open.filter(|_| known) {
                    None if !known => reply.extend(format!("{tag} BAD what\r\n").bytes()),
                    None => reply.extend(format!("{tag} NO nothing is open\r\n").bytes()),
                    Some(_) if changes && read_only => {
                        reply.extend(format!("{tag} NO [READ-ONLY] the mailbox was opened to be looked at\r\n").bytes())
                    }
                    Some(at) if what == "STORE" => {
                        // `+FLAGS.SILENT (\Seen \Flagged)`
                        let on = rest.starts_with('+');
                        let flags = rest.split_once('(').and_then(|(_, rest)| rest.split_once(')')).map_or("", |(flags, _)| flags).to_string();
                        for message in world.mailboxes[at].messages.iter_mut().filter(|m| in_set(set, m.uid)) {
                            for flag in flags.split_whitespace() {
                                message.set(flag, on);
                            }
                        }
                        reply.extend(format!("{tag} OK stored\r\n").bytes());
                    }
                    Some(at) if what == "EXPUNGE" => {
                        world.mailboxes[at].remove(|m| m.has("\\Deleted") && in_set(set, m.uid));
                        reply.extend(format!("{tag} OK gone\r\n").bytes());
                    }
                    Some(at) => {
                        let name = words(rest).into_iter().next().unwrap_or_default();
                        match world.mailboxes.iter().position(|m| m.raw == name) {
                            None => reply.extend(format!("{tag} NO [TRYCREATE] no such mailbox\r\n").bytes()),
                            Some(to) => {
                                let taken: Vec<FakeMessage> = world.mailboxes[at].messages.iter().filter(|m| in_set(set, m.uid)).cloned().collect();
                                let role = |m: &FakeMailbox, flag: &str| world.gmail && m.flags.split_whitespace().any(|f| f == flag);
                                let (to_all, to_trash, from_all) =
                                    (role(&world.mailboxes[to], "\\All"), role(&world.mailboxes[to], "\\Trash"), role(&world.mailboxes[at], "\\All"));
                                let (mut from_uids, mut to_uids) = (Vec::new(), Vec::new());
                                for mut message in taken {
                                    from_uids.push(message.uid.to_string());
                                    // Gmail: all mail has it already.
                                    if let Some(there) = world.mailboxes[to].messages.iter().find(|m| to_all && m.body == message.body) {
                                        to_uids.push(there.uid.to_string());
                                        continue;
                                    }
                                    // Gmail: in the trash, and nowhere else.
                                    if to_trash && what == "MOVE" {
                                        for (i, other) in world.mailboxes.iter_mut().enumerate() {
                                            if i != to && i != at {
                                                other.remove(|m| m.body == message.body);
                                            }
                                        }
                                    }
                                    message.uid = world.mailboxes[to].take_uid();
                                    to_uids.push(message.uid.to_string());
                                    world.mailboxes[to].messages.push(message);
                                }
                                let truth = format!("{} {} {}", world.mailboxes[to].uid_validity, from_uids.join(","), to_uids.join(","));
                                let code = match (uidplus && !from_uids.is_empty(), copyuid) {
                                    (_, Some(lie)) => format!("[COPYUID {lie}] "),
                                    (true, None) => format!("[COPYUID {truth}] "),
                                    (false, None) => String::new(),
                                };
                                if what == "MOVE" {
                                    // As RFC 6851 has it: where they went, then that they left.
                                    if !code.is_empty() {
                                        reply.extend(format!("* OK {code}moved\r\n").bytes());
                                    }
                                    // Gmail: a label is added, and the message stays in all mail.
                                    if !from_all || to_trash {
                                        world.mailboxes[at].remove(|m| in_set(set, m.uid));
                                        reply.extend("* 1 EXPUNGE\r\n".repeat(from_uids.len()).bytes());
                                    }
                                    reply.extend(format!("{tag} OK done\r\n").bytes());
                                } else {
                                    reply.extend(format!("{tag} OK {code}copied\r\n").bytes());
                                }
                            }
                        }
                    }
                }
            }
            "UID" => {
                let mut parts = args.splitn(3, ' ');
                let (fetch, set, items) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                let (whole, header) = (items.contains("BODY.PEEK[]"), items.contains("BODY.PEEK[HEADER]"));
                let mut refused = !fetch.eq_ignore_ascii_case("FETCH");
                let messages = open.and_then(|i| world.mailboxes.get(i)).map_or(&[][..], |m| &m.messages[..]);
                for (index, message) in messages.iter().enumerate().filter(|(_, m)| in_set(set, m.uid)) {
                    let (seq, uid, flags) = (index + 1, message.uid, &message.flags);
                    if whole && world.hang_up_at_uid == Some(uid) {
                        return;
                    }
                    if whole && world.cut_at_uid == Some(uid) {
                        reply.extend(format!("* {seq} FETCH (UID {uid} FLAGS ({flags}) BODY[] {{{}}}\r\n", message.body.len()).bytes());
                        reply.extend(&message.body[..message.body.len() / 2]);
                        let _ = out.write_all(&reply);
                        return;
                    }
                    if whole && world.refuse_uid == Some(uid) {
                        refused = true;
                    } else if whole {
                        let date = &message.internal_date;
                        reply.extend(
                            format!("* {seq} FETCH (UID {uid} FLAGS ({flags}) INTERNALDATE \"{date}\" BODY[] {{{}}}\r\n", message.body.len()).bytes(),
                        );
                        reply.extend(&message.body);
                        reply.extend(b")\r\n");
                    } else if header {
                        // Up to and with the empty line that ends it.
                        let end = message.body.windows(4).position(|w| w == b"\r\n\r\n").map_or(message.body.len(), |at| at + 4);
                        let date = &message.internal_date;
                        reply.extend(format!("* {seq} FETCH (UID {uid} FLAGS ({flags}) INTERNALDATE \"{date}\" BODY[HEADER] {{{end}}}\r\n").bytes());
                        reply.extend(&message.body[..end]);
                        reply.extend(b")\r\n");
                    } else if items.contains("RFC822.SIZE") {
                        let size = message.claimed_size.unwrap_or(message.body.len() as u64);
                        reply.extend(format!("* {seq} FETCH (UID {uid} RFC822.SIZE {size})\r\n").bytes());
                    } else {
                        reply.extend(format!("* {seq} FETCH (FLAGS ({flags}) UID {uid})\r\n").bytes());
                    }
                }
                let end = if refused { "NO [SERVERBUG] that can't be done" } else { "OK done" };
                reply.extend(format!("{tag} {end}\r\n").bytes());
            }
            _ => reply.extend(format!("{tag} BAD what\r\n").bytes()),
        }
        drop(world);
        let written = match drip {
            Some(pause) => reply.iter().try_for_each(|byte| {
                std::thread::sleep(pause);
                out.write_all(&[*byte])
            }),
            None => out.write_all(&reply),
        };
        if written.is_err() {
            return;
        }
    }
}

/// The address between `<` and `>`.
fn path(line: &str) -> String {
    line.split_once('<').and_then(|(_, rest)| rest.split_once('>')).map(|(address, _)| address.to_string()).unwrap_or_default()
}

/// The lines of a message up to the closing dot, with the dot-stuffing undone; `None` when the client hung up.
fn read_data(reader: &mut BufReader<TcpStream>) -> Option<String> {
    let mut data = String::new();
    loop {
        let line = read_line(reader)?;
        if line == "." {
            return Some(data);
        }
        data.push_str(line.strip_prefix('.').unwrap_or(&line));
        data.push_str("\r\n");
    }
}

fn serve_smtp(stream: TcpStream, shared: Arc<Mutex<SmtpWorld>>) {
    let mut out = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    shared.lock().unwrap().connections += 1;
    let _ = out.write_all(b"220 fake.example.org ESMTP\r\n");
    let mut signed_in = false;
    let (mut from, mut recipients) = (String::new(), Vec::new());
    while let Some(line) = read_line(&mut reader) {
        let upper = line.to_ascii_uppercase();
        let mut world = shared.lock().unwrap();
        world.log.push(line.clone());
        let reply = if upper.starts_with("EHLO") {
            let auth = match (world.auth, !world.token.is_empty()) {
                (true, true) => "250-AUTH PLAIN LOGIN XOAUTH2\r\n",
                (true, false) => "250-AUTH PLAIN LOGIN\r\n",
                (false, true) => "250-AUTH XOAUTH2\r\n",
                (false, false) => "",
            };
            let starttls = if world.offer_starttls { "250-STARTTLS\r\n" } else { "" };
            format!("250-fake.example.org\r\n250-8BITMIME\r\n250-SIZE 10485760\r\n{auth}{starttls}250 HELP")
        } else if upper == "STARTTLS" && world.offer_starttls {
            drop(world);
            pretend_tls(&mut out, &mut reader, "220 go ahead\r\n");
            return;
        } else if let Some(payload) = line.strip_prefix("AUTH PLAIN ") {
            let decoded = base64::engine::general_purpose::STANDARD.decode(payload).unwrap_or_default();
            let decoded = String::from_utf8_lossy(&decoded).into_owned();
            let parts: Vec<&str> = decoded.split('\0').collect();
            if parts == ["", world.user.as_str(), world.password.as_str()] {
                signed_in = true;
                world.sign_ins += 1;
                "235 2.7.0 welcome".to_string()
            } else {
                // A careless server: it repeats the password it was sent.
                format!("535 5.7.8 no user with the password {}", parts.get(2).copied().unwrap_or(""))
            }
        } else if upper == "AUTH XOAUTH2" || upper.starts_with("AUTH XOAUTH2 ") {
            if world.token.is_empty() {
                "504 5.5.4 no such way to sign in".to_string()
            } else {
                let (user, token) = (world.user.clone(), world.token.clone());
                // The rest of the exchange is read without holding the state.
                drop(world);
                let with_command = line.get("AUTH XOAUTH2 ".len()..).unwrap_or("");
                let payload = if with_command.is_empty() {
                    let _ = out.write_all(b"334 \r\n");
                    let Some(line) = read_line(&mut reader) else { return };
                    shared.lock().unwrap().log.push(line.clone());
                    line
                } else {
                    with_command.to_string()
                };
                let (given_user, given_token) = bearer(&payload);
                if (given_user, &given_token) == (user, &token) {
                    signed_in = true;
                    shared.lock().unwrap().sign_ins += 1;
                    let _ = out.write_all(b"235 2.7.0 welcome\r\n");
                } else {
                    // The reasons, and after the client's empty line the verdict. A careless
                    // server: it repeats the token it was sent.
                    let _ = out.write_all(format!("334 {REASONS}\r\n").as_bytes());
                    let Some(line) = read_line(&mut reader) else { return };
                    shared.lock().unwrap().log.push(line);
                    let _ = out.write_all(format!("535 5.7.3 no user with the token {given_token}\r\n").as_bytes());
                }
                continue;
            }
        } else if upper.starts_with("MAIL FROM:") {
            if (world.auth || !world.token.is_empty()) && !signed_in {
                "530 5.7.0 sign in first".to_string()
            } else {
                from = path(&line);
                recipients.clear();
                "250 ok".to_string()
            }
        } else if upper.starts_with("RCPT TO:") {
            let address = path(&line);
            if world.refuse.contains(&address) {
                "550 5.1.1 no such user".to_string()
            } else {
                recipients.push(address);
                "250 ok".to_string()
            }
        } else if upper == "DATA" && recipients.is_empty() {
            "554 5.5.1 no recipients".to_string()
        } else if upper == "DATA" {
            // The message is read without holding the state.
            drop(world);
            let _ = out.write_all(b"354 go ahead\r\n");
            let Some(data) = read_data(&mut reader) else { return };
            let delivery = Delivery { from: std::mem::take(&mut from), recipients: std::mem::take(&mut recipients), data };
            shared.lock().unwrap().delivered.push(delivery);
            let _ = out.write_all(b"250 2.0.0 queued\r\n");
            continue;
        } else if upper == "RSET" {
            from.clear();
            recipients.clear();
            "250 ok".to_string()
        } else if upper == "QUIT" {
            let _ = out.write_all(b"221 bye\r\n");
            return;
        } else {
            "500 what".to_string()
        };
        drop(world);
        if out.write_all(format!("{reply}\r\n").as_bytes()).is_err() {
            return;
        }
    }
}
