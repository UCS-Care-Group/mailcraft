// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The part of a run that goes up (#252, E6): the changes of folders, the changes of messages, the
//! drafts and the messages to send, in this order, before anything is read; what became of each
//! goes back in one piece.
//!
//! **A change that is not answered** (the server was busy past the waits, the connection went
//! before it was asked) is not in the piece: it is worked out again next run. A change whose
//! answer was **lost after it went out** is not sent again in this run either, and is not in the
//! piece: an update or a delete is the same when it is made twice, a move finds nothing under its
//! old id and the read that follows says where it went.
//!
//! **A send is never made twice.** A message whose send was lost is `Held { lost: true }`: it may
//! have gone, and the owner looks in Sent Items before sending it again. A draft whose make was
//! lost is found by its marker next run, never made a second time. Any word of the server that
//! may follow its having sent the message (an internal error, a timeout, a code not known here,
//! a fault, a 5xx or an answer that can't be read) is held as "may have been sent", never as "was
//! not sent"; only the codes in [`NOT_SENT`], which turn a send away before it is done, are.
//!
//! **The crash window.** The window takes in what became of a send when the run's piece with
//! the answer reaches it (`apply_ews_mail`), and the piece is emitted as soon as the sends of the
//! run are done. A crash or a power cut after the server sent a message and before that piece was
//! taken in leaves the message in the Outbox as it was, and the next run sends it again: the
//! one way a message can go twice. The window is as short as the time between the answer and the
//! hand-over of the piece, and nothing here can close it without the server's help (a draft's
//! copy in Sent Items is what the owner would look for).
//!
//! **What a server lacks** (E7) is left as it is and said once a run, without an id: a server
//! that has no follow-up flag keeps the flags here.

use std::collections::HashMap;

use mailcraft_model::sync::{
    ChangeOutcome, DraftDone, DraftUp, EwsMailRequest, EwsMailResult, FlagUp, FolderChange, FolderChangeDone, GraphFolder, MAX_DRAFTS,
    MAX_FOLDER_CHANGES, MAX_MESSAGE_CHANGES, MAX_SENDS, MessageChange, MessageChangeDone, MessageOutcome, SendDone, SendOutcome, SendUp,
    SyncedAttachment, marker_id, marker_of, merge_categories,
};
use mailcraft_model::{FolderKind, Id, Importance};

use crate::changes::{CreateFolder, DeleteFolder, DeleteItem, DeleteMode, GetItemNow, ItemUpdate, MoveFolder, MoveItem, RenameFolder, UpdateItem};
use crate::compose::{AddAttachments, CreateDraft, DraftContent, FindDraft, RemoveAttachments, SendMessage};
use crate::error::ResponseCode;
use crate::http::HttpExchange;
use crate::run::{Asked, Run, base};

/// Codes a server gives when it doesn't have a property (the flag, here).
const LACKS_PROPERTY: [&str; 3] = ["ErrorInvalidPropertyRequest", "ErrorInvalidPropertyForOperation", "ErrorInvalidPropertySet"];

fn refused_status(code: &ResponseCode) -> u16 {
    if *code == ResponseCode::AccessDenied { 403 } else { 400 }
}

/// Why a folder change was refused, in words that name nothing.
fn why(code: &ResponseCode) -> String {
    let text = match code.to_string().as_str() {
        "ErrorAccessDenied" => "the server doesn't allow it",
        "ErrorFolderExists" => "a folder of that name is there already",
        "ErrorCannotDeleteObject" | "ErrorDeleteDistinguishedFolder" => "the server doesn't let that folder be deleted",
        _ => "the server won't do it",
    };
    text.to_owned()
}

fn plain(code: &ResponseCode) -> String {
    match code.to_string().as_str() {
        "ErrorSendAsDenied" => mailcraft_model::sync::SEND_AS_DENIED.to_owned(),
        "ErrorAccessDenied" => "the server doesn't allow it".to_owned(),
        "ErrorQuotaExceeded" | "ErrorMessageSizeExceeded" => "the mailbox or the message is over a limit the server holds".to_owned(),
        "ErrorInvalidRecipients" | "ErrorMessageDispositionRequired" | "ErrorRecipientNotFound" => {
            "a recipient's address isn't one the server accepts".to_owned()
        }
        _ => "the server won't take it".to_owned(),
    }
}

