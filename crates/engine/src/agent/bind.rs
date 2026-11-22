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
//!   folder with what came, does nothing, and so does Run Rules Now over it, which would act on
//!   what came: put it on another server, say, #566); nor does Run Rules Now when a
//!   message shown was changed meanwhile in what the rules read of it (by an agent, which may
//!   change a message unasked: its categories or flag, a draft's subject, an undo that puts an
//!   attachment back), which they may then take, or the rules were (a category they name renamed,
//!   one switched on or off);
//! - anything else is described again: when what the command would read from the window, or the
//!   question's text, differs, or what it names is gone, nothing runs.
//!
//! Every command an agent is asked about says what it is about ([`object`]): one that doesn't is
//! refused, never asked about ([`super::check`]).

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::Arc;

use mailcraft_model::{Id, Message, Rule};
use mailcraft_time::Date;
use serde_json::Value;

use crate::cmd::calendar::Part;
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
        | "mail.react"
        | "mail.setSendTime"
        | "mail.dismissWarnings"
        | "junk.blockSender"
        | "junk.neverBlockSender"
        | "junk.neverBlockDomain"
        | "rules.fromMessage"
        | "ui.downloadPictures"
        | "ui.unsubscribe"
        | "mail.unsubscribe"
        | "ui.unsubscribeMany"
        | "ui.retryPictures"
        | "ui.alwaysDownloadPictures"
        | "mail.categorize" => Messages,
        "folder.empty" | "folder.delete" | "rules.run" => FolderContent,
        // (The clean-up of #1401 reaches every folder when it names none: its question counts
        // what goes, and is described again on the yes.)
        "file.import" | "calendar.deleteAnsweredRequests" | "mail.removeImportedDuplicates" => Folder,
        "people.delete" | "people.merge" | "people.categorize" => Contact,
        "tasks.delete" | "tasks.edit" | "tasks.complete" | "tasks.categorize" | "tasks.new" | "tasks.fromMessage" => Task,
        // A calendar command that writes to people is asked about whatever it is (`classify`).
        "calendar.forward" | "calendar.propose" | "calendar.delete" | "people.meeting" | "ui.joinMeeting" => Event,
        _ if id.starts_with("calendar.") && !CALENDARS.contains(&id) => Event,
        _ if CALENDARS.contains(&id) => Calendar,
        "category.remove" | "category.rename" | "category.addToOutlook" | "category.set" => Category,
        "account.setAutomaticReplies"
        | "account.add"
        | "account.edit"
        | "account.remove"
        | "account.forgetEwsCertificate"
        | "account.davSet"
        | "account.davOff"
        | "account.graphSwitch"
        | "account.addSharedMailbox"
        | "account.removeSharedMailbox"
        | "account.setSenderAddresses"
        | "account.thinOffline"
        | "account.deviceLimits"
        | "account.trimDevice"
        | "ui.keepOffline"
        | "ui.connectMicrosoft"
        | "ui.automaticRepliesSignIn"
        | "ui.connectFavorites"
        | "ui.allowSharedSend"
        | "ui.allowSharedCalendars"
        | "ui.allowTasks"
        | "ui.allowRules"
        | "ui.allowAutomaticReplies"
        | "ui.allowSync"
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
        "compose.send" | "compose.sendLater" | "compose.discard" | "compose.close" => Compose,
        "ui.closeWindow" | "ui.eventWindow" | "ui.dialog" | "ui.closeDialog" => Window,
        "file.new" | "file.open" | "file.save" | "file.saveAs" => Mailbox,
        "edit.undo" | "edit.redo" => Undo,
        "file.options"
        | "sync.offline"
        | "sync.waitBeforeSending"
        | "ui.downloadPicturesAutomatically"
        | "ui.senderImages"
        | "ui.desktopAlerts"
        | "ui.closeToTray"
        | "ui.repliesInWindow"
        | "ui.attachmentPictures"
        | "ui.swipeAction"
        | "ui.updateCheck"
        | "ui.updateInstall"
        | "ui.dismissUpdateNotice"
        | "ui.dismissSettingsNotice"
        | "ui.dismissSoftwareNotice"
        | "ui.reportProblem"
        // The zone the calendar is shown in, which a rule reads the days mail came by (#566).
        | "view.timeZone"
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
    "calendar.subscribe",
    "calendar.unsubscribe",
    "calendar.subscriptionSettings",
    "calendar.refreshSubscribed",
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
    /// Where the account a calendar and contacts server command acts on had its server when the
    /// question was asked (homes, user name, pin), as a digest keyed for this question alone: the
    /// text of the question names none of it, and a yes must not reach another server than the
    /// owner was asked about (#209).
    dav: u64,
    /// The calendar items (or the one event a legacy one-event command names) and the destination
    /// state, independent of unrelated UI selection.
    pinned: Option<Pinned>,
}

