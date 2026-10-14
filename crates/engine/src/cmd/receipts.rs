//! Read receipts that others ask for (RFC 8098 message disposition notifications): when a
//! message that is opened asks for one, the owner's choice in File › Options says whether one is
//! sent, never, or asked about in a bar of the reading pane. A receipt is a short message queued
//! in the Outbox; it says the message was displayed, and nothing else of the owner's.

use mailcraft_model::{AccountKind, Address, Attachment, Body, FolderKind, Id, Message, ReceiptAnswer};
use serde_json::{Value, json};

use super::{fail, needs_message};
use crate::{CmdError, CmdResult, CommandSpec, ReceiptPolicy, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.readReceipt", "Send Read Receipt", "Mail", respond)
            .params(r#"{"send": bool, "id"?: number (default: the message in the reading pane)} — answers the sender's request for a read receipt: true queues one in the Outbox (it says only that the message was displayed), false sends none and asks no more"#)
            .selection(needs_message),
    ]
}

/// Longest original subject a receipt repeats.
const MAX_SUBJECT: usize = 200;

/// Whether `m` asks for a read receipt that the owner can still answer: it names where the
/// receipt goes, nothing was done about it yet, it came from somebody else into a folder of
/// received mail (not Junk Email), and its account has a mail server to send one through.
pub fn asked(s: &Session, m: &Message) -> bool {
    if !m.receipt.is_pending() || m.draft || m.receipt_to().is_none() {
        return false;
    }
    let mine = s.mailbox.accounts.iter().any(|a| !a.email.trim().is_empty() && a.email.trim().eq_ignore_ascii_case(m.from.email.trim()));
    let folder = s.mailbox.folder(m.folder).map(|f| f.kind);
    let received = !matches!(folder, Some(FolderKind::Junk | FolderKind::Sent | FolderKind::Drafts | FolderKind::Outbox) | None);
    let sends = s.mailbox.account(m.account).is_some_and(|a| a.kind == AccountKind::Imap);
    !mine && received && sends
}

/// Whether a receipt may go without asking (RFC 8098 §2.1): the request names one address only,
/// and it is the address the message's `Return-Path` says it came from. Otherwise the owner is
/// asked even when they chose to always send one.
pub fn may_send_unasked(m: &Message) -> bool {
    let Some(to) = m.receipt_to() else { return false };
    let named = m.header(super::compose::READ_RECEIPT).map_or(0, |f| Address::parse_list(f).len());
    let returned = m.header("Return-Path").and_then(|f| Address::parse_list(f).into_iter().next());
    named == 1 && returned.is_some_and(|r| r.same_mailbox(&to))
}

/// Whether the reading pane asks the owner about `m`'s request for a receipt.
pub fn ask_in_reading_pane(s: &Session, m: &Message) -> bool {
    asked(s, m)
        && match s.prefs.read_receipts {
            ReceiptPolicy::Ask => true,
            ReceiptPolicy::Always => !may_send_unasked(m),
            ReceiptPolicy::Never => false,
        }
}

/// A message was opened: with the owner's say-so to always send receipts, one goes now where
/// that is safe. (Called as the selection settles on one message.)
pub(crate) fn on_open(s: &mut Session, id: Id) {
    if s.prefs.read_receipts != ReceiptPolicy::Always {
        return;
    }
    let Some(m) = s.mailbox.message(id) else { return };
    if !asked(s, m) || !may_send_unasked(m) {
        return;
    }
    match queue(s, id, false) {
        // (Opening a message is no undo step: what it does is saved all the same.)
        Ok(_) => s.dirty = true,
        Err(e) => log::warn!("a read receipt couldn't be queued: {e}"),
    }
}

/// One line for a field of the receipt: no line breaks, bounded.
fn line(text: &str, max: usize) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect::<String>().trim().to_string()
}

