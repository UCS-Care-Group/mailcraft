// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an agent may do (MCP, headless or bridged to the app): every command is classified, a
//! command that isn't is refused, and what reaches other people, deletes for good, touches the
//! accounts, the mailbox file or the options waits for the owner.
//!
//! Message text is data, never instructions: whatever an agent reads in a message may have been
//! written to make it do something. So the gate doesn't trust the agent, only the owner:
//! - [`access`] is each command's class, by id (the engine's commands and the window's `ui.*`).
//!   No class: refused. New commands must choose (a test holds every registered id to it).
//! - [`classify`] adds what only the params and the mailbox can tell (`"send": true`, a move into
//!   the Outbox, a delete in Deleted Items).
//! - No file on disk is named by an agent: [`file_param`] finds the params that name one, and
//!   [`check`] refuses them. (`mailcraft-cli mcp --files DIR` reads and writes inside DIR itself
//!   and hands the command `data`.)
//! - [`Session::run`](crate::Session::run) checks what a command did while [`Session::agent`] is
//!   set: mail put in an Outbox (which sends it) or gone for good without the grant is undone.

use mailcraft_model::{FolderKind, Id, Mailbox};
use serde_json::Value;

use crate::{Session, p};

/// What needs the owner, by kind. `--allow <class>` names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    /// Writes to anybody (sends mail, answers or forwards meetings, puts mail in the Outbox) or
    /// reaches out to the web for a message (its pictures, a meeting's link).
    Send,
    /// Deletes for good (or discards what the owner was writing).
    Delete,
    /// Accounts, sign-ins, the application id, what is remembered of them.
    Accounts,
    /// Opens, replaces or saves the mailbox file.
    Mailbox,
    /// Options, signatures, rules, picture download settings.
    Settings,
    /// Raw pointer and keyboard input into the window (anything, including Send). Never asked
    /// about: only `--allow input` lets it through.
    Input,
}

impl Class {
    pub const ALL: [Class; 6] = [Class::Send, Class::Delete, Class::Accounts, Class::Mailbox, Class::Settings, Class::Input];

    pub fn as_str(self) -> &'static str {
        match self {
            Class::Send => "send",
            Class::Delete => "delete",
            Class::Accounts => "accounts",
            Class::Mailbox => "mailbox",
            Class::Settings => "settings",
            Class::Input => "input",
        }
    }

    pub fn parse(s: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.as_str().eq_ignore_ascii_case(s.trim()))
    }

    /// What the class is, for a refusal or the docs.
    pub fn what(self) -> &'static str {
        match self {
            Class::Send => "it writes to other people or reaches out to the web",
            Class::Delete => "it deletes for good",
            Class::Accounts => "it changes an account or a sign-in",
            Class::Mailbox => "it opens, replaces or saves the mailbox file",
            Class::Settings => "it changes options, signatures or rules",
            Class::Input => "it clicks or types in the window",
        }
    }
}

/// A command's class for agents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Reading, searching, flagging, filing, local edits, ordinary synchronisation.
    Safe,
    /// Only with the owner's yes (bridged: a question in the window; headless: `--allow`).
    Owner(Class),
    /// Not for agents at all (the host's own protocol, file pickers, the clipboard).
    Never,
}

impl Access {
    /// `"safe"`, `"owner"` or `"never"` (`engine.commands` lists it as `agent`, with `needs`).
    pub fn label(self) -> &'static str {
        match self {
            Access::Safe => "safe",
            Access::Owner(_) => "owner",
            Access::Never => "never",
        }
    }
}