impl<E: HttpExchange> Run<'_, E> {
    /// Everything of `request` that goes up, emitted as one piece (before it is read from).
    pub(crate) async fn push(&mut self, request: &EwsMailRequest, emit: &mut dyn FnMut(EwsMailResult)) -> Result<(), Asked> {
        if request.changes.is_empty() && request.messages.is_empty() && request.drafts.is_empty() && request.sends.is_empty() {
            return Ok(());
        }
        let mut piece = EwsMailResult { partial: true, ..base(request) };
        let outcome = self.push_into(request, &mut piece).await;
        log::info!(
            "ews up: {} folder changes, {} message changes, {} drafts, {} messages sent of those asked",
            piece.changes.len(),
            piece.messages.len(),
            piece.drafts.len(),
            piece.sent.iter().filter(|d| d.outcome == SendOutcome::Sent).count()
        );
        emit(piece);
        outcome
    }

    async fn push_into(&mut self, request: &EwsMailRequest, piece: &mut EwsMailResult) -> Result<(), Asked> {
        // The folders made in this run, by the id they have here.
        let mut made: HashMap<Id, String> = HashMap::new();
        for change in request.changes.iter().take(MAX_FOLDER_CHANGES) {
            if let Some(outcome) = self.folder_change(change, &mut made).await? {
                piece.changes.push(FolderChangeDone { change: change.clone(), outcome });
            }
        }
        for change in request.messages.iter().take(MAX_MESSAGE_CHANGES) {
            if let Some(outcome) = self.message_change(change, &made).await? {
                piece.messages.push(MessageChangeDone { change: change.clone(), outcome, there: None });
            }
        }
        for draft in request.drafts.iter().take(MAX_DRAFTS) {
            let (done, ended) = self.draft(draft).await;
            if !done.id.is_empty() || done.refused.is_some() || done.conflict {
                piece.drafts.push(done);
            }
            ended?;
        }
        for send in request.sends.iter().take(MAX_SENDS) {
            let (done, ended) = self.send(send).await;
            if done.draft.is_some() || done.outcome != SendOutcome::Waits {
                piece.sent.push(done);
            }
            ended?;
        }
        Ok(())
    }

    // ---- folders ----

    async fn folder_change(&mut self, change: &FolderChange, made: &mut HashMap<Id, String>) -> Result<Option<ChangeOutcome>, Asked> {
        let folder = |id: &str, name: &str, parent: &str| GraphFolder {
            id: id.to_owned(),
            name: name.to_owned(),
            parent: parent.to_owned(),
            kind: FolderKind::Custom,
            ..GraphFolder::default()
        };
        let asked = match change {
            FolderChange::Create { folder: local, name, parent, parent_folder } => {
                let parent = match parent_folder {
                    Some(l) => match made.get(l) {
                        Some(id) => id.clone(),
                        // (Its parent was not made: this waits for the next run.)
                        None => return Ok(None),
                    },
                    None => parent.clone(),
                };
                let op = CreateFolder::new(&parent, name);
                let made_now = self.ask_once(&op.element(), &|reply| op.answer(reply)).await;
                if let Ok(id) = &made_now {
                    made.insert(*local, id.clone());
                }
                made_now.map(|id| folder(&id, name, &parent))
            }
            FolderChange::Rename { id, name, .. } => {
                let op = RenameFolder::new(id, name);
                // (The server's parent is kept as it was: the caller reads only the name.)
                self.ask(&op.element(), &|reply| op.answer(reply)).await.map(|()| folder(id, name, ""))
            }
            FolderChange::Move { id, parent, .. } => {
                let op = MoveFolder::new(id, parent);
                self.ask_once(&op.element(), &|reply| op.answer(reply)).await.map(|id| folder(&id, "", parent))
            }
            FolderChange::Delete { id } => {
                let op = DeleteFolder::new(id, DeleteMode::SoftDelete);
                self.ask(&op.element(), &|reply| op.answer(reply)).await.map(|()| GraphFolder::default())
            }
        };
        match asked {
            Ok(mut there) => {
                // A rename says the name and no parent; a move says the parent and no name: the
                // engine reads only the one that changed, from the folder it has.
                if let (FolderChange::Rename { .. }, true) = (change, there.parent.is_empty()) {
                    there.parent.clear();
                }
                Ok(Some(ChangeOutcome::Done { folder: there }))
            }
            Err(Asked::Code(ResponseCode::FolderNotFound)) => Ok(Some(ChangeOutcome::Gone)),
            Err(Asked::Code(code)) => Ok(Some(ChangeOutcome::Refused { why: why(&code) })),
            Err(Asked::Bad | Asked::Unreadable) => Ok(Some(ChangeOutcome::Refused { why: "the server's answer couldn't be used".to_owned() })),
            Err(Asked::Lost) => Ok(None),
            Err(stop @ Asked::Stop(_)) => Err(stop),
        }
    }

    // ---- messages ----

    async fn message_change(&mut self, change: &MessageChange, made: &HashMap<Id, String>) -> Result<Option<MessageOutcome>, Asked> {
        let done = |folder: &str, id: &str| MessageOutcome::Done { categories: None, flag: None, folder: folder.to_owned(), id: id.to_owned() };
        let asked: Result<MessageOutcome, Asked> = match change {
            MessageChange::Update { id, read, flag, categories, importance, .. } => {
                return self.update(id, *read, flag.as_ref(), categories.as_ref().map(|c| (&c.synced, &c.here)), *importance).await;
            }
            MessageChange::Move { id, to, to_folder, .. } | MessageChange::Copy { id, to, to_folder, .. } => {
                let target = match to_folder {
                    Some(l) => match made.get(l) {
                        Some(id) => id.clone(),
                        None => return Ok(None),
                    },
                    None => to.clone(),
                };
                let copy = matches!(change, MessageChange::Copy { .. });
                let op = if copy { MoveItem::copy(id, &target) } else { MoveItem::new(id, &target) };
                self.ask_once(&op.element(), &|reply| op.answer(reply)).await.map(|saved| {
                    // A moved item has a new id; the id of a copy is the one the engine keeps.
                    let new = if saved.id == *id && !copy { "" } else { saved.id.as_str() };
                    done(&target, new)
                })
            }
            MessageChange::Delete { id } => {
                // (A draft only known by its marker is found first; none found is gone.)
                let target = match marker_of(id) {
                    Some(marker) => {
                        let find = FindDraft::new(None, marker);
                        match self.ask(&find.element(), &|reply| find.answer(reply)).await {
                            Ok(Some(found)) => found.id,
                            Ok(None) => return Ok(Some(MessageOutcome::Gone)),
                            Err(error) => return settle(error),
                        }
                    }
                    None => id.clone(),
                };
                let op = DeleteItem::new(&target, DeleteMode::SoftDelete);
                self.ask(&op.element(), &|reply| op.answer(reply)).await.map(|()| done("", ""))
            }
            // (A message moved out of the account is not one of an Exchange server account's.)
            MessageChange::MovedOut { .. } => return Ok(None),
        };
        match asked {
            Ok(outcome) => Ok(Some(outcome)),
            Err(error) => settle(error),
        }
    }

    async fn update(
        &mut self,
        id: &str,
        read: Option<bool>,
        flag: Option<&FlagUp>,
        categories: Option<(&Vec<String>, &Vec<String>)>,
        importance: Option<Importance>,
    ) -> Result<Option<MessageOutcome>, Asked> {
        // The categories as the server has them now, with what was added and taken away here applied.
        let mut merged = None;
        if let Some((synced, here)) = categories {
            let op = GetItemNow::new(id);
            match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(now) => merged = Some(merge_categories(synced, here, &now.categories)),
                Err(error) => return settle(error),
            }
        }
        loop {
            let update = ItemUpdate {
                read,
                flag: flag.filter(|_| !self.flag_lacking).cloned(),
                categories: merged.clone(),
                importance,
                ..ItemUpdate::default()
            };
            let op = UpdateItem::marks(id, update);
            let outcome = MessageOutcome::Done { categories: merged.clone(), flag: flag.cloned(), folder: String::new(), id: String::new() };
            if op.is_empty() {
                // (Only a flag the server doesn't have: it stays here.)
                return Ok(Some(outcome));
            }
            match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(_) => return Ok(Some(outcome)),
                Err(Asked::Code(code)) if flag.is_some() && !self.flag_lacking && LACKS_PROPERTY.contains(&code.to_string().as_str()) => {
                    self.flag_lacking = true;
                    log::info!("ews up: the server has no follow-up flag; flags stay here");
                }
                Err(error) => return settle(error),
            }
        }
    }

    // ---- drafts ----

    /// One draft brought up to date: made (found by its marker first), or changed in place.
    /// What became of it, and why the run must stop if it must.
    pub(crate) async fn draft(&mut self, up: &DraftUp) -> (DraftDone, Result<(), Asked>) {
        let mut done = DraftDone { change: up.without_data(), id: up.id.clone(), ..DraftDone::default() };
        let ended = self.draft_into(up, &mut done).await;
        (done, ended)
    }

    async fn draft_into(&mut self, up: &DraftUp, done: &mut DraftDone) -> Result<(), Asked> {
        let (mut id, mut key) = (up.id.clone(), up.change_key.clone());
        let mut adopted = false;
        if id.is_empty() {
            // A make whose answer was lost is found by its marker.
            let find = FindDraft::new(None, &up.marker).knowing(&up.change_key);
            match self.ask(&find.element(), &|reply| find.answer(reply)).await {
                Ok(Some(found)) => {
                    (id, key) = (found.id, found.change_key);
                    adopted = true;
                }
                Ok(None) => {}
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                // (Not found, not made: it waits for the next run.)
                Err(_) => return Ok(()),
            }
        }
        if id.is_empty() {
            return self.make_draft(up, done).await;
        }
        // Changed in place, field by field. A draft changed elsewhere since is not written over.
        let update = ItemUpdate {
            subject: up.subject.clone(),
            body: up.body.as_ref().map(|b| (b.html, b.content.clone())),
            to: up.to.clone(),
            cc: up.cc.clone(),
            bcc: up.bcc.clone(),
            importance: up.importance,
            ..ItemUpdate::default()
        };
        let op = UpdateItem::draft(&id, if adopted { "" } else { &key }, update);
        if !op.is_empty() {
            match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(saved) => key = saved.change_key,
                Err(Asked::Code(code)) if is_code_name(&code, "ErrorIrresolvableConflict") => {
                    done.conflict = true;
                    return Ok(());
                }
                Err(Asked::Code(ResponseCode::ItemNotFound)) => {
                    done.refused = Some(404);
                    return Ok(());
                }
                Err(Asked::Code(code)) => {
                    done.refused = Some(refused_status(&code));
                    return Ok(());
                }
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                Err(_) => return Ok(()),
            }
        }
        done.id = id.clone();
        done.fields = true;
        if let Some(attachments) = &up.attachments {
            match self.sync_attachments(&id, &key, attachments).await {
                Ok((synced, now)) => {
                    done.attachments = Some(synced);
                    key = now.unwrap_or(key);
                }
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                // (They go next run, or the owner is told by the draft not being whole.)
                Err(_) => {}
            }
        }
        done.change_key = key;
        Ok(())
    }

    /// Makes the draft in the server's Drafts, with its files.
    async fn make_draft(&mut self, up: &DraftUp, done: &mut DraftDone) -> Result<(), Asked> {
        let here = up.attachments.as_ref().map(|a| a.here.clone()).unwrap_or_default();
        let content = DraftContent {
            subject: up.subject.clone().unwrap_or_default(),
            body: up.body.as_ref().map(|b| (b.html, b.content.clone())).unwrap_or_default(),
            to: up.to.clone().unwrap_or_default(),
            cc: up.cc.clone().unwrap_or_default(),
            bcc: up.bcc.clone().unwrap_or_default(),
            importance: up.importance.unwrap_or_default(),
            attachments: here.clone(),
            marker: up.marker.clone(),
        };
        let op = CreateDraft::new(None, content);
        match self.ask_once(&op.element(), &|reply| op.answer(reply)).await {
            Ok(saved) => {
                done.id = saved.id.clone();
                done.change_key = saved.change_key;
                done.fields = true;
            }
            Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
            Err(Asked::Code(code)) => {
                done.refused = Some(refused_status(&code));
                return Ok(());
            }
            // (Lost: found by its marker next run, never made twice.)
            Err(_) => return Ok(()),
        }
        if up.attachments.is_some() {
            // The files went with it; which ids they have is read.
            let now = GetItemNow::new(&done.id);
            let ids = self.ask(&now.element(), &|reply| now.answer(reply)).await.ok();
            done.attachments = Some(
                here.iter()
                    .map(|a| SyncedAttachment {
                        id: ids.as_ref().and_then(|n| n.attachments.iter().find(|f| f.name == a.name)).map(|f| f.id.clone()).unwrap_or_default(),
                        name: a.name.clone(),
                        hash: a.hash,
                    })
                    .collect(),
            );
            if let Some(n) = ids
                && !n.change_key.is_empty()
            {
                done.change_key = n.change_key;
            }
        }
        Ok(())
    }

    /// Brings a draft's files in step: those taken off here are taken off there, those added here
    /// are added. The files the server has now, and the draft's change key after.
    async fn sync_attachments(
        &mut self,
        id: &str,
        key: &str,
        up: &mailcraft_model::sync::AttachmentsUp,
    ) -> Result<(Vec<SyncedAttachment>, Option<String>), Asked> {
        let now = GetItemNow::new(id);
        let there = self.ask(&now.element(), &|reply| now.answer(reply)).await?;
        let mut key = (!key.is_empty()).then(|| key.to_owned());
        // (The files the server holds that are not yet matched to one known to be synced.)
        let mut free = there.attachments.clone();
        let same = |name: &str, hash: u64, other: &SyncedAttachment| other.name == name && other.hash == hash;
        // Taken off here: what was synced and is no longer here.
        let mut remove = Vec::new();
        let mut kept: Vec<SyncedAttachment> = Vec::new();
        for s in &up.synced {
            if up.here.iter().any(|h| same(&h.name, h.hash, s)) {
                // (Still there: its id is the one known, or the one by its name.)
                let at = free.iter().position(|f| (!s.id.is_empty() && f.id == s.id) || (s.id.is_empty() && f.name == s.name));
                let found = at.map(|i| free.remove(i));
                kept.push(SyncedAttachment { id: found.map(|f| f.id).unwrap_or_else(|| s.id.clone()), name: s.name.clone(), hash: s.hash });
            } else if let Some(at) = free.iter().position(|f| (!s.id.is_empty() && f.id == s.id) || (s.id.is_empty() && f.name == s.name)) {
                remove.push(free.remove(at).id);
            }
        }
        if !remove.is_empty() {
            let op = RemoveAttachments::new(remove);
            let new = self.ask(&op.element(), &|reply| op.answer(reply)).await?;
            if !new.is_empty() {
                key = Some(new);
            }
        }
        // Added here: what is here and was not synced. A file the server draft already holds (a
        // make or an add whose answer was lost, so that this draft's `synced` doesn't know it) is
        // taken as added, by its name and size (and content id, for an inline one), and never
        // added again: the draft would be sent with it twice.
        let mut added = Vec::new();
        let mut add = Vec::new();
        for h in up.here.iter().filter(|h| !kept.iter().any(|k| same(&h.name, h.hash, k))) {
            let held = free.iter().position(|f| {
                f.name == h.name
                    && (f.size == 0 || f.size == h.size)
                    && (h.content_id.is_empty() || f.content_id.is_empty() || f.content_id == h.content_id)
            });
            match held {
                Some(at) => added.push(SyncedAttachment { id: free.remove(at).id, name: h.name.clone(), hash: h.hash }),
                None => add.push(h.clone()),
            }
        }
        if !add.is_empty() {
            let op = AddAttachments::new(id, key.as_deref().unwrap_or(&there.change_key), add.clone());
            match self.ask_once(&op.element(), &|reply| op.answer(reply)).await {
                Ok((ids, new)) => {
                    if !new.is_empty() {
                        key = Some(new);
                    }
                    added.extend(add.iter().zip(ids).map(|(a, aid)| SyncedAttachment { id: aid, name: a.name.clone(), hash: a.hash }));
                }
                // (It may have gone: the files are read again next run, and not added twice by name and hash.)
                Err(Asked::Lost) => return Err(Asked::Lost),
                Err(other) => return Err(other),
            }
        }
        // In the order they are here.
        let all = up.here.iter().filter_map(|h| kept.iter().chain(added.iter()).find(|s| same(&h.name, h.hash, s)).cloned()).collect();
        Ok((all, key))
    }

    // ---- sending ----

    /// One message sent (a saved draft is brought up to date and sent as the draft; one never saved
    /// goes as its MIME). Never twice: a lost answer is held.
    pub(crate) async fn send(&mut self, send: &SendUp) -> (SendDone, Result<(), Asked>) {
        let mut done = SendDone { message: send.message, outcome: SendOutcome::Waits, draft: None, draft_left: String::new() };
        let ended = self.send_into(send, &mut done).await;
        (done, ended)
    }

    async fn send_into(&mut self, send: &SendUp, done: &mut SendDone) -> Result<(), Asked> {
        // Every message goes as its MIME with a copy asked for in Sent Items, a saved draft too:
        // `SendItem` of a draft leaves it in Drafts and files no copy on some servers (Grommunio,
        // E8). A send with no MIME (not one the engine makes for an Exchange account) waits.
        let Some(message) = &send.mime else {
            log::info!("ews sending: a message without its content waits");
            return Ok(());
        };
        let op = match SendMessage::new(message) {
            Ok(op) => op,
            Err(e) => {
                done.outcome = SendOutcome::Held { lost: false, why: e.to_string() };
                return Ok(());
            }
        };
        match self.ask_once(&op.element(), &|reply| op.answer(reply)).await {
            Ok(()) => done.outcome = SendOutcome::Sent,
            Err(error) => return self.sent(error, done, plain_or_lost),
        }
        // The draft it was saved as goes only after a clear success, as a soft delete (kept
        // recoverable); never a hard one. Gone already is as good. A draft whose make was not
        // answered has no id here: it is found by its marker first. One that could not be
        // found or taken away is named (by its id, or its marker), and goes with the next run as
        // an ordinary delete: the send is not made again.
        let Some(draft) = send.draft.as_ref().filter(|d| !d.id.is_empty() || !d.marker.is_empty()) else { return Ok(()) };
        let id = if draft.id.is_empty() {
            let find = FindDraft::new(None, &draft.marker);
            match self.ask(&find.element(), &|reply| find.answer(reply)).await {
                Ok(Some(found)) => found.id,
                // (None: it was never made.)
                Ok(None) => return Ok(()),
                Err(Asked::Stop(stop)) => {
                    done.draft_left = marker_id(&draft.marker);
                    return Err(Asked::Stop(stop));
                }
                Err(_) => {
                    done.draft_left = marker_id(&draft.marker);
                    log::info!("ews sending: the draft of a message sent could not be looked for; it is taken away next run");
                    return Ok(());
                }
            }
        } else {
            draft.id.clone()
        };
        let op = DeleteItem::new(&id, DeleteMode::SoftDelete);
        match self.ask(&op.element(), &|reply| op.answer(reply)).await {
            Ok(()) | Err(Asked::Code(ResponseCode::ItemNotFound)) => Ok(()),
            Err(Asked::Stop(stop)) => {
                done.draft_left = id;
                Err(Asked::Stop(stop))
            }
            Err(_) => {
                done.draft_left = id;
                log::info!("ews sending: the draft of a message sent stays on the server for now; it is taken away next run");
                Ok(())
            }
        }
    }

    /// What an error of a send means: held (the answer was lost, or the server refused it), or the
    /// error that stops the run (it wasn't taken: it waits).
    fn sent(&mut self, error: Asked, done: &mut SendDone, words: fn(&Asked) -> SendOutcome) -> Result<(), Asked> {
        match error {
            Asked::Stop(stop) => Err(Asked::Stop(stop)),
            other => {
                done.outcome = words(&other);
                if done.outcome == (SendOutcome::Held { lost: true, why: String::new() }) {
                    log::info!("ews sending: the answer to a send was lost; the message is held, not sent again by itself");
                }
                Ok(())
            }
        }
    }
}

