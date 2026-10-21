// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dialogs: command search, folder names, Go to Date, the password prompt, confirmations, About,
//! and the window that hosts the forms (`forms`: appointment, contact, task, account, categories,
//! rules).
//!
//! Dialogs only collect input: what they do, they do by running a command.

use egui::{Align2, Key, Ui, vec2};
use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::forms::Form;
use crate::theme::{Tokens, regular, semibold};
use crate::widgets::primary_button;
use crate::{MailApp, UI_COMMANDS};

#[derive(Clone, Debug, PartialEq)]
pub enum FolderMode {
    New { parent: Option<Id> },
    Rename { id: Id },
}

pub enum Dialog {
    /// Search every command by name and run one.
    Commands {
        query: String,
        selected: usize,
    },
    Folder {
        mode: FolderMode,
        name: String,
        error: String,
    },
    GoToDate {
        text: String,
        error: String,
    },
    /// The address of a shared mailbox to open under a Microsoft account (#70): it only collects
    /// the address, then runs `account.addSharedMailbox`.
    SharedMailbox {
        account: Id,
        address: String,
        error: String,
        /// The shared mailbox added for this address, while its Inbox is being read (S1): it is
        /// kept when that works, and removed again when it doesn't.
        checking: Option<Id>,
    },
    /// Where the selected text of the message being written leads (Ctrl+K).
    Link {
        url: String,
        error: String,
        /// The selection is a link already, which can be removed.
        linked: bool,
    },
    /// The password for a Send/Receive. Kept in memory only.
    Password {
        account: Id,
        password: String,
    },
    /// A message being written has unsaved changes and something wants to close it.
    DiscardDraft {
        then: String,
    },
    /// Lines to read and copy: what a check of the calendar connection came to
    /// (`calendarCheck`). (What went wrong in a Send/Receive is in Send/Receive Progress.)
    Report {
        /// What `ui.inspect` calls it.
        name: &'static str,
        title: String,
        lines: Vec<String>,
    },
    /// What the owner has just done by hand was refused at once, and why (#152): a message box
    /// with OK, as Outlook's. Never opened for the control channel, an agent or the background.
    Refused {
        text: String,
    },
    /// Something that can't be taken back is about to happen, and the owner is asked first.
    Confirm {
        command: String,
        params: Value,
        ask: Question,
    },
    /// About MailCraft, on one of its tabs: [`AboutTab`].
    About(AboutTab),
    /// The page that offers a Microsoft account kept in step by IMAP the switch to Microsoft
    /// Graph ([`crate::graph_switch`]).
    GraphSwitch {
        account: Id,
    },
    Form(Form),
    /// Propose New Time, Forward, or the list of what was proposed ([`crate::meeting_dialogs`]).
    Meeting(crate::meeting_dialogs::MeetingDialog),
}

/// The tabs of About: the program, the people who made it, and the people who test it
/// ([`crate::credits`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AboutTab {
    #[default]
    About,
    Contributors,
    Testers,
}

/// What a [`Dialog::Confirm`] asks.
pub struct Question {
    pub title: String,
    pub text: String,
    /// The button that goes ahead (`Delete`, `Empty`, `Remove`).
    pub action: String,
    /// What going ahead adds to the command's parameters; nothing (`null`) for a question whose
    /// only answer is yes, which is noted as `"confirmed": true` and asks nothing more.
    pub with: Value,
    /// Other ways of going ahead: a button each, with what it adds to the parameters.
    pub more: Vec<(String, Value)>,
    /// A note the owner may write to go along (with a cancellation, with an answer): the
    /// parameter it becomes, what is written so far, and what the empty field says.
    pub note: Option<(&'static str, String, String)>,
}

impl Question {
    /// A question with one way of going ahead, which adds `with` to the parameters.
    pub(crate) fn new(title: &str, text: String, action: &str, with: Value) -> Question {
        Question { title: title.to_string(), text, action: action.to_string(), with, more: Vec::new(), note: None }
    }

    /// One more way of going ahead.
    fn or(mut self, action: &str, with: Value) -> Question {
        self.more.push((action.to_string(), with));
        self
    }

    /// With a field for a note that becomes the parameter `key`.
    fn noted(mut self, key: &'static str, hint: &str) -> Question {
        self.note = Some((key, String::new(), hint.to_string()));
        self
    }
}

impl Dialog {
    /// The name `ui.dialog` opens it by (and `ui.inspect` reports).
    pub fn name(&self) -> &'static str {
        match self {
            Dialog::Commands { .. } => "commands",
            Dialog::Folder { .. } => "folder",
            Dialog::GoToDate { .. } => "goToDate",
            Dialog::SharedMailbox { .. } => "sharedMailbox",
            Dialog::Link { .. } => "link",
            Dialog::Password { .. } => "password",
            Dialog::DiscardDraft { .. } => "discardDraft",
            Dialog::Confirm { .. } => "confirm",
            Dialog::Report { name, .. } => name,
            Dialog::Refused { .. } => "refused",
            Dialog::About(AboutTab::About) => "about",
            Dialog::About(AboutTab::Contributors) => "contributors",
            Dialog::About(AboutTab::Testers) => "testers",
            Dialog::GraphSwitch { .. } => "graphSwitch",
            Dialog::Form(f) => f.name(),
            Dialog::Meeting(d) => d.name(),
        }
    }

    /// The dialog called `name`, set up from the app's state and `params`; `None` when it can't
    /// open now (nothing selected to edit, an unknown name).
    pub fn open(name: &str, app: &MailApp, params: &Value) -> Option<Dialog> {
        let id = |k: &str| params.get(k).and_then(Value::as_u64).filter(|i| *i != 0);
        Some(match name {
            "commands" => Dialog::Commands { query: params.get("query").and_then(Value::as_str).unwrap_or("").to_string(), selected: 0 },
            "folder" => {
                let rename = params.get("mode").and_then(Value::as_str) == Some("rename");
                if rename {
                    let f = app.session.mailbox.folder(id("id").unwrap_or(app.session.view.folder))?;
                    Dialog::Folder { mode: FolderMode::Rename { id: f.id }, name: f.name.clone(), error: String::new() }
                } else {
                    // Under the folder named, else the one in view; `"parent": null` asks for the top level.
                    let parent = match params.get("parent") {
                        Some(Value::Null) => None,
                        _ => id("parent").or(Some(app.session.view.folder)).filter(|p| app.session.mailbox.folder(*p).is_some()),
                    };
                    Dialog::Folder { mode: FolderMode::New { parent }, name: String::new(), error: String::new() }
                }
            }
            "goToDate" => Dialog::GoToDate { text: app.session.view.date.iso(), error: String::new() },
            "sharedMailbox" => {
                // Under the account named, else the first that can open one.
                let can = |a: &&mailcraft_model::Account| crate::shared_mailboxes::can_open_shared(a);
                let mailbox = &app.session.mailbox;
                let account = match id("account") {
                    Some(id) => mailbox.account(id).filter(can)?.id,
                    None => mailbox.accounts.iter().find(can)?.id,
                };
                Dialog::SharedMailbox { account, address: String::new(), error: String::new(), checking: None }
            }
            "link" => {
                let c = app.session.compose.as_ref().filter(|c| c.format == mailcraft_engine::BodyFormat::Html)?;
                let sel = c.body.selection();
                // The link the caret is in, or the selection starts in.
                let url = c.body.text().link_span(sel.start()).map(|(from, _)| c.body.text().style_at(from + 1)).and_then(|s| s.link);
                Dialog::Link { linked: url.is_some(), url: url.unwrap_or_default(), error: String::new() }
            }
            "password" => Dialog::Password { account: id("account")?, password: String::new() },
            "discardDraft" => Dialog::DiscardDraft { then: params.get("then").and_then(Value::as_str).unwrap_or("").to_string() },
            "about" => match params.get("tab").and_then(Value::as_str) {
                Some("contributors") => Dialog::About(AboutTab::Contributors),
                Some("testers") => Dialog::About(AboutTab::Testers),
                _ => Dialog::About(AboutTab::About),
            },
            "contributors" => Dialog::About(AboutTab::Contributors),
            "testers" => Dialog::About(AboutTab::Testers),
            // An item that repeats is asked about before it opens: just this one, or the series?
            "editEvent" if opening_question(app, params).is_some() => {
                Dialog::Confirm { command: "calendar.edit".to_string(), params: json!({}), ask: opening_question(app, params)? }
            }
            other => Dialog::Form(Form::open(other, app, params)?),
        })
    }
}

