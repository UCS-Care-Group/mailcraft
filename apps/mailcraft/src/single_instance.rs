// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One MailCraft per mailbox.
//!
//! Two windows on one mailbox file would each save over the other's work. So the first to start
//! takes a lock beside the mailbox and listens on a loopback port; one started later (a second
//! click on the shortcut, a `mailto:` link, a file opened from the file manager) finds the lock
//! taken, hands what it was started with to the first, which comes to the front, and exits.
//!
//! Beside the mailbox: `<mailbox>.lock`, held locked for as long as the first one runs (the
//! system lets go of it however the program ends, so a crash leaves nothing to clean up), and
//! `<mailbox>.instance`, the port and a random key (mode 0600 on Unix; elsewhere what the
//! mailbox's folder allows, which is the user's own unless the mailbox was put somewhere shared),
//! removed on exit.
//!
//! The hand-over is a trust boundary (`docs/threat-model.md`): only `127.0.0.1`, one line of
//! JSON of at most [`MAX_LINE`] bytes that must carry the key (compared in constant time), at most
//! [`MAX_ARGS`] things of at most [`MAX_ARG`] bytes each, and each of them checked as anything
//! from outside is ([`checked`]): a `mailto:` link, or a file of a kind MailCraft opens (a
//! message, a calendar or contact file, a folder of mail, a MailCraft mailbox) named by its whole
//! path, that is there, is a file and can be read. Everything else is dropped and counted.
//!
//! Which runs take part: one started on a mailbox file takes part, `--control` or not (a second
//! start on the same file never opens it again, so a run for a test that names the owner's mailbox
//! hands over and exits); one on the sample mailbox, and one without preferences
//! (`MAILCRAFT_NO_PREFS`) that names no `--mailbox`, keep nothing in a file and neither take a
//! lock nor listen, so they never meet the owner's window. A run without preferences that names a
//! mailbox file is guarded for that file. The lock is taken for the mailbox a run starts with
//! only: a `.mailcraft` file opened later in the window (File › Open, a drop, or handed over
//! here) is opened with `file.open` without its own lock, and another window may have that one
//! open.
//! A lock left by a run that crashed was let go of by the system; its `.instance` file names a
//! port nobody answers on and is replaced by the next first start.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_engine::io::Format;
use serde_json::{Value, json};

use crate::control_server::{keys_match, random_key, write_key_file};

/// Longer lines are refused.
pub(crate) const MAX_LINE: usize = 1 << 20;
/// The most things one start can hand over, and the longest of them.
pub(crate) const MAX_ARGS: usize = 256;
pub(crate) const MAX_ARG: usize = 32 * 1024;
/// The most message files one start can hand over (each is an import and a window).
pub(crate) const MAX_MESSAGES: usize = 32;
/// How long either side waits for the other on the connection.
const ANSWER: Duration = Duration::from_secs(5);
/// How long a later start keeps trying to reach the first (which may itself be starting).
const REACH: Duration = Duration::from_secs(8);

/// What starting found.
pub enum Start {
    /// Nobody else has this mailbox open: carry on, and keep the guard for as long as it is open.
    First(Guard),
    /// Another MailCraft has it open and took what this one was started with: exit.
    Handed,
    /// Another MailCraft has it open and could not be reached: say so and exit.
    Unreachable(String),
}

/// What later starts handed over: the links and files that passed [`checked`], and how many
/// things didn't.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Handed {
    pub open: Vec<String>,
    pub refused: usize,
}

/// The first instance's hold on a mailbox, and what later starts hand to it.
pub struct Guard {
    /// Held, not read: the lock goes when the file is closed.
    _lock: Option<File>,
    info: Option<PathBuf>,
    rx: Receiver<Handed>,
    wake: Arc<Mutex<Option<egui::Context>>>,
}

impl Guard {
    /// The window to wake when something is handed over.
    pub fn wake(&self, ctx: &egui::Context) {
        *self.wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx.clone());
    }

    /// What later starts were started with, since the last look; `Some` (even when empty) says
    /// one of them asked for the window.
    pub fn handed_over(&self) -> Option<Handed> {
        let mut all: Option<Handed> = None;
        while let Ok(handed) = self.rx.try_recv() {
            let all = all.get_or_insert_default();
            all.open.extend(handed.open);
            all.refused = all.refused.saturating_add(handed.refused);
        }
        all
    }

    /// The window is closing: later starts have nobody to reach from here on.
    pub fn close(&self) {
        if let Some(info) = &self.info {
            let _ = std::fs::remove_file(info);
        }
    }
}

