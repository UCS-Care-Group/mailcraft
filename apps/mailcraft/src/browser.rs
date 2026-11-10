// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening a page in the system's browser, with the browser brought in front of the window.
//!
//! Every page MailCraft opens comes through here: Microsoft's sign-in, consent and approval
//! pages ([`open_page`]) and every link the window follows (egui's `OpenUrl` output, which
//! [`take_links`] takes out of a frame before eframe would open it behind our back).
//!
//! On Windows and macOS the system's own way of opening a page brings the browser forward, and
//! that is all that is done there: `rundll32 url.dll,FileProtocolHandler <address>` and
//! `open <address>`, started as programs with the address as their one argument, never through a
//! shell ([`windows_command`], [`macos_command`]). No crate opens pages for us (#350), and the
//! address is first written by [`normalised`], so that it holds only what is safe to pass. On Linux and FreeBSD a program started from another one only
//! comes in front when it is handed an activation token from the program the owner was using:
//! under Wayland an xdg-activation token, under X11 a startup-notification id. So a page waits
//! for one: the window asks winit for a token ([`Browser::pump`]), winit hands it back as a
//! window event (`WithTokens` in `main.rs`), and the opener is started with it in its
//! environment (`XDG_ACTIVATION_TOKEN`, `DESKTOP_STARTUP_ID`). A compositor that gives none, or
//! doesn't answer within [`TOKEN_WAIT`], has the page opened without one, as before.
//!
//! The address of a page is never logged: it can name who is signing in.
#![cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long a page waits for its activation token before it is opened without one.
pub const TOKEN_WAIT: Duration = Duration::from_millis(1000);
/// How long [`Browser::open_and_wait`] waits for the window to take a page in hand before it
/// opens the page itself.
const WINDOW_WAIT: Duration = if cfg!(test) { Duration::from_millis(200) } else { Duration::from_secs(5) };
/// At most this many pages wait at once; more are opened at once, without a token.
const MAX_WAITING: usize = 32;

/// Starts the opener: with the activation token when there is one. Whether it could be started.
pub trait Launch: Send + Sync {
    fn launch(&self, url: &str, token: Option<&str>) -> bool;
}

/// A page asked for and not yet opened.
struct Asked<S> {
    id: u64,
    url: String,
    reply: Option<Sender<bool>>,
    /// The token asked for, and when; `None` while the window hasn't taken the page in hand.
    token: Option<(S, Instant)>,
}

struct State<S> {
    next_id: u64,
    asked: Vec<Asked<S>>,
    wake: Option<egui::Context>,
}

/// The pages waiting for the window, or for their activation token. `S` is the window system's
/// name for one request of a token (winit's `AsyncRequestSerial`).
pub struct Browser<S> {
    state: Mutex<State<S>>,
    launcher: Arc<dyn Launch>,
}