/// [`Dialog::open`], and the dialogs of a meeting beside it (`proposeTime`, `forwardMeeting`,
/// `proposals`), which ask the engine what they are about.
pub(crate) fn open_dialog(app: &mut MailApp, name: &str, params: &Value) -> Option<Dialog> {
    match crate::meeting_dialogs::MeetingDialog::open(name, app, params) {
        Some(dialog) => Some(Dialog::Meeting(dialog)),
        None => Dialog::open(name, app, params),
    }
}

/// What to ask before a user-run command that can't be taken back: deleting for good (also on
/// the mail server, for an account that has one), deleting a contact, emptying or deleting a
/// folder, removing an account. `None` for everything else. Scripts and agents are not asked: they said what they meant.
pub fn confirmation(app: &MailApp, id: &str, params: &Value) -> Option<Question> {
    let mb = &app.session.mailbox;
    let folder = |key: &str| params.get(key).and_then(Value::as_u64).or(Some(app.session.view.folder)).and_then(|f| mb.folder(f));
    let on_server = |account: u64| mb.account(account).is_some_and(|a| a.kind.has_server());
    let server_too = |yes: bool| if yes { format!(" {}", tl!("They are removed from the mail server too.")) } else { String::new() };
    let question = |title: &str, text: String, action: &str| Some(Question::new(title, text, action, Value::Null));
    match id {
        // Everything MailCraft keeps on this computer (#177).
        "ui.removeAllData" if app.services.data_wipe.is_some() && !app.data_removed => Some(crate::data_removal::question()),
        // An Office file with macros, or a web page, from a message (#126).
        "mail.openAttachment" => {
            let (title, text, action) = crate::attachment_open::question(app, params)?;
            question(&title, text, &action)
        }
        "mail.delete" | "mail.deletePermanently" => {
            let ids: Vec<u64> = match params.get("ids").and_then(Value::as_array) {
                Some(ids) => ids.iter().filter_map(Value::as_u64).collect(),
                None => params.get("id").and_then(Value::as_u64).map_or_else(|| app.session.sel.messages.to_vec(), |id| vec![id]),
            };
            // An ordinary delete only asks when it is for good: the message is in Deleted Items already.
            let for_good = |m: &mailcraft_model::Message| {
                id == "mail.deletePermanently"
                    || mb.folder_of_kind(m.account, mailcraft_model::FolderKind::Deleted).is_none_or(|bin| mb.subtree(bin).contains(&m.folder))
            };
            let going: Vec<&mailcraft_model::Message> = ids.iter().filter_map(|id| mb.message(*id)).filter(|m| for_good(m)).collect();
            if going.is_empty() {
                return None;
            }
            let n = going.len();
            let what = if n == 1 {
                tl!("This message will be deleted for good.").to_string()
            } else {
                tln!(n, "{n} message will be deleted for good.", "{n} messages will be deleted for good.")
            };
            let text = if going.iter().any(|m| on_server(m.account)) {
                let too = if n == 1 {
                    tl!("It is removed from the mail server too, and that can't be undone.")
                } else {
                    tl!("They are removed from the mail server too, and that can't be undone.")
                };
                format!("{what} {too}")
            } else {
                what
            };
            question(tl!("Delete for Good"), text, tl!("Delete"))
        }
        // A contact has no Deleted Items to go to: deleting one is for good (#186). One the
        // account's server keeps can't be deleted here, and the command says so without asking.
        "people.delete" => {
            let c = mb.contact(params.get("id").and_then(Value::as_u64).or(app.session.sel.contact)?)?;
            if mailcraft_engine::cmd::remote::keeper(mb, &c.origin).is_some() {
                return None;
            }
            let name = c.name();
            let name = name.trim();
            let what = if name.is_empty() {
                tl!("This contact will be deleted for good.").to_string()
            } else {
                crate::i18n::fmt(tl!("\"{name}\" will be deleted for good."), &[("name", name)])
            };
            question(tl!("Delete Contact"), format!("{what} {}", tl!("Contacts don't go to Deleted Items.")), tl!("Delete"))
        }
        "folder.empty" => {
            let f = folder("id")?;
            if mb.count(f.id) == 0 {
                return None;
            }
            let everything = crate::i18n::fmt(tl!("Everything in \"{folder}\" will be deleted for good."), &[("folder", crate::nav::folder_name(f))]);
            let text = format!("{everything}{}", server_too(on_server(f.account)));
            question(tl!("Empty Folder"), text, tl!("Empty"))
        }
        "folder.delete" => {
            let f = folder("id")?;
            let text =
                crate::i18n::fmt(tl!("The folder \"{folder}\" and everything in it will be deleted."), &[("folder", crate::nav::folder_name(f))]);
            question(tl!("Delete Folder"), text, tl!("Delete"))
        }
        "account.remove" => {
            let a = mb.account(params.get("id").and_then(Value::as_u64)?)?;
            let text = crate::i18n::fmt(
                tl!("{address} and the mail kept for it on this device will be removed. Mail on the server is not touched."),
                &[("address", &a.email)],
            );
            // Pictures and opened attachments are kept by their address and by file, not by account.
            let text = format!(
                "{text} {} {}",
                tl!("Its messages, attachments, settings and remembered sign-in go with it."),
                tl!(
                    "Downloaded pictures and opened attachments can't be told apart by account: File › Info › Remove MailCraft's data from this computer clears them."
                )
            );
            // (The shared mailboxes opened through it go with it, #70.)
            let shared = mb.shared_mailboxes(a.id).len();
            let text = match shared {
                0 => text,
                1 => format!("{text} {}", tl!("The shared mailbox opened through it is removed from this device too.")),
                n => format!(
                    "{text} {}",
                    crate::i18n::fmt(tl!("The {n} shared mailboxes opened through it are removed from this device too."), &[("n", &n.to_string())])
                ),
            };
            question(tl!("Remove Account"), text, tl!("Remove"))
        }
        // A shared mailbox is closed here only; what waits to go to it is named first (#70).
        "account.removeSharedMailbox" => {
            let a = mb.account(params.get("id").and_then(Value::as_u64)?).filter(|a| a.is_shared())?;
            let waiting = app.waiting_to_go(a.id);
            let text = crate::i18n::fmt(
                tl!("{address} and the mail kept for it on this device will be removed. The shared mailbox on the server is not touched."),
                &[("address", a.email.trim())],
            );
            let text = match waiting {
                0 => text,
                n => format!(
                    "{text} {}",
                    tln!(
                        n,
                        "{n} change made here hasn't reached the server and will be lost.",
                        "{n} changes made here haven't reached the server and will be lost."
                    )
                ),
            };
            question(tl!("Remove Shared Mailbox"), text, tl!("Remove"))
        }
        // A shorter window says what it removes from this computer, and that the server keeps it
        // (#143). Nothing to remove: nothing to ask.
        "ui.keepOffline" => {
            let account = mb.account(params.get("account").and_then(Value::as_u64)?)?;
            let keep = params.get("keepOffline").and_then(Value::as_str).and_then(mailcraft_model::KeepOffline::parse)?;
            if keep >= account.keep_offline {
                return None;
            }
            let (messages, bytes) = mailcraft_engine::cmd::offline::would_free(&app.session, account.id, keep);
            if messages == 0 {
                return None;
            }
            let text = crate::i18n::fmt(
                tl!("About {size} of mail older than {window} will be removed from this computer. It stays on the server."),
                &[("size", &mailcraft_model::size_label_in(crate::i18n::current(), bytes)), ("window", crate::keep_offline::label(keep))],
            );
            question(tl!("Mail to Keep Offline"), text, tl!("OK"))
        }
        // A check on the account's real calendar says exactly what it is about to do there.
        "ui.checkCalendar" => {
            let named = params.get("account").and_then(Value::as_u64).and_then(|id| mb.account(id));
            let account = named
                .or_else(|| mb.accounts.iter().find(|a| a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft)))?;
            let whose = if account.email.trim().is_empty() { account.name.as_str() } else { account.email.as_str() };
            let text = crate::i18n::fmt(
                tl!(
                    "MailCraft will make one appointment called \"MailCraft connection check\" in the calendar of {account}, a year from today. It reads it back, changes its time twice (the second time said in this computer's time zone), reads it back each time, and deletes it again. Nothing else in your calendar is read or changed, nobody is invited or written to, and no reminder is set. Afterwards you see what the server answered at each step, and whether what was sent is what the server has."
                ),
                &[("account", whose)],
            );
            question(tl!("Check Calendar Connection"), text, tl!("Check Now"))
        }
        // Moving what is only on this computer into the account's calendar puts it on the
        // server: asked about once, and about the invitations a meeting would send.
        "calendar.moveToAccount" if params.get("send").is_none() => {
            let only_here = mailcraft_engine::cmd::remote::only_here(mb);
            let calendars: Vec<u64> = match params.get("calendar").and_then(Value::as_u64) {
                Some(one) => vec![one],
                None => only_here.iter().map(|(calendar, _)| *calendar).collect(),
            };
            let items: Vec<&mailcraft_model::Event> =
                mb.events.values().filter(|e| calendars.contains(&e.calendar)).map(|e| e.as_ref()).take(10_000).collect();
            if items.is_empty() || mailcraft_engine::cmd::remote::server_calendar_id(mb).is_none() {
                return None;
            }
            let meetings = items.iter().filter(|e| !e.attendees.is_empty()).count();
            let count = tln!(
                items.len(),
                "{n} item is only on this computer, and not in your Outlook calendar.",
                "{n} items are only on this computer, and not in your Outlook calendar."
            );
            let mut text = format!(
                "{count} {} {}",
                tl!("Moving them puts them into your account's calendar, where Outlook and your other devices have them too."),
                tl!("One that is there already (the same title at the same time) is not made a second time: the copy on this computer goes.")
            );
            if meetings == 0 {
                return Some(Question::new(tl!("Move to Your Account's Calendar"), text, tl!("Move"), json!({"send": false})));
            }
            let invited = tln!(
                meetings,
                "{n} of them is a meeting with people invited: moving it makes the account's server send them the invitation by mail.",
                "{n} of them are meetings with people invited: moving those makes the account's server send them the invitations by mail."
            );
            text.push_str(&format!(" {invited}"));
            let ask = Question::new(tl!("Move to Your Account's Calendar"), text, tl!("Move All and Send Invitations"), json!({"send": true}));
            Some(ask.or(tl!("Move the Appointments Only"), json!({"send": false})))
        }
        // The calendar asks as Outlook does: which of a series is meant, and before anything
        // that makes the account's server write to other people ([`calendar_question`]).
        "calendar.new"
        | "calendar.newMeeting"
        | "calendar.move"
        | "calendar.edit"
        | "calendar.delete"
        | "calendar.private"
        | "calendar.importance"
        | "calendar.responseOptions"
        | "calendar.teamsMeeting"
        | "calendar.respond"
        | "ui.openWindow" => calendar_question(app, id, params),
        _ => None,
    }
}