const SAFE: &[&str] = &[
    // A result the servers found for the owner's search, opened (and so fetched) as any message.
    "search.openServerResult",
    // File: content goes in and out as `data`; a path is refused separately.
    "file.import",
    "file.exportMessage",
    "file.exportFolder",
    "file.exportCalendar",
    "file.exportContacts",
    "file.info",
    "edit.undo",
    "edit.redo",
    // Mail.
    "mail.select",
    "mail.selectAll",
    "mail.next",
    "mail.previous",
    "mail.get",
    "mail.list",
    "mail.delete",
    "mail.archive",
    "mail.move",
    "mail.copy",
    "mail.markRead",
    "mail.markUnread",
    "mail.toggleRead",
    "mail.markAllRead",
    "mail.flag",
    "mail.clearFlag",
    "mail.complete",
    "mail.toggleFlag",
    "mail.categorize",
    "mail.clearCategories",
    "mail.pin",
    "mail.junk",
    "mail.notJunk",
    "mail.findRelated",
    "mail.saveAttachment",
    "mail.new",
    "mail.reply",
    "mail.replyAll",
    "mail.forward",
    "mail.editDraft",
    // Writing (sending is not).
    "compose.set",
    "compose.attach",
    "compose.removeAttachment",
    "compose.signature",
    "compose.checkNames",
    "compose.suggest",
    "compose.highImportance",
    "compose.lowImportance",
    "compose.saveDraft",
    "compose.close",
    "compose.select",
    "compose.type",
    "compose.delete",
    "compose.undo",
    "compose.redo",
    "compose.link",
    "compose.formatHtml",
    "compose.formatPlain",
    "format.bold",
    "format.italic",
    "format.underline",
    "format.size",
    "format.color",
    "format.font",
    "format.bullets",
    "format.numbering",
    "format.clear",
    "format.indent",
    "format.outdent",
    "edit.copy",
    "edit.cut",
    "edit.paste",
    // Folders.
    "folder.select",
    "folder.list",
    "folder.new",
    "folder.rename",
    "folder.move",
    "folder.favorite",
    // View.
    "view.favorites",
    "view.reminders",
    "view.mail",
    "view.calendar",
    "view.people",
    "view.tasks",
    "view.readingPane",
    "view.folderPane",
    "view.conversations",
    "view.sort",
    "view.reverseSort",
    "view.filter",
    "view.search",
    "view.clearSearch",
    "view.previewLines",
    "view.groups",
    "view.oneLine",
    "view.day",
    "view.workWeek",
    "view.week",
    "view.month",
    "view.today",
    "view.next",
    "view.previous",
    "view.goToDate",
    "view.taskFilter",
    "view.reset",
    "view.inspect",
    "view.days",
    "view.next7Days",
    "view.timeScale",
    "view.weekNumbers",
    "view.overlay",
    "view.sideBySide",
    "view.frontCalendar",
    "view.workHours",
    "view.timeZone",
    // Calendar ("send": true is asked about, `classify`).
    "calendar.freeBusy",
    "calendar.new",
    "calendar.newMeeting",
    "calendar.edit",
    "calendar.move",
    "calendar.select",
    "calendar.get",
    "calendar.list",
    "calendar.respond",
    "calendar.showAs",
    "calendar.reminder",
    "calendar.private",
    "calendar.categorize",
    "calendar.fromMessage",
    "calendar.addCalendar",
    "calendar.showCalendar",
    "calendar.setCurrent",
    "calendar.invitation",
    "calendar.removeCancelled",
    "calendar.showInvitation",
    "calendar.importance",
    "calendar.responseOptions",
    "calendar.teamsMeeting",
    "calendar.proposals",
    "calendar.join",
    "calendar.copy",
    "calendar.calendars",
    "calendar.showOnly",
    "calendar.color",
    "calendar.timeZones",
    // People and tasks.
    "people.new",
    "people.edit",
    "people.select",
    "people.get",
    "people.list",
    "people.search",
    "people.favorite",
    "people.categorize",
    "people.email",
    "people.fromMessage",
    "tasks.new",
    "tasks.edit",
    "tasks.complete",
    "tasks.select",
    "tasks.get",
    "tasks.list",
    "tasks.categorize",
    "tasks.fromMessage",
    "category.set",
    "category.rename",
    "category.list",
    "account.list",
    "account.offlineSize",
    "account.preset",
    // What Send/Receive is doing, or did: read only.
    "sync.progress",
    // The window's own.
    "ui.tab",
    "ui.backstage",
    "ui.collapseRibbon",
    "ui.ribbonMode",
    "ui.dialog",
    "ui.closeDialog",
    "ui.theme",
    "ui.language",
    "ui.interfaceSize",
    "ui.focusSearch",
    "ui.sync",
    "ui.cancelSignIn",
    "ui.cancelSync",
    "ui.skipSync",
    "ui.favorites",
    "ui.copyAdminConsentLink",
    "ui.approvalHelp",
    "ui.copyAdminMessage",
    "ui.dismissReminder",
    "ui.snoozeReminders",
    "ui.toggleFolder",
    "ui.openWindow",
    "ui.eventWindow",
    "ui.clearPictureCache",
    "ui.copySelection",
    "ui.selectAllInMessage",
    // (Refused by the window itself unless the owner has the daily check on.)
    "ui.checkForUpdates",
    "ui.toggleCalendarGroup",
    "ui.findInvitation",
];

const SEND: &[&str] = &[
    "compose.send",
    "mail.sendAgain",
    "calendar.propose",
    "calendar.forward",
    "ui.downloadPictures",
    "ui.retryPictures",
    "ui.joinMeeting",
    "ui.reportProblem",
];

