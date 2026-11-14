// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the owner's yes to an agent's question reaches: what the question showed, and nothing
//! else (AU3-24, AU3-25, AU3-23, AU3-20).
//!
//! A command that waits for the owner may take what it acts on from the window when its params
//! leave it out (the selection, the folder in view, the message being written, the first account
//! of a kind), and what it names may change while the question is up: other agent commands run
//! meanwhile (selecting, moving, writing), and are never held off. So when the question is asked,
//! [`Shown`] keeps, in the question's own record and never in the agent's params, everything the
//! command can read from the window, the text the owner is shown, and for a folder's messages the
//! ones it holds. On the yes, [`Shown::check`]:
//! - a folder emptied or deleted ([`Object::FolderContent`]) reaches only the messages shown: what
//!   came into it meanwhile stays (an empty takes the shown ones; a delete, which would take the
//!   folder with what came, does nothing);
//! - anything else is described again: when what the command would read from the window, or the
//!   question's text, differs, or what it names is gone, nothing runs.
//!
//! Every command an agent is asked about says what it is about ([`object`]): one that doesn't is
//! refused, never asked about ([`super::check`]).

use std::collections::HashSet;
use std::hash::{BuildHasher, Hasher, RandomState};

use mailcraft_model::Id;
use serde_json::Value;

use crate::{Compose, Selection, Session, ViewState, p};

/// What a question is about, by the command asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    /// Messages: those it names (`ids`, `id`, `message`), or the selected or focused one.
    Messages,
    /// A folder: the one named, or the one in view.
    Folder,
    /// A folder and the messages it holds, which go with it: only those shown do.
    FolderContent,
    /// A contact: the one named, or the selected one.
    Contact,
    /// A task: the one named, or the selected one.
    Task,
    /// An appointment or a meeting: the one named, or the selected one.
    Event,
    /// A calendar.
    Calendar,
    /// A category.
    Category,
    /// An account: the one named, or the first of a kind.
    Account,
    /// A rule.
    Rule,
    /// A signature.
    Signature,
    /// A message being written: the one named, or the current one, whole.
    Compose,
    /// A window of the program, or its dialog: the one named, or the one in front. (The window
    /// keeps what its windows show beside [`Shown`].)
    Window,
    /// The mailbox file, and what another one in its place closes.
    Mailbox,
    /// The step that an undo or a redo takes back.
    Undo,
    /// The program's own options.
    Options,
}

/// What the command `id` is about when an agent's question asks the owner about it. `None`: it
/// isn't asked about, and an agent's question about it is refused ([`super::check`]). Every
/// command an agent may be asked about is here (a test holds every one to it).
pub fn object(id: &str) -> Option<Object> {
    use Object::*;
    Some(match id {
        // Messages named, selected or focused.
        "mail.delete"
        | "mail.deletePermanently"
        | "mail.move"
        | "mail.copy"
        | "mail.removeAttachment"
        | "mail.sendAgain"
        | "mail.dismissWarnings"
        | "junk.blockSender"
        | "junk.neverBlockSender"
        | "junk.neverBlockDomain"
        | "rules.fromMessage"
        | "ui.downloadPictures"
        | "ui.retryPictures"
        | "ui.alwaysDownloadPictures" => Messages,
        "folder.empty" | "folder.delete" => FolderContent,
        "rules.run" | "file.import" | "calendar.deleteAnsweredRequests" => Folder,
        "people.delete" | "people.merge" => Contact,
        "tasks.delete" | "tasks.edit" | "tasks.complete" | "tasks.categorize" | "tasks.new" | "tasks.fromMessage" => Task,
        // A calendar command that writes to people is asked about whatever it is (`classify`).
        "calendar.forward" | "calendar.propose" | "calendar.delete" | "people.meeting" | "ui.joinMeeting" => Event,
        _ if id.starts_with("calendar.") && !CALENDARS.contains(&id) => Event,
        _ if CALENDARS.contains(&id) => Calendar,
        "category.remove" | "category.rename" => Category,
        "account.setAutomaticReplies"
        | "account.add"
        | "account.edit"
        | "account.remove"
        | "account.forgetEwsCertificate"
        | "account.graphSwitch"
        | "account.addSharedMailbox"
        | "account.removeSharedMailbox"
        | "account.thinOffline"
        | "ui.keepOffline"
        | "ui.connectMicrosoft"
        | "ui.automaticRepliesSignIn"
        | "ui.connectFavorites"
        | "ui.allowSharedSend"
        | "ui.allowSharedCalendars"
        | "ui.allowTasks"
        | "ui.allowRules"
        | "ui.allowAutomaticReplies"
        | "ui.adminConsent"
        | "ui.signInAgain"
        | "ui.microsoftClientId"
        | "ui.rememberSignIn"
        | "ui.graphSwitch"
        | "ui.switchToGraph"
        | "ui.confirmServers"
        | "ui.sync"
        | "junk.add"
        | "junk.remove"
        | "junk.list" => Account,
        "rules.add" | "rules.remove" | "rules.enable" | "rules.list" | "rules.move" | "rules.keepOnServer" => Rule,
        _ if id.starts_with("signature.") => Signature,
        "compose.send" | "compose.discard" | "compose.close" => Compose,
        "ui.closeWindow" | "ui.eventWindow" | "ui.dialog" | "ui.closeDialog" => Window,
        "file.new" | "file.open" | "file.save" | "file.saveAs" => Mailbox,
        "edit.undo" | "edit.redo" => Undo,
        "file.options"
        | "sync.offline"
        | "ui.downloadPicturesAutomatically"
        | "ui.senderImages"
        | "ui.desktopAlerts"
        | "ui.closeToTray"
        | "ui.repliesInWindow"
        | "ui.updateCheck"
        | "ui.updateInstall"
        | "ui.dismissUpdateNotice"
        | "ui.dismissSettingsNotice"
        | "ui.dismissSoftwareNotice"
        | "ui.reportProblem"
        // The Public keys kept (File › Options › Security, #30): the question names the key.
        | "keys.import"
        | "keys.remove" => Options,
        _ => return None,
    })
}