/// The parameters of a command after its question was answered by a way of going ahead that
/// adds `with` (nothing: only that the owner said yes, `"confirmed": true`), and with the note
/// that was written, when there is one and anybody is to read it.
pub(crate) fn answered(params: &Value, with: &Value, note: Option<(&str, &str)>) -> Value {
    let mut params = if params.is_object() { params.clone() } else { json!({}) };
    let Some(map) = params.as_object_mut() else { return params };
    match with.as_object() {
        Some(more) => map.extend(more.iter().map(|(key, value)| (key.clone(), value.clone()))),
        None => {
            map.insert("confirmed".into(), json!(true));
        }
    }
    // A note goes only with what is sent.
    let sent = map.get("send").and_then(Value::as_bool) == Some(true);
    if let Some((key, note)) = note.filter(|(_, note)| sent && !note.trim().is_empty()) {
        map.insert(key.to_string(), json!(note.trim()));
    }
    params
}

/// Run a command whose question was just answered (it may have another: [`MailApp::run`] asks).
/// A recurring item that is being opened remembers which of it was meant, for what its form
/// saves ([`as_opened`]); and where an item has no window of its own, it opens in the form.
pub(crate) fn go_ahead(app: &mut MailApp, command: &str, params: Value) {
    let opening = command == "ui.openWindow" || input_dialog(command, &params) == Some("editEvent");
    if opening && let Some(event) = params.get("event").or(params.get("id")).and_then(Value::as_u64).or(app.session.sel.event) {
        let one = params.get("scope").and_then(Value::as_str) == Some("occurrence");
        let day = params.get("occurrence").and_then(Value::as_str).and_then(mailcraft_time::Date::parse_iso).filter(|_| one);
        app.calendar.opened = params.get("scope").is_some().then_some((event, day));
    }
    let opened = if command == "ui.openWindow" { app.run_or_else(command, params.clone()) } else { app.run(command, params.clone()) };
    if opened.is_err() && command == "ui.openWindow" {
        // No windows of their own here: the form, as a double click does without them.
        let which: serde_json::Map<String, Value> = params
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| matches!(key.as_str(), "scope" | "occurrence"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let _ = app.run("calendar.edit", Value::Object(which));
    }
}

/// The question an item that repeats is opened with in the form (`calendar.edit` with nothing
/// to change), unless `params` say already which of it is meant.
fn opening_question(app: &MailApp, params: &Value) -> Option<Question> {
    let mut probe = json!({"event": params.get("id").and_then(Value::as_u64).or(app.session.sel.event)?});
    for key in ["scope", "occurrence"] {
        if let Some(said) = params.get(key).filter(|said| !said.is_null()) {
            probe[key] = said.clone();
        }
    }
    calendar_question(app, "ui.openWindow", &probe)
}

/// "1 person is invited to this meeting." / "3 people are invited to this meeting."
fn invited_line(n: usize) -> String {
    tln!(n, "{n} person is invited to this meeting.", "{n} people are invited to this meeting.")
}

/// What a calendar command the user ran is asked about first, as Outlook asks. One question at
/// a time, each answered by a parameter the command takes (so nothing is asked twice, and a
/// script that says what it means is asked nothing):
///
/// 1. An item that repeats, when it is opened, moved, changed, deleted or answered: just this
///    one, or the entire series (`scope`, `occurrence`).
/// 2. Whatever makes the account's server write to other people, which it only does on the
///    owner's yes (`send`): the invitations to a new meeting, an update of a meeting the owner
///    organizes, its cancellation (with a note), the answer to somebody else's invitation (with a
///    note), and declining an invitation that is being deleted (`decline`).
/// 3. A change to the owner's own copy of somebody else's meeting, which nobody else hears of.
fn calendar_question(app: &MailApp, id: &str, params: &Value) -> Option<Question> {
    use mailcraft_engine::cmd::calendar::{Mail, mail_of};
    use mailcraft_engine::cmd::remote::keeper;
    let mb = &app.session.mailbox;
    let has = |key: &str| params.get(key).is_some_and(|v| !v.is_null());
    let note_hint = tl!("A note to go with it (optional)");
    if matches!(id, "calendar.new" | "calendar.newMeeting") {
        // A new meeting in a calendar an account's server keeps: the server sends the invitations.
        let Some(Mail::Invitation(n)) = mail_of(&app.session, id, params).filter(|_| has("title") && !has("send")) else { return None };
        let text = format!(
            "{} {}",
            invited_line(n),
            tl!("Send the invitation to them? The account's server sends it by mail as soon as the meeting is saved.")
        );
        let keep = tl!("Don't Send, Keep on This Computer");
        return Some(Question::new(tl!("Send This Invitation?"), text, tl!("Send"), json!({"send": true})).or(keep, json!({"send": false})));
    }
    let opening = id == "ui.openWindow";
    let named = if opening { params.get("event") } else { params.get("id") };
    let e = mb.event(named.and_then(Value::as_u64).or(app.session.sel.event)?)?;
    // 1. Just this one, or the entire series? Asked of the occurrence that is selected (the
    // grid selects the one it acts on); nothing is asked about what is the same for both.
    // (An occurrence is named by the day it starts on here, whatever zone its series repeats in.)
    let here = |instant: mailcraft_time::Timestamp| app.session.offset_at(instant);
    let day = app.session.sel.event_day.filter(|day| app.session.sel.event == Some(e.id) && e.series_day_on(*day, &here).is_some());
    let about_days = match id {
        "calendar.move" | "calendar.delete" | "calendar.respond" | "ui.openWindow" => true,
        // (Putting it into another calendar, and how it shows, is about the series.)
        "calendar.edit" => params.as_object().is_some_and(|m| m.keys().any(|key| !matches!(key.as_str(), "id" | "calendar" | "confirmed" | "send"))),
        _ => false,
    };
    // What the form of an item saves that was opened as one occurrence (or as the series) is
    // meant for that, whatever is selected by now: [`as_opened`] says so when the command is
    // run, so it has to be run that way (and is then asked nothing).
    let opened = app.calendar.opened.filter(|(event, _)| *event == e.id && from_form(id, params));
    let day = match opened {
        Some((_, said)) => said.or(day).or(Some(e.start.date)),
        None => day,
    };
    if let Some(day) = day.filter(|_| e.recurrence.is_some() && about_days && !has("scope") && !has("occurrence")) {
        let title = match id {
            "calendar.delete" => tl!("Delete Recurring Item"),
            "calendar.move" => tl!("Move Recurring Item"),
            "calendar.respond" => tl!("Respond to Recurring Item"),
            "ui.openWindow" => tl!("Open Recurring Item"),
            _ => tl!("Change Recurring Item"),
        };
        let name: String = e.title.chars().filter(|c| !c.is_control()).take(80).collect();
        let text = crate::i18n::fmt(
            tl!("\"{name}\" is one of a series. Do you mean just the one on {day}, or the entire series?"),
            &[("name", &name), ("day", &crate::dates::long(day))],
        );
        let one = json!({"scope": "occurrence", "occurrence": day.iso()});
        return Some(Question::new(title, text, tl!("Just This One"), one).or(tl!("The Entire Series"), json!({"scope": "series"})));
    }
    if opening {
        return None;
    }
    // 2. Mail to other people.
    match mail_of(&app.session, id, params).filter(|_| !has("send")) {
        Some(Mail::Invitation(n)) => {
            let text = format!(
                "{} {}",
                invited_line(n),
                tl!("Send the invitation to them? The account's server sends it by mail once the meeting is in its calendar.")
            );
            return Some(Question::new(tl!("Send This Invitation?"), text, tl!("Send"), json!({"send": true})));
        }
        Some(Mail::Update(n)) => {
            let text = format!("{} {}", invited_line(n), tl!("Send the update to them? The account's server sends them the change by mail."));
            return Some(Question::new(tl!("Change This Meeting?"), text, tl!("Send Update"), json!({"send": true})));
        }
        Some(Mail::Cancellation(n)) => {
            let rest = tl!("Send the cancellation to them? Deleting it cancels the meeting: the account's server tells them so by mail.");
            let ask =
                Question::new(tl!("Cancel This Meeting?"), format!("{} {rest}", invited_line(n)), tl!("Send Cancellation"), json!({"send": true}));
            return Some(ask.noted("comment", note_hint));
        }
        None => {}
    }
    // 3. Somebody else's meeting that the account's server keeps.
    keeper(mb, &e.origin)?;
    if !e.origin.guest {
        return None;
    }
    let organizer = e.organizer.as_ref().map(|who| who.display().trim().chars().take(80).collect::<String>()).filter(|who| !who.is_empty());
    let who = organizer.unwrap_or_else(|| tl!("Somebody else").to_string());
    let lead = crate::i18n::fmt(tl!("{name} organizes this meeting; you are invited to it."), &[("name", &who)]);
    match id {
        // The answer: sent now (with a note, when one is written), or kept to the calendar.
        "calendar.respond" if !has("send") => {
            let text = format!("{lead} {}", tl!("Send your answer to the organizer? You can write a note to go with it."));
            let ask = Question::new(tl!("Send a Response?"), text, tl!("Send the Response Now"), json!({"send": true}));
            Some(ask.or(tl!("Don't Send a Response"), json!({"send": false})).noted("comment", note_hint))
        }
        "calendar.delete" if !has("decline") => {
            let rest = tl!(
                "Deleting it takes it out of your calendar only. The organizer is not told that you won't come, and the other people invited still have the meeting. An update from the organizer can put it back. Declining it tells the organizer, if you choose."
            );
            let ask =
                Question::new(tl!("Delete Your Copy of This Meeting?"), format!("{lead} {rest}"), tl!("Delete My Copy"), json!({"decline": false}));
            let ask = ask.or(tl!("Decline and Tell the Organizer"), json!({"decline": true, "send": true}));
            Some(ask.or(tl!("Decline without Telling"), json!({"decline": true, "send": false})).noted("comment", note_hint))
        }
        // How it shows in the owner's own calendar (free or busy, the reminder, private) is
        // nobody else's business: no question about that.
        "calendar.move" | "calendar.edit" => {
            let visible = ["title", "location", "description", "start", "end", "allDay"].iter().any(|key| params.get(*key).is_some());
            if id == "calendar.edit" && !visible {
                return None;
            }
            let rest = tl!(
                "What you change is changed in your calendar only. The organizer and the other people invited are not told and still have the meeting as it was, so you could miss it. An update from the organizer can replace your change."
            );
            Some(Question::new(tl!("Change Your Copy of This Meeting?"), format!("{lead} {rest}"), tl!("OK"), Value::Null))
        }
        _ => None,
    }
}

/// Whether a command is what an appointment's form saves: `calendar.edit` with everything the
/// form has, and nothing said of which of a series is meant.
fn from_form(id: &str, params: &Value) -> bool {
    id == "calendar.edit" && params.get("title").is_some() && params.get("scope").is_none() && params.get("occurrence").is_none()
}

/// The parameters of a command the user ran, as they are meant. When a recurring item was opened
/// and the owner said whether one occurrence or the whole series was meant, what its form saves
/// (`calendar.edit` with everything the form has) is saved for that, without asking again.
pub fn as_opened(app: &mut MailApp, id: &str, params: Value) -> Value {
    // Opened anew, with nothing said of which is meant: what was said the last time no longer
    // holds. (What is said now, when the question is asked, is noted by [`go_ahead`].)
    if (id == "ui.openWindow" || input_dialog(id, &params) == Some("editEvent")) && params.get("scope").is_none() {
        app.calendar.opened = None;
        return params;
    }
    let Some((event, day)) = app.calendar.opened.filter(|_| from_form(id, &params)) else { return params };
    if params.get("id").and_then(Value::as_u64).or(app.session.sel.event) != Some(event) {
        return params;
    }
    // Said once, used once: the next time the item is opened, the owner is asked again.
    app.calendar.opened = None;
    let mut params = params;
    if let Some(map) = params.as_object_mut() {
        match day {
            Some(day) => {
                map.insert("scope".into(), json!("occurrence"));
                map.insert("occurrence".into(), json!(day.iso()));
            }
            None => {
                map.insert("scope".into(), json!("series"));
            }
        }
    }
    params
}

/// The dialog a user-run command opens when it lacks the input it needs (scripts and agents get
/// the command's own error instead).
pub fn input_dialog(id: &str, params: &Value) -> Option<&'static str> {
    let has = |k: &str| params.get(k).is_some_and(|v| !v.is_null());
    let empty = params.as_object().is_none_or(|m| m.is_empty());
    match id {
        "folder.new" | "folder.rename" if !has("name") => Some("folder"),
        "calendar.new" if !has("title") => Some("event"),
        "calendar.newMeeting" if !has("title") => Some("meeting"),
        // (Which of a series is meant says nothing of what to change: the form is still to come.)
        "calendar.edit" if empty || params.as_object().is_some_and(|m| m.keys().all(|key| matches!(key.as_str(), "scope" | "occurrence"))) => {
            Some("editEvent")
        }
        // What writes to other people asks who and what in a dialog of its own, and sends from
        // that dialog's button alone; what was proposed is a list to look at.
        "calendar.propose" if !has("start") => Some("proposeTime"),
        "calendar.forward" if !has("to") => Some("forwardMeeting"),
        "calendar.proposals" => Some("proposals"),
        "people.new" if empty => Some("contact"),
        "people.edit" if empty => Some("editContact"),
        "tasks.new" if !has("title") => Some("task"),
        "tasks.edit" if empty => Some("editTask"),
        "view.goToDate" if !has("date") => Some("goToDate"),
        // (Without an address, a script's `compose.link` removes the link; a person is asked.)
        "compose.link" if !has("url") => Some("link"),
        "account.add" if !has("email") => Some("account"),
        "rules.list" => Some("rules"),
        "category.list" => Some("categories"),
        _ => None,
    }
}

