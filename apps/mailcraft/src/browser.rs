// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening a page in the system's browser, with the browser brought in front of the window.
//!
//! Every page MailCraft opens comes through here: Microsoft's sign-in, consent and approval
//! pages ([`open_page`]) and every link the window follows (egui's `OpenUrl` output, which
//! [`take_links`] takes out of a frame before eframe would open it behind our back).
//!
//! On Windows and macOS the system's own way of opening a page brings the browser forward, and
//! that is all that is done there. On Linux and FreeBSD a program started from another one only
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
const OPENED: [&str; 4] = ["http:", "https:", "mailto:", "tel:"];

/// Whether an address may reach the system's opener. A link in a message is checked where it is
/// read; this is the last check before anything leaves the program, whoever asked, so that a
/// `file:`, `smb:` or `ms-msdt:` address can't be launched by a way in that forgot the first.
pub fn openable(url: &str) -> bool {
    if url.chars().any(char::is_control) || !OPENED.iter().any(|s| url.get(..s.len()).is_some_and(|p| p.eq_ignore_ascii_case(s))) {
        return false;
    }
    // A web address the parser rejects would be taken for a relative file path by the opener
    // (and become a `file:` address): refuse it here.
    if url.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("http")) {
        return url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty()));
    }
    true
}

/// The system's own way of opening a page (ShellExecute, Launch Services, the desktop's opener).
pub fn system_open(url: &str) -> bool {
    if !openable(url) {
        log::warn!("browser: an address of a kind that is never opened was refused");
        return false;
    }
    match webbrowser::open(url) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("couldn't start a browser ({:?})", e.kind());
            false
        }
    }
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
            && xdg_open(&mut command)
        {
            return true;
        }
        system_open(url)
    }
}

/// How long `xdg-open` is given to say it couldn't open a page before it is taken to have.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const OPENER_WAIT: Duration = Duration::from_secs(2);

/// Start `xdg-open` and see whether it opened the page: it exits at once on the desktops (it
/// hands the page to the desktop's own opener); where it starts the browser itself, it is still
/// running, and is left to it.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn xdg_open(command: &mut std::process::Command) -> bool {
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
mod tests {
    use super::*;

    /// Records what it was asked to open, and with which token.
    #[derive(Default)]
    struct Fake(Mutex<Vec<(String, Option<String>)>>);

    impl Launch for Fake {
        fn launch(&self, url: &str, token: Option<&str>) -> bool {
            self.0.lock().unwrap().push((url.to_string(), token.map(str::to_string)));
            true
        }
    }

    fn browser() -> (Arc<Fake>, Browser<u32>) {
        let fake = Arc::new(Fake::default());
        (fake.clone(), Browser::new(fake))
    }

    #[test]
    fn a_page_opens_with_the_token_the_window_was_given() {
        let (fake, browser) = browser();
        let (_, rx) = browser.open("https://login.example/authorize");
        let now = Instant::now();
        let next = browser.pump(&mut || Some(7), now);
        assert_eq!(next, Some(TOKEN_WAIT));
        assert!(fake.0.lock().unwrap().is_empty(), "waits for its token");
        browser.token_arrived(&8, "other".into());
        browser.token_arrived(&7, "tok-7".into());
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        assert_eq!(*fake.0.lock().unwrap(), vec![("https://login.example/authorize".to_string(), Some("tok-7".to_string()))]);
        // Nothing left waiting, and a late answer for it opens nothing more.
        browser.token_arrived(&7, "tok-7".into());
        assert_eq!(browser.pump(&mut || None, now), None);
    }

    #[test]
    fn without_a_token_a_page_opens_at_once_or_when_the_wait_is_over() {
        let (fake, browser) = browser();
        let (_, rx) = browser.open("https://one.example/");
        browser.pump(&mut || None, Instant::now());
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        let (_, rx) = browser.open("https://two.example/");
        let then = Instant::now();
        browser.pump(&mut || Some(1), then);
        assert_eq!(browser.pump(&mut || Some(2), then + TOKEN_WAIT / 2), Some(TOKEN_WAIT / 2));
        assert_eq!(browser.pump(&mut || Some(3), then + TOKEN_WAIT), None);
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        let opened = fake.0.lock().unwrap().clone();
        assert_eq!(opened, vec![("https://one.example/".to_string(), None), ("https://two.example/".to_string(), None)]);
    }