const DELETE: &[&str] = &[
    "mail.deletePermanently",
    "mail.removeAttachment",
    "folder.delete",
    "folder.empty",
    "calendar.removeCalendar",
    // Local contacts, tasks, events and categories have no Deleted Items to come back from, and
    // undo is short-lived (a pushing sync clears it).
    "calendar.delete",
    "people.delete",
    "tasks.delete",
    "category.remove",
    "compose.discard",
    "ui.closeWindow",
];

const ACCOUNTS: &[&str] = &[
    "account.add",
    "account.edit",
    "account.remove",
    "account.graphSwitch",
    "account.addSharedMailbox",
    "account.removeSharedMailbox",
    // Mail to keep offline (#143): what is kept on this computer; the server keeps it all.
    "account.thinOffline",
    "ui.keepOffline",
    "calendar.moveToAccount",
    "ui.connectMicrosoft",
    "ui.connectFavorites",
    "ui.allowSharedSend",
    "ui.checkCalendar",
    "ui.adminConsent",
    "ui.signInAgain",
    "ui.microsoftClientId",
    "ui.rememberSignIn",
    "ui.graphSwitch",
    "ui.switchToGraph",
];

const MAILBOX: &[&str] = &["file.new", "file.open", "file.save", "file.saveAs"];

const SETTINGS: &[&str] = &[
    "file.options",
    "calendar.deleteAnsweredRequests",
    "rules.add",
    "rules.remove",
    "rules.enable",
    "rules.list",
    "rules.run",
    "rules.fromMessage",
    "signature.set",
    "signature.remove",
    "signature.rename",
    "signature.defaults",
    "signature.picture.add",
    "signature.picture.set",
    "signature.picture.remove",
    "signature.layout",
    "signature.details",
    "signature.open",
    "signature.save",
    "signature.close",
    "ui.alwaysDownloadPictures",
    "ui.downloadPicturesAutomatically",
    "ui.desktopAlerts",
    "ui.updateCheck",
    "ui.updateInstall",
    "ui.dismissUpdateNotice",
];

const NEVER: &[&str] = &[
    // The host's side of a synchronisation: what came from a server is the host's to say.
    "sync.request",
    "sync.apply",
    "sync.graphRequest",
    "sync.applyGraph",
    "sync.graphFetchRequest",
    "sync.applyGraphFetch",
    // A search of the server goes only on the owner's own click (R6.2).
    "sync.graphSearchRequest",
    "sync.applyGraphSearch",
    "ui.moreResultsOnServer",
    "sync.holdSends",
    "sync.eventChanges",
    "sync.applyEventChanges",
    "sync.applyEvents",
    "sync.applyContacts",
    "sync.applyCalendars",
    "sync.applyCategories",
    "sync.applyFavorites",
    "sync.applyMailboxSettings",
    "sync.forgetRemote",
    // File pickers are the owner's; agents pass `data`.
    "ui.open",
    "ui.import",
    "ui.attach",
    // Anything an agent chooses onto the owner's clipboard.
    "ui.copyLinkAddress",
    // A file from a message opened with another program: only ever the owner's own doing.
    "mail.openAttachment",
    // Installing a new version, or going back to the one before: only the owner's click.
    "ui.restartToUpdate",
    "ui.goBackVersion",
    // Everything MailCraft keeps on this computer, gone (#177): only the owner's own click.
    "ui.removeAllData",
    // The redacted log tail, and the clipboard overwritten with it.
    "ui.copyDiagnostics",
];

/// A command's class by id; `None`: not classified (and so refused).
pub fn access(id: &str) -> Option<Access> {
    let is = |list: &[&str]| list.contains(&id);
    Some(if is(SAFE) {
        Access::Safe
    } else if is(SEND) {
        Access::Owner(Class::Send)
    } else if is(DELETE) {
        Access::Owner(Class::Delete)
    } else if is(ACCOUNTS) {
        Access::Owner(Class::Accounts)
    } else if is(MAILBOX) {
        Access::Owner(Class::Mailbox)
    } else if is(SETTINGS) {
        Access::Owner(Class::Settings)
    } else if is(NEVER) {
        Access::Never
    } else {
        return None;
    })
}