/// An address as typed into the link dialog, as a link: `someone@example.org` is a `mailto:`
/// address and `www.example.org` a web one; anything else as it was (the engine says whether it
/// may be a link).
pub(crate) fn link_address(typed: &str) -> String {
    let typed = typed.trim();
    if typed.contains(':') {
        typed.to_string()
    } else if typed.contains('@') && !typed.contains('/') {
        format!("mailto:{typed}")
    } else if typed.contains('.') && !typed.contains(char::is_whitespace) {
        format!("https://{typed}")
    } else {
        typed.to_string()
    }
}

/// Every command (engine and window) as `(id, label, location, shortcut, enabled)`, for search.
fn all_commands(app: &MailApp) -> Vec<(String, String, String, String, bool)> {
    let mut out: Vec<(String, String, String, String, bool)> = app
        .session
        .registry
        .all()
        .iter()
        // Commands only scripts use stay out of the list people search.
        .filter(|c| {
            !matches!(
                c.id,
                "sync.apply"
                    | "sync.request"
                    | "sync.applyEvents"
                    | "sync.applyContacts"
                    | "sync.applyCalendars"
                    | "sync.applyCategories"
                    | "calendar.freeBusy"
                    | "calendar.invitation"
                    | "sync.forgetRemote"
                    | "sync.eventChanges"
                    | "sync.applyEventChanges"
                    | "compose.set"
                    | "compose.suggest"
                    | "account.preset"
                    | "mail.select"
                    | "calendar.select"
                    | "people.select"
                    | "tasks.select"
            ) && !c.id.ends_with(".get")
                && !c.id.ends_with(".list")
        })
        .map(|c| {
            (
                c.id.to_string(),
                tl!(c.label).to_string(),
                crate::i18n::location(c.location),
                crate::widgets::shortcut_text(app, c.id),
                (c.enabled)(&app.session).is_none(),
            )
        })
        .collect();
    out.extend(UI_COMMANDS.iter().filter(|c| matches!(c.0, "ui.sync" | "ui.open" | "ui.import" | "ui.attach" | "ui.collapseRibbon")).map(
        |(id, label, location, _)| ((*id).to_string(), tl!(label).to_string(), crate::i18n::location(location), String::new(), app.enabled(id)),
    ));
    out
}

