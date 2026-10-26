// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
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
//! `{"empty": true}`. `{"overlayScrollBars": true}` (or `false`) on any line has the message
//! list's scroll bar float over the rows as on a Mac (or keep its lane as elsewhere), whatever
//! the computer the shot is taken on. The computer is in UTC unless a line says where it is (`{"zone":
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
//! The stand-in host is a copy that updates itself, whose downloads are checked at once (nothing
//! is downloaded); `{"host": {"updates": "organisation"}}` on any line has the organisation
//! turn that off for the computer from the start, and `{"host": {"updates": "didntStart 0.4.2"}}`
//! has the running version be one newly installed that didn't start twice running, with 0.4.2
//! kept to go back to.
//! `{"host": {"tasksWork": true}}` has every Microsoft account's To Do work, so that the window
//! asks whether the tasks that exist only here are to be sent to it (#248).
//! And a Graph job (`{"host": {"graph": …}}`): `"consent"` (Microsoft wants the owner's
//! permission first), `"refused"` (it refused the sign-in) or `"sendConsent"` (a shared
//! mailbox's mail waits for permission to send from shared mailboxes, #70), `"opened"` (the
//! calendars opened by address are listed, each named from its address, with read rights, #70),
//! `"openedRefused"` (the server wouldn't give them) or `"openedConsent"` (the sign-in doesn't
//! cover reading other people's calendars), `"listedTwice operations@example.org Example Operations"`
//! (the account's list has the calendar of the shared mailbox at that address too, by that name,
//! unless the owner hid it); anything else answers with nothing.
//! With a stand-in host, every sender the owner knows has a picture drawn for them, for
//! `ui.senderImages`.
//! With a stand-in host, the copy of the mailbox kept before a switch to Microsoft Graph is never
//! finished, so the switch's page keeps saying that it is being saved. And the background
//! saver (`{"host": {"saving": …}}`): `"messages 12000 200000"` or `"content 3 90000"` (a save
//! that has been at it for a minute, at that stage, done of total; it never ends), `"sweeping"`,
//! or `""` (nothing being written).
//!
//! A folder of mailboxes being looked at (#420, `ui.dialog` `importTree`) never finishes; `{"host": {"treeProgress": [folders, files, messages]}}` sets the counts it shows, and `{"host": {"treeDir": "/path"}}` has it look into that folder at once.
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
//!
//! And a search on the server (R6.2, `{"host": {"serverSearch": {"messages": [GraphMessage, …],
//! "next": bool, "offline": bool}}}`): from that line on, every Microsoft mailbox has a Graph
//! token of this session (an invented one), and More results on the server answers each mailbox
//! with those of the messages whose folder it has, saying there are more when `next` is true, or
//! that the server can't be reached when `offline` is.
//!
//! And the system's notifications (R7.1), which every script has: each desktop alert the window
//! asks for is taken and shown nowhere, and `ui.alerts` says what it asked for.
//! `{"host": {"alerts": "noService"}}` (or `"notInstalled"`, `"outsideApp"`, `"turnedOff"`; any
//! other word: none) has the system refuse them for that reason from that line on, as File ›
//! Options then says, and `{"host": {"unfocused": true}}` has the window be one that isn't in
//! front, so that new mail is told of.

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
    /// The folder imports begun (#420): their progress, never finished; `treeProgress` moves it.
    tree_jobs: Vec<(Arc<mailcraft_engine::import_tree::Progress>, Sender<Result<mailcraft_engine::import_tree::Parsed, String>>)>,
    /// A folder the import looks into at once instead of waiting (`treeDir`).
    tree_dir: String,
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
    /// What a search on the server finds (see the header), when the script said.
    server_search: Option<serde_json::Value>,
    /// Why the system won't show desktop alerts, when the script said (see the header).
    alert_trouble: Option<mailcraft_ui_egui::alerts::AlertTrouble>,
    /// The window is not the one in front (see the header).
    unfocused: bool,
}