#[derive(Clone, Debug)]
struct Pinned {
    opening: u64,
    /// Which reader holds it: a batch's items, or the one event a legacy command names. Chosen
    /// once, when the question is asked, so that the yes is checked by the same one.
    shape: Shape,
    /// The day the question offered the owner, when it offered one: resolved again on the yes,
    /// so that a clock whose rules moved cannot approve another occurrence than the one shown.
    planned: Option<Date>,
    state: u64,
}

/// How the calendar items of a question are held: as the batch its `items` name (or the typed set
/// a Delete stands for), or as the one event the older one-event commands name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    Batch,
    Legacy,
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
    /// For Run Rules Now, its messages as they were (#566): what the rules would do to them is
    /// what the question was about.
    as_shown: AsShown,
    /// And the zone the rules read the days mail came by ([`Session::zone`]).
    zone: mailcraft_time::Zone,
    /// And the rules for its account, in the order they run, in what they read and do: one
    /// rewritten, switched on or off or moved meanwhile (a category renamed rewrites them,
    /// unasked) may take other mail.
    rules: Vec<Rule>,
}

/// Messages as a question showed them, held as they were (the same in memory while nothing
/// changes them). What only a server changes (what it last said, its stamps, a read mark) is no
/// change to the rules ([`mailcraft_model::Mailbox::rules_read_alike`]).
#[derive(Clone, Default, PartialEq, Eq)]
struct AsShown(HashMap<Id, Arc<Message>>);

impl std::fmt::Debug for AsShown {
    // (Their count alone: nothing of the mail goes into a debug line.)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AsShown({} messages)", self.0.len())
    }
}

