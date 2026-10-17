//! Headless UI screenshots: renders the whole MailCraft window offscreen (wgpu), so it works
//! with a locked screen or a hidden window.
//!
//! `cargo run -p mailcraft-ui-egui --example ui_shot -- script.jsonl`
//!
//! The script is JSON lines: control-channel requests (`{"method": …, "params": …}`, see
//! `docs/control-protocol.md`), `{"shot": "/abs/out.png"}` to save the window as PNG,
//! `{"steps": n}` to run extra frames (a request with `"agent": {"allow": [...]}` comes as the MCP
//! bridge's would: what needs the owner puts its question over the window), or `{"wait": ms}` to keep running frames for a while. The
//! window is 1440×900 pt at 2× (override with `{"width": w, "height": h}` and `{"scale": s}` on
//! any line; `{"scale": 1}` is a screen at 100 %, where the fonts' hinting shows) and opens the
//! sample mailbox, as of a fixed Wednesday afternoon so shots compare, unless the first line is
//! `{"empty": true}`. The computer is in UTC unless a line says where it is (`{"zone":
//! "America/Chicago"}`: the calendar is then shown in that zone, and `view.timeZone` can show it
//! in another).
//!
//! The window has no host: nothing in it can sign in, send or receive. A script that wants to
//! look at what Add Account says about a sign-in on Microsoft's page gives it a stand-in for
//! one with `{"host": {"signIn": …, "adminConsent": …}}` on any line (`Host`): from that line
//! on, a sign-in ends the way `signIn` says (`"waits"`: the owner is still in the browser;
//! `"approval"`, `"declined"`, `"timedOut"`, `"noBrowser"`, `"signedIn"`, or any other text as
//! Microsoft's own words; `"signedInAs": "address"` has it sign in as that address rather than
//! the one asked for, as an owner who picks another account on Microsoft's page does), and an administrator's approval the way `adminConsent` says
//! (`"waits"`, `"approved"`, `"notAdministrator"`, `"declined"`, `"timedOut"`, `"noBrowser"`).
//! One that is waiting when the line comes is answered by it. With a stand-in host, every
//! address at a company's own domain is one whose mail Microsoft 365 takes. Nobody is signed
//! in, no browser is opened and nothing leaves this machine. The same host answers a check for a
//! new version (`{"host": {"update": …}}`): `"0.9.0"` (that is the newest release), `"0.9.0
//! security"` (and it fixes a security problem), `"same"` (the running version), or `"fails"`.
//! And a Graph job (`{"host": {"graph": …}}`): `"consent"` (Microsoft wants the owner's
//! permission first), `"refused"` (it refused the sign-in) or `"sendConsent"` (a shared
//! mailbox's mail waits for permission to send from shared mailboxes, #70); anything else
//! answers with nothing.
//! With a stand-in host, the copy of the mailbox kept before a switch to Microsoft Graph is never
//! finished, so the switch's page keeps saying that it is being saved. And the background
//! saver (`{"host": {"saving": …}}`): `"messages 12000 200000"` or `"content 3 90000"` (a save
//! that has been at it for a minute, at that stage, done of total; it never ends), `"sweeping"`,
//! or `""` (nothing being written).
//!
//! And Send/Receive, for Send/Receive Progress (`{"host": {"sync": [[piece, …], …]}}`): one list
//! of pieces for each run of an account from that line on, in turn, each piece a `sync.apply`
//! result (`docs/control-protocol.md`) with its account filled in, and `"newMessages": n` to have
//! that many made-up messages arrive in the Inbox (`"sendOutbox": true`: what waits in the
//! account's Outbox is sent). A run whose last piece is `"partial": true` is
//! still under way when the pieces are out, until `{"host": {"syncMore": [piece, …]}}` sends it
//! more (`{"stopped": true}` ends it as Stop does). A run with no list of its own brings nothing.
//! From that line on, every account on a mail server has a password at hand (an invented one), so
//! a round of all of them goes through without asking. `{"host": {"fullMessages": [[account,
//! done, total], …]}}` has Send/Receive Progress say that many of an account's messages are
//! fetched whole of that many (Full messages), as the background would have counted them for a
//! Microsoft account; Stop (`ui.cancelSync`) during a run then pauses them as it does.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::io::{BackgroundSave, SaveProgress, SaveReport, SaveStage};
use mailcraft_model::sync::{SyncResult, Tokens};
use mailcraft_ui_egui::approval::{NEEDS_APPROVAL, NO_BROWSER, SIGN_IN_DECLINED, SIGN_IN_TIMED_OUT};
use mailcraft_ui_egui::{AdminConsentAnswer, ControlRequest, GraphResult, MailApp, Services};

