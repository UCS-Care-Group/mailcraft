// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drafts of a Microsoft account up through Microsoft Graph (R3.4.1; R3 spec S23–S26).
//!
//! A draft saved here ([`ServerDraft::pending`]) goes up with the next run: what differs from what
//! the server last had of it ([`ServerDraft::synced`]), field by field, its attachments when they
//! changed, everything for one the server hasn't got yet (made with MailCraft's marker, as an
//! answer or a forward when it was begun as one to a message the server has). What comes back is
//! taken in by the draft's id here and its marker: the server's id, what went up, its change key.
//! A draft saved again while its run was under way stays pending for the next one.

use mailcraft_model::sync::{AttachmentUp, AttachmentsUp, DraftBody, DraftDone, DraftState, DraftUp, ServerDraft, ServerMessage, fingerprint};
use mailcraft_model::{FolderKind, Id, Mailbox, Message};

use crate::Session;

/// The drafts of `account` that wait to go up: at most `limit` spelled out, and how many there are.
pub fn draft_changes(s: &Session, account: Id, limit: usize) -> (Vec<DraftUp>, usize) {
    let mb = &s.mailbox;
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    let mut waiting: Vec<&Message> = mb
        .messages
        .values()
        .map(AsRef::as_ref)
        .filter(|m| m.account == account && m.draft && Some(m.folder) != outbox)
        .filter(|m| m.server_draft.as_ref().is_some_and(|d| d.pending))
        .collect();
    waiting.sort_by_key(|m| m.id);
    let count = waiting.len();
    // Compared and sent whole: a draft's body and files kept on disk (R6.1) are read for it, so
    // that it neither looks changed nor goes up empty. One that can't be read waits for the next run.
    let ups = waiting
        .into_iter()
        .take(limit)
        .filter_map(|m| match s.loaded_message(m.id) {
            Ok(m) => draft_up(&m),
            Err(e) => {
                log::warn!("draft {} waits to go up: {e}", m.id);
                None
            }
        })
        .collect();
    (ups, count)
}

/// How many drafts of `account` wait to go up (cheap: nothing is compared).
pub fn pending(mb: &Mailbox, account: Id) -> usize {
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox);
    mb.messages
        .values()
        .filter(|m| m.account == account && m.draft && Some(m.folder) != outbox && m.server_draft.as_ref().is_some_and(|d| d.pending))
        .count()
}

/// What of `m` goes up: what differs from what the server last had.
pub fn draft_up(m: &Message) -> Option<DraftUp> {
    let sd = m.server_draft.as_ref()?;
    let id = m.server.as_ref().filter(|s| !s.gone).map(|s| s.id.clone()).unwrap_or_default();
    let now = DraftState::of(m);
    let synced = sd.synced.as_ref();
    let differs = |same: &dyn Fn(&DraftState) -> bool| !synced.is_some_and(same);
    let names = |state: &DraftState| {
        let mut all: Vec<(String, u64)> = state.attachments.iter().map(|a| (a.name.clone(), a.hash)).collect();
        all.sort();
        all
    };
    let attachments = differs(&|was| names(was) == names(&now)).then(|| AttachmentsUp {
        here: m.attachments.iter().map(AttachmentUp::of).collect(),
        synced: synced.map(|was| was.attachments.clone()).unwrap_or_default(),
    });
    Some(DraftUp {
        message: m.id,
        of: if id.is_empty() { sd.of.clone() } else { None },
        id,
        marker: sd.marker.clone(),
        change_key: m.server.as_ref().filter(|s| !s.gone).map(|s| s.change_key.clone()).unwrap_or_default(),
        saves: sd.saves,
        subject: differs(&|was| was.subject == now.subject).then(|| m.subject.clone()),
        body: differs(&|was| was.body == now.body).then(|| DraftBody::of(m)),
        to: differs(&|was| was.to == now.to).then(|| m.to.clone()),
        cc: differs(&|was| was.cc == now.cc).then(|| m.cc.clone()),
        bcc: differs(&|was| was.bcc == now.bcc).then(|| m.bcc.clone()),
        // (Nothing to say of where replies go, for a draft the server never had: none is set.)
        reply_to: differs(&|was| was.reply_to == now.reply_to).then(|| m.reply_to.clone()).filter(|r| synced.is_some() || !r.is_empty()),
        importance: differs(&|was| was.importance == now.importance).then_some(m.importance),
        attachments,
    })
}