/// How well `query` matches a command: lower is better, `None` is no match. Every word must be
/// found in the label, the location or the id.
fn score(query: &str, label: &str, location: &str, id: &str) -> Option<u32> {
    let (l, loc, i) = (label.to_lowercase(), location.to_lowercase(), id.to_lowercase());
    let mut total = 0;
    for word in query.to_lowercase().split_whitespace() {
        total += if l.starts_with(word) {
            0
        } else if l.split_whitespace().any(|w| w.starts_with(word)) {
            1
        } else if l.contains(word) {
            2
        } else if i.contains(word) {
            3
        } else if loc.contains(word) {
            4
        } else {
            return None;
        };
    }
    Some(total)
}

/// The frame shared by the dialogs: a centred window with a title. Returns `false` when it was
/// closed with its close button.
pub(crate) fn window(ctx: &egui::Context, title: &str, width: f32, add: impl FnOnce(&mut Ui)) -> bool {
    let mut open = true;
    egui::Window::new(egui::RichText::new(title).font(semibold(14.0)))
        .id(egui::Id::new("dialog"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, -40.0))
        .default_width(width)
        .min_width(width)
        .max_width(width)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            add(ui);
        });
    open
}

/// A row of buttons along the right edge, one button high and no more. (A right-to-left layout
/// that centres its buttons otherwise takes all the height that is left, which leaves a gap
/// above and below them in a dialog that has room to spare.)
pub(crate) fn button_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 26.0), egui::Layout::right_to_left(egui::Align::Center), add);
}

