//! Headless UI screenshots: renders the whole MailCraft window offscreen (wgpu), so it works
//! with a locked screen or a hidden window.
//!
//! `cargo run -p mailcraft-ui-egui --example ui_shot -- script.jsonl`
//!
//! The script is JSON lines: control-channel requests (`{"method": …, "params": …}`, see
//! `docs/control-protocol.md`), `{"shot": "/abs/out.png"}` to save the window as PNG,
//! `{"steps": n}` to run extra frames, or `{"wait": ms}` to keep running frames for a while. The
//! window is 1440×900 pt at 2× (override with `{"width": w, "height": h}` and `{"scale": s}` on
//! any line; `{"scale": 1}` is a screen at 100 %, where the fonts' hinting shows) and opens the
//! sample mailbox, as of a fixed Wednesday afternoon so shots compare, unless the first line is
//! `{"empty": true}`. The computer is in UTC unless a line says where it is (`{"zone":
//! "America/Chicago"}`: the calendar is then shown in that zone, and `view.timeZone` can show it
//! in another). The window is in American English unless a line runs `ui.language` (`{"method":
//! "ui.language", "params": {"value": "de"}}`).
//!
//! The window has no host: nothing in it can sign in, send or receive. A script that wants to
//! look at what Add Account says about a sign-in on Microsoft's page gives it a stand-in for
//! one with `{"host": {"signIn": …, "adminConsent": …}}` on any line (`Host`): from that line
//! on, a sign-in ends the way `signIn` says (`"waits"`: the owner is still in the browser;
//! `"approval"`, `"declined"`, `"timedOut"`, `"noBrowser"`, `"signedIn"`, or any other text as
//! Microsoft's own words), and an administrator's approval the way `adminConsent` says
//! (`"waits"`, `"approved"`, `"notAdministrator"`, `"declined"`, `"timedOut"`, `"noBrowser"`).
//! One that is waiting when the line comes is answered by it. With a stand-in host, every
//! address at a company's own domain is one whose mail Microsoft 365 takes. Nobody is signed
//! in, no browser is opened and nothing leaves this machine.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
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
    admin_consent: String,
    /// The sign-ins and approvals that wait for the script to say how they end, each with the
    /// address it was for.
    sign_ins: Vec<(String, Sender<Result<Tokens, String>>)>,
    approvals: Vec<Sender<AdminConsentAnswer>>,
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
                email: email.to_string(),
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
        if let Some(how) = said.get("signIn").and_then(|v| v.as_str()) {
            self.sign_in = how.to_string();
        }
        if let Some(how) = said.get("adminConsent").and_then(|v| v.as_str()) {
            self.admin_consent = how.to_string();
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

/// The services of a stand-in host: Microsoft 365 takes the mail of every company's domain,
/// sign-ins and approvals end the way the script says, and nothing else is ever asked.
fn stand_in(host: &Arc<Mutex<Host>>) -> Services {
    /// A channel that already holds its answer.
    fn answered<T>(answer: T) -> Receiver<T> {
        let (tx, rx) = channel();
        let _ = tx.send(answer);
        rx
    }
    let (signing, approving) = (host.clone(), host.clone());
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
        admin_consent_link: Some(Box::new(|oauth, tenant, _| {
            let directory = if tenant.is_empty() { "organizations" } else { tenant };
            Some(format!("https://login.example.org/{directory}/v2.0/adminconsent?client_id={}", oauth.client_id))
        })),
        // (So that an account of an organisation has its last step, and its first download ends.)
        graph: Some(Box::new(|request| answered(GraphResult { account: request.account.id, ..Default::default() }))),
        sync: Some(Box::new(|request| answered(SyncResult { account: request.account.id, ..Default::default() }))),
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
    let session = match zone {
        Some(zone) => Session::in_zone(mailbox, NOW, zone),
        None => Session::at(mailbox, NOW, 0),
    };
    // A stand-in for the host's part in signing in, when a line of the script speaks to one.
    let host: Arc<Mutex<Host>> = Default::default();
    let stood_in = lines.iter().any(|l| l.get("host").is_some());
    let services = if stood_in { stand_in(&host) } else { Services::default() };
    let mut app = MailApp::new(session, services).with_control(rx);
    app.autosave = false;
    // In American English whatever the machine speaks, so shots compare; `ui.language` shows another.
    app.ui.language = "en-US".into();
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