/// The codes that say a send was turned away before anything was done with it. Any other (an
/// internal error, a timeout, a code not known here) may follow the server having sent it.
const NOT_SENT: [&str; 10] = [
    "ErrorSendAsDenied",
    "ErrorAccessDenied",
    "ErrorQuotaExceeded",
    "ErrorMessageSizeExceeded",
    "ErrorInvalidRecipients",
    "ErrorMessageDispositionRequired",
    "ErrorRecipientNotFound",
    "ErrorItemNotFound",
    "ErrorSchemaValidation",
    "ErrorInvalidRequest",
];

fn plain_or_lost(error: &Asked) -> SendOutcome {
    match error {
        Asked::Code(code) if NOT_SENT.contains(&code.to_string().as_str()) => SendOutcome::Held { lost: false, why: plain(code) },
        // (An answer that is none the service gives, a fault that may follow processing, or none
        // at all: it may have gone, and it is held as "may have been sent", never as "wasn't".)
        _ => SendOutcome::Held { lost: true, why: String::new() },
    }
}

fn is_code_name(code: &ResponseCode, name: &str) -> bool {
    code.to_string() == name
}

/// What an error of a change of a message means: its outcome, nothing (waits), or the stop.
fn settle(error: Asked) -> Result<Option<MessageOutcome>, Asked> {
    match error {
        Asked::Code(ResponseCode::ItemNotFound) => Ok(Some(MessageOutcome::Gone)),
        Asked::Code(code) => Ok(Some(MessageOutcome::Refused { status: refused_status(&code) })),
        Asked::Bad | Asked::Unreadable => Ok(Some(MessageOutcome::Refused { status: 400 })),
        Asked::Lost => Ok(None),
        stop @ Asked::Stop(_) => Err(stop),
    }
}