/// OK and Cancel, right-aligned. Returns `(ok, cancel)`.
pub(crate) fn buttons(ui: &mut Ui, ok_label: &str, ok_enabled: bool) -> (bool, bool) {
    let mut result = (false, false);
    ui.add_space(4.0);
    button_row(ui, |ui| {
        result.1 = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked();
        result.0 = primary_button(ui, ok_label, ok_enabled).clicked();
    });
    result
}

pub fn show(app: &mut MailApp, ctx: &egui::Context) {
    let Some(mut dialog) = app.dialog.take() else { return };
    let escape = ctx.input(|i| i.key_pressed(Key::Escape));
    let enter = ctx.input(|i| i.key_pressed(Key::Enter));
    let t = Tokens::get(ctx);
    // A scrim: what is behind the dialog is out of reach until it is answered.
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("dialog_scrim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let resp = ui.allocate_response(screen.size(), egui::Sense::click());
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(if t.dark { 110 } else { 60 }));
        resp
    });
    let mut close = escape;
    let mut reopen: Option<Dialog> = None;
    match &mut dialog {
        Dialog::Commands { query, selected } => {
            let open = window(ctx, tl!("Search Commands"), 520.0, |ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(query).hint_text(tl!("Type what you want to do")).desired_width(f32::INFINITY).font(regular(14.0)),
                );
                resp.request_focus();
                let mut hits: Vec<(u32, (String, String, String, String, bool))> =
                    all_commands(app).into_iter().filter_map(|c| score(query, &c.1, &c.2, &c.0).map(|s| (s + u32::from(!c.4) * 10, c))).collect();
                hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.1.cmp(&b.1.1)));
                hits.truncate(200);
                if resp.changed() {
                    *selected = 0;
                }
                if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
                    *selected = (*selected + 1).min(hits.len().saturating_sub(1));
                }
                if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
                    *selected = selected.saturating_sub(1);
                }
                let mut run: Option<String> = None;
                egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                    for (i, (_, (id, label, location, shortcut, enabled))) in hits.iter().enumerate() {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), egui::Sense::click());
                        if i == *selected {
                            ui.painter().rect_filled(r, 4.0, t.selection);
                            if ui.input(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::ArrowUp)) {
                                ui.scroll_to_rect(r, None);
                            }
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 4.0, t.hover);
                        }
                        let ink = if *enabled { t.text } else { t.text_disabled };
                        let w = crate::theme::paint_clipped(
                            ui.painter(),
                            egui::pos2(r.min.x + 10.0, r.center().y),
                            r.width() * 0.5,
                            label,
                            regular(13.0),
                            ink,
                        );
                        crate::theme::paint_clipped(
                            ui.painter(),
                            egui::pos2(r.min.x + 22.0 + w, r.center().y),
                            (r.width() * 0.45 - w).max(60.0),
                            location,
                            regular(11.0),
                            t.text_dim,
                        );
                        ui.painter().text(egui::pos2(r.max.x - 10.0, r.center().y), Align2::RIGHT_CENTER, shortcut, regular(11.0), t.text_dim);
                        if resp.clicked() || (enter && i == *selected) {
                            run = Some(id.clone());
                        }
                    }
                    if hits.is_empty() {
                        ui.label(egui::RichText::new(tl!("No command matches.")).color(t.text_dim));
                    }
                });
                if let Some(id) = run {
                    close = true;
                    // After the dialog is gone, so a command that opens its own dialog can.
                    reopen = Some(Dialog::Commands { query: id, selected: usize::MAX });
                }
            });
            close |= !open;
        }
        Dialog::Folder { mode, name, error } => {
            let title = if matches!(mode, FolderMode::New { .. }) { tl!("New Folder") } else { tl!("Rename Folder") };
            let mut submit = false;
            let open = window(ctx, title, 380.0, |ui| {
                if let FolderMode::New { parent } = mode {
                    let place = parent.map(|p| crate::nav::folder_path(&app.session.mailbox, p)).unwrap_or_else(|| tl!("the top level").to_string());
                    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("In {place}"), &[("place", &place)])).color(t.text_dim));
                }
                let resp = ui.add(egui::TextEdit::singleline(name).hint_text(tl!("Folder name")).desired_width(f32::INFINITY));
                if !resp.has_focus() && error.is_empty() {
                    resp.request_focus();
                }
                if !error.is_empty() {
                    ui.label(egui::RichText::new(error.as_str()).color(t.red));
                }
                let (ok, cancel) = buttons(ui, tl!("OK"), !name.trim().is_empty());
                submit = ok || (enter && !name.trim().is_empty());
                close |= cancel;
            });
            close |= !open;
            if submit {
                let r = match mode {
                    FolderMode::New { parent } => app.execute("folder.new", json!({"name": name, "parent": parent})),
                    FolderMode::Rename { id } => app.execute("folder.rename", json!({"id": id, "name": name})),
                };
                match r {
                    Ok(_) => close = true,
                    Err(e) => *error = e,
                }
            }
        }
        Dialog::GoToDate { text, error } => {
            let mut submit = false;
            let open = window(ctx, tl!("Go to Date"), 320.0, |ui| {
                ui.label(tl!("Date (YYYY-MM-DD)"));
                let resp = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                if !resp.has_focus() && error.is_empty() {
                    resp.request_focus();
                }
                if !error.is_empty() {
                    ui.label(egui::RichText::new(error.as_str()).color(t.red));
                }
                let (ok, cancel) = buttons(ui, tl!("Go"), !text.trim().is_empty());
                submit = ok || enter;
                close |= cancel;
            });
            close |= !open;
            if submit {
                match app.execute("view.goToDate", json!({"date": text.trim()})) {
                    Ok(_) => close = true,
                    Err(e) => *error = e,
                }
            }
        }
        Dialog::SharedMailbox { account, address, error, checking } => {
            let mut submit = false;
            let owner = app.session.mailbox.account(*account).map(|a| a.email.clone()).unwrap_or_default();
            // (One that is being checked and is gone was answered: [`MailApp::finish_shared_check`].)
            if checking.is_some_and(|id| !app.shared_checking.contains(&id)) {
                *checking = None;
            }
            let open = window(ctx, tl!("Add Shared Mailbox"), 420.0, |ui| {
                ui.label(tl!("The shared mailbox's e-mail address"));
                let field = egui::TextEdit::singleline(address).hint_text("support@example.org").desired_width(f32::INFINITY);
                let resp = ui.add_enabled(checking.is_none(), field);
                if checking.is_none() && !resp.has_focus() && error.is_empty() {
                    resp.request_focus();
                }
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("Opened with {owner}'s sign-in. It needs Full Access to the mailbox, which your organisation's administrator gives. Microsoft asks you once to let MailCraft read and change shared mailboxes. What you change in it goes to it. Mail you write in it is from its address, and sending as it needs Send As or Send on Behalf permission: Microsoft asks you once more at your first send."),
                        &[("owner", &owner)],
                    ))
                    .color(t.text_dim)
                    .size(11.0),
                );
                if checking.is_some() {
                    ui.label(egui::RichText::new(tl!("Checking the address with Microsoft…")).color(t.text_dim));
                }
                if !error.is_empty() {
                    ui.label(egui::RichText::new(error.as_str()).color(t.red));
                }
                let (ok, cancel) = buttons(ui, tl!("Add"), checking.is_none() && !address.trim().is_empty());
                submit = checking.is_none() && (ok || enter);
                close |= cancel;
            });
            close |= !open;
            // Called off while it was being checked: it isn't kept.
            if close && let Some(id) = checking.take() {
                app.drop_shared_check(id);
            }
            if submit && !close {
                error.clear();
                match app.execute("account.addSharedMailbox", json!({"account": *account, "address": address.trim()})) {
                    // Kept only once its Inbox has been read: that is also when the owner is asked
                    // for the permission (S1).
                    Ok(added) => match added.get("id").and_then(Value::as_u64) {
                        Some(id) => match app.begin_shared_check(id) {
                            Ok(()) => *checking = Some(id),
                            Err(e) => *error = e,
                        },
                        None => *error = tl!("The shared mailbox couldn't be added").to_string(),
                    },
                    Err(e) => *error = e,
                }
            }
        }
        Dialog::Link { url, error, linked } => {
            let (mut submit, mut remove) = (false, false);
            let open = window(ctx, tl!("Insert Link"), 420.0, |ui| {
                ui.label(tl!("Address"));
                let resp = ui.add(egui::TextEdit::singleline(url).hint_text("https://www.example.org").desired_width(f32::INFINITY));
                if !resp.has_focus() && error.is_empty() {
                    resp.request_focus();
                }
                ui.label(
                    egui::RichText::new(tl!(
                        "A web address, an email address or a phone number (tel:). With nothing selected, the address is put in as it is."
                    ))
                    .color(t.text_dim)
                    .size(11.0),
                );
                if !error.is_empty() {
                    ui.label(egui::RichText::new(error.as_str()).color(t.red));
                }
                if *linked {
                    remove = ui.button(tl!("Remove Link")).clicked();
                }
                let (ok, cancel) = buttons(ui, tl!("OK"), !url.trim().is_empty());
                submit = ok || (enter && !url.trim().is_empty());
                close |= cancel;
            });
            close |= !open;
            if remove {
                let _ = app.execute("compose.link", json!({}));
                close = true;
            } else if submit {
                match app.execute("compose.link", json!({"url": link_address(url)})) {
                    Ok(_) => close = true,
                    Err(e) => *error = e,
                }
            }
            // The keyboard goes back to the text.
            app.body_focus_wanted |= close;
        }
        Dialog::Password { account, password } => {
            let who = app.session.mailbox.account(*account).map(|a| a.email.clone()).unwrap_or_default();
            let mut submit = false;
            let open = window(ctx, tl!("Sign In"), 380.0, |ui| {
                ui.label(crate::i18n::fmt(tl!("Password for {account}"), &[("account", &who)]));
                let resp = ui.add(egui::TextEdit::singleline(password).password(true).desired_width(f32::INFINITY));
                if !resp.has_focus() {
                    resp.request_focus();
                }
                ui.label(
                    egui::RichText::new(tl!("MailCraft keeps it in memory until you close the app. It is never written to disk."))
                        .color(t.text_dim)
                        .size(11.0),
                );
                let (ok, cancel) = buttons(ui, tl!("Send/Receive"), !password.is_empty());
                submit = ok || (enter && !password.is_empty());
                close |= cancel;
            });
            close |= !open;
            if submit {
                let r = app.execute("ui.sync", json!({"account": account, "password": password}));
                // The typed password doesn't linger in the dialog's memory.
                password.clear();
                if let Err(e) = r {
                    app.status(e);
                }
                close = true;
            }
        }
        Dialog::Report { title, lines, .. } => {
            let open = window(ctx, title, 560.0, |ui| {
                egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                    for line in lines.iter() {
                        // Selectable, so a line can be copied by itself too.
                        ui.add(egui::Label::new(line.as_str()).selectable(true).wrap());
                    }
                });
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0))).clicked() {
                        close = true;
                    }
                    if ui.add(egui::Button::new(tl!("Copy")).min_size(vec2(72.0, 26.0))).on_hover_text(tl!("Copy all of it")).clicked() {
                        ui.ctx().copy_text(lines.join("\n"));
                    }
                });
            });
            close |= !open;
        }
        Dialog::Refused { text } => {
            let open = window(ctx, "MailCraft", 420.0, |ui| {
                ui.horizontal_top(|ui| {
                    // A warning mark drawn here: a round red dot with a white bar.
                    let (mark, _) = ui.allocate_exact_size(vec2(28.0, 28.0), egui::Sense::hover());
                    ui.painter().circle_filled(mark.center(), 13.0, t.red);
                    let bar = egui::Rect::from_center_size(mark.center(), vec2(3.0, 12.0)).translate(vec2(0.0, -2.5));
                    ui.painter().rect_filled(bar, 1.0, t.on_accent);
                    ui.painter().circle_filled(mark.center() + vec2(0.0, 7.0), 1.8, t.on_accent);
                    ui.add(egui::Label::new(text.as_str()).selectable(true).wrap());
                });
                ui.add_space(6.0);
                button_row(ui, |ui| {
                    let ok = primary_button(ui, tl!("OK"), true);
                    app.hit.add("refusedOk", 0, ok.rect);
                    close |= ok.clicked();
                });
            });
            // OK is what Enter means here too.
            close |= !open || enter;
        }
        Dialog::Confirm { command, params, ask } => {
            // The way of going ahead that was picked: what it adds to the parameters.
            let mut picked: Option<Value> = None;
            let open = window(ctx, &ask.title, if ask.more.is_empty() { 420.0 } else { 480.0 }, |ui| {
                ui.label(ask.text.as_str());
                if let Some((_, note, hint)) = &mut ask.note {
                    ui.add(egui::TextEdit::multiline(note).hint_text(hint.as_str()).desired_rows(2).desired_width(f32::INFINITY).char_limit(2_000));
                }
                ui.add_space(6.0);
                // Cancel is the one Enter and Escape reach: going ahead takes a deliberate click.
                if ask.more.len() >= 2 {
                    // Several ways of going ahead: one under the other, each saying what it does.
                    for (label, with) in std::iter::once((&ask.action, &ask.with)).chain(ask.more.iter().map(|(label, with)| (label, with))) {
                        if ui.add(egui::Button::new(label.as_str()).min_size(vec2(ui.available_width(), 28.0))).clicked() {
                            picked = Some(with.clone());
                        }
                    }
                    button_row(ui, |ui| {
                        if ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked() {
                            close = true;
                        }
                    });
                } else {
                    button_row(ui, |ui| {
                        if ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked() {
                            close = true;
                        }
                        for (label, with) in ask.more.iter().rev() {
                            if ui.add(egui::Button::new(label.as_str()).min_size(vec2(72.0, 26.0))).clicked() {
                                picked = Some(with.clone());
                            }
                        }
                        if ui.add(egui::Button::new(ask.action.as_str()).min_size(vec2(72.0, 26.0))).clicked() {
                            picked = Some(ask.with.clone());
                        }
                    });
                }
            });
            close |= !open;
            if let Some(with) = picked {
                close = true;
                let command = command.clone();
                let params = answered(params, &with, ask.note.as_ref().map(|(key, note, _)| (*key, note.as_str())));
                go_ahead(app, &command, params);
            }
        }
        Dialog::DiscardDraft { then } => {
            let mut answer: Option<&str> = None;
            let open = window(ctx, tl!("Unsaved Message"), 400.0, |ui| {
                ui.label(tl!("The message you are writing has changes that aren't saved."));
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked() {
                        answer = Some("cancel");
                    }
                    if ui.add(egui::Button::new(tl!("Discard")).min_size(vec2(72.0, 26.0))).clicked() {
                        answer = Some("discard");
                    }
                    if primary_button(ui, tl!("Save Draft"), true).clicked() {
                        answer = Some("save");
                    }
                });
            });
            close |= !open;
            if let Some(a) = answer {
                close = true;
                let done = match a {
                    "save" => app.execute("compose.saveDraft", json!({})).and_then(|_| app.execute("compose.close", json!({}))).is_ok(),
                    "discard" => app.execute("compose.close", json!({})).is_ok(),
                    _ => false,
                };
                if done && then == "quit" {
                    app.quit_requested = true;
                }
            }
        }
        Dialog::About(tab) => {
            let width = match tab {
                AboutTab::About => 420.0,
                AboutTab::Contributors => 720.0,
                AboutTab::Testers => 520.0,
            };
            let open = window(ctx, tl!("About MailCraft"), width, |ui| {
                ui.horizontal(|ui| {
                    for (which, label) in [(AboutTab::About, "About"), (AboutTab::Contributors, "Contributors"), (AboutTab::Testers, "Testers")] {
                        if ui.selectable_label(*tab == which, tl!(label)).clicked() {
                            *tab = which;
                        }
                    }
                });
                ui.separator();
                if *tab != AboutTab::About {
                    if *tab == AboutTab::Testers {
                        crate::credits::testers_ui(ui, &t);
                    } else {
                        crate::credits::contributors_ui(ui, &t);
                    }
                    ui.add_space(4.0);
                    button_row(ui, |ui| {
                        if primary_button(ui, tl!("Close"), true).clicked() || enter {
                            close = true;
                        }
                    });
                    return;
                }
                ui.label(egui::RichText::new("MailCraft").font(semibold(20.0)).color(t.accent_text));
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(tl!("Version {version}"), &[("version", env!("CARGO_PKG_VERSION"))])).color(t.text_dim),
                );
                ui.label(tl!("Mail, calendar, people and tasks, written from scratch in Rust."));
                crate::backstage::contributors(ui);
                ui.label(tl!("Open source under the MIT License or the Apache License 2.0, at your option. The interface fonts are Inter, Source Sans 3 and JetBrains Mono (SIL Open Font License). Every icon is drawn in code."));
                let parity = mailcraft_engine::catalog::parity(&app.session.registry);
                ui.label(
                    egui::RichText::new(crate::i18n::fmt(
                        tl!("{commands} commands; {percent}% of the feature catalog has one."),
                        &[("commands", &parity["commands"].to_string()), ("percent", &parity["percent"].to_string())],
                    ))
                    .color(t.text_dim),
                );
                ui.add_space(4.0);
                button_row(ui, |ui| {
                    if primary_button(ui, tl!("Close"), true).clicked() || enter {
                        close = true;
                    }
                });
            });
            close |= !open;
        }
        Dialog::GraphSwitch { account } => {
            let account = *account;
            // (Escape doesn't answer it: closed with its close button, it is asked again next time.)
            close = crate::graph_switch::show(app, ctx, account);
        }
        Dialog::Form(form) => {
            let title = form.title();
            let mut done = false;
            let open = window(ctx, &title, form.width(), |ui| done = form.show(app, ui));
            close |= !open || done;
        }
        // (Enter is nobody's in these: what they send, they send from a button.)
        Dialog::Meeting(dialog) => {
            let mut done = false;
            let open = window(ctx, dialog.title(), dialog.width(), |ui| done = dialog.show(app, ui));
            close |= !open || done;
        }
    }
    if !close {
        app.dialog = Some(dialog);
        return;
    }
    // Command search hands over the command it picked.
    if let Some(Dialog::Commands { query: id, selected: usize::MAX }) = reopen {
        let id = match id.as_str() {
            "sync.request" => "ui.sync".to_string(),
            // Asked on its page, which says what the switch does.
            "account.graphSwitch" => "ui.graphSwitch".to_string(),
            _ => id,
        };
        let _ = app.run(&id, json!({}));
    }
}