/// A stand-in for the system's notifications: every alert is taken (and shown nowhere), unless
/// the script said why the system won't show them. The window's own list (`ui.alerts`) says what
/// it asked for.
struct StandInAlerts(Arc<Mutex<Host>>);

impl mailcraft_ui_egui::alerts::AlertHost for StandInAlerts {
    fn show(&self, _: &mailcraft_ui_egui::alerts::DesktopAlert) -> Result<(), String> {
        match self.trouble() {
            Some(why) => Err(why.code().to_string()),
            None => Ok(()),
        }
    }
    fn trouble(&self) -> Option<mailcraft_ui_egui::alerts::AlertTrouble> {
        self.0.lock().ok().and_then(|host| host.alert_trouble)
    }
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

/// Removing MailCraft's data: nothing is kept by this window, so nothing is removed.
struct StandInWipe;

impl mailcraft_ui_egui::data_removal::DataWipe for StandInWipe {
    fn erase(&self, _: Option<&std::path::Path>) -> mailcraft_ui_egui::data_removal::WipeReport {
        Default::default()
    }
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
        if let Some(why) = said.get("alerts").and_then(|v| v.as_str()) {
            use mailcraft_ui_egui::alerts::AlertTrouble;
            self.alert_trouble = [AlertTrouble::NotInstalled, AlertTrouble::OutsideApp, AlertTrouble::TurnedOff, AlertTrouble::NoService]
                .into_iter()
                .find(|t| t.code() == why);
        }
        if let Some(unfocused) = said.get("unfocused").and_then(|v| v.as_bool()) {
            self.unfocused = unfocused;
        }
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
        if let Some(found) = said.get("serverSearch") {
            self.server_search = Some(found.clone());
        }
        if let Some(dir) = said.get("treeDir").and_then(|v| v.as_str()) {
            self.tree_dir = dir.to_string();
        }
        if let Some(n) = said.get("treeProgress").and_then(|v| v.as_array()) {
            let n: Vec<usize> = n.iter().map(|v| v.as_u64().unwrap_or(0) as usize).collect();
            if let (Some(job), [folders, files, messages]) = (self.tree_jobs.last().map(|(p, _)| p), n.as_slice()) {
                job.folders.store(*folders, std::sync::atomic::Ordering::Relaxed);
                job.files.store(*files, std::sync::atomic::Ordering::Relaxed);
                job.messages.store(*messages, std::sync::atomic::Ordering::Relaxed);
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
    let searching = host.clone();
    let tree = host.clone();
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
            // Calendars opened by address (#70): "opened" lists each with a name made of its address
            // (read rights), "openedRefused" says the server wouldn't give any, "openedConsent" says
            // the sign-in doesn't cover reading other people's calendars.
            let main = mailcraft_model::remote::RemoteCalendar { id: "c-main".into(), name: "Calendar".into(), main: true, ..Default::default() };
            let mut calendars = vec![main];
            let mut refused = Vec::new();
            for one in &request.account.opened_calendars {
                if how == "opened" {
                    let local = one.address.split('@').next().unwrap_or_default();
                    let name: String = local
                        .split('.')
                        .map(|w| w.chars().next().map(|c| c.to_uppercase().collect::<String>() + &w[c.len_utf8()..]).unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join(" ");
                    let owner = mailcraft_model::CalendarOwner { address: one.address.clone(), name: name.clone() };
                    calendars.push(mailcraft_model::remote::RemoteCalendar {
                        id: mailcraft_model::remote::opened_id(&one.address),
                        name,
                        rights: mailcraft_model::Rights::Read,
                        owner: Some(owner),
                        ..Default::default()
                    });
                } else if how == "openedRefused" {
                    refused.push(mailcraft_model::remote::OpenedRefusal {
                        address: one.address.clone(),
                        why: mailcraft_model::remote::OpenedWhy::NoRights,
                    });
                }
            }
            // "listedTwice address Name": the account's list has, besides its own calendar, the
            // calendar of the shared mailbox at that address as well (an invitation accepted in
            // Outlook, #70), called Name, unless the owner hid it (the host leaves it out then).
            let twice = how.strip_prefix("listedTwice ").and_then(|rest| rest.split_once(' '));
            if let Some((address, name)) = twice {
                let owner = mailcraft_model::CalendarOwner { address: address.to_string(), name: name.to_string() };
                if !request.account.hides_listed(Some(&owner)) {
                    calendars.push(mailcraft_model::remote::RemoteCalendar {
                        id: "AAMk-listed-twice".into(),
                        name: name.to_string(),
                        group: "Other Calendars".into(),
                        owner: Some(owner),
                        ..Default::default()
                    });
                }
            }
            let listed = (how.starts_with("opened") && how != "openedConsent") || twice.is_some();
            let calendars = listed.then(|| mailcraft_model::remote::CalendarsDownload {
                account: request.account.id,
                email: request.account.email.clone(),
                complete: true,
                calendars,
                refused,
            });
            answered(GraphResult {
                account: request.account.id,
                consent_needed,
                sign_in_refused,
                send_consent_needed,
                shared_calendars_consent_needed: how == "openedConsent",
                calendars,
                ..Default::default()
            })
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
        // A search on the server answers what the script said it finds (see the header).
        graph_search: Some(Box::new(move |job: mailcraft_ui_egui::server_search::GraphSearchJob| {
            let said = searching.lock().ok().and_then(|host| host.server_search.clone()).unwrap_or_default();
            let request = &job.request;
            let messages: Vec<mailcraft_model::sync::GraphMessage> =
                said.get("messages").cloned().and_then(|m| serde_json::from_value(m).ok()).unwrap_or_default();
            let offline = said.get("offline").and_then(|v| v.as_bool()) == Some(true);
            let more = said.get("next").and_then(|v| v.as_bool()) == Some(true) && request.next.is_empty();
            answered(mailcraft_model::sync::GraphSearchResult {
                account: request.account,
                email: request.email.clone(),
                ticket: request.ticket,
                messages: if offline || !request.next.is_empty() { Vec::new() } else { messages },
                next: if more { "https://graph.microsoft.com/v1.0/me/messages?$skip=50".into() } else { String::new() },
                offline,
                errors: if offline {
                    vec!["Couldn't search on Microsoft's server: couldn't reach graph.microsoft.com: no network".into()]
                } else {
                    Vec::new()
                },
                ..Default::default()
            })
        })),
        secrets: Some(Box::new(StandInSecrets(host.clone()))),
        // A folder import that is looked at for ever: `{"host": {"treeProgress": [folders, files, messages]}}` moves its counts.
        import_tree: Some(Box::new(move |job| {
            let dir = tree.lock().map(|host| host.tree_dir.clone()).unwrap_or_default();
            if !dir.is_empty() {
                // Answered at once from a folder on this computer (`treeDir`).
                return answered(mailcraft_engine::import_tree::load_path(&dir, None, job.count_only, &job.progress).map_err(|e| e.to_string()));
            }
            let (tx, rx) = channel();
            if let Ok(mut host) = tree.lock() {
                host.tree_jobs.push((job.progress, tx));
            }
            rx
        })),
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
                    assets: Vec::new(),
                }),
                None => Err("GitHub couldn't be reached".to_string()),
            })
        })),
        // A copy that updates itself: what it finds is downloaded and checked at once (nothing
        // is downloaded and nothing is kept).
        update_download: Some(Box::new(|_| answered(Ok(std::path::PathBuf::from("updates"))))),
        saver: Some(Box::new(StandInSaver(host.clone()))),
        // Every sender has a picture: a disc on a ground of two colours from their address,
        // drawn here (nothing is downloaded).
        sender_images: Some(Box::new(|address: String, _| answered(Ok(stand_in_picture(&address))))),
        ..Default::default()
    }
}