impl<S: PartialEq + Clone + Send + 'static> Browser<S> {
    pub fn new(launcher: Arc<dyn Launch>) -> Self {
        Browser { state: Mutex::new(State { next_id: 0, asked: Vec::new(), wake: None }), launcher }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State<S>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The window to wake when a page is asked for from another thread.
    pub fn set_window(&self, ctx: &egui::Context) {
        self.state().wake = Some(ctx.clone());
    }

    /// Ask for `url` to be opened; the window opens it at its next frame. The answer (whether an
    /// opener could be started) comes on the receiver.
    pub fn open(&self, url: &str) -> (u64, Receiver<bool>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let (id, wake) = {
            let mut state = self.state();
            let id = state.next_id;
            state.next_id = state.next_id.wrapping_add(1);
            state.asked.push(Asked { id, url: url.to_string(), reply: Some(tx), token: None });
            (id, state.wake.clone())
        };
        if let Some(ctx) = wake {
            ctx.request_repaint();
        }
        (id, rx)
    }

    /// [`Browser::open`], waiting for the answer: for a background thread (a sign-in). When the
    /// window doesn't take the page in hand in time, it is opened from here, without a token.
    pub fn open_and_wait(&self, url: &str) -> bool {
        let (id, rx) = self.open(url);
        match rx.recv_timeout(WINDOW_WAIT) {
            Ok(opened) => opened,
            Err(RecvTimeoutError::Disconnected) => false,
            Err(RecvTimeoutError::Timeout) => match self.withdraw(id) {
                Some(url) => self.launcher.launch(&url, None),
                // Already being opened: its answer is on the way.
                None => rx.recv().unwrap_or(false),
            },
        }
    }

    /// Take back a page nobody has started opening yet.
    fn withdraw(&self, id: u64) -> Option<String> {
        let mut state = self.state();
        let at = state.asked.iter().position(|a| a.id == id)?;
        Some(state.asked.remove(at).url)
    }

    /// Links the window followed in this frame: opened like any page, nobody waiting for the answer.
    pub fn follow(&self, urls: Vec<String>) {
        if urls.is_empty() {
            return;
        }
        let mut state = self.state();
        for url in urls {
            let id = state.next_id;
            state.next_id = state.next_id.wrapping_add(1);
            state.asked.push(Asked { id, url, reply: None, token: None });
        }
    }

    /// On the window's thread, every frame: ask a token for each page the window hasn't taken in
    /// hand yet (`request_token`; `None` when the window system gives none, and the page is
    /// opened at once), and open those whose token is overdue without one. How long until the
    /// next is overdue, for the window to look again.
    pub fn pump(&self, request_token: &mut dyn FnMut() -> Option<S>, now: Instant) -> Option<Duration> {
        let mut now_open = Vec::new();
        let next = {
            let mut state = self.state();
            let mut waiting = state.asked.iter().filter(|a| a.token.is_some()).count();
            let mut kept = Vec::with_capacity(state.asked.len());
            for mut asked in std::mem::take(&mut state.asked) {
                match &asked.token {
                    None => match (waiting < MAX_WAITING).then(&mut *request_token).flatten() {
                        Some(serial) => {
                            asked.token = Some((serial, now));
                            waiting += 1;
                            kept.push(asked);
                        }
                        None => now_open.push(asked),
                    },
                    Some((_, since)) if now.saturating_duration_since(*since) >= TOKEN_WAIT => now_open.push(asked),
                    Some(_) => kept.push(asked),
                }
            }
            state.asked = kept;
            state
                .asked
                .iter()
                .filter_map(|a| a.token.as_ref())
                .map(|(_, since)| TOKEN_WAIT.saturating_sub(now.saturating_duration_since(*since)))
                .min()
        };
        for asked in now_open {
            self.start(asked.url, None, asked.reply);
        }
        next
    }

    /// The window system's answer to a request of a token: the page that waited for it opens with it.
    pub fn token_arrived(&self, serial: &S, token: String) {
        let found = {
            let mut state = self.state();
            let at = state.asked.iter().position(|a| a.token.as_ref().is_some_and(|(s, _)| s == serial));
            at.map(|at| state.asked.remove(at))
        };
        if let Some(asked) = found {
            self.start(asked.url, Some(token), asked.reply);
        }
    }

    /// Start the opener on a thread of its own (it can take a moment to say whether it started),
    /// and answer whoever waits.
    fn start(&self, url: String, token: Option<String>, reply: Option<Sender<bool>>) {
        // The last check before anything leaves the program, for every way in and every system.
        let Some(url) = normalised(&url) else {
            log::warn!("browser: an address of a kind that is never opened was refused");
            if let Some(reply) = reply {
                let _ = reply.send(false);
            }
            return;
        };
        let launcher = self.launcher.clone();
        let work = move || {
            let opened = launcher.launch(&url, token.as_deref());
            if let Some(reply) = reply {
                let _ = reply.send(opened);
            }
        };
        // No thread for it: the dropped answer reads as "couldn't be started".
        if let Err(e) = std::thread::Builder::new().name("browser".into()).spawn(work) {
            log::warn!("couldn't start a browser: no thread for it ({e})");
        }
    }
}

/// Take the links the window followed in this frame out of its output, before eframe opens them
/// itself (which would leave the browser behind the window).
pub fn take_links(ctx: &egui::Context) -> Vec<String> {
    ctx.output_mut(|o| {
        let mut urls = Vec::new();
        o.commands.retain(|c| match c {
            egui::OutputCommand::OpenUrl(open) => {
                urls.push(open.url.clone());
                false
            }
            _ => true,
        });
        urls
    })
}

/// The kinds of address the system's opener is ever handed.
const OPENED: [&str; 4] = ["http", "https", "mailto", "tel"];

/// Whether an address may reach the system's opener. A link in a message is checked where it is
/// read; this is the last check before anything leaves the program, whoever asked, so that a
/// `file:`, `smb:` or `ms-msdt:` address can't be launched by a way in that forgot the first.
#[cfg(test)]
pub fn openable(url: &str) -> bool {
    normalised(url).is_some()
}