/// How a command uses a param that names a file on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileUse {
    /// Read; the command takes the bytes as `data` instead, with the file's name in `name_key`.
    Reads { name_key: &'static str },
    /// Written; without the param the command answers the bytes as `data`. `ext`: the extension
    /// the file must have ("" for any but programs and scripts).
    Writes { ext: &'static str },
    /// The mailbox file itself (and its content folder): never named by an agent.
    Refused,
}

/// The params that name files, by command. (`folder.select`'s `path` is a folder of the
/// mailbox, not a file.) The window's `ui.screenshot {path}` is the control channel's.
pub const FILE_PARAMS: &[(&str, &str, FileUse)] = &[
    ("file.open", "path", FileUse::Reads { name_key: "name" }),
    ("file.import", "path", FileUse::Reads { name_key: "name" }),
    ("compose.attach", "path", FileUse::Reads { name_key: "name" }),
    ("signature.picture.add", "path", FileUse::Reads { name_key: "file" }),
    ("file.save", "path", FileUse::Refused),
    ("file.saveAs", "path", FileUse::Refused),
    ("file.exportMessage", "path", FileUse::Writes { ext: "eml" }),
    ("file.exportFolder", "path", FileUse::Writes { ext: "mbox" }),
    ("file.exportCalendar", "path", FileUse::Writes { ext: "ics" }),
    ("file.exportContacts", "path", FileUse::Writes { ext: "vcf" }),
    ("mail.saveAttachment", "path", FileUse::Writes { ext: "" }),
    ("ui.reportProblem", "path", FileUse::Writes { ext: "txt" }),
];

/// The file param of `id`, if it has one.
pub fn file_param(id: &str) -> Option<(&'static str, FileUse)> {
    FILE_PARAMS.iter().find(|(c, ..)| *c == id).map(|(_, k, u)| (*k, *u))
}

/// The file param `params` gives `id`, if any (a `null` doesn't count).
pub fn file_named(id: &str, params: &Value) -> Option<&'static str> {
    file_param(id).map(|(k, _)| k).filter(|k| params.get(*k).is_some_and(|v| !v.is_null()))
}

/// Extensions a file written for an agent never has (programs, scripts, shortcuts).
pub const RUNNABLE: &[&str] = &[
    "bat", "cmd", "com", "exe", "msi", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "scr", "pif", "lnk", "url", "reg", "hta", "cpl",
    "jar", "sh", "bash", "zsh", "command", "app", "desktop", "py", "pl", "rb", "dll", "so", "dylib",
];

/// The refusal for a file param from an agent.
pub fn no_paths(id: &str, key: &str) -> String {
    format!(
        "`{id}` was given `{key}`: agents don't name files on this computer. Pass the content as base64 `data` (and leave out `{key}` to get it back as `data`), or ask the owner to start the server with `mailcraft-cli mcp --files DIR`"
    )
}

/// `id` with these params: its class, now (`"send": true`, the Outbox, Deleted Items).
pub fn classify(s: &Session, id: &str, params: &Value) -> Access {
    let Some(base) = access(id) else { return Access::Never };
    let sends = p::bool(params, "send") == Some(true);
    // A cancellation sent to the people invited is the stronger question.
    if id == "calendar.delete" && sends {
        return Access::Owner(Class::Send);
    }
    if base != Access::Safe {
        return base;
    }
    match id {
        // Undoing can put back the accounts, rules or signatures of before.
        "edit.undo" | "edit.redo" => s.undo_reverts(id == "edit.redo").map_or(Access::Safe, Access::Owner),
        _ if id.starts_with("calendar.") && sends => Access::Owner(Class::Send),
        "mail.move" | "mail.copy" | "file.import" => {
            let folder = p::id(params, "folder").or(if id == "file.import" { Some(s.view.folder) } else { None });
            if folder.is_some_and(|f| is_outbox(&s.mailbox, f)) { Access::Owner(Class::Send) } else { Access::Safe }
        }
        "mail.delete" => {
            let ids = p::ids(params).unwrap_or_else(|| s.sel.messages.clone());
            if ids.iter().any(|m| deletes_for_good(&s.mailbox, *m)) { Access::Owner(Class::Delete) } else { Access::Safe }
        }
        "ui.sync" if params.get("password").is_some_and(|v| !v.is_null()) => Access::Owner(Class::Accounts),
        _ => Access::Safe,
    }
}

fn is_outbox(mb: &Mailbox, folder: Id) -> bool {
    mb.folder(folder).is_some_and(|f| f.kind == FolderKind::Outbox)
}

/// Deleting this message removes it (it is in Deleted Items, or its account has none).
fn deletes_for_good(mb: &Mailbox, id: Id) -> bool {
    let Some(m) = mb.message(id) else { return false };
    match mb.folder_of_kind(m.account, FolderKind::Deleted) {
        Some(d) => mb.subtree(d).contains(&m.folder),
        None => true,
    }
}

/// What the gate says about one call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Run,
    /// Needs the owner's yes for `Class`; the text says what it would do.
    Ask(Class, String),
    Refuse(String),
}