/// Build the receipt for `id` and queue it in the Outbox of the message's account; mark the
/// message as answered. `manual`: the owner pressed Send (else it went by their standing choice).
fn queue(s: &mut Session, id: Id, manual: bool) -> Result<Id, CmdError> {
    let m = s.mailbox.message(id).cloned().ok_or_else(|| fail(format!("no message with id {id}")))?;
    let to = m.receipt_to().ok_or_else(|| fail("the message asks for no read receipt"))?;
    let owner = s.mailbox.account(m.account).cloned().ok_or_else(|| fail("the message's account is gone"))?;
    if owner.kind != AccountKind::Imap {
        return Err(fail("this account has no mail server to send a receipt through"));
    }
    let outbox = s.mailbox.folder_of_kind(owner.id, FolderKind::Outbox).ok_or_else(|| fail("this account has no Outbox folder"))?;
    let now = s.now();
    let me = owner.address();
    let subject = line(&m.subject, MAX_SUBJECT);
    let read_at = s.local(now);
    let sent_at = s.local(m.date);
    let twelve = s.prefs.twelve_hour;
    // For people: what was read, and when.
    let text = format!(
        "This is a receipt for the message you sent to {}.\n\nSubject: {subject}\nSent: {} {}\n\nIt was displayed on {} at {}. That doesn't mean it was read or understood.\n",
        line(&me.email, 320),
        sent_at.date.long(),
        sent_at.time_short(twelve),
        read_at.date.long(),
        read_at.time_short(twelve),
    );
    // For programs (RFC 8098 §3.2): who reports, for whom, about which message, and what happened.
    let original_id: String = m.message_id.chars().filter(|c| !c.is_control() && !c.is_whitespace() && !matches!(c, '<' | '>')).take(400).collect();
    let mode = if manual { "manual-action/MDN-sent-manually" } else { "automatic-action/MDN-sent-automatically" };
    let mut fields = format!("Reporting-UA: MailCraft; MailCraft {}\r\n", env!("CARGO_PKG_VERSION"));
    fields.push_str(&format!("Final-Recipient: rfc822;{}\r\n", line(&me.email, 320)));
    if !original_id.is_empty() {
        fields.push_str(&format!("Original-Message-ID: <{original_id}>\r\n"));
    }
    fields.push_str(&format!("Disposition: {mode}; displayed\r\n"));
    // Only plain ASCII travels in the notification: anything else is left out.
    let fields: String = fields.chars().filter(|c| c.is_ascii() && (!c.is_ascii_control() || matches!(c, '\r' | '\n'))).collect();
    let domain = owner.email.rsplit_once('@').map_or("", |(_, d)| d);
    let receipt = Message {
        account: owner.id,
        folder: outbox,
        message_id: mailcraft_mime::new_message_id(domain, now, s.rev().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ id.rotate_left(29)),
        in_reply_to: m.message_id.clone(),
        references: Some(m.message_id.clone()).filter(|i| !i.is_empty()).into_iter().collect(),
        from: me,
        to: vec![to],
        subject: format!("Read: {subject}"),
        date: now,
        received: now,
        body: Body { text, html: None },
        attachments: vec![Attachment::new("", "message/disposition-notification", fields.into_bytes())],
        read: true,
        ..Default::default()
    };
    let rid = s.mailbox.add_message(receipt)?;
    s.mailbox.message_mut(id)?.receipt = ReceiptAnswer::Sent;
    Ok(rid)
}