impl AsShown {
    /// Whether the rules may read one of `ids` otherwise than they read it when it was shown.
    fn changed(&self, s: &Session, ids: &HashSet<Id>) -> bool {
        let mb = &s.mailbox;
        ids.iter().any(|id| match (self.0.get(id), mb.messages.get(id)) {
            (Some(then), Some(now)) => !Arc::ptr_eq(then, now) && !mb.rules_read_alike(then, now, s.zone()),
            _ => true,
        })
    }
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
        Self::new_for_owner(s, id, params, what, None)
    }

    /// [`Shown::new`] for a question the window asks about an occurrence of a series: `planned`
    /// is the day the question itself offers the owner as "just this one", read when it is asked
    /// and never from the answer or from what is selected by the time it is answered. It is
    /// supplemental: a day the parameters name (even `null`, which names none), or the day the
    /// selection names, is bound as it is, and a whole-series question that offered no day binds
    /// none of its own.
    pub fn new_for_owner(s: &Session, id: &str, params: &Value, what: &str, planned: Option<Date>) -> Option<Shown> {
        let object = object(id)?;
        let keys = RandomState::new();
        let dav = dav_identity(&keys, s, id, params);
        let held = (object == Object::FolderContent).then(|| held(s, id, params)).flatten();
        let shape = shape(s, id, params);
        let pinned = shape.and_then(|shape| pinned(s, id, params, &keys, shape, planned));
        // A question about calendar items is refused when what it names can't be read, a batch's
        // items as much as the one event a legacy command names: nothing is asked about a yes that
        // could not be held to it, and a batch is never left to the looser check of the window.
        if shape.is_some() && pinned.is_none() {
            return None;
        }
        Some(Shown {
            object,
            what: what.to_string(),
            sel: s.sel.clone(),
            view: s.view.clone(),
            accounts: s.mailbox.accounts.iter().map(|a| a.id).collect(),
            composing: digest(&keys, s.all_composing()),
            keys,
            held,
            people: people(s, id, object, params),
            dav,
            pinned,
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
        if let Some(pinned) = &self.pinned {
            // The same reader that pinned it, batch or legacy, so that an untouched question is
            // answered and one whose event, destination or routing changed is not.
            let unchanged = s.openings() == pinned.opening && state(&self.keys, s, id, params, pinned.shape, pinned.planned) == Some(pinned.state);
            if !unchanged {
                return changed();
            }
            if people(s, id, self.object, params) != self.people || super::describe(s, id, params) != self.what {
                return changed();
            }
            return Ok(Reach::All);
        }
        // A folder's messages are bound by the folder and the messages themselves, below: the
        // command reads nothing else of the window but the folder in view.
        let accounts: Vec<Id> = s.mailbox.accounts.iter().map(|a| a.id).collect();
        let window = s.sel != self.sel || s.view != self.view || accounts != self.accounts || digest(&self.keys, s.all_composing()) != self.composing;
        // The messages of a Delete or a Move are named (`NEEDS_IDS`) and counted where they are by
        // the window, which takes on the yes those of them still there (#503).
        if window && !COUNTED.contains(&id) && (self.object != Object::FolderContent || s.view.folder != self.view.folder) {
            return changed();
        }
        if people(s, id, self.object, params) != self.people || dav_identity(&self.keys, s, id, params) != self.dav {
            return changed();
        }
        if self.object == Object::FolderContent {
            let (Some(shown), Some(now)) = (&self.held, held(s, id, params)) else { return changed() };
            if shown.folder != now.folder {
                return changed();
            }
            if now.messages.is_subset(&shown.messages) && now.folders.is_subset(&shown.folders) {
                // What the rules would do is described again, as for anything else, and they act on
                // the messages as they were shown: one changed since may now be one they take.
                if id == "rules.run" && super::describe(s, id, params) != self.what {
                    return changed();
                }
                if id == "rules.run" && shown.zone != now.zone {
                    return Err(
                        "the time zone the rules read the days mail came by has changed since the owner was asked about `rules.run`, so they may act on other mail: nothing was done. Ask the owner again"
                            .to_string(),
                    );
                }
                if id == "rules.run" && shown.rules != now.rules {
                    return Err(
                        "the rules `rules.run` was asked about were changed since the owner was asked (a category they name renamed, a rule switched on or off, say), so they may act on other mail: nothing was done. Ask the owner again"
                            .to_string(),
                    );
                }
                if id == "rules.run" && shown.as_shown.changed(s, &now.messages) {
                    return Err(
                        "a message in the folder `rules.run` was asked about was changed since the owner was asked in what the rules read of it (its categories or flag, its subject, an attachment, say), so they may act on it otherwise: nothing was done. Ask the owner again"
                            .to_string(),
                    );
                }
                return Ok(Reach::All);
            }
            if id == "rules.run" {
                return Err(
                    "the folder `rules.run` was asked about holds messages that came into it since the owner was asked, which the rules would act on: nothing was done. Ask the owner again"
                        .to_string(),
                );
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

/// The folder `folder.empty`, `folder.delete` or `rules.run` with `params` acts on, as the command
/// finds it, and the messages and folders that go with it (the rules run over its messages alone).
fn held(s: &Session, id: &str, params: &Value) -> Option<Held> {
    let folder = p::id_strict(params, if id == "rules.run" { "folder" } else { "id" }).ok()?.unwrap_or(s.view.folder);
    let account = s.mailbox.folder(folder)?.account;
    let mut as_shown = AsShown::default();
    let mut rules = Vec::new();
    let (messages, folders) = if id == "rules.run" {
        as_shown.0 = s.mailbox.messages.values().filter(|m| m.folder == folder).map(|m| (m.id, Arc::clone(m))).collect();
        rules = s.mailbox.rules_in_order(account).into_iter().filter_map(|i| s.mailbox.rules.get(i)).map(as_ruled).collect();
        (as_shown.0.keys().copied().collect(), Vec::new())
    } else if id == "folder.empty" {
        (crate::cmd::folder::emptied(s, folder), crate::cmd::folder::emptied_with(s, folder))
    } else {
        (crate::cmd::folder::deleted_with(s, folder), s.mailbox.subtree(folder))
    };
    Some(Held { folder, messages: messages.into_iter().collect(), folders: folders.into_iter().collect(), as_shown, zone: s.zone(), rules })
}

/// A rule as far as what it reads and does: what only its server's answer changes (its copy
/// there, a refusal, an answer awaited) leaves it as it was.
fn as_ruled(r: &Rule) -> Rule {
    Rule { server: None, refused: String::new(), unanswered: false, ..r.clone() }
}

/// Everybody a calendar command would write to, in full: the question names only the first few.
/// Read through the one recipient reader ([`super::reaches`]) so the yes is held to the people
/// the command would actually reach — a cancellation's invitees or a decline's organizers.
fn people(s: &Session, id: &str, object: Object, params: &Value) -> Vec<String> {
    if object != Object::Event {
        return Vec::new();
    }
    let mut who = super::reaches(s, id, params).people();
    who.extend(super::addresses(params.get("to")));
    super::unique(who)
}

/// The older one-appointment calendar commands whose question names the event by `id` (or, when
/// it names none, by the event in focus): one more question is put about that event, and a yes
/// must reach it as it was.
const LEGACY_EVENT: &[&str] = &["calendar.delete", "calendar.edit", "calendar.move"];

/// How `id` with `params` names the calendar items it acts on: by its `items` (or, for a Delete
/// that names no `id`, by the owner's typed set) as a batch, or by the one event the older
/// one-event commands name — the event `id` names, or, when it names none, the one in focus.
/// `None` for a command that names neither, which has nothing to pin.
pub(super) fn shape(s: &Session, id: &str, params: &Value) -> Option<Shape> {
    // A paste is pinned from its named `items` (which it can't be asked about without); a delete
    // from its named `items` or, without an `id`, from the owner's typed selection.
    // (Whether a delete is a set is the command's own reading, through `is_batch`.)
    let batch = if id == "calendar.delete" { super::is_batch(s, id, params) } else { params.get("items").is_some() };
    if super::BATCH.contains(&id) && batch {
        Some(Shape::Batch)
    } else if legacy_event(s, id, params) {
        Some(Shape::Legacy)
    } else {
        None
    }
}

/// Whether `id` with `params` asks about one appointment the older way. A Delete of a typed set
/// is the batch form instead ([`Shape::Batch`]); an explicit `id` is never read as the typed
/// selection beside it, and an unnamed one is the event in focus, as the command reads it.
fn legacy_event(s: &Session, id: &str, params: &Value) -> bool {
    LEGACY_EVENT.contains(&id) && !(id == "calendar.delete" && super::is_batch(s, id, params))
}

fn pinned(s: &Session, id: &str, params: &Value, keys: &RandomState, shape: Shape, planned: Option<Date>) -> Option<Pinned> {
    Some(Pinned { opening: s.openings(), shape, planned, state: state(keys, s, id, params, shape, planned)? })
}

/// What the question about `id` with `params` is held to, as the one number its `shape` says: a
/// batch's items and where they go ([`batch_state`]), or the one event a legacy command names
/// ([`legacy_state`]). Read this way when the question is asked and again on the yes, so that the
/// two can never be about a different set, or a different event, than the owner was shown.
fn state(keys: &RandomState, s: &Session, id: &str, params: &Value, shape: Shape, planned: Option<Date>) -> Option<u64> {
    match shape {
        Shape::Batch => batch_state(keys, s, id, params),
        Shape::Legacy => legacy_state(keys, s, id, params, planned),
    }
}

fn batch_state(keys: &RandomState, s: &Session, id: &str, params: &Value) -> Option<u64> {
    let items = super::batch_items(s, id, params).ok()?;
    // (Read strictly, as the command reads it: a malformed destination never falls back to each
    // item's own calendar in the binding either. Only a paste has a destination: a Delete reads
    // no `calendar`, so what is written there is pinned with the raw parameters and nothing else.)
    let target = if id == "calendar.paste" { p::id_strict(params, "calendar").ok()? } else { None };
    // The clock the command reads a recurring item's times by, read once as the command does.
    let here = crate::cmd::remote::offset_of(s);
    let mut out = Digest(keys.build_hasher());
    // Approval covers this operation and its exact placement/flags, not just these sources.
    written(&mut out, &(id, params))?;
    out.0.write_u64(s.openings());
    if id == "calendar.delete" {
        // How the Delete reads them, as the command does: a typed day of a series and the older
        // focus on that day name the same item, but the one deletes that day and the other the
        // whole series. (The counterpart of the byte `legacy_state` writes for a typed set beside
        // the focus.)
        out.0.write_u8(match crate::cmd::calendar::delete_form(&s.sel, params) {
            Ok(crate::cmd::calendar::DeleteForm::Batch) => 1,
            Ok(crate::cmd::calendar::DeleteForm::One) => 2,
            Err(_) => 0,
        });
    }
    out.0.write_u64(u64::try_from(items.len()).unwrap_or(u64::MAX));
    // The accounts whose servers the change would reach: the one that keeps each source item and
    // its calendar, and the one that keeps where it goes. Only these are pinned; another
    // account's settings have nothing to do with the owner's yes.
    let mut touched: Vec<Id> = Vec::new();
    for item in &items {
        out.0.write_u64(item.id);
        match item.day {
            Some(day) => {
                out.0.write_u8(1);
                out.0.write(day.iso().as_bytes());
            }
            None => out.0.write_u8(0),
        }
        let event = s.mailbox.event(item.id);
        match event {
            Some(event) => {
                out.0.write_u8(1);
                written(&mut out, event)?;
                // When a recurring item happens here: a paste spaces its items by these times, and
                // a host whose clock rules move shows them elsewhere without a byte of the event
                // changing.
                if let Some(day) = item.day.filter(|_| event.recurrence.is_some()) {
                    written(&mut out, &event.occurrence_times(day, &here))?;
                }
                if event.origin.is_remote() {
                    touched.push(event.origin.account);
                }
                if let Some(calendar) = s.mailbox.calendar(event.calendar)
                    && calendar.origin.is_remote()
                {
                    touched.push(calendar.origin.account);
                }
            }
            None => out.0.write_u8(0),
        }
        let destination = target.unwrap_or_else(|| event.map_or(0, |e| e.calendar));
        out.0.write_u64(destination);
        match s.mailbox.calendar(destination) {
            Some(calendar) => {
                out.0.write_u8(1);
                written(&mut out, calendar)?;
                if calendar.origin.is_remote() {
                    touched.push(calendar.origin.account);
                }
            }
            None => out.0.write_u8(0),
        }
    }
    // The routing of each touched account, in id order so that the digest doesn't depend on the
    // order the items happen to name them in.
    touched.sort_unstable();
    touched.dedup();
    for account in touched {
        out.0.write_u64(account);
        match s.mailbox.account(account) {
            Some(account) => {
                out.0.write_u8(1);
                routing(&mut out, account)?;
            }
            None => out.0.write_u8(0),
        }
    }
    Some(out.0.finish())
}

/// The one event a legacy question names, and where it would go, as one number: the raw
/// parameters, the mailbox that was open, the whole event, its calendar, the destination and the
/// routing of the accounts whose servers the change would reach. An explicit `id` is read alone
/// (never the typed selection beside it, and never the event in focus when it names none that
/// reads); an unnamed one is the event in focus, as the command reads it. A target or a calendar
/// that can't be read pins nothing.
fn legacy_state(keys: &RandomState, s: &Session, id: &str, params: &Value, planned: Option<Date>) -> Option<u64> {
    // (An `id` named at all — even `null`, which the command refuses — is never the focus: the
    // one-event commands read it strictly, so a question whose target can't be read is refused
    // rather than held to whatever happens to be selected.)
    let focused = params.get("id").is_none();
    let named = if focused { s.sel.event? } else { p::id_strict(params, "id").ok()?? };
    let event = s.mailbox.event(named)?;
    // Which of it the command's own parameters name, and the day that occurrence is shown on
    // here: the one resolver the command itself runs by. A legacy occurrence day is displayed,
    // not intrinsic, so an anonymous clock is held to what the day resolves to rather than
    // refused: a whole-series question resolves no day at all, and a day that resolves elsewhere
    // retracts the yes by itself.
    let target = crate::cmd::calendar::resolve_target(s, event, params).ok()?;
    let one = matches!(target.part, Part::One(_));
    // The occurrence the question offered the owner, when it offered one, resolved by that same
    // resolver with the answer the question would add: its intrinsic day and, when the series has
    // one there, the date it is shown on. It is read apart from `target`, so it can never replace
    // the command's own target (and with it the destination, mode and calendar routing the raw
    // parameters fixed). A reader error, or an event that has no occurrence to offer, pins
    // nothing; a day the series skips is kept below with no displayed date, because the command
    // still resolves it as a target.
    let offered = match planned {
        Some(day) => {
            let answer = serde_json::json!({ "scope": "occurrence", "occurrence": day.iso() });
            let occurrence = crate::cmd::calendar::resolve_target(s, event, &answer).ok()?;
            match occurrence.part {
                // A skipped day is still the target the command resolves (deleting a day the
                // series already skips is a no-op that succeeds): keep its intrinsic key.
                Part::One(intrinsic) => Some((intrinsic, occurrence.shown)),
                Part::Series => return None,
            }
        }
        None => None,
    };
    // The clock's own name, where it has one: a named zone or a fixed offset pins the rules
    // themselves, and for an anonymous one the resolved day below says what they resolve to.
    let zone = s.zone();
    // (A Move and a Delete keep the event in its calendar, and an Edit of one occurrence keeps it
    // there too, because its form takes no calendar with that day. Only an Edit of the whole event
    // may name a calendar to take it to, and `calendar` is read as that command reads it: one that
    // isn't a calendar at all leaves the event where it is.)
    let destination = if id == "calendar.edit" && !one { p::id(params, "calendar").unwrap_or(event.calendar) } else { event.calendar };
    let source = s.mailbox.calendar(event.calendar)?;
    let into = s.mailbox.calendar(destination)?;
    let mut out = Digest(keys.build_hasher());
    // Approval covers this operation and its exact parameters, the event as it was, its placement
    // and flags, and the mailbox that was open — not only the event's id.
    written(&mut out, &(id, params))?;
    // Include the raw fixed override too: offset_of reads it, while Zone clamps its public name.
    written(&mut out, &(zone.id(), s.clock.offset))?;
    out.0.write_u64(s.openings());
    if focused && id == "calendar.delete" {
        // What the command reads from the window when it names no `id`: a Delete takes the owner's
        // typed set instead when it reads it as a set (two or more, or one that isn't this event in
        // focus: `delete_form`). (A typed selection appearing while the question waits therefore
        // retracts the yes, even one of this event alone in focus, which is the same Delete: that
        // only fails closed, and the agent asks again.)
        out.0.write_u8(if s.sel.events.is_empty() { 0 } else { 1 });
    }
    // The day the question offered the owner, when it offered one: kept raw, so that a question
    // asked about one day can't be approved as another.
    match planned {
        Some(day) => {
            out.0.write_u8(1);
            written(&mut out, &day)?;
        }
        None => out.0.write_u8(0),
    }
    // The target the command's own parameters name, and the date it is shown on here.
    match target.part {
        Part::Series => out.0.write_u8(0),
        Part::One(day) => {
            out.0.write_u8(1);
            written(&mut out, &day)?;
        }
    }
    match target.shown {
        Some(day) => {
            out.0.write_u8(1);
            written(&mut out, &day)?;
        }
        None => out.0.write_u8(0),
    }
    // The occurrence the question offered, resolved now: its intrinsic day and the date it is
    // shown on, or an explicit "no date" mark for a day the series skips. A clock or rule change
    // that moves either the intrinsic day or its visibility retracts the yes; one that leaves
    // both alone keeps it answerable.
    match offered {
        Some((intrinsic, shown)) => {
            out.0.write_u8(1);
            written(&mut out, &intrinsic)?;
            match shown {
                Some(day) => {
                    out.0.write_u8(1);
                    written(&mut out, &day)?;
                }
                None => out.0.write_u8(0),
            }
        }
        None => out.0.write_u8(0),
    }
    written(&mut out, event)?;
    out.0.write_u64(destination);
    written(&mut out, into)?;
    // The accounts whose servers the change would reach: the one that keeps the event, its
    // calendar, and where it goes. Only these are pinned; another account's settings have nothing
    // to do with the owner's yes.
    let mut touched: Vec<Id> = Vec::new();
    if event.origin.is_remote() {
        touched.push(event.origin.account);
    }
    if source.origin.is_remote() {
        touched.push(source.origin.account);
    }
    if into.origin.is_remote() {
        touched.push(into.origin.account);
    }
    touched.sort_unstable();
    touched.dedup();
    for account in touched {
        out.0.write_u64(account);
        match s.mailbox.account(account) {
            Some(account) => {
                out.0.write_u8(1);
                routing(&mut out, account)?;
            }
            None => out.0.write_u8(0),
        }
    }
    Some(out.0.finish())
}

/// The settings of an account that decide where a calendar change goes and which sign-in carries
/// it: its identity, the kind of server, the login and the servers themselves (a calendar and
/// contacts server's homes, user name, trusted certificate and password flag among them). A
/// colour, a name, a signature or how much mail is kept offline routes nothing, and hashing the
/// account whole would retract the owner's yes for an edit that never reached a server.
fn routing(out: &mut Digest, account: &mailcraft_model::Account) -> Option<()> {
    written(out, &account.id)?;
    written(out, &account.email)?;
    written(out, &account.kind)?;
    written(out, &account.username)?;
    written(out, &account.incoming)?;
    written(out, &account.outgoing)?;
    written(out, &account.shared_of)?;
    if let Some(oauth) = &account.oauth {
        written(out, &oauth.provider)?;
        written(out, &oauth.client_id)?;
        written(out, &oauth.tenant)?;
        written(out, &oauth.directory)?;
    }
    if let Some(server) = &account.jmap {
        written(out, &server.session)?;
        written(out, &server.bearer)?;
    }
    if let Some(server) = &account.ews {
        // (`hierarchy` is a sync state, not a setting: a run that read the folder list must not
        // retract a question the owner is answering.)
        written(out, &server.url)?;
        written(out, &server.username)?;
        written(out, &server.trusted)?;
    }
    if let Some(dav) = &account.dav {
        // Where a calendar and contacts server takes the change, and the sign-in that carries it.
        // (What is known of each collection is a sync state, not a setting.)
        written(out, &dav.calendar_homes)?;
        written(out, &dav.contact_homes)?;
        written(out, &dav.username)?;
        written(out, &dav.trusted)?;
        written(out, &dav.separate_password)?;
    }
    Some(())
}

fn written<T: serde::Serialize>(out: &mut Digest, value: &T) -> Option<()> {
    serde_json::to_writer(out, value).ok()
}

/// The identity of the calendar and contacts server of the account `account.davSet` or
/// `account.davOff` acts on, as one number keyed by `keys`: its homes, user name, pinned
/// certificate and password flag. Zero for any other command.
fn dav_identity(keys: &RandomState, s: &Session, id: &str, params: &Value) -> u64 {
    if !matches!(id, "account.davSet" | "account.davOff") {
        return 0;
    }
    let mut out = keys.build_hasher();
    let account = p::id(params, "id").and_then(|a| s.mailbox.account(a));
    // The address is what a request with no user name signs in as, so a change of it between the
    // question and the yes is a change of what was approved, whatever the request names.
    if let Some(account) = account {
        out.write(account.email.trim().to_ascii_lowercase().as_bytes());
        out.write_u8(0xff);
    }
    match account.and_then(|a| a.dav.as_ref().map(|dav| (a, dav))) {
        None => out.write_u8(0),
        Some((account, dav)) => {
            out.write_u8(1);
            for list in [&dav.calendar_homes, &dav.contact_homes] {
                out.write_usize(list.len());
                for home in list {
                    out.write(home.trim().as_bytes());
                    out.write_u8(0xff);
                }
            }
            // The name that signs in, as the credential's name has it: the one given, else the
            // account's address (case-folded).
            let login = match dav.username.trim() {
                "" => account.email.trim().to_ascii_lowercase(),
                name => name.to_string(),
            };
            for text in [login.as_str(), dav.trusted.trim()] {
                out.write(text.as_bytes());
                out.write_u8(0xff);
            }
            out.write_u8(u8::from(dav.separate_password));
        }
    }
    out.finish()
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