/// The address as the opener is handed it, and `None` when it may not be opened. A web address
/// is written by `mailcraft_link` (a host with a name and a parse, as a browser reads it): what
/// is launched is what was checked. A `mailto:` or `tel:` address has its spaces, quotes and
/// anything else outside ASCII written as `%XX`. So what comes out starts with its scheme in
/// lower case and holds no space, quote, control character or backslash: no opener can take it
/// for an option, for more than one argument, or (a web address the parser rejects, which an
/// opener takes for a relative file path and turns into a `file:` address) for a file.
pub fn normalised(url: &str) -> Option<String> {
    // Before anything is encoded, so that no address is written out at unbounded length.
    if url.len() > mailcraft_link::MAX_LINK || url.chars().any(char::is_control) {
        return None;
    }
    let (scheme, rest) = url.split_once(':')?;
    let scheme = scheme.to_ascii_lowercase();
    match scheme.as_str() {
        "http" | "https" => mailcraft_link::WebLink::parse(url).map(mailcraft_link::WebLink::into_string),
        "mailto" | "tel" if !rest.is_empty() && !rest.contains('\\') => Some(format!("{scheme}:{}", mailcraft_link::encode_opaque(rest))),
        _ => None,
    }
}

/// Whether an address, as [`normalised`] writes it, is safe to hand to a program as one
/// argument: one of the allowed schemes first (so never `-x` or a path), then printable ASCII
/// only, and no quote (which Windows' command lines would read as the end of the argument).
fn plain_argument(url: &str) -> bool {
    OPENED.iter().any(|s| url.strip_prefix(s).and_then(|rest| rest.strip_prefix(':')).is_some_and(|rest| !rest.is_empty()))
        && url.len() <= 1 << 16
        && url.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\'))
}

/// The longest address given to a Windows launcher: `CreateProcess` takes a command line of at
/// most 32,767 characters, and the program's path and `url.dll,FileProtocolHandler` come first.
/// A longer address is refused (and said so as a browser that couldn't be started), not cut.
pub const WINDOWS_MAX_ARGUMENT: usize = 32_000;

/// `rundll32.exe url.dll,FileProtocolHandler <url>`: how a page is opened on Windows.
///
/// `url.dll`'s `FileProtocolHandler` hands the address to the shell's own handler for its scheme
/// (what ShellExecute does), so the default browser or mail program opens, brought in front by
/// Windows. It is a program started with an argument list, not a command line given to `cmd.exe`:
/// nothing reads `&`, `^`, `|`, `%` or a quote in the address (`start` does, and so does a
/// `.cmd` file). The address is one argument that [`plain_argument`] has checked: it starts with
/// its scheme, so rundll32 can't take it for another entry point or an option; it holds no
/// space or quote, so the command line Rust builds for it is the address and nothing more.
/// `ShellExecuteW` itself is `unsafe` to call and this program forbids unsafe code; `explorer.exe
/// <url>` was the other choice, and it reads its argument as a path or a folder when it doesn't
/// care for the address. The program is the one in Windows' own directory, not whatever the
/// search path finds first. `None` when the address is not for a program's argument.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn windows_command(url: &str, system_root: Option<std::ffi::OsString>) -> Option<std::process::Command> {
    use std::process::{Command, Stdio};
    if !plain_argument(url) || url.len() > WINDOWS_MAX_ARGUMENT {
        return None;
    }
    let program = match system_root.filter(|r| !r.is_empty()) {
        Some(root) => std::path::Path::new(&root).join("System32").join("rundll32.exe").into_os_string(),
        None => "rundll32.exe".into(),
    };
    let mut command = Command::new(program);
    command.arg("url.dll,FileProtocolHandler").arg(url);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashes up for it.
        command.creation_flags(0x0800_0000);
    }
    Some(command)
}

/// `open <url>`: how a page is opened on macOS (Launch Services). The address starts with its
/// scheme, so `open` can't take it for an option; it is one argument, no shell.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn macos_command(url: &str) -> Option<std::process::Command> {
    use std::process::{Command, Stdio};
    if !plain_argument(url) {
        return None;
    }
    let mut command = Command::new("/usr/bin/open");
    command.arg(url);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    Some(command)
}