/// The gate: `allow` is what the owner allowed in advance.
pub fn check(s: &Session, id: &str, params: &Value, allow: &[Class]) -> Verdict {
    if access(id).is_none() {
        return Verdict::Refuse(format!("`{id}` isn't available to agents"));
    }
    if let Some(key) = file_named(id, params) {
        return Verdict::Refuse(no_paths(id, key));
    }
    match classify(s, id, params) {
        Access::Safe => Verdict::Run,
        Access::Never => Verdict::Refuse(format!("`{id}` isn't available to agents")),
        Access::Owner(c) if allow.contains(&c) => Verdict::Run,
        Access::Owner(c) => {
            let what = describe(s, id, params);
            if what.chars().count() > MAX_QUESTION {
                return Verdict::Refuse(format!(
                    "`{id}` is too large to show the owner in a question, and the owner is never asked about what they can't read in full: do it in smaller steps"
                ));
            }
            Verdict::Ask(c, what)
        }
    }
}

/// The most characters of a question about what an agent wants (the window shows all of them;
/// a call whose description is longer is refused rather than clipped).
pub const MAX_QUESTION: usize = 900;

/// The refusal when nobody can be asked (headless).
pub fn needs_owner(id: &str, class: Class, what: &str) -> String {
    format!(
        "`{id}` needs the owner ({}): it would {what}. Only the owner can let an agent do this, by starting the server with `mailcraft-cli mcp --allow {}`",
        class.what(),
        class.as_str()
    )
}

fn clip(s: &str, n: usize) -> String {
    let clean: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let clean = clean.trim();
    if clean.chars().count() > n { format!("{}…", clean.chars().take(n).collect::<String>()) } else { clean.to_string() }
}

fn quoted(s: &str) -> String {
    let c = clip(s, 80);
    if c.is_empty() { "(no subject)".to_string() } else { format!("\u{201c}{c}\u{201d}") }
}

/// `{n} message` / `{n} messages` for `n`, by the language's plural rule.
// ponytail: always English for now; the session's `interface_language` comes in with the engine's text (slice 4).
fn plural(n: usize, one: &str, many: &str) -> String {
    let n = u64::try_from(n).unwrap_or(u64::MAX);
    mailcraft_i18n::fmt(mailcraft_i18n::Lang::English.tn(n, one, many), &[("n", &n.to_string())])
}

fn messages_text(s: &Session, ids: &[Id]) -> String {
    let subjects: Vec<String> = ids.iter().take(3).filter_map(|id| s.mailbox.message(*id)).map(|m| quoted(&m.subject)).collect();
    let more = if ids.len() > 3 { ", …" } else { "" };
    format!("{} ({}{more})", plural(ids.len(), "{n} message", "{n} messages"), subjects.join(", "))
}