static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A stand-in for the host's part in signing in with Microsoft: it answers what the script
/// last told it to, and holds on to what is still waited for. Every id in it is invented.
#[derive(Default)]
struct Host {
    sign_in: String,
    /// Who a sign-in signs in as; empty: the address it was asked for.
    signed_in_as: String,
    admin_consent: String,
    /// What a check for a new version finds.
    update: String,
    /// How a Graph job ends.
    graph: String,
    /// The sign-ins and approvals that wait for the script to say how they end, each with the
    /// address it was for.
    sign_ins: Vec<(String, Sender<Result<Tokens, String>>)>,
    approvals: Vec<Sender<AdminConsentAnswer>>,
    /// How far the background saver has got (see the header).
    saving: Option<SaveProgress>,
    /// Copies of the mailbox being "written": never finished (see the header).
    copies: Vec<Sender<Result<(), String>>>,
    /// The pieces of the Send/Receive runs to come, one list for each, and whether the script
    /// spoke of Send/Receive at all (then passwords are at hand).
    sync_runs: VecDeque<Vec<serde_json::Value>>,
    syncing: bool,
    /// The run under way, still waiting for more of its pieces: its account, the Inbox's id and
    /// where its pieces go.
    sync_open: Option<(u64, u64, Sender<SyncResult>)>,
    /// The last UID a made-up message was given.
    uid: u32,
}

/// A piece of a stand-in Send/Receive, from what the script says: for `account`, whose Inbox is
/// `inbox`, with any made-up new messages in it, and with what waits in its Outbox sent when it
/// says `"sendOutbox": true`. Nothing in it is anybody's.
fn sync_piece(said: &serde_json::Value, account: u64, inbox: u64, outbox: &[u64], uid: &mut u32) -> SyncResult {
    let mut piece: SyncResult = serde_json::from_value(said.clone()).unwrap_or_default();
    piece.account = account;
    if said.get("sendOutbox").and_then(|v| v.as_bool()) == Some(true) {
        piece.sent = outbox.to_vec();
        piece.filed = outbox.to_vec();
    }
    let new = said.get("newMessages").and_then(|v| v.as_u64()).unwrap_or(0).min(500);
    if new > 0 && inbox != 0 {
        let messages = (0..new)
            .map(|_| {
                *uid += 1;
                mailcraft_model::Message {
                    uid: *uid,
                    subject: format!("Update {uid}"),
                    from: mailcraft_model::Address::new("Lumen Updates", "news@lumen.example"),
                    date: NOW - i64::from(*uid) * 60,
                    ..Default::default()
                }
            })
            .collect();
        piece.folders.push(mailcraft_model::sync::FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: mailcraft_model::FolderKind::Inbox,
            uid_validity: 1,
            uid_seen: *uid,
            new: messages,
            ..Default::default()
        });
    }
    piece
}

/// Every password asked for is an invented one, once the script speaks of Send/Receive.
struct StandInSecrets(Arc<Mutex<Host>>);

impl mailcraft_ui_egui::SecretStore for StandInSecrets {
    fn get(&self, key: &str) -> Option<String> {
        let syncing = self.0.lock().map(|host| host.syncing).unwrap_or(false);
        (syncing && key.starts_with("password:")).then(|| "stand-in-password".to_string())
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _: &str) {}
}

impl Host {
    /// How a sign-in of `email` ends now; `None` while the owner is still in the browser.
    fn signed_in(&self, email: &str) -> Option<Result<Tokens, String>> {
        Some(match self.sign_in.as_str() {
            "" | "waits" => return None,
            "signedIn" => Ok(Tokens {
                access: "stand-in-access".into(),
                refresh: "stand-in-refresh".into(),
                expires_at: mailcraft_time::now() + 3_600,
                name: "Noor Haddad".into(),
                email: if self.signed_in_as.is_empty() { email.to_string() } else { self.signed_in_as.clone() },
                tenant: "3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b".into(),
            }),
            "approval" => Err(NEEDS_APPROVAL.into()),
            "declined" => Err(SIGN_IN_DECLINED.into()),
            "timedOut" => Err(SIGN_IN_TIMED_OUT.into()),
            "noBrowser" => Err(NO_BROWSER.into()),
            words => Err(words.to_string()),
        })
    }