/// The calendar commands that are about a calendar rather than an event in it.
const CALENDARS: &[&str] = &[
    "calendar.removeCalendar",
    "calendar.openShared",
    "calendar.removeShared",
    "calendar.hideListedCopy",
    "calendar.moveToAccount",
    "calendar.addCalendar",
    "calendar.showCalendar",
    "calendar.setCurrent",
    "calendar.calendars",
    "calendar.showOnly",
    "calendar.color",
    "ui.checkCalendar",
];

/// The commands whose messages the window counts where they are when it asks, and of which it
/// takes only those still there on the yes (#503): their question isn't described again.
pub const COUNTED: &[&str] = &["mail.delete", "mail.deletePermanently", "mail.move"];

/// What a question showed the owner, kept with it until the owner answers.
#[derive(Clone, Debug)]
pub struct Shown {
    object: Object,
    /// The question's text.
    what: String,
    /// What the command can take from the window when its params leave it out.
    sel: Selection,
    view: ViewState,
    accounts: Vec<Id>,
    /// The messages being written, whole, each with its id, as a digest keyed for this question
    /// alone.
    composing: u64,
    keys: RandomState,
    /// A folder emptied or deleted, and the messages and folders that go with it.
    held: Option<Held>,
    /// Everybody a calendar command writes to, in full (the question names the first few).
    people: Vec<String>,
}

/// A folder emptied or deleted, as the question showed it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Held {
    folder: Id,
    /// The messages that go with it.
    messages: HashSet<Id>,
    /// The folders that go with it: those deleted into Deleted Items that its emptying takes, or
    /// the folder deleted and every folder under it.
    folders: HashSet<Id>,
}

/// What of a folder's content the owner's yes reaches, when something came into it since the
/// question was asked ([`Session::only_shown`]): the messages and the folders shown that are still
/// there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Only {
    pub messages: HashSet<Id>,
    pub folders: HashSet<Id>,
}

/// How far the owner's yes reaches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    /// What the command would do now is what the question showed.
    All,
    /// Of a folder's messages and folders, only these: those shown that are still there. What
    /// came in meanwhile stays ([`Session::only_shown`]).
    Only(Only),
}

impl Shown {
    /// What the question about `id` with `params` shows the owner (`what`), now. `None` for a
    /// command that isn't asked about ([`object`]).
    pub fn new(s: &Session, id: &str, params: &Value, what: &str) -> Option<Shown> {
        let object = object(id)?;
        let keys = RandomState::new();
        let held = (object == Object::FolderContent).then(|| held(s, id, params)).flatten();
        Some(Shown {
            object,
            what: what.to_string(),
            sel: s.sel.clone(),
            view: s.view.clone(),
            accounts: s.mailbox.accounts.iter().map(|a| a.id).collect(),
            composing: digest(&keys, s.all_composing()),
            keys,
            held,
            people: people(s, object, params),
        })
    }