/// What the owner answered when a window with unsaved changes was about to close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unsaved {
    Save,
    Discard,
    Cancel,
}

/// The question a window of its own asks before it closes on changes that aren't saved, as
/// Outlook asks it: save them, don't, or stay. It is laid out in the window that is being laid
/// out (`ctx`), over everything else in it, and nothing under it can be reached until it is
/// answered. `window` tells the questions of several windows apart; where its three buttons are
/// goes into `hit` (`unsavedSave`, `unsavedDiscard`, `unsavedCancel`, by the window's number).
/// Enter saves, Escape stays. `None` until there is an answer.
pub(crate) fn unsaved_changes(ctx: &egui::Context, hit: &mut crate::control::HitMap, window: u64, name: &str) -> Option<Unsaved> {
    let t = Tokens::get(ctx);
    let mut answer = None;
    let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
    let text = if name.trim().is_empty() {
        tl!("This item has changes that aren't saved. Save them?").to_string()
    } else {
        crate::i18n::fmt(tl!("\"{name}\" has changes that aren't saved. Save them?"), &[("name", name.trim())])
    };
    let frame = egui::Frame::window(&ctx.global_style()).fill(t.surface).inner_margin(egui::Margin::same(16));
    let width = (ctx.content_rect().width() - 48.0).clamp(120.0, 420.0);
    egui::Modal::new(egui::Id::new(("unsaved-changes", window)))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(if t.dark { 110 } else { 60 }))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.label(egui::RichText::new(tl!("Unsaved Changes")).font(semibold(14.0)));
            ui.label(text);
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cancel = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0)));
                let discard = ui.add(egui::Button::new(tl!("Don't Save")).min_size(vec2(72.0, 26.0)));
                let save = primary_button(ui, tl!("Save"), true);
                for (kind, button, said) in [
                    ("unsavedCancel", &cancel, Unsaved::Cancel),
                    ("unsavedDiscard", &discard, Unsaved::Discard),
                    ("unsavedSave", &save, Unsaved::Save),
                ] {
                    hit.add(kind, window, button.rect);
                    if button.clicked() {
                        answer = Some(said);
                    }
                }
            });
        });
    // Saving is what Enter means, staying what Escape means; neither is anything else's key
    // while the question is up.
    let (enter, escape) = ctx.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, Key::Enter), i.consume_key(egui::Modifiers::NONE, Key::Escape)));
    answer.or(if escape {
        Some(Unsaved::Cancel)
    } else if enter {
        Some(Unsaved::Save)
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_scores_prefixes_first() {
        assert_eq!(score("rep", "Reply", "Home › Respond", "mail.reply"), Some(0));
        assert_eq!(score("all", "Reply All", "Home › Respond", "mail.replyAll"), Some(1));
        assert_eq!(score("ply", "Reply", "Home › Respond", "mail.reply"), Some(2));
        assert_eq!(score("respond", "Reply", "Home › Respond", "mail.reply"), Some(4));
        assert_eq!(score("reply all", "Reply All", "", ""), Some(1));
        assert_eq!(score("reply zebra", "Reply All", "", ""), None);
        assert_eq!(score("", "Anything", "", ""), Some(0));
    }

    #[test]
    fn commands_missing_input_ask_for_it() {
        assert_eq!(input_dialog("folder.new", &json!({})), Some("folder"));
        assert_eq!(input_dialog("folder.new", &json!({"name": "x"})), None);
        assert_eq!(input_dialog("calendar.new", &json!({})), Some("event"));
        assert_eq!(input_dialog("calendar.edit", &json!({})), Some("editEvent"));
        assert_eq!(input_dialog("calendar.edit", &json!({"title": "x"})), None);
        assert_eq!(input_dialog("tasks.edit", &json!({"due": null})), None);
        assert_eq!(input_dialog("mail.delete", &json!({})), None);
    }
}