    /// How an administrator's approval ends now; `None` while Microsoft's page is still open.
    fn approved(&self) -> Option<AdminConsentAnswer> {
        Some(match self.admin_consent.as_str() {
            "" | "waits" => return None,
            "approved" => AdminConsentAnswer::Approved,
            "notAdministrator" => AdminConsentAnswer::NotAdministrator("AADSTS90094: Administrator consent is required.".into()),
            "declined" => AdminConsentAnswer::Declined("AADSTS65004: User declined to consent to access the app.".into()),
            "timedOut" => AdminConsentAnswer::TimedOut,
            "noBrowser" => AdminConsentAnswer::NoBrowser,
            words => AdminConsentAnswer::Failed(words.to_string()),
        })
    }

    /// Take in what a line of the script says, and answer what was waiting for it.
    fn told(&mut self, said: &serde_json::Value) {
        if let Some(who) = said.get("signedInAs").and_then(|v| v.as_str()) {
            self.signed_in_as = who.to_string();
        }
        if let Some(how) = said.get("signIn").and_then(|v| v.as_str()) {
            self.sign_in = how.to_string();
        }
        if let Some(how) = said.get("adminConsent").and_then(|v| v.as_str()) {
            self.admin_consent = how.to_string();
        }
        if let Some(found) = said.get("update").and_then(|v| v.as_str()) {
            self.update = found.to_string();
        }
        if let Some(how) = said.get("graph").and_then(|v| v.as_str()) {
            self.graph = how.to_string();
        }
        if let Some(runs) = said.get("sync").and_then(|v| v.as_array()) {
            self.syncing = true;
            self.sync_runs = runs.iter().map(|run| run.as_array().cloned().unwrap_or_default()).collect();
        }
        if let Some(more) = said.get("syncMore").and_then(|v| v.as_array())
            && let Some((account, inbox, to)) = self.sync_open.take()
        {
            let mut open = true;
            for said in more {
                let piece = sync_piece(said, account, inbox, &[], &mut self.uid);
                open = piece.partial;
                let _ = to.send(piece);
            }
            if open {
                self.sync_open = Some((account, inbox, to));
            }
        }
        if let Some(how) = said.get("saving").and_then(|v| v.as_str()) {
            let mut words = how.split_whitespace();
            let stage = match words.next() {
                Some("messages") => Some(SaveStage::Messages),
                Some("content") => Some(SaveStage::Content),
                Some("sweeping") => Some(SaveStage::Sweeping),
                _ => None,
            };
            let mut number = || words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
            let (done, total) = (number(), number());
            self.saving = stage.map(|stage| SaveProgress { stage, done, total, millis: 60_000 });
        }
        if self.sign_in != "waits" && !self.sign_in.is_empty() {
            for (email, waiting) in std::mem::take(&mut self.sign_ins) {
                if let Some(answer) = self.signed_in(&email) {
                    let _ = waiting.send(answer);
                }
            }
        }
        if let Some(answer) = self.approved() {
            for waiting in std::mem::take(&mut self.approvals) {
                let _ = waiting.send(answer.clone());
            }
        }
    }
}

/// The background saver of a stand-in host: it is at what the script last said, and writes
/// nothing.
struct StandInSaver(Arc<Mutex<Host>>);

impl BackgroundSave for StandInSaver {
    fn follow(&self, _: &std::path::Path) {}
    fn submit(&self, _: &std::path::Path, _: &mailcraft_model::Mailbox, _: u64) {}
    fn reports(&self) -> Vec<SaveReport> {
        Vec::new()
    }
    fn progress(&self) -> Option<SaveProgress> {
        self.0.lock().ok().and_then(|host| host.saving)
    }
    fn flush(&self) {}
}