/// `<mailbox>.<what>`, beside the mailbox.
pub(crate) fn beside(mailbox: &Path, what: &str) -> PathBuf {
    let mut name = mailbox.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(what);
    mailbox.with_file_name(name)
}

/// Take the mailbox, or hand `args` (files, `mailto:` links) to the MailCraft that has it.
pub fn start(mailbox: &Path, args: &[String]) -> Start {
    start_within(mailbox, args, REACH)
}

pub(crate) fn start_within(mailbox: &Path, args: &[String], reach: Duration) -> Start {
    let (lock_path, info_path) = (beside(mailbox, "lock"), beside(mailbox, "instance"));
    if let Some(dir) = lock_path.parent() {
        let _ = mailcraft_engine::store::create_private_dir_all(dir);
    }
    let lock = match OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path) {
        Ok(f) => f,
        // Nowhere to keep a lock: there is nothing to guard with, so start as if alone.
        Err(e) => {
            log::warn!("no lock file {} ({e}): not guarding against a second window", lock_path.display());
            return Start::First(unguarded());
        }
    };
    match lock.try_lock() {
        Ok(()) => Start::First(first(lock, info_path)),
        Err(TryLockError::WouldBlock) => hand_over(&info_path, args, reach),
        // A file system without locks: start as if alone, as before there was a guard.
        Err(TryLockError::Error(e)) => {
            log::warn!("{} can't be locked ({e}): not guarding against a second window", lock_path.display());
            Start::First(unguarded())
        }
    }
}

fn unguarded() -> Guard {
    Guard { _lock: None, info: None, rx: channel().1, wake: Default::default() }
}

/// Listen for later starts. Without a port, a key or the file that tells them, the lock is still
/// held: a later start can't reach this one and says so instead of opening the mailbox twice.
fn first(lock: File, info_path: PathBuf) -> Guard {
    let (tx, rx) = channel();
    let wake: Arc<Mutex<Option<egui::Context>>> = Default::default();
    let mut guard = Guard { _lock: Some(lock), info: None, rx, wake: wake.clone() };
    // A file left by a run that didn't end cleanly names a port nobody answers on.
    let _ = std::fs::remove_file(&info_path);
    let listening = TcpListener::bind(("127.0.0.1", 0)).and_then(|l| l.local_addr().map(|a| (l, a.port()))).map_err(|e| e.to_string());
    let (listener, port, key) = match listening.and_then(|(l, port)| random_key().map(|key| (l, port, key))) {
        Ok(ready) => ready,
        Err(e) => {
            log::error!("a second MailCraft won't be able to reach this one: {e}");
            return guard;
        }
    };
    if let Err(e) = write_key_file(&info_path, &json!({"port": port, "key": key}).to_string()) {
        log::error!("a second MailCraft won't be able to reach this one: {e}");
        return guard;
    }
    guard.info = Some(info_path);
    let key: Arc<str> = key.into();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (tx, wake, key) = (tx.clone(), wake.clone(), key.clone());
            std::thread::spawn(move || serve(stream, &tx, &wake, &key));
        }
    });
    guard
}

/// One line, at most [`MAX_LINE`] long; `None` for anything else.
pub(crate) fn read_line(stream: &TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let n = BufReader::new(stream).take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf).ok()?;
    if n == 0 || buf.len() > MAX_LINE {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// A later start's one request: its key and what it was started with.
fn serve(mut stream: TcpStream, tx: &Sender<Handed>, wake: &Mutex<Option<egui::Context>>, key: &str) {
    let _ = stream.set_read_timeout(Some(ANSWER));
    let _ = stream.set_write_timeout(Some(ANSWER));
    let Some(request) = read_line(&stream).and_then(|line| serde_json::from_str::<Value>(&line).ok()) else { return };
    if !keys_match(request.get("key").and_then(Value::as_str).unwrap_or(""), key) {
        let _ = writeln!(stream, "{}", json!({"ok": false}));
        return;
    }
    let handed = sorted(request.get("open"));
    if handed.refused > 0 {
        // (What was refused is not written down: a link can name people.)
        log::warn!("a later start handed over {} things that aren't links or files MailCraft opens", handed.refused);
    }
    if tx.send(handed).is_err() {
        let _ = writeln!(stream, "{}", json!({"ok": false}));
        return;
    }
    if let Some(ctx) = wake.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        ctx.request_repaint();
    }
    let _ = writeln!(stream, "{}", json!({"ok": true}));
}