/// A sender's stand-in picture, as PNG.
fn stand_in_picture(address: &str) -> Vec<u8> {
    let seed = address.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let [r, g, b, _] = seed.to_le_bytes();
    let picture = image::RgbaImage::from_fn(96, 96, |x, y| {
        let inside = (x as i32 - 48).pow(2) + (y as i32 - 40).pow(2) < 22 * 22 || (y > 70 && (x as i32 - 48).abs() < 34);
        if inside { image::Rgba([255, 255, 255, 255]) } else { image::Rgba([r, g / 2, b, 255]) }
    });
    let mut png = std::io::Cursor::new(Vec::new());
    let _ = picture.write_to(&mut png, image::ImageFormat::Png);
    png.into_inner()
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
    // A word index beside a mailbox opened from a file (`file.open`), as the window keeps one
    // (R6.2); the sample is kept in no file and has none.
    session.index_words = true;
    // A stand-in for the host's part in signing in, when a line of the script speaks to one.
    let host: Arc<Mutex<Host>> = Default::default();
    let stood_in = lines.iter().any(|l| l.get("host").is_some());
    let mut services = if stood_in { stand_in(&host) } else { Services::default() };
    // The system's notifications, always (File › Options shows its switches where there are some).
    services.alert = Some(Box::new(StandInAlerts(host.clone())));
    // Remove MailCraft's data (#177): offered, and removes nothing here.
    services.data_wipe = Some(Box::new(StandInWipe));
    // Open MailCraft's data folder (#460): offered, and opens nothing here.
    services.data_folder = Some(mailcraft_ui_egui::data_removal::DataFolder {
        path: std::path::PathBuf::from("/home/you/.config/mailcraft"),
        open: Box::new(|_| Ok(())),
    });
    // The organisation has turned updating by itself off for the computer.
    services.updates_turned_off_by_organisation = lines.iter().any(|l| l.pointer("/host/updates").and_then(|v| v.as_str()) == Some("organisation"));
    // A newly installed version that didn't start twice running, with the one before it kept.
    if let Some(kept) =
        lines.iter().find_map(|l| l.pointer("/host/updates").and_then(|v| v.as_str())?.strip_prefix("didntStart ").map(str::to_string))
    {
        services.update_start.didnt_start = Some((env!("CARGO_PKG_VERSION").to_string(), kept));
    }
    let mut app = MailApp::new(session, services).with_control(rx);
    app.autosave = false;
    // Every frame may be a shot: a password shown with its eye is drawn as dots (#438).
    app.headless_shots = true;
    // (With a host that answers, nothing is to happen by itself: what a shot shows, its script asked for.)
    app.auto_sync = !stood_in;
    if let Some(overlay) = lines.iter().find_map(|l| l.get("overlayScrollBars").and_then(|v| v.as_bool())) {
        app.overlay_scroll_bars = overlay;
    }
    let w = lines.iter().find_map(|l| l.get("width").and_then(|v| v.as_f64())).unwrap_or(1440.0) as f32;
    let h = lines.iter().find_map(|l| l.get("height").and_then(|v| v.as_f64())).unwrap_or(900.0) as f32;
    let scale = lines.iter().find_map(|l| l.get("scale").and_then(|v| v.as_f64())).unwrap_or(2.0).clamp(0.5, 4.0) as f32;
    let mut harness =
        // Frames a sixtieth of a second apart, as on a screen: two clicks one frame after the other
        // then count as a double click (egui allows 0.3 s between them).
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs()
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
                // In front or not, for desktop alerts about new mail.
                harness.input_mut().focused = !host.unfocused;
            }
            // Every Microsoft account's To Do works (the question about the local tasks follows).
            if said.get("tasksWork").is_some() {
                harness.state_mut().stand_in_tasks_work();
            }
            // A search on the server is asked with a token of this session (invented).
            if said.get("serverSearch").is_some() {
                let app = harness.state_mut();
                let ids: Vec<u64> =
                    app.session.mailbox.accounts.iter().filter(|a| a.kind == mailcraft_model::AccountKind::Microsoft).map(|a| a.id).collect();
                for id in ids {
                    let tokens = Tokens {
                        access: "stand-in-access".into(),
                        refresh: "stand-in-refresh".into(),
                        expires_at: mailcraft_time::now() + 3_600,
                        ..Default::default()
                    };
                    app.seed_graph_token(id, tokens);
                }
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