/// Everything of `m` as the owner has it, for a draft that is to be sent: every field and the
/// whole set of attachments, whatever the server last had. What was changed on the server since
/// (a Bcc, the body, an attachment added) is written over before the draft is sent, never sent
/// (AU2-14). `None` when `m` is no draft of the server's.
pub fn draft_up_whole(m: &Message) -> Option<DraftUp> {
    let up = draft_up(m)?;
    let synced = m.server_draft.as_ref().and_then(|sd| sd.synced.as_ref()).map(|was| was.attachments.clone()).unwrap_or_default();
    Some(DraftUp {
        subject: Some(m.subject.clone()),
        body: Some(DraftBody::of(m)),
        to: Some(m.to.clone()),
        cc: Some(m.cc.clone()),
        bcc: Some(m.bcc.clone()),
        reply_to: Some(m.reply_to.clone()),
        importance: Some(m.importance),
        attachments: Some(up.attachments.unwrap_or_else(|| AttachmentsUp { here: m.attachments.iter().map(AttachmentUp::of).collect(), synced })),
        ..up
    })
}

/// A marker for a draft: a value used once, of the draft's id here and `seed`.
pub fn marker(id: Id, seed: u64) -> String {
    format!("mailcraft-draft-{id:x}-{seed:x}")
}

/// Take in what became of one draft. Whether anything changed here; a line for the owner, when
/// the server wouldn't take it.
pub fn draft_done(mb: &mut Mailbox, account: Id, done: &DraftDone, lang: mailcraft_i18n::Lang, said: &mut Vec<String>) -> bool {
    let change = &done.change;
    let Some(m) = mb.message(change.message).filter(|m| m.account == account) else { return false };
    let Some(sd) = m.server_draft.clone().filter(|sd| sd.marker == change.marker) else { return false };
    let subject: String = m.subject.chars().filter(|c| !c.is_control()).take(60).collect();
    let subject = if subject.trim().is_empty() { lang.t("(no subject)").to_string() } else { subject };
    let drafts_there =
        mb.folder_of_kind(account, FolderKind::Drafts).and_then(|f| mb.folder(f)).and_then(|f| f.server.as_ref()).map(|s| s.id.clone());
    if done.conflict {
        // Changed elsewhere too: the server's version comes down by itself, as a draft of its own.
        return keep_twice(mb, change.message, None).is_some();
    }
    if let Some(status) = done.refused {
        let Ok(m) = mb.message_mut(change.message) else { return false };
        let Some(sd) = m.server_draft.as_mut() else { return false };
        if matches!(status, 404 | 410) && !change.id.is_empty() {
            // The server's copy is gone: this one is made there again, nothing of it lost.
            log::info!("graph drafts: a draft was gone from the server; it is made there again");
            m.server = None;
            sd.synced = None;
        } else {
            // Not taken: it waits until it is saved again, and that is said once.
            sd.pending = false;
            let why = if status == 403 { lang.t("this account may not save it") } else { lang.t("it can't take it") };
            let line = lang.t("Couldn't save the draft '{subject}' on the server: the server says {why}.");
            said.push(mailcraft_i18n::fmt(line, &[("subject", &subject), ("why", why)]));
        }
        return true;
    }
    if !done.id.is_empty() {
        // Come down meanwhile (its make's answer was lost, and the delta brought it): that is this one.
        let twins: Vec<Id> = mb
            .messages
            .values()
            .filter(|o| o.account == account && o.id != change.message && o.server.as_ref().is_some_and(|s| s.id == done.id))
            .map(|o| o.id)
            .collect();
        for twin in twins {
            let _ = mb.forget_message(twin);
        }
    }
    let Ok(m) = mb.message_mut(change.message) else { return false };
    if !done.id.is_empty() {
        let folder = if done.folder.is_empty() { drafts_there.unwrap_or_default() } else { done.folder.clone() };
        match m.server.as_mut().filter(|s| s.id == done.id) {
            Some(server) => {
                if !done.change_key.is_empty() {
                    server.change_key = done.change_key.clone();
                }
                if !folder.is_empty() {
                    server.folder = folder;
                }
            }
            None => {
                m.server = Some(ServerMessage {
                    id: done.id.clone(),
                    folder,
                    read: m.read,
                    flag: m.flag,
                    flag_due: m.flag_due,
                    categories: m.categories.clone(),
                    importance: m.importance,
                    change_key: done.change_key.clone(),
                    ..Default::default()
                })
            }
        }
    }
    let mut synced = sd.synced.clone();
    if done.fields {
        let state = synced.get_or_insert_with(DraftState::default);
        if let Some(subject) = &change.subject {
            state.subject = subject.clone();
        }
        if let Some(body) = &change.body {
            state.body = body.hash;
        }
        if let Some(to) = &change.to {
            state.to = to.clone();
        }
        if let Some(cc) = &change.cc {
            state.cc = cc.clone();
        }
        if let Some(bcc) = &change.bcc {
            state.bcc = bcc.clone();
        }
        if let Some(reply_to) = &change.reply_to {
            state.reply_to = reply_to.clone();
        }
        if let Some(importance) = change.importance {
            state.importance = importance;
            if let Some(server) = m.server.as_mut() {
                server.importance = importance;
            }
        }
    }
    if let Some(attachments) = &done.attachments {
        synced.get_or_insert_with(DraftState::default).attachments = attachments.clone();
    }
    let complete = !done.id.is_empty() && done.fields && (change.attachments.is_none() || done.attachments.is_some());
    let sd = ServerDraft { synced, pending: sd.pending && !(complete && sd.saves == change.saves), ..sd };
    m.server_draft = Some(sd);
    true
}