/// `s` with control characters made spaces, in full.
fn plain(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// Up to `n` of `items`, then how many more there are.
fn some(items: &[String], n: usize) -> String {
    let more = items.len().saturating_sub(n);
    let shown = items.iter().take(n).map(|x| plain(x)).collect::<Vec<_>>().join("; ");
    if more == 0 { shown } else { format!("{shown} and {more} more") }
}

/// The addresses in a list the way params write them (`a@x; b@y`, or an array).
fn addresses(v: Option<&Value>) -> Vec<String> {
    let text = v.map(value_text).unwrap_or_default();
    text.split([';', ',']).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()
}

/// The event a calendar call is about: the one `params` creates, or the one it names (or that is
/// selected), and when it starts.
fn event_title(s: &Session, id: &str, params: &Value) -> String {
    if matches!(id, "calendar.new" | "calendar.newMeeting") {
        let (title, start) = (p::str(params, "title").unwrap_or(""), p::str(params, "start").unwrap_or(""));
        let at = if start.is_empty() { String::new() } else { format!(" at {}", clip(start, 40)) };
        return format!("the new event {}{at}", quoted(title));
    }
    match p::id(params, "id").or(s.sel.event).and_then(|e| s.mailbox.event(e)) {
        Some(e) => quoted(&e.title),
        None => "an item of the calendar".to_string(),
    }
}

/// The people a calendar call writes to: those it names and those the event already has.
fn invitees(s: &Session, params: &Value) -> Vec<String> {
    let mut who = addresses(params.get("attendees"));
    who.extend(addresses(params.get("optional")));
    if let Some(e) = p::id(params, "id").or(s.sel.event).and_then(|e| s.mailbox.event(e)) {
        who.extend(e.attendees.iter().map(|a| a.address.email.clone()));
    }
    who.dedup();
    who
}

/// What a call would do, in a sentence that follows "An agent wants to …" (for the owner's
/// question; never trusted for anything else).
pub fn describe(s: &Session, id: &str, params: &Value) -> String {
    let label = s.registry.get(id).map(|c| c.label).unwrap_or(id);
    let st = |k: &str| p::str(params, k).unwrap_or("");
    match id {
        "compose.send" => match &s.compose {
            Some(c) => {
                let to: Vec<String> =
                    [c.to.as_str(), c.cc.as_str(), c.bcc.as_str()].into_iter().flat_map(|l| addresses(Some(&Value::String(l.to_string())))).collect();
                let to = if to.is_empty() { "nobody yet".to_string() } else { some(&to, 12) };
                let files = if c.attachments.is_empty() {
                    String::new()
                } else {
                    let names: Vec<String> = c.attachments.iter().take(5).map(|a| clip(&a.name, 40)).collect();
                    format!(" with {} ({})", plural(c.attachments.len(), "{n} attachment", "{n} attachments"), names.join(", "))
                };
                format!("send {} to {to}{files}", quoted(&c.subject))
            }
            None => "send the message being written".to_string(),
        },
        "mail.move" | "mail.copy" => {
            let ids = p::ids(params).unwrap_or_else(|| s.sel.messages.clone());
            format!("put {} in the Outbox, which sends them", messages_text(s, &ids))
        }
        "file.import" => "import messages into the Outbox, which sends them".to_string(),
        "mail.delete" | "mail.deletePermanently" => {
            let ids = p::ids(params).unwrap_or_else(|| s.sel.messages.clone());
            format!("delete {} for good", messages_text(s, &ids))
        }
        "folder.empty" | "folder.delete" => {
            let f = p::id(params, "id").unwrap_or(s.view.folder);
            let name = s.mailbox.folder(f).map(|f| quoted(&f.name)).unwrap_or_else(|| "a folder".to_string());
            let n = s.mailbox.count(f);
            let verb = if id == "folder.empty" { "empty" } else { "delete the folder" };
            format!("{verb} {name} and the {} in it, for good", plural(n, "{n} message", "{n} messages"))
        }
        "calendar.forward" => format!("forward {} to {}", event_title(s, id, params), some(&addresses(params.get("to")), 12)),
        "calendar.propose" => format!("propose another time for {} to its organizer", event_title(s, id, params)),
        "calendar.respond" => format!("answer {} ({}) and tell its organizer", event_title(s, id, params), clip(st("response"), 20)),
        _ if id.starts_with("calendar.") && p::bool(params, "send") == Some(true) => {
            let who = invitees(s, params);
            let to = if who.is_empty() {
                "the people it has invited".to_string()
            } else {
                format!("{} ({})", plural(who.len(), "{n} person", "{n} people"), some(&who, 8))
            };
            format!("{label}: {}, and write to {to}", event_title(s, id, params))
        }
        "calendar.delete" => format!("delete {} from the calendar", event_title(s, id, params)),
        "people.delete" => {
            let c = p::id(params, "id").or(s.sel.contact).and_then(|c| s.mailbox.contact(c));
            format!(
                "delete the contact {} for good; contacts don't go to Deleted Items",
                c.map(|c| quoted(&c.name())).unwrap_or_else(|| "(none selected)".to_string())
            )
        }
        "tasks.delete" => {
            let t = p::id(params, "id").or(s.sel.task).and_then(|t| s.mailbox.task(t));
            format!("delete the task {} for good", t.map(|t| quoted(&t.title)).unwrap_or_else(|| "(none selected)".to_string()))
        }
        "category.remove" => format!("delete the category {}, taking it off everything that has it", quoted(st("name"))),
        "edit.undo" | "edit.redo" => {
            let redo = id == "edit.redo";
            let label = if redo { s.redo_label() } else { s.undo_label() };
            let what = match s.undo_reverts(redo) {
                Some(Class::Accounts) => "the accounts",
                _ => "the rules or signatures",
            };
            format!("{} {}, which also puts {what} back as they were", if redo { "redo" } else { "undo" }, quoted(label.unwrap_or("the last change")))
        }
        "rules.add" => rule_text(s, params),
        "rules.fromMessage" => {
            let folder = p::id(params, "folder").filter(|f| s.mailbox.folder(*f).is_some());
            let to = folder.map(|f| quoted(&s.mailbox.folder_path(f))).unwrap_or_else(|| "a folder".to_string());
            let from = p::id(params, "id").or(s.sel.focus()).and_then(|m| s.mailbox.message(m)).map(|m| plain(&m.from.email)).unwrap_or_default();
            format!("make a rule that always moves mail from {} to {to}", quoted(&from))
        }
        "rules.enable" | "rules.remove" => {
            let r = p::id(params, "id").and_then(|r| s.mailbox.rules.iter().find(|r2| r2.id == r));
            let verb = if id == "rules.remove" { "delete" } else { "switch on or off" };
            match r {
                Some(r) => format!("{verb} the rule {}: {}", quoted(&r.name), rule_words(s, r)),
                None => format!("{verb} a rule that isn't there"),
            }
        }
        "account.add" => {
            let to = crate::cmd::account::from_params(params)
                .map(|a| crate::cmd::account::servers_text(&a))
                .unwrap_or_else(|| "no valid server".to_string());
            format!("add the account {}, which connects to {to}", clip(st("email"), 80))
        }
        "file.open" => format!("open the mailbox {} as the mailbox in view; its {}", clip(st("path"), 160), crate::cmd::file::servers_in(params)),
        "account.addSharedMailbox" => format!("open the shared mailbox {}", clip(st("address"), 80)),
        "account.removeSharedMailbox" => {
            let a = p::id(params, "id").and_then(|a| s.mailbox.account(a)).map(|a| a.email.clone()).unwrap_or_else(|| st("address").to_string());
            format!("close the shared mailbox {} and remove its mail from this computer", clip(&a, 80))
        }
        "account.graphSwitch" | "ui.switchToGraph" => {
            let a = p::id(params, "account").and_then(|a| s.mailbox.account(a)).map(|a| clip(&a.email, 80)).unwrap_or_default();
            match st("answer") {
                "later" => format!("keep the account {a} on IMAP for now"),
                "switchWithout" => format!(
                    "switch the account {a} to Microsoft Graph now, leaving the changes that wait for its IMAP server in the copy of the mailbox kept beside it"
                ),
                _ => format!("switch the account {a} to Microsoft Graph (its mail is downloaded again; a copy of the mailbox is kept beside it)"),
            }
        }
        "account.edit" | "account.remove" => {
            let a = p::id(params, "id").and_then(|a| s.mailbox.account(a)).map(|a| clip(&a.email, 80)).unwrap_or_default();
            if id == "account.edit" {
                let old = p::id(params, "id").and_then(|a| s.mailbox.account(a));
                let moved = old
                    .and_then(|old| {
                        crate::cmd::account::edited(old, params)
                            .map(|new| (crate::cmd::account::servers_text(old), crate::cmd::account::servers_text(&new)))
                    })
                    .filter(|(was, now)| was != now);
                return match moved {
                    Some((was, now)) => format!("change the account {a}; its servers change from {was} to {now}"),
                    None => format!("change the account {a}"),
                };
            }
            format!("remove the account {a}")
        }
        _ => {
            let mut shown = params.to_string();
            if shown == "{}" || shown == "null" {
                shown.clear();
            }
            // In full: `check` refuses what is too long to show, rather than a cut hiding the end.
            let shown = if shown.is_empty() { String::new() } else { format!(" {}", plain(&shown)) };
            format!("run \u{201c}{label}\u{201d} ({id}){shown}")
        }
    }
}

/// A rule as it would be made, in words, naming the folders by their paths.
fn rule_words(s: &Session, r: &mailcraft_model::Rule) -> String {
    let when = r.conditions.iter().map(|c| plain(&c.describe())).collect::<Vec<_>>().join(" and ");
    let then = r
        .actions
        .iter()
        .map(|a| match a {
            mailcraft_model::Action::MoveTo(f) if s.mailbox.folder(*f).is_some() => format!("move it to {}", plain(&s.mailbox.folder_path(*f))),
            mailcraft_model::Action::MoveTo(_) => "move it to a folder that is gone".to_string(),
            other => plain(&other.describe()),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("when {when}: {then}")
}

fn rule_text(s: &Session, params: &Value) -> String {
    match crate::cmd::rules::rule_from_params(s, params) {
        Ok(r) => format!("create the rule {}: {}", quoted(&r.name), rule_words(s, &r)),
        Err(_) => "create a rule that can't be made as asked".to_string(),
    }
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; "),
        _ => String::new(),
    }
}

/// What a command did that its grant didn't cover: mail put into an Outbox (it would be sent),
/// or messages gone for good. Checked by [`Session::run`](crate::Session::run) for agents.
pub(crate) fn overstep(before: &Mailbox, after: &Mailbox, allow: &[Class]) -> Option<Class> {
    if !allow.contains(&Class::Send) {
        let outboxes: Vec<Id> = after.folders.iter().filter(|f| f.kind == FolderKind::Outbox).map(|f| f.id).collect();
        if !outboxes.is_empty() {
            let new_out = after
                .messages
                .iter()
                .any(|(id, m)| outboxes.contains(&m.folder) && !before.messages.get(id).is_some_and(|b| outboxes.contains(&b.folder)));
            if new_out {
                return Some(Class::Send);
            }
        }
    }
    if !allow.contains(&Class::Delete) {
        let gone = before.messages.iter().any(|(id, m)| !m.draft && !after.messages.contains_key(id));
        if gone {
            return Some(Class::Delete);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_command_has_a_class() {
        let s = Session::empty();
        let missing: Vec<&str> = s.registry.all().iter().map(|c| c.id).filter(|id| access(id).is_none()).collect();
        assert!(missing.is_empty(), "classify these for agents in crates/engine/src/agent.rs: {missing:?}");
        // Each id in one list only.
        let lists = [SAFE, SEND, DELETE, ACCOUNTS, MAILBOX, SETTINGS, NEVER];
        let mut all: Vec<&str> = lists.iter().flat_map(|l| l.iter().copied()).collect();
        let n = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), n, "an id is in two lists");
        // Every file param is a registered command's, or one of the window's own (`ui.*`).
        for (id, ..) in FILE_PARAMS {
            assert!(s.registry.get(id).is_some() || id.starts_with("ui."), "{id}");
        }
    }

    #[test]
    fn what_needs_the_owner() {
        let mut s = Session::new(crate::sample::mailbox(1_791_993_600, 0));
        for (id, want) in [
            ("compose.send", Access::Owner(Class::Send)),
            ("account.add", Access::Owner(Class::Accounts)),
            ("mail.deletePermanently", Access::Owner(Class::Delete)),
            ("file.open", Access::Owner(Class::Mailbox)),
            ("rules.add", Access::Owner(Class::Settings)),
            ("rules.list", Access::Owner(Class::Settings)),
            ("sync.apply", Access::Never),
            ("mail.flag", Access::Safe),
            ("nope.nothing", Access::Never),
        ] {
            assert_eq!(classify(&s, id, &json!({})), want, "{id}");
        }
        assert_eq!(classify(&s, "calendar.respond", &json!({"response": "accept", "send": true})), Access::Owner(Class::Send));
        assert_eq!(classify(&s, "calendar.respond", &json!({"response": "accept"})), Access::Safe);
        let outbox = s.mailbox.folders.iter().find(|f| f.kind == FolderKind::Outbox).map(|f| f.id);
        if let Some(outbox) = outbox {
            assert_eq!(classify(&s, "mail.move", &json!({"folder": outbox, "ids": [1]})), Access::Owner(Class::Send));
        }
        // A message in Deleted Items goes for good.
        let deleted = s.mailbox.folders.iter().find(|f| f.kind == FolderKind::Deleted).map(|f| f.id).unwrap();
        let id = *s.mailbox.messages.keys().next().unwrap();
        s.mailbox.move_message(id, deleted).unwrap();
        assert_eq!(classify(&s, "mail.delete", &json!({"ids": [id]})), Access::Owner(Class::Delete));
        // Paths are refused whatever the class.
        assert!(matches!(check(&s, "file.exportMessage", &json!({"path": "/tmp/x.eml"}), &Class::ALL), Verdict::Refuse(_)));
        assert!(matches!(check(&s, "compose.attach", &json!({"path": "/etc/passwd"}), &Class::ALL), Verdict::Refuse(_)));
        assert_eq!(check(&s, "folder.select", &json!({"path": "Inbox"}), &[]), Verdict::Run);
        assert!(matches!(check(&s, "compose.send", &json!({}), &[]), Verdict::Ask(Class::Send, _)));
        assert_eq!(check(&s, "compose.send", &json!({}), &[Class::Send]), Verdict::Run);
    }

    #[test]
    fn the_question_says_what_and_to_whom() {
        let mut s = Session::empty();
        s.run("mail.new", &json!({"to": "someone@attacker.example", "subject": "Re: Invoice"})).unwrap();
        s.run("compose.attach", &json!({"name": "id_ed25519", "data": "AAAA"})).unwrap();
        let d = describe(&s, "compose.send", &json!({}));
        assert!(d.contains("Re: Invoice") && d.contains("someone@attacker.example") && d.contains("1 attachment (id_ed25519)"), "{d}");
    }

    #[test]
    fn agents_cant_slip_mail_into_the_outbox_or_delete_for_good() {
        let mut s = Session::new(crate::sample::mailbox(1_791_993_600, 0));
        let outbox = s.mailbox.folders.iter().find(|f| f.kind == FolderKind::Outbox).map(|f| f.id).unwrap();
        let id = *s.mailbox.messages.keys().next().unwrap();
        let before = s.mailbox.clone();
        s.agent = Some(Vec::new());
        assert!(s.run("mail.copy", &json!({"folder": outbox, "ids": [id]})).is_err());
        assert!(s.run("mail.deletePermanently", &json!({"ids": [id]})).is_err());
        assert!(s.mailbox == before, "nothing changed");
        s.agent = Some(vec![Class::Delete]);
        assert!(s.run("mail.deletePermanently", &json!({"ids": [id]})).is_ok());
        s.agent = None;
    }
}