fn respond(s: &mut Session, v: &Value) -> CmdResult {
    let send = p::bool(v, "send").ok_or_else(|| super::bad("`send` (true or false) is required"))?;
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let m = s.mailbox.message(id).ok_or_else(|| fail(format!("no message with id {id}")))?;
    if !asked(s, m) {
        return Err(fail("the message asks for no read receipt that can be answered"));
    }
    if send {
        let receipt = queue(s, id, true)?;
        Ok(json!({"sent": true, "receipt": receipt}))
    } else {
        s.mailbox.message_mut(id)?.receipt = ReceiptAnswer::Declined;
        Ok(json!({"sent": false}))
    }
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{Account, AccountKind, Address, FolderKind, Mailbox, Message, ReceiptAnswer, Security, Server};
    use serde_json::json;

    use crate::{ReceiptPolicy, Session};

    fn session() -> (Session, u64, u64) {
        let mut mb = Mailbox::default();
        let account = mb.add_account(Account {
            name: "me@example.org".into(),
            email: "me@example.org".into(),
            display_name: "Me".into(),
            kind: AccountKind::Imap,
            incoming: Some(Server { host: "imap.example.org".into(), port: 993, security: Security::Tls }),
            outgoing: Some(Server { host: "smtp.example.org".into(), port: 465, security: Security::Tls }),
            ..Default::default()
        });
        let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let id = mb
            .add_message(Message {
                account,
                folder: inbox,
                message_id: "plans@example.org".into(),
                from: Address::new("Ada", "ada@example.org"),
                to: vec![Address::new("Me", "me@example.org")],
                subject: "Plans\r\nBcc: x@example.org".into(),
                date: 1_791_990_000,
                headers: vec![("Disposition-Notification-To".into(), "Ada <ada@example.org>".into())],
                ..Default::default()
            })
            .unwrap();
        let s = Session::at(mb, 1_791_993_600, 0);
        (s, account, id)
    }

    fn outbox(s: &Session, account: u64) -> Vec<Message> {
        let f = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
        s.mailbox.messages_in(f).map(|m| m.as_ref().clone()).collect()
    }

    #[test]
    fn asking_shows_the_bar_and_sending_queues_an_mdn() {
        let (mut s, account, id) = session();
        assert_eq!(s.prefs.read_receipts, ReceiptPolicy::Ask);
        s.run("mail.select", &json!({"id": id})).unwrap();
        assert!(outbox(&s, account).is_empty(), "nothing goes without asking");
        assert!(super::ask_in_reading_pane(&s, s.mailbox.message(id).unwrap()));
        s.run("mail.readReceipt", &json!({"send": true})).unwrap();
        let sent = outbox(&s, account);
        assert_eq!(sent.len(), 1);
        let r = &sent[0];
        assert_eq!(r.to[0].email, "ada@example.org");
        assert_eq!(r.subject, "Read: Plans  Bcc: x@example.org");
        assert_eq!(r.in_reply_to, "plans@example.org");
        let bytes = mailcraft_mime::write(r, &mailcraft_mime::WriteOptions::default());
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("multipart/report; report-type=disposition-notification"), "{text}");
        assert!(text.contains("Original-Message-ID: <plans@example.org>\r\n"));
        assert!(text.contains("Disposition: manual-action/MDN-sent-manually; displayed"));
        assert!(text.contains("Final-Recipient: rfc822;me@example.org"));
        assert!(!text.contains("\nBcc:"), "nothing of the subject becomes a header");
        assert_eq!(s.mailbox.message(id).unwrap().receipt, ReceiptAnswer::Sent);
        assert!(!super::ask_in_reading_pane(&s, s.mailbox.message(id).unwrap()), "asked once");
        assert!(s.run("mail.readReceipt", &json!({"send": true})).is_err(), "and answered once");
    }

    #[test]
    fn declining_never_and_always() {
        let (mut s, account, id) = session();
        s.run("mail.readReceipt", &json!({"id": id, "send": false})).unwrap();
        assert_eq!(s.mailbox.message(id).unwrap().receipt, ReceiptAnswer::Declined);
        assert!(outbox(&s, account).is_empty());

        let (mut s, account, id) = session();
        s.run("file.options", &json!({"readReceipts": "never"})).unwrap();
        s.run("mail.select", &json!({"id": id})).unwrap();
        assert!(outbox(&s, account).is_empty() && !super::ask_in_reading_pane(&s, s.mailbox.message(id).unwrap()));

        // Always: without a Return-Path that agrees, it still asks (RFC 8098 §2.1).
        let (mut s, account, id) = session();
        s.run("file.options", &json!({"readReceipts": "always"})).unwrap();
        s.run("mail.select", &json!({"id": id})).unwrap();
        assert!(outbox(&s, account).is_empty());
        assert!(super::ask_in_reading_pane(&s, s.mailbox.message(id).unwrap()));
        // With one, it goes when the message is opened, and says it went by itself.
        s.run("mail.select", &json!({"ids": []})).unwrap();
        s.mailbox.message_mut(id).unwrap().headers.push(("Return-Path".into(), "<ADA@example.org>".into()));
        s.run("mail.select", &json!({"id": id})).unwrap();
        let sent = outbox(&s, account);
        assert_eq!(sent.len(), 1);
        let fields = String::from_utf8(sent[0].attachments[0].data.to_vec()).unwrap();
        assert!(fields.contains("automatic-action/MDN-sent-automatically"));
        assert!(s.run("file.options", &json!({"readReceipts": "sometimes"})).is_err());
    }

    #[test]
    fn junk_my_own_mail_and_local_accounts_ask_nothing() {
        let (mut s, account, id) = session();
        let junk = s.mailbox.folder_of_kind(account, FolderKind::Junk).unwrap();
        s.mailbox.move_message(id, junk).unwrap();
        assert!(!super::asked(&s, s.mailbox.message(id).unwrap()));
        let (mut s, _, id) = session();
        s.mailbox.message_mut(id).unwrap().from = Address::new("", "ME@example.org");
        assert!(!super::asked(&s, s.mailbox.message(id).unwrap()));
        let (mut s, account, id) = session();
        s.mailbox.account_mut(account).unwrap().kind = AccountKind::Local;
        assert!(!super::asked(&s, s.mailbox.message(id).unwrap()));
        let (mut s, _, id) = session();
        s.mailbox.message_mut(id).unwrap().headers = vec![("Disposition-Notification-To".into(), "not an address".into())];
        assert!(!super::asked(&s, s.mailbox.message(id).unwrap()));
    }
}