/// A draft here and on the server each changed since the last run (S26): both are kept. The
/// server's comes down as a draft of its own (`there`, under its server id); this one lets go of
/// the server's copy and is made there anew with the next run. The id of the server's version here.
pub fn keep_twice(mb: &mut Mailbox, here: Id, there: Option<Message>) -> Option<Id> {
    let m = mb.message_mut(here).ok()?;
    let key = m.server.as_ref().map(|s| s.change_key.clone()).unwrap_or_default();
    m.server = None;
    if let Some(sd) = m.server_draft.as_mut() {
        sd.synced = None;
        sd.pending = true;
        sd.saves = sd.saves.saturating_add(1);
        sd.marker = marker(here, fingerprint(key.as_bytes()) ^ u64::from(sd.saves));
    }
    log::info!("graph drafts: a draft was changed here and elsewhere; both are kept");
    match there {
        Some(there) => mb.add_message(Message { draft: true, ..there }).ok(),
        None => Some(here),
    }
}

#[cfg(test)]
mod tests {
    use mailcraft_model::Mailbox;
    use mailcraft_model::sync::ServerMessage;
    use serde_json::json;

    use crate::Session;

    /// A draft saved again keeps its server id (it was dropped, and the server would have had a
    /// second draft made), its marker and its Message-ID; and it waits to go up again.
    #[test]
    fn a_draft_saved_again_keeps_its_server_id_and_waits_to_go_up() {
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let account = s.run("account.add", &json!({"email": "avery@mail.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        s.run("mail.new", &json!({"account": account, "to": "kit@example.org", "subject": "One"})).unwrap();
        let draft = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
        let first = s.mailbox.message(draft).unwrap().clone();
        let sd = first.server_draft.clone().unwrap();
        assert!(sd.pending && sd.saves == 1 && !sd.marker.is_empty());
        assert_eq!(super::pending(&s.mailbox, account), 1);
        s.mailbox.message_mut(draft).unwrap().server = Some(ServerMessage { id: "AAkA-draft".into(), ..Default::default() });
        s.run("compose.set", &json!({"subject": "Two"})).unwrap();
        s.run("compose.saveDraft", &json!({})).unwrap();
        let again = s.mailbox.message(draft).unwrap();
        assert_eq!(again.server.as_ref().unwrap().id, "AAkA-draft");
        assert_eq!(again.message_id, first.message_id);
        let sd2 = again.server_draft.clone().unwrap();
        assert_eq!((sd2.marker.as_str(), sd2.saves, sd2.pending), (sd.marker.as_str(), 2, true));
        let up = super::draft_up(again).unwrap();
        assert_eq!((up.id.as_str(), up.subject.as_deref()), ("AAkA-draft", Some("Two")));
    }
}