/// The services of a stand-in host: Microsoft 365 takes the mail of every company's domain,
/// sign-ins and approvals end the way the script says, and nothing else is ever asked.
fn stand_in(host: &Arc<Mutex<Host>>) -> Services {
    /// A channel that already holds its answer.
    fn answered<T>(answer: T) -> Receiver<T> {
        let (tx, rx) = channel();
        let _ = tx.send(answer);
        rx
    }
    let (signing, approving, updating, graphing, copying) = (host.clone(), host.clone(), host.clone(), host.clone(), host.clone());
    let syncing = host.clone();
    Services {
        microsoft_client_id: "11111111-2222-3333-4444-555555555555".into(),
        lookup_mx: Some(Box::new(|_| answered(Ok(vec!["example-org.mail.protection.outlook.com".to_string()])))),
        sign_in: Some(Box::new(move |request| {
            let (tx, rx) = channel();
            let Ok(mut host) = signing.lock() else { return rx };
            match host.signed_in(&request.email) {
                Some(answer) => {
                    let _ = tx.send(answer);
                }
                None => host.sign_ins.push((request.email.clone(), tx)),
            }
            rx
        })),
        admin_consent: Some(Box::new(move |_| {
            let (tx, rx) = channel();
            let Ok(mut host) = approving.lock() else { return rx };
            match host.approved() {
                Some(answer) => {
                    let _ = tx.send(answer);
                }
                None => host.approvals.push(tx),
            }
            rx
        })),
        admin_consent_link: Some(Box::new(|oauth, tenant| {
            let directory = if tenant.is_empty() { "organizations" } else { tenant };
            Some(format!("https://login.example.org/{directory}/v2.0/adminconsent?client_id={}", oauth.client_id))
        })),
        // (So that an account of an organisation has its last step, and its first download ends.)
        graph: Some(Box::new(move |request| {
            let how = graphing.lock().map(|host| host.graph.clone()).unwrap_or_default();
            let (consent_needed, sign_in_refused, send_consent_needed) = (how == "consent", how == "refused", how == "sendConsent");
            answered(GraphResult { account: request.account.id, consent_needed, sign_in_refused, send_consent_needed, ..Default::default() })
        })),
        sync: Some(Box::new(move |request| {
            let (account, (tx, rx)) = (request.account.id, channel());
            let inbox = request.folders.iter().find(|f| f.kind == mailcraft_model::FolderKind::Inbox).map_or(0, |f| f.id);
            let outbox: Vec<u64> = request.outbox.iter().map(|m| m.id).collect();
            let Ok(mut host) = syncing.lock() else { return rx };
            let pieces = host.sync_runs.pop_front().unwrap_or_default();
            let mut open = true;
            for said in &pieces {
                let piece = sync_piece(said, account, inbox, &outbox, &mut host.uid);
                open = piece.partial;
                let _ = tx.send(piece);
            }
            if pieces.is_empty() {
                let _ = tx.send(SyncResult { account, ..Default::default() });
            } else if open {
                host.sync_open = Some((account, inbox, tx));
            }
            rx
        })),
        secrets: Some(Box::new(StandInSecrets(host.clone()))),
        write_copy: Some(Box::new(move |_, _| {
            let (tx, rx) = channel();
            if let Ok(mut host) = copying.lock() {
                host.copies.push(tx);
            }
            rx
        })),
        update_check: Some(Box::new(move |_| {
            let found = updating.lock().map(|host| host.update.clone()).unwrap_or_default();
            let (version, security) = match found.split_once(' ') {
                Some((version, marked)) => (version.to_string(), marked == "security"),
                None if found == "same" => (env!("CARGO_PKG_VERSION").to_string(), false),
                None => (found, false),
            };
            answered(match mailcraft_model::update::Version::parse(&version) {
                Some(version) => Ok(mailcraft_model::update::Release {
                    page: format!("https://github.com/UCS-Care-Group/mailcraft/releases/tag/v{version}"),
                    version,
                    security,
                }),
                None => Err("GitHub couldn't be reached".to_string()),
            })
        })),
        saver: Some(Box::new(StandInSaver(host.clone()))),
        ..Default::default()
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: ui_shot script.jsonl");
    let script = std::fs::read_to_string(&path).expect("read script");
    let (tx, rx) = std::sync::mpsc::channel::<ControlRequest>();
    let lines: Vec<serde_json::Value> =
        script.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).expect("json line")).collect();
    // Where the computer is, when a line says: a zone with a name, so that it has daylight saving.
    let zone = lines.iter().find_map(|l| l.get("zone").and_then(|v| v.as_str())).and_then(mailcraft_time::Zone::named);
    let offset = zone.map_or(0, |zone| zone.offset_at(NOW));
    let mailbox = if lines.first().is_some_and(|l| l.get("empty").is_some()) {
        mailcraft_model::Mailbox::with_local_account("", "")
    } else {
        mailcraft_engine::sample::mailbox(NOW, offset)
    };
    let mut session = match zone {
        Some(zone) => Session::in_zone(mailbox, NOW, zone),
        None => Session::at(mailbox, NOW, 0),
    };
    // Shared mailboxes, as the window has them with `MAILCRAFT_SHARED_MAILBOXES=1` (#70).
    session.shared_mailboxes = std::env::var("MAILCRAFT_SHARED_MAILBOXES").is_ok_and(|value| value.trim() == "1");
    // A stand-in for the host's part in signing in, when a line of the script speaks to one.
    let host: Arc<Mutex<Host>> = Default::default();
    let stood_in = lines.iter().any(|l| l.get("host").is_some());
    let services = if stood_in { stand_in(&host) } else { Services::default() };
    let mut app = MailApp::new(session, services).with_control(rx);
    app.autosave = false;
    // (With a host that answers, nothing is to happen by itself: what a shot shows, its script asked for.)
    app.auto_sync = !stood_in;
    let w = lines.iter().find_map(|l| l.get("width").and_then(|v| v.as_f64())).unwrap_or(1440.0) as f32;
    let h = lines.iter().find_map(|l| l.get("height").and_then(|v| v.as_f64())).unwrap_or(900.0) as f32;
    let scale = lines.iter().find_map(|l| l.get("scale").and_then(|v| v.as_f64())).unwrap_or(2.0).clamp(0.5, 4.0) as f32;
    let mut harness =
        // Frames a sixtieth of a second apart, as on a screen: two clicks one frame after the other
        // then count as a double click (egui allows 0.3 s between them).
        egui_kittest::Harness::builder()
            .with_size(egui::vec2(w, h))
            .with_pixels_per_point(scale)
            .with_max_steps(1_000_000)
            .with_step_dt(1.0 / 60.0)
            .wgpu()
            .build_ui_state(
            |ui, app: &mut MailApp| {
                if !READY.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.input_mut().max_texture_side = Some(8192);
    READY.store(true, std::sync::atomic::Ordering::Relaxed);
    step_n(&mut harness, 6);
    for l in lines {
        if let Some(said) = l.get("host") {
            if let Ok(mut host) = host.lock() {
                host.told(said);
            }
            for full in said.get("fullMessages").and_then(|v| v.as_array()).into_iter().flatten() {
                let n = |k: usize| full.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                let count = |k: usize| u32::try_from(n(k)).unwrap_or(u32::MAX);
                harness.state_mut().session.sync_progress.set_full(n(0), count(1), count(2));
            }
            step_n(&mut harness, 2);
        }
        if let Some(p) = l.get("shot").and_then(|v| v.as_str()) {
            step_n(&mut harness, 4);
            match harness.render() {
                Ok(img) => {
                    img.save(p).expect("save png");
                    println!("{p}");
                }
                Err(e) => eprintln!("render failed: {e}"),
            }
        } else if let Some(n) = l.get("steps").and_then(|v| v.as_u64()) {
            step_n(&mut harness, n as usize);
        } else if let Some(ms) = l.get("wait").and_then(|v| v.as_u64()) {
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < end {
                step(&mut harness);
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        } else if let Some(m) = l.get("method").and_then(|v| v.as_str()) {
            let (req, reply) = ControlRequest::new(m, l.get("params").cloned().unwrap_or_default());
            let req = match l.get("agent") {
                Some(a) => req.from_agent(mailcraft_ui_egui::control::agent_allow(a)),
                None => req,
            };
            tx.send(req).expect("send");
            for _ in 0..60 {
                step(&mut harness);
                if let Ok(r) = reply.try_recv() {
                    let s = r.to_string();
                    // `UI_SHOT_FULL=1` prints whole replies (e.g. `ui.inspect` for scripted checks).
                    let cut = if std::env::var_os("UI_SHOT_FULL").is_some() { s.len() } else { s.len().min(300) };
                    println!("{m}: {}", s.get(..cut).unwrap_or(&s));
                    break;
                }
            }
            step_n(&mut harness, 2);
        }
    }
}

fn step(harness: &mut egui_kittest::Harness<'_, MailApp>) {
    let mut raw = std::mem::take(harness.input_mut());
    harness.state_mut().raw_input_hook(&mut raw);
    *harness.input_mut() = raw;
    harness.step();
}

fn step_n(harness: &mut egui_kittest::Harness<'_, MailApp>, n: usize) {
    for _ in 0..n {
        step(harness);
    }
}