/// What a later start sent under `open`: at most [`MAX_ARGS`] things are looked at, and those
/// that pass [`checked`] are kept. Anything that isn't a list is refused whole.
pub(crate) fn sorted(open: Option<&Value>) -> Handed {
    let mut handed = Handed::default();
    let Some(list) = open else { return handed };
    let Some(list) = list.as_array() else {
        handed.refused = 1;
        return handed;
    };
    handed.refused = list.len().saturating_sub(MAX_ARGS);
    let mut messages = 0usize;
    for item in list.iter().take(MAX_ARGS) {
        match item.as_str().and_then(checked) {
            // Each message file is an import and a window: no more than [`MAX_MESSAGES`] at once.
            Some(ok) if is_message_file(&ok) && messages >= MAX_MESSAGES => handed.refused = handed.refused.saturating_add(1),
            Some(ok) => {
                messages = messages.saturating_add(usize::from(is_message_file(&ok)));
                handed.open.push(ok);
            }
            None => handed.refused = handed.refused.saturating_add(1),
        }
    }
    handed
}

/// Whether `what` names a message file (`.eml`), which opens in a window of its own.
fn is_message_file(what: &str) -> bool {
    Format::from_extension(what) == Some(Format::Eml) && !what.get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
}

/// One thing a later start handed over, as the window is to open it, or `None` when it isn't
/// something MailCraft opens from outside: a `mailto:` link (the compose command reads it as RFC
/// 6068 says, and takes nothing but the recipients, the subject and the body from it), or the
/// whole path of a file of a kind MailCraft opens (`file://` as desktops write one is read as
/// its path) that is there, is a file (not a folder, a device or a pipe) and can be read.
/// Nothing with a control character in it passes, nor anything over [`MAX_ARG`] bytes.
pub(crate) fn checked(arg: &str) -> Option<String> {
    if arg.is_empty() || arg.len() > MAX_ARG || arg.chars().any(char::is_control) {
        return None;
    }
    if arg.get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:")) {
        return Some(arg.to_string());
    }
    let path = mailcraft_ui_egui::files::local_path(arg);
    if path.chars().any(char::is_control) || !Path::new(&path).is_absolute() {
        return None;
    }
    // A mailbox only by its own extension (`.json` is a mailbox to the import, not to a file
    // manager).
    let mailbox = path.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case(mailcraft_engine::io::MAILBOX_EXT));
    if Format::from_extension(&path)? == Format::Mailbox && !mailbox {
        return None;
    }
    // Opened without waiting and checked through that handle: a pipe or a device where a file was
    // named neither passes nor holds this thread. (The window reads it again the same way, so a
    // file swapped after this check is refused there too.)
    mailcraft_engine::io::regular_file(Path::new(&path)).ok()?;
    Some(path)
}

/// A file named on the command line is named from where this start was run: the first instance
/// runs somewhere else, so it gets the whole path. Links are left as they are.
fn whole(arg: &str) -> String {
    if arg.contains("://") || arg.get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:")) {
        return arg.to_string();
    }
    std::path::absolute(arg).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| arg.to_string())
}

/// Give `args` to the instance `info_path` tells of, trying for as long as `reach`.
pub(crate) fn hand_over(info_path: &Path, args: &[String], reach: Duration) -> Start {
    let args: Vec<String> = args.iter().map(|a| whole(a)).collect();
    let until = Instant::now() + reach;
    let mut why;
    loop {
        match try_hand_over(info_path, &args) {
            Ok(()) => return Start::Handed,
            Err(e) => why = e,
        }
        if Instant::now() >= until {
            return Start::Unreachable(why);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn try_hand_over(info_path: &Path, args: &[String]) -> Result<(), String> {
    let info = std::fs::read(info_path).map_err(|e| format!("{}: {e}", info_path.display()))?;
    let info: Value = serde_json::from_slice(&info).map_err(|e| format!("{}: {e}", info_path.display()))?;
    let port = info.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0).ok_or("no port")?;
    let key = info.get("key").and_then(Value::as_str).ok_or("no key")?;
    let mut stream = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), ANSWER).map_err(|e| format!("127.0.0.1:{port}: {e}"))?;
    let _ = stream.set_read_timeout(Some(ANSWER));
    let _ = stream.set_write_timeout(Some(ANSWER));
    writeln!(stream, "{}", json!({"key": key, "open": args})).map_err(|e| e.to_string())?;
    let answer = read_line(&stream).and_then(|line| serde_json::from_str::<Value>(&line).ok()).ok_or("no answer")?;
    if answer.get("ok").and_then(Value::as_bool) == Some(true) { Ok(()) } else { Err("it refused".to_string()) }
}

#[cfg(test)]
#[path = "single_instance_tests.rs"]
mod single_instance_tests;