/// The commands that open a page on the other systems, in the order they are tried: what
/// `$BROWSER` names (a list of programs divided by `:`, each with an optional `%s` for the
/// address, split at spaces and never through a shell), then the desktops' own openers.
pub fn unix_commands(url: &str, browser: Option<&str>) -> Vec<std::process::Command> {
    use std::process::{Command, Stdio};
    if !plain_argument(url) {
        return Vec::new();
    }
    let mut commands = Vec::new();
    for entry in browser.unwrap_or("").split(':').take(8) {
        let mut words = entry.split_whitespace();
        let Some(program) = words.next() else { continue };
        let mut command = Command::new(program);
        let mut given = false;
        for word in words {
            if word.contains("%s") {
                given = true;
            }
            command.arg(word.replace("%s", url));
        }
        if !given {
            command.arg(url);
        }
        commands.push(command);
    }
    for (program, first) in
        [("xdg-open", None), ("gio", Some("open")), ("kde-open", None), ("gnome-open", None), ("x-www-browser", None), ("www-browser", None)]
    {
        let mut command = Command::new(program);
        if let Some(first) = first {
            command.arg(first);
        }
        command.arg(url);
        commands.push(command);
    }
    for command in &mut commands {
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    }
    commands
}

/// The system's own way of opening a page: Windows' shell handler (`rundll32`), macOS' `open`,
/// elsewhere `$BROWSER` and the desktop's openers. Nothing else is run, and never through a shell.
pub fn system_open(url: &str) -> bool {
    let Some(url) = normalised(url) else {
        log::warn!("browser: an address of a kind that is never opened was refused");
        return false;
    };
    #[cfg(windows)]
    let commands: Vec<_> = windows_command(&url, std::env::var_os("SystemRoot")).into_iter().collect();
    #[cfg(target_os = "macos")]
    let commands: Vec<_> = macos_command(&url).into_iter().collect();
    #[cfg(not(any(windows, target_os = "macos")))]
    let commands = unix_commands(&url, std::env::var("BROWSER").ok().as_deref());
    for mut command in commands {
        if run_opener(&mut command) {
            return true;
        }
    }
    log::warn!("couldn't start a browser");
    false
}

/// Opens pages on Linux and FreeBSD: `xdg-open`, with the activation token in its environment,
/// for a web page that has one; the system's own way otherwise, or when `xdg-open` can't be
/// started. (`$BROWSER`, when set, is the owner's choice, and the system's own way honours it.)
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub struct Desktop;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl Launch for Desktop {
    fn launch(&self, url: &str, token: Option<&str>) -> bool {
        log::info!("browser: opening a page {}", if token.is_some() { "with an activation token" } else { "without an activation token" });
        if let Some(token) = token
            && std::env::var_os("BROWSER").is_none()
            && let Some(mut command) = opener_command(url, token)
            && run_opener(&mut command)
        {
            return true;
        }
        system_open(url)
    }
}

/// How long an opener is given to say it couldn't open a page before it is taken to have.
const OPENER_WAIT: Duration = Duration::from_secs(2);

/// Start an opener and see whether it opened the page: it exits at once on the desktops (it
/// hands the page to the desktop's own opener); where it starts the browser itself, it is still
/// running, and is left to it.
fn run_opener(command: &mut std::process::Command) -> bool {
    let Ok(mut child) = command.spawn() else { return false };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < OPENER_WAIT => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                // Reaped when it is done, so no process is left behind it.
                let _ = std::thread::Builder::new().name("browser".into()).spawn(move || child.wait());
                return true;
            }
            Err(_) => return true,
        }
    }
}

/// `xdg-open <url>` with the activation token in its environment, under both the Wayland and the
/// X11 name (as winit itself hands one on); `None` for anything but a plain web address, which
/// keeps the system's own way, so that nothing an address says can be taken for an option.
pub fn opener_command(url: &str, token: &str) -> Option<std::process::Command> {
    use std::process::{Command, Stdio};
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let web = lower.starts_with("https://") || lower.starts_with("http://");
    let clean = |s: &str| !s.is_empty() && !s.chars().any(|c| c.is_control() || c.is_whitespace());
    if !web || !clean(url) || !clean(token) || token.len() > 4096 {
        return None;
    }
    let mut command = Command::new("xdg-open");
    command.arg(url).env("XDG_ACTIVATION_TOKEN", token).env("DESKTOP_STARTUP_ID", token);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    Some(command)
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod browser_tests;