    pub fn object(&self) -> Object {
        self.object
    }

    /// On the owner's yes: how far it reaches, or why nothing runs (what the question showed has
    /// changed since, or is gone).
    pub fn check(&self, s: &Session, id: &str, params: &Value) -> Result<Reach, String> {
        let changed = || {
            Err(format!(
                "what `{id}` would do has changed since the owner was asked (or what it names is gone): nothing was done. Ask the owner again"
            ))
        };
        // A folder's messages are bound by the folder and the messages themselves, below: the
        // command reads nothing else of the window but the folder in view.
        let accounts: Vec<Id> = s.mailbox.accounts.iter().map(|a| a.id).collect();
        let window = s.sel != self.sel || s.view != self.view || accounts != self.accounts || digest(&self.keys, s.all_composing()) != self.composing;
        // The messages of a Delete or a Move are named (`NEEDS_IDS`) and counted where they are by
        // the window, which takes on the yes those of them still there (#503).
        if window && !COUNTED.contains(&id) && (self.object != Object::FolderContent || s.view.folder != self.view.folder) {
            return changed();
        }
        if people(s, self.object, params) != self.people {
            return changed();
        }
        if self.object == Object::FolderContent {
            let (Some(shown), Some(now)) = (&self.held, held(s, id, params)) else { return changed() };
            if shown.folder != now.folder {
                return changed();
            }
            if now.messages.is_subset(&shown.messages) && now.folders.is_subset(&shown.folders) {
                return Ok(Reach::All);
            }
            if id != "folder.empty" {
                return Err(format!(
                    "the folder `{id}` was asked about holds messages or folders that came into it since the owner was asked, which would go with it: nothing was done. Ask the owner again"
                ));
            }
            return Ok(Reach::Only(Only {
                messages: now.messages.intersection(&shown.messages).copied().collect(),
                folders: now.folders.intersection(&shown.folders).copied().collect(),
            }));
        }
        if !COUNTED.contains(&id) && super::describe(s, id, params) != self.what {
            return changed();
        }
        Ok(Reach::All)
    }
}

/// The folder `folder.empty` or `folder.delete` with `params` acts on, as the command finds it,
/// and the messages and folders that go with it.
fn held(s: &Session, id: &str, params: &Value) -> Option<Held> {
    let folder = p::id_strict(params, "id").ok()?.unwrap_or(s.view.folder);
    s.mailbox.folder(folder)?;
    let (messages, folders) = if id == "folder.empty" {
        (crate::cmd::folder::emptied(s, folder), crate::cmd::folder::emptied_with(s, folder))
    } else {
        (crate::cmd::folder::deleted_with(s, folder), s.mailbox.subtree(folder))
    };
    Some(Held { folder, messages: messages.into_iter().collect(), folders: folders.into_iter().collect() })
}

/// Everybody a calendar command would write to, in full: the question names only the first few.
fn people(s: &Session, object: Object, params: &Value) -> Vec<String> {
    if object != Object::Event {
        return Vec::new();
    }
    let mut who = super::invitees(s, params);
    who.extend(super::addresses(params.get("to")));
    who
}

/// The messages being written, whole, in order, as one number keyed by `keys`: an agent can't
/// write two that come to the same one without knowing the key.
fn digest<'a>(keys: &RandomState, composing: impl Iterator<Item = &'a Compose>) -> u64 {
    let mut out = Digest(keys.build_hasher());
    // By id: which one is current changes as the windows take turns, not what they say.
    let mut all: Vec<&Compose> = composing.collect();
    all.sort_by_key(|c| c.id);
    for c in all {
        out.0.write_u64(c.id);
        // Written into the hasher as it is made: a message's attachments aren't copied for it.
        if serde_json::to_writer(&mut out, c).is_err() {
            // What can't be written can't be told from another: no two such ever match.
            out.0.write_u64(RandomState::new().build_hasher().finish());
        }
        out.0.write_u8(0xff);
    }
    out.0.finish()
}

/// A hasher that serde writes into.
struct Digest(std::hash::DefaultHasher);

impl std::io::Write for Digest {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