    #[test]
    fn followed_links_wait_for_tokens_too_and_their_number_is_capped() {
        let (fake, browser) = browser();
        browser.follow((0..40).map(|i| format!("https://site.example/{i}")).collect());
        let mut serial = 0;
        browser.pump(
            &mut || {
                serial += 1;
                Some(serial)
            },
            Instant::now(),
        );
        // The ones past the cap don't wait.
        let deadline = Instant::now() + Duration::from_secs(5);
        while fake.0.lock().unwrap().len() < 40 - MAX_WAITING && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(fake.0.lock().unwrap().len(), 40 - MAX_WAITING);
        assert!(fake.0.lock().unwrap().iter().all(|(_, token)| token.is_none()));
        browser.token_arrived(&1, "t1".into());
        let deadline = Instant::now() + Duration::from_secs(5);
        while fake.0.lock().unwrap().len() < 40 - MAX_WAITING + 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(fake.0.lock().unwrap().contains(&("https://site.example/0".to_string(), Some("t1".to_string()))));
    }

    #[test]
    fn a_page_the_window_never_takes_is_opened_by_whoever_waits() {
        let (fake, browser) = browser();
        // No window pumps: the page is withdrawn and opened here, without a token.
        assert!(browser.open_and_wait("https://login.example/"));
        assert_eq!(*fake.0.lock().unwrap(), vec![("https://login.example/".to_string(), None)]);
        assert_eq!(browser.pump(&mut || Some(1), Instant::now()), None, "nothing left behind");
    }

    #[test]
    fn the_windows_links_are_taken_out_of_its_output() {
        let ctx = egui::Context::default();
        let mut taken = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.ctx().open_url(egui::OpenUrl::new_tab("https://news.example/a"));
            ui.ctx().copy_text("kept".into());
            ui.ctx().open_url(egui::OpenUrl::same_tab("https://news.example/b"));
            taken = take_links(ui.ctx());
        });
        assert_eq!(taken, ["https://news.example/a", "https://news.example/b"]);
        output.textures_delta.clear();
        assert!(matches!(output.platform_output.commands.as_slice(), [egui::OutputCommand::CopyText(t)] if t == "kept"));
    }

    #[test]
    fn web_addresses_the_parser_rejects_are_refused() {
        // `webbrowser` would take these for relative file paths and open a `file:` address.
        for url in ["http://a b/../../../etc/hosts", "https://x:99999/", "http://[bad/x", "http://", "https://"] {
            assert!(!openable(url), "{url:?}");
            assert!(!system_open(url), "{url:?}");
        }
        assert!(openable("https://a.example:8443/x?y#z"));
    }

    #[test]
    fn only_web_mail_and_phone_addresses_reach_the_system() {
        for url in ["https://a.example/", "HTTP://a.example", "mailto:ada@example.org", "tel:+441234"] {
            assert!(openable(url), "{url:?}");
        }
        for url in [
            "file:///etc/passwd",
            "smb://host/share",
            "ms-msdt:/id",
            "search-ms:query=x",
            "javascript:x",
            "data:,x",
            "cid:x",
            "",
            "https://a.example/\n",
        ] {
            assert!(!openable(url), "{url:?}");
            assert!(!system_open(url), "{url:?}");
        }
    }

    #[test]
    fn the_opener_is_given_the_token_and_nothing_an_address_could_misuse() {
        let command = opener_command("https://login.example/authorize?x=1", "abc_123").unwrap();
        assert_eq!(command.get_program(), "xdg-open");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["https://login.example/authorize?x=1"]);
        let envs: Vec<_> = command.get_envs().map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned()))).collect();
        assert!(envs.contains(&("XDG_ACTIVATION_TOKEN".into(), Some("abc_123".into()))));
        assert!(envs.contains(&("DESKTOP_STARTUP_ID".into(), Some("abc_123".into()))));
        for url in ["-x https://a.example/", "mailto:a@example.org", "file:///etc/passwd", "https://a.example/\nb", "", "ht"] {
            assert!(opener_command(url, "abc").is_none(), "{url:?}");
        }
        assert!(opener_command("https://a.example/", "a b").is_none());
        assert!(opener_command("https://a.example/", "").is_none());
    }

    /// Every page the program opens comes through this module: nothing else names the opener.
    #[test]
    fn nothing_else_opens_a_page() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let roots = [here.join("src"), here.join("../../crates")];
        let mut stack: Vec<std::path::PathBuf> = roots.to_vec();
        let mut offenders = Vec::new();
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            // ponytail: opened_files.rs opens saved attachments (files, not pages) with `open`;
            // it could take the activation token too if those programs should come in front.
            } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("src/browser.rs") && !path.ends_with("src/opened_files.rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains(concat!("webbrowser", "::"))
                    || text.contains(concat!("open", "::that"))
                    || text.contains(concat!("\"xdg", "-open\""))
                {
                    offenders.push(path);
                }
            }
        }
        assert!(offenders.is_empty(), "pages opened outside browser.rs: {offenders:?}");
    }
}
