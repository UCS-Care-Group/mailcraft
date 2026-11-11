// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Digitally signed or Encrypted message (#30) deleted for good while a copy of it waits to go
//! onto another account's server (#566): its server keeps it until the bytes it came as are whole
//! on the disk here, and one put back after a refused copy has them. Synthetic messages with
//! dummy signatures only.

use mailcraft_model::sync::{FolderDelta, Placed, RemoteFlags, RemoteOp, ServerMessage, SyncRequest, SyncResult};
use mailcraft_model::{FolderKind, Id, Message, Pending};
use serde_json::json;

use crate::Session;
use crate::cmd::account::{apply_sync, sync_request};

const SIGNED: &str = "From: Boss <boss@example.org>\r\nTo: avery@mail.example\r\nSubject: Signed\r\nMessage-ID: <pm-1@example.org>\r\n\
MIME-Version: 1.0\r\nContent-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"b\"\r\n\r\n\
--b\r\nContent-Type: text/plain\r\n\r\nPay the invoice.\r\n--b\r\nContent-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\n\
Content-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"smime.p7s\"\r\n\r\nZHVtbXk=\r\n--b--\r\n";

/// Long enough that its original and its text are kept outside the mailbox file.
fn long_signed() -> Vec<u8> {
    SIGNED.replace("Pay the invoice.", &"Pay the invoice. ".repeat(100)).into_bytes()
}

/// [`long_signed`] unprotected, as a server's copy made of its parts is.
fn long_unprotected() -> Vec<u8> {
    format!("From: Boss <boss@example.org>\r\nSubject: Signed\r\nMessage-ID: <pm-1@example.org>\r\n\r\n{}\r\n", "Pay the invoice. ".repeat(100))
        .into_bytes()
}

/// An IMAP account whose Inbox follows "INBOX", with `new` in it.
fn imap_account(s: &mut Session, email: &str, new: Vec<mailcraft_model::Message>) -> Id {
    let servers = json!({"email": email, "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let a = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let uid_seen = new.iter().map(|m| m.uid).max().unwrap_or(0);
    let inbox = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen, new, ..Default::default() };
    apply_sync(s, SyncResult { account: a, folders: vec![inbox], ..Default::default() }).unwrap();
    a
}

/// Two IMAP accounts, the first with [`long_signed`] as UID 1 of its Inbox: the session, the two
/// accounts and the message.
fn two_imap() -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let read = mailcraft_model::Message { uid: 1, ..mailcraft_mime::parse_fetched(&long_signed()) };
    let from = imap_account(&mut s, "avery@mail.example", vec![read]);
    let to = imap_account(&mut s, "work@mail.example", vec![]);
    let id = s.mailbox.messages.values().find(|m| m.account == from).unwrap().id;
    (s, from, to, id)
}

/// A Microsoft account whose server lists an Inbox ("in").
fn microsoft(s: &mut Session, email: &str) -> Id {
    use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult};
    let account = s.run("account.add", &json!({"email": email, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let listing = GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(s, listing).unwrap();
    account
}

/// A folder of its own for test `name`'s mailbox file.
fn folder(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-held-delete-originals-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Saved at `path`, as File › Save does.
fn save(s: &mut Session, path: &std::path::Path) {
    crate::store::save(path, &s.mailbox).unwrap();
    s.path = Some(path.to_path_buf());
    s.dirty = false;
    s.note_saved();
}

/// Saved at `path` and opened from there: every text, file and original kept outside the file.
fn reopened(s: &Session, path: &std::path::Path) -> Session {
    crate::store::save(path, &s.mailbox).unwrap();
    crate::store::clear_cache();
    let mut s = Session::new(crate::store::open(path).unwrap().mailbox);
    s.path = Some(path.to_path_buf());
    s
}

/// [`reopened`], with the file that keeps message `id`'s original gone.
fn reopened_without_original(s: &Session, id: Id, path: &std::path::Path) -> Session {
    let s = reopened(s, path);
    let kept = s.mailbox.message(id).unwrap().original.as_ref().and_then(|o| o.kept).unwrap();
    let digest: String = kept.sha256.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::remove_file(crate::store::blob_dir(path).join(&digest[..2]).join(&digest)).unwrap();
    crate::store::clear_cache();
    s
}

/// What the IMAP account `account`'s server gives of UID `uid` of its Inbox, downloaded again.
fn refetched(s: &mut Session, account: Id, uid: u32, bytes: &[u8]) {
    let read = mailcraft_model::Message { uid, ..mailcraft_mime::parse_fetched(bytes) };
    let inbox =
        FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: uid, refetched: vec![read], ..Default::default() };
    apply_sync(s, SyncResult { account, folders: vec![inbox], ..Default::default() }).unwrap();
}

/// The IMAP server took `message` as `asked` gave it, as UID `uid`.
fn put(asked: &SyncRequest, message: Id, uid: u32) -> Placed {
    let place = asked.places.iter().find(|p| p.message.id == message).unwrap();
    let flags = RemoteFlags { draft: false, ..RemoteFlags::of(&place.message, 0) };
    Placed { message, to_folder: place.to_folder, uid, uid_validity: 7, flags, original: mailcraft_mime::writes_original(&place.message) }
}

/// Whether a request of `account`'s asks its server to delete anything.
fn deletes(s: &Session, account: Id) -> bool {
    sync_request(s, account).unwrap().ops.iter().any(|op| matches!(op, RemoteOp::Delete { .. }))
}

/// Whether a run of the Microsoft account `ms` asks its server to delete or move out anything.
fn graph_deletes(s: &Session, ms: Id) -> bool {
    use mailcraft_model::sync::MessageChange;
    let asked = crate::cmd::graph_mail::graph_mail_request(s, ms).unwrap();
    asked.messages.iter().any(|c| matches!(c, MessageChange::Delete { .. } | MessageChange::MovedOut { .. }))
}

/// A Microsoft account with [`long_signed`] in its Inbox as "AAkA7", and an IMAP account, saved
/// at `path` and opened with that message's original gone: the session, the two accounts and
/// the message.
fn microsoft_and_imap(path: &std::path::Path) -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let ms = microsoft(&mut s, "owner@example.org");
    let to = imap_account(&mut s, "work@mail.example", vec![]);
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let server = ServerMessage { id: "AAkA7".into(), folder: "in".into(), ..Default::default() };
    let read = mailcraft_mime::parse_fetched(&long_signed());
    let id = s.mailbox.add_message(Message { folder: inbox, server: Some(server), ..read }).unwrap();
    (reopened_without_original(&s, id, path), ms, to, id)
}

#[test]
fn a_message_put_back_after_a_refused_copy_has_the_bytes_it_came_as_also_once_the_file_was_opened_again() {
    for again in [false, true] {
        let dir = folder(&format!("put-back-{again}"));
        let path = dir.join("mailbox.mailcraft");
        let (s, from, to, id) = two_imap();
        let mut s = reopened(&s, &path);
        let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
        let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
        s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
        if again {
            s = reopened(&s, &path);
        }
        apply_sync(&mut s, SyncResult { account: to, unplaced: vec![copy], ..Default::default() }).unwrap();
        let m = s.mailbox.message(id).expect("put back");
        assert_eq!(m.account, from);
        assert!(m.has_original(), "{again}: {:?}", m.original);
        assert_eq!(s.loaded_message(id).unwrap().original_bytes(), Some(long_signed().as_slice()), "{again}");
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn a_message_put_back_on_a_microsoft_account_after_a_refused_copy_has_the_bytes_it_came_as() {
    let dir = folder("graph");
    let path = dir.join("mailbox.mailcraft");
    let mut s = Session::empty();
    let ms = microsoft(&mut s, "owner@example.org");
    let to = imap_account(&mut s, "work@mail.example", vec![]);
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let server = mailcraft_model::sync::ServerMessage { id: "AAkA7".into(), folder: "in".into(), ..Default::default() };
    let read = mailcraft_mime::parse_fetched(&long_signed());
    let id = s.mailbox.add_message(mailcraft_model::Message { folder: inbox, server: Some(server), ..read }).unwrap();
    let mut s = reopened(&s, &path);
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    apply_sync(&mut s, SyncResult { account: to, unplaced: vec![copy], ..Default::default() }).unwrap();
    assert!(s.mailbox.message(copy).is_none());
    let m = s.mailbox.message(id).expect("put back");
    assert_eq!(m.account, ms);
    assert!(m.has_original(), "{:?}", m.original);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_delete_waiting_for_a_copy_doesnt_write_the_bytes_the_message_came_as_into_the_mailbox_file() {
    let (mut s, _, to, id) = two_imap();
    let bytes = s.mailbox.message(id).unwrap().original_bytes().unwrap().len();
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(s.mailbox.pending.iter().any(|p| matches!(p, Pending::Delete { kept: Some(_), .. })));
    let written = serde_json::to_string(&s.mailbox.pending).unwrap().len();
    assert!(written < bytes, "{written} bytes written for an original of {bytes}");
}

/// The rule's copy waits for the bytes, which are asked for from the server that still has the
/// message, and goes as it came once they are here and saved; only then is that server's copy
/// deleted.
#[test]
fn a_rule_that_copies_a_signed_message_and_deletes_it_for_good_waits_for_the_bytes_from_its_server() {
    let dir = folder("rule");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let inbox = s.mailbox.folder_of_kind(from, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let rule = json!({"name": "Hand on", "account": from, "conditions": [{"type": "subjectContains", "value": "Signed"}],
        "actions": [{"type": "copyTo", "value": theirs}, {"type": "deleteForGood"}]});
    s.run("rules.add", &rule).unwrap();
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    assert!(s.mailbox.message(id).is_none(), "deleted for good here");
    let copy = s.mailbox.messages.values().find(|m| m.account == to).unwrap().id;
    assert!(sync_request(&s, to).unwrap().places.is_empty(), "made of its parts, it would be taken for the message");
    let said = crate::cmd::graph_place::unreadable_waiting(&s, to);
    assert!(matches!(said.as_slice(), [one] if one.contains("waits to go")), "{said:?}");
    s.ask_for_lost_originals();
    let m = s.mailbox.message(copy).unwrap();
    assert!(m.original_wanted && m.refetch, "asked for from the server that has the message");
    assert_eq!(crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).get(&inbox).map(Vec::as_slice), Some(&[1u32][..]));
    assert!(!deletes(&s, from));

    refetched(&mut s, from, 1, &long_signed());
    assert!(s.mailbox.message(copy).unwrap().has_original());
    // In memory alone, which a failed save or a crash would lose: not yet.
    assert!(sync_request(&s, to).unwrap().places.is_empty());
    assert!(!deletes(&s, from));
    save(&mut s, &path);
    let asked = sync_request(&s, to).unwrap();
    let [place] = asked.places.as_slice() else { panic!("{:?}", asked.places) };
    assert_eq!(place.message.original_bytes(), Some(long_signed().as_slice()));
    assert!(!deletes(&s, from), "not before the copy is there");
    apply_sync(&mut s, SyncResult { account: to, placed: vec![put(&asked, copy, 60)], ..Default::default() }).unwrap();
    assert!(deletes(&s, from));
    let _ = std::fs::remove_dir_all(dir);
}

/// The owner's Copy and then Delete Permanently alike. When the server's copy brings no bytes
/// either (one put there from its parts), nothing can be lost with it: the copy goes made of its
/// parts, and the server's copy is deleted.
#[test]
fn copying_a_signed_message_and_deleting_it_for_good_keeps_it_on_its_server_until_the_copy_has_its_bytes() {
    let dir = folder("owner");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(sync_request(&s, to).unwrap().places.is_empty());
    let _ = crate::cmd::graph_place::unreadable_waiting(&s, to);
    s.ask_for_lost_originals();
    assert!(!deletes(&s, from));

    refetched(&mut s, from, 1, &long_unprotected());
    assert!(s.mailbox.message(copy).unwrap().original_asked);
    let asked = sync_request(&s, to).unwrap();
    let [place] = asked.places.as_slice() else { panic!("{:?}", asked.places) };
    assert!(place.message.original_bytes().is_none(), "made of its parts");
    apply_sync(&mut s, SyncResult { account: to, placed: vec![put(&asked, copy, 60)], ..Default::default() }).unwrap();
    assert!(deletes(&s, from));
    let _ = std::fs::remove_dir_all(dir);
}

/// Into a Microsoft account, where a copy is always made of its parts: the bytes stay on this
/// computer only, so the copy goes once they are saved here.
#[test]
fn a_copy_into_microsoft_of_a_signed_message_deleted_for_good_goes_once_its_bytes_are_saved_here() {
    let dir = folder("to-ms");
    let path = dir.join("mailbox.mailcraft");
    let (mut s, from, _, id) = two_imap();
    let ms = microsoft(&mut s, "owner@example.org");
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(crate::cmd::graph_mail::graph_mail_request(&s, ms).unwrap().places.is_empty(), "in memory alone");
    assert!(!deletes(&s, from));
    save(&mut s, &path);
    assert_eq!(crate::cmd::graph_mail::graph_mail_request(&s, ms).unwrap().places.len(), 1);
    assert!(!deletes(&s, from), "not before the copy is there");
    let _ = std::fs::remove_dir_all(dir);
}

/// Out of a Microsoft account whose server has the message, the bytes it came as are fetched
/// from there for the copy, which goes as it came once they are saved here; only then is the
/// server's copy deleted.
#[test]
fn a_copy_of_a_microsoft_message_whose_bytes_cant_be_read_here_has_them_fetched_from_its_server() {
    use mailcraft_model::sync::{GraphFetchResult, GraphFetched};
    let dir = folder("from-ms");
    let path = dir.join("mailbox.mailcraft");
    let (mut s, ms, to, id) = microsoft_and_imap(&path);
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(sync_request(&s, to).unwrap().places.is_empty(), "waits");
    let _ = crate::cmd::graph_place::unreadable_waiting(&s, to);
    s.ask_for_lost_originals();
    assert!(!graph_deletes(&s, ms));
    let asked = crate::cmd::graph_mail::graph_fetch_request(&s, ms, None, 10).unwrap();
    assert_eq!(asked.ids, ["AAkA7"]);

    let fetched = vec![GraphFetched { id: "AAkA7".into(), mime: long_signed() }];
    let result = GraphFetchResult { account: ms, email: "owner@example.org".into(), fetched, ..Default::default() };
    crate::cmd::graph_mail::apply_graph_fetch(&mut s, result).unwrap();
    assert!(s.mailbox.message(copy).unwrap().has_original());
    assert!(crate::cmd::graph_down::fetch_queue(&s.mailbox, ms, 10, s.now()).is_empty(), "asked once");
    assert!(sync_request(&s, to).unwrap().places.is_empty(), "in memory alone");
    save(&mut s, &path);
    let asked = sync_request(&s, to).unwrap();
    let [place] = asked.places.as_slice() else { panic!("{:?}", asked.places) };
    assert_eq!(place.message.original_bytes(), Some(long_signed().as_slice()));
    assert!(!graph_deletes(&s, ms), "not before the copy is there");
    apply_sync(&mut s, SyncResult { account: to, placed: vec![put(&asked, copy, 60)], ..Default::default() }).unwrap();
    assert!(graph_deletes(&s, ms));
    let _ = std::fs::remove_dir_all(dir);
}

/// Whether moving `copy` into `folder` is refused as one still on its way to its server.
fn refused_on_its_way(s: &mut Session, copy: Id, folder: Id) -> bool {
    s.run("mail.move", &json!({"ids": [copy], "folder": folder})).is_err_and(|e| e.to_string().contains("still on its way"))
}

/// The copy, still on its way to the other server, isn't moved onto this computer, where nothing
/// would hold the delete of the server's copy: it goes there once it is on its own server.
#[test]
fn a_waiting_copy_of_a_signed_message_deleted_for_good_goes_onto_this_computer_once_it_is_on_its_server() {
    let dir = folder("local");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(refused_on_its_way(&mut s, copy, here));
    assert_eq!(s.mailbox.message(copy).unwrap().folder, theirs);
    let _ = crate::cmd::graph_place::unreadable_waiting(&s, to);
    s.ask_for_lost_originals();
    assert!(!deletes(&s, from));
    refetched(&mut s, from, 1, &long_signed());
    save(&mut s, &path);
    let asked = sync_request(&s, to).unwrap();
    apply_sync(&mut s, SyncResult { account: to, placed: vec![put(&asked, copy, 60)], ..Default::default() }).unwrap();
    assert!(deletes(&s, from));
    s.run("mail.move", &json!({"ids": [copy], "folder": here})).unwrap();
    assert_eq!(s.mailbox.message(copy).unwrap().account, local);
    let _ = std::fs::remove_dir_all(dir);
}

/// Out of a Microsoft account alike.
#[test]
fn a_waiting_copy_of_a_signed_microsoft_message_deleted_for_good_isnt_moved_onto_this_computer() {
    let dir = folder("ms-local");
    let path = dir.join("mailbox.mailcraft");
    let (mut s, ms, to, id) = microsoft_and_imap(&path);
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(refused_on_its_way(&mut s, copy, here));
    save(&mut s, &path);
    assert!(!graph_deletes(&s, ms));
    let _ = std::fs::remove_dir_all(dir);
}

/// Two copies: one isn't moved onto this computer while the other waits too, and the other
/// deleted for good leaves the first to hold the server's copy.
#[test]
fn of_two_waiting_copies_one_isnt_moved_onto_this_computer_and_holds_the_delete_once_the_other_is_deleted() {
    let dir = folder("two-then-delete");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let third = imap_account(&mut s, "home@mail.example", vec![]);
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let (b, c) = (s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(third, FolderKind::Inbox).unwrap());
    let copy_b = s.run("mail.copy", &json!({"ids": [id], "folder": b})).unwrap()["copies"][0].as_u64().unwrap();
    let copy_c = s.run("mail.copy", &json!({"ids": [id], "folder": c})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(refused_on_its_way(&mut s, copy_b, here));
    s.run("mail.deletePermanently", &json!({"ids": [copy_c]})).unwrap();
    save(&mut s, &path);
    assert!(!deletes(&s, from), "copy B still waits");
    let _ = std::fs::remove_dir_all(dir);
}

/// Nor into a folder of an Exchange server account's, which no server is given either.
#[test]
fn a_waiting_copy_of_a_signed_message_deleted_for_good_isnt_moved_into_an_exchange_accounts_folder() {
    let dir = folder("exchange");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let ews = json!({"email": "ada@example.org", "kind": "exchange", "ews": {"url": "https://mail.example.org/EWS/Exchange.asmx", "username": "CORP\\ada"}});
    let exchange = s.run("account.add", &ews).unwrap()["id"].as_u64().unwrap();
    let there = match s.mailbox.folder_of_kind(exchange, FolderKind::Inbox) {
        Some(f) => f,
        None => s.mailbox.add_folder(exchange, None, "Kept").unwrap(),
    };
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(refused_on_its_way(&mut s, copy, there));
    save(&mut s, &path);
    assert!(!deletes(&s, from));
    let _ = std::fs::remove_dir_all(dir);
}

/// A copy into a Microsoft account alike; and Undo after the refused move undoes the delete for
/// good, which lets nothing go.
#[test]
fn a_waiting_copy_into_microsoft_isnt_moved_onto_this_computer_and_undo_lets_nothing_go() {
    let dir = folder("undo-ms");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, _, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let ms = microsoft(&mut s, "owner@example.org");
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(refused_on_its_way(&mut s, copy, here));
    save(&mut s, &path);
    assert!(!deletes(&s, from));
    assert!(s.undo());
    save(&mut s, &path);
    assert!(s.mailbox.message(id).is_some(), "the delete for good undone");
    assert!(!deletes(&s, from));
    let _ = std::fs::remove_dir_all(dir);
}

/// A copy into a Microsoft account moved on into an IMAP account, and that move undone: it stays
/// where it was moved, as one moved out of a Microsoft account does, and still waits to be made
/// on that account's server, so the delete still waits for it; Redo and Undo again alike.
#[test]
fn undoing_a_move_of_a_waiting_copy_out_of_microsoft_keeps_it_waiting_and_the_delete_held() {
    let dir = folder("undo-out-of-ms");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, _, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let ms = microsoft(&mut s, "owner@example.org");
    let third = imap_account(&mut s, "home@mail.example", vec![]);
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let c = s.mailbox.folder_of_kind(third, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    s.run("mail.move", &json!({"ids": [copy], "folder": c})).unwrap();
    let waits = |s: &Session| {
        s.mailbox
            .queued
            .iter()
            .filter(|q| matches!(q, mailcraft_model::Queued::PlaceMessage { account, message, .. } if *message == copy && *account == third))
            .count()
    };
    for step in ["undo", "redo", "undo again"] {
        assert!(if step == "redo" { s.redo() } else { s.undo() }, "{step}");
        save(&mut s, &path);
        assert_eq!(s.mailbox.message(copy).map(|m| m.account), Some(third), "{step}");
        assert_eq!(waits(&s), 1, "{step}");
        assert!(!deletes(&s, from), "{step}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A copy on its way into a Microsoft account deleted for good, and that undone: it is back, and
/// waits again to be made on that server, so the delete of the message it was copied from waits
/// for it again; Redo and Undo again alike.
#[test]
fn undoing_a_delete_for_good_of_a_copy_waiting_into_microsoft_keeps_the_delete_held() {
    let dir = folder("undo-delete-copy");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, _, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let ms = microsoft(&mut s, "owner@example.org");
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [copy]})).unwrap();
    let waits = |s: &Session| {
        s.mailbox
            .queued
            .iter()
            .filter(|q| matches!(q, mailcraft_model::Queued::PlaceMessage { account, message, .. } if *message == copy && *account == ms))
            .count()
    };
    for (step, back) in [("undo", true), ("redo", false), ("undo again", true)] {
        assert!(if back { s.undo() } else { s.redo() }, "{step}");
        save(&mut s, &path);
        assert_eq!(s.mailbox.message(copy).map(|m| m.account), back.then_some(ms), "{step}");
        assert_eq!(waits(&s), usize::from(back), "{step}");
        assert_eq!(deletes(&s, from), !back, "{step}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Copies into two accounts: each is asked for the bytes from the server that has the message,
/// one after the other, and its delete goes once both are there.
#[test]
fn two_copies_of_a_signed_message_deleted_for_good_both_get_its_bytes() {
    let dir = folder("two");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let third = imap_account(&mut s, "home@mail.example", vec![]);
    let (b, c) = (s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(third, FolderKind::Inbox).unwrap());
    let copy_b = s.run("mail.copy", &json!({"ids": [id], "folder": b})).unwrap()["copies"][0].as_u64().unwrap();
    let copy_c = s.run("mail.copy", &json!({"ids": [id], "folder": c})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    let _ = crate::cmd::graph_place::unreadable_waiting(&s, to);
    let _ = crate::cmd::graph_place::unreadable_waiting(&s, third);
    s.ask_for_lost_originals();
    let inbox = s.mailbox.folder_of_kind(from, FolderKind::Inbox).unwrap();
    let asked = |s: &Session| crate::cmd::imap_again::moved_out(&s.mailbox, from).get(&(inbox, 1)).copied();
    assert_eq!(asked(&s), Some(copy_b));
    refetched(&mut s, from, 1, &long_signed());
    assert_eq!(asked(&s), Some(copy_c));
    refetched(&mut s, from, 1, &long_signed());
    assert_eq!(asked(&s), None);
    save(&mut s, &path);
    let (asked_b, asked_c) = (sync_request(&s, to).unwrap(), sync_request(&s, third).unwrap());
    assert_eq!((asked_b.places.len(), asked_c.places.len()), (1, 1));
    apply_sync(&mut s, SyncResult { account: to, placed: vec![put(&asked_b, copy_b, 60)], ..Default::default() }).unwrap();
    assert!(!deletes(&s, from), "the other copy still waits");
    apply_sync(&mut s, SyncResult { account: third, placed: vec![put(&asked_c, copy_c, 61)], ..Default::default() }).unwrap();
    assert!(deletes(&s, from));
    let _ = std::fs::remove_dir_all(dir);
}

/// At the mailbox's cap on the bytes protected messages came as, a copy is made without them:
/// deleted for good, the message gives them to the copy, which doesn't go made of its parts.
#[test]
fn at_the_cap_a_copy_of_a_signed_message_deleted_for_good_gets_the_bytes_it_came_as() {
    let (mut s, from, to, id) = two_imap();
    let used = s.mailbox.messages.originals_size();
    let inbox = s.mailbox.folder_of_kind(from, FolderKind::Inbox).unwrap();
    let rest = mailcraft_model::ContentRef { sha256: [7; 32], len: mailcraft_model::MAX_ORIGINALS - used };
    let original = Some(mailcraft_model::Original { data: std::sync::Arc::default(), kept: Some(rest) });
    s.mailbox.add_message(Message { folder: inbox, subject: "Other".into(), original, ..Default::default() }).unwrap();
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    assert!(s.mailbox.message(copy).unwrap().original.is_none(), "no room");
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert_eq!(s.mailbox.message(copy).unwrap().original_bytes(), Some(long_signed().as_slice()));
    let asked = sync_request(&s, to).unwrap();
    assert!(asked.places.iter().all(|p| p.message.original_bytes().is_some()), "not made of its parts");
    assert!(!deletes(&s, from));
}

/// The copy moved back into the account the message was deleted from, before it is there: it
/// waits for the bytes from that server alike, and goes as it came.
#[test]
fn a_waiting_copy_moved_back_into_its_messages_account_waits_for_the_bytes_alike() {
    let dir = folder("back");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let (mine, theirs) = (s.mailbox.folder_of_kind(from, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap());
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    s.run("mail.move", &json!({"ids": [copy], "folder": mine})).unwrap();
    save(&mut s, &path);
    assert!(sync_request(&s, from).unwrap().places.is_empty(), "made of its parts, it would be taken for the message");
    s.ask_for_lost_originals();
    assert_eq!(crate::cmd::imap_again::wanted(&s.mailbox, from, s.now()).get(&mine).map(Vec::as_slice), Some(&[1u32][..]));
    refetched(&mut s, from, 1, &long_signed());
    save(&mut s, &path);
    let asked = sync_request(&s, from).unwrap();
    let [place] = asked.places.as_slice() else { panic!("{:?}", asked.places) };
    assert_eq!(place.message.original_bytes(), Some(long_signed().as_slice()));
    assert!(!asked.ops.iter().any(|op| matches!(op, RemoteOp::Delete { .. })));
    apply_sync(&mut s, SyncResult { account: from, placed: vec![put(&asked, copy, 60)], ..Default::default() }).unwrap();
    assert!(deletes(&s, from));
    let _ = std::fs::remove_dir_all(dir);
}

/// The server refuses the bytes: the copy waits, and Check again asks once more.
#[test]
fn a_refused_ask_for_a_waiting_copys_bytes_can_be_checked_again() {
    let dir = folder("refused");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    let _ = crate::cmd::graph_place::unreadable_waiting(&s, to);
    s.ask_for_lost_originals();
    let inbox =
        FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 1, refetch_done: vec![1], ..Default::default() };
    apply_sync(&mut s, SyncResult { account: from, folders: vec![inbox], ..Default::default() }).unwrap();
    let m = s.mailbox.message(copy).unwrap();
    assert_ne!(m.original_retry, 0);
    assert!(crate::cmd::mail::check_again::why_not(&s.mailbox, m).is_none());
    s.run("mail.checkAgain", &json!({"id": copy})).unwrap();
    assert!(!deletes(&s, from));
    let _ = std::fs::remove_dir_all(dir);
}

/// A message that isn't protected: its copy goes at once, as before.
#[test]
fn a_copy_of_an_unprotected_message_deleted_for_good_goes_at_once() {
    let mut s = Session::empty();
    let read = Message { uid: 1, ..mailcraft_mime::parse_fetched(&long_unprotected()) };
    let from = imap_account(&mut s, "avery@mail.example", vec![read]);
    let to = imap_account(&mut s, "work@mail.example", vec![]);
    let id = s.mailbox.messages.values().find(|m| m.account == from).unwrap().id;
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert_eq!(sync_request(&s, to).unwrap().places.len(), 1);
}

/// A page of the Microsoft account `ms`'s delta saying the message `id` left `folder`.
fn removed_from(s: &mut Session, ms: Id, folder: &str, id: &str) {
    use mailcraft_model::sync::{GraphMailResult, MessagePage};
    let pages = vec![MessagePage { folder: folder.into(), removed: vec![id.into()], ..Default::default() }];
    let result = GraphMailResult { account: ms, email: "owner@example.org".into(), pages, ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(s, result).unwrap();
}

/// [`microsoft_and_imap`], its message copied into the IMAP account and deleted for good, the
/// copy asking the Microsoft server for the bytes it came as: the session, the two accounts and
/// the copy.
fn copy_asking_microsoft(path: &std::path::Path) -> (Session, Id, Id, Id) {
    let (mut s, ms, to, id) = microsoft_and_imap(path);
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    let _ = crate::cmd::graph_place::unreadable_waiting(&s, to);
    s.ask_for_lost_originals();
    (s, ms, to, copy)
}

/// The message the copy came from is deleted on a phone: the delta says it left the Inbox, and
/// the fetch for its bytes answers that the server has it no more. The copy waits no more: it
/// goes made of its parts, nothing is left to delete, and it may be moved onto this computer.
#[test]
fn a_waiting_copy_goes_made_of_its_parts_once_its_microsoft_original_is_gone_from_the_server() {
    use mailcraft_model::sync::GraphFetchResult;
    let dir = folder("ms-gone");
    let path = dir.join("mailbox.mailcraft");
    let (mut s, ms, to, copy) = copy_asking_microsoft(&path);
    removed_from(&mut s, ms, "in", "AAkA7");
    save(&mut s, &path);
    assert!(sync_request(&s, to).unwrap().places.is_empty(), "a page alone settles nothing");
    assert_eq!(crate::cmd::graph_mail::graph_fetch_request(&s, ms, None, 10).unwrap().ids, ["AAkA7"]);

    let result = GraphFetchResult { account: ms, email: "owner@example.org".into(), missing: vec!["AAkA7".into()], ..Default::default() };
    crate::cmd::graph_mail::apply_graph_fetch(&mut s, result).unwrap();
    assert!(crate::cmd::graph_down::fetch_queue(&s.mailbox, ms, 10, s.now()).is_empty(), "not asked again");
    assert!(!graph_deletes(&s, ms), "nothing left to delete");
    assert!(!s.mailbox.message(copy).unwrap().original_wanted);
    save(&mut s, &path);
    let asked = sync_request(&s, to).unwrap();
    let [place] = asked.places.as_slice() else { panic!("{:?}", asked.places) };
    assert_eq!((place.message.id, place.message.original_bytes()), (copy, None));
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    assert!(!refused_on_its_way(&mut s, copy, here));
    let _ = std::fs::remove_dir_all(dir);
}

/// Moved on a phone instead, the message leaves the Inbox and is still on the server, under the
/// same id: the copy waits through the page that says it left, and the fetch brings its bytes.
#[test]
fn a_waiting_copy_whose_microsoft_original_left_its_folder_still_gets_its_bytes() {
    use mailcraft_model::sync::{GraphFetchResult, GraphFetched};
    let dir = folder("ms-moved");
    let path = dir.join("mailbox.mailcraft");
    let (mut s, ms, to, copy) = copy_asking_microsoft(&path);
    removed_from(&mut s, ms, "in", "AAkA7");
    let fetched = vec![GraphFetched { id: "AAkA7".into(), mime: long_signed() }];
    let result = GraphFetchResult { account: ms, email: "owner@example.org".into(), fetched, ..Default::default() };
    crate::cmd::graph_mail::apply_graph_fetch(&mut s, result).unwrap();
    assert!(s.mailbox.message(copy).unwrap().has_original());
    save(&mut s, &path);
    let asked = sync_request(&s, to).unwrap();
    let [place] = asked.places.as_slice() else { panic!("{:?}", asked.places) };
    assert_eq!(place.message.original_bytes(), Some(long_signed().as_slice()));
    assert!(!graph_deletes(&s, ms), "not before the copy is there");
    apply_sync(&mut s, SyncResult { account: to, placed: vec![put(&asked, copy, 60)], ..Default::default() }).unwrap();
    assert!(graph_deletes(&s, ms));
    let _ = std::fs::remove_dir_all(dir);
}

/// On an IMAP server alike: once its listing no longer has the message the copy came from,
/// the copy goes made of its parts, and nothing is deleted there.
#[test]
fn a_waiting_copy_goes_made_of_its_parts_once_its_imap_original_is_gone_from_the_server() {
    let dir = folder("imap-gone");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    let _ = crate::cmd::graph_place::unreadable_waiting(&s, to);
    s.ask_for_lost_originals();
    assert!(sync_request(&s, to).unwrap().places.is_empty(), "waits");
    let inbox = FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 1, removed: vec![1], ..Default::default() };
    apply_sync(&mut s, SyncResult { account: from, folders: vec![inbox], ..Default::default() }).unwrap();
    assert!(!deletes(&s, from), "nothing left to delete");
    assert!(!s.mailbox.message(copy).unwrap().original_wanted);
    save(&mut s, &path);
    let asked = sync_request(&s, to).unwrap();
    let [place] = asked.places.as_slice() else { panic!("{:?}", asked.places) };
    assert_eq!((place.message.id, place.message.original_bytes()), (copy, None));
    let _ = std::fs::remove_dir_all(dir);
}

/// An emptied Deleted Items queues a delete, keeping the message, for each message in it: the
/// background's next fetch is worked out in time in proportion to them, not to their square.
#[test]
fn many_deletes_for_good_keep_the_background_fetch_quick() {
    let mut s = Session::empty();
    let ms = microsoft(&mut s, "owner@example.org");
    for i in 0..50_000u64 {
        let kept = Box::new(Message { id: 1_000_000 + i, ..Default::default() });
        s.mailbox.queued.push(mailcraft_model::Queued::DeleteMessage { account: ms, id: format!("D{i}"), kept: Some(kept) });
    }
    let started = std::time::Instant::now();
    assert!(crate::cmd::graph_down::fetch_queue(&s.mailbox, ms, 10, s.now()).is_empty());
    let took = started.elapsed();
    assert!(took.as_millis() < 200, "{took:?}");
}

/// The same deletes waiting: the count of changes the window shows while they wait, which it
/// asks for every second or two, and the list a run sends, come quickly too.
#[test]
fn many_deletes_for_good_keep_the_count_of_changes_quick() {
    let mut s = Session::empty();
    let ms = microsoft(&mut s, "owner@example.org");
    for i in 0..50_000u64 {
        let kept = Box::new(Message { id: 1_000_000 + i, ..Default::default() });
        s.mailbox.queued.push(mailcraft_model::Queued::DeleteMessage { account: ms, id: format!("D{i}"), kept: Some(kept) });
    }
    let started = std::time::Instant::now();
    assert_eq!(crate::cmd::graph_mail::unpushed(&s, ms), 50_000);
    let took = started.elapsed();
    assert!(took.as_millis() < 200, "{took:?}");
}

/// Two copies made at the cap: deleted for good, the message gives the bytes it came as to both,
/// which share them. One of them deleted for good too, the other still has them, and the
/// message's server keeps it meanwhile.
#[test]
fn at_the_cap_two_copies_of_a_signed_message_deleted_for_good_both_get_its_bytes() {
    let (mut s, from, to, id) = two_imap();
    let third = imap_account(&mut s, "home@mail.example", vec![]);
    let used = s.mailbox.messages.originals_size();
    let inbox = s.mailbox.folder_of_kind(from, FolderKind::Inbox).unwrap();
    let rest = mailcraft_model::ContentRef { sha256: [7; 32], len: mailcraft_model::MAX_ORIGINALS - used };
    let original = Some(mailcraft_model::Original { data: std::sync::Arc::default(), kept: Some(rest) });
    s.mailbox.add_message(Message { folder: inbox, subject: "Other".into(), original, ..Default::default() }).unwrap();
    let (b, c) = (s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(third, FolderKind::Inbox).unwrap());
    let copy_b = s.run("mail.copy", &json!({"ids": [id], "folder": b})).unwrap()["copies"][0].as_u64().unwrap();
    let copy_c = s.run("mail.copy", &json!({"ids": [id], "folder": c})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [copy_b]})).unwrap();
    assert_eq!(s.mailbox.message(copy_c).unwrap().original_bytes(), Some(long_signed().as_slice()));
    assert!(!deletes(&s, from), "held for the other copy");
}

/// The bytes the message came as whole on the disk here: its waiting copy may be moved onto this
/// computer, which holds them, and the message then goes from its server once the mailbox file
/// holds the copy.
#[test]
fn a_waiting_copy_whose_bytes_are_whole_here_may_be_moved_onto_this_computer() {
    let dir = folder("readable-local");
    let path = dir.join("mailbox.mailcraft");
    let (s, from, to, id) = two_imap();
    let mut s = reopened(&s, &path);
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert!(!deletes(&s, from));
    s.run("mail.move", &json!({"ids": [copy], "folder": here})).unwrap();
    let m = s.mailbox.message(copy).unwrap();
    assert_eq!(m.account, local);
    assert!(s.original_readable(m, true), "the bytes it came as are here");
    assert!(!deletes(&s, from), "not before the file holds the copy");
    crate::store::save(&path, &s.mailbox).unwrap();
    s.dirty = false;
    assert!(deletes(&s, from));
    let _ = std::fs::remove_dir_all(dir);
}

/// Their bytes not here, the refusal is said in the window's language, as other refusals are.
#[test]
fn a_waiting_copy_refused_onto_this_computer_is_told_in_the_windows_language() {
    let dir = folder("said");
    let path = dir.join("mailbox.mailcraft");
    let (s, _, to, id) = two_imap();
    let mut s = reopened_without_original(&s, id, &path);
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    let refused = s.run("mail.move", &json!({"ids": [copy], "folder": here})).unwrap_err();
    assert!(matches!(&refused, crate::CmdError::Said(_)), "{refused:?}");
    assert!(refused.to_string().starts_with("“Signed” is still on its way"), "{refused}");
    let _ = std::fs::remove_dir_all(dir);
}

/// `n` messages in the Microsoft account `ms`'s Inbox, which its server has as "M0", "M1"...
fn many_on_microsoft(s: &mut Session, ms: Id, n: u64) -> Vec<Id> {
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    (0..n)
        .map(|i| {
            let server = Some(ServerMessage { id: format!("M{i}"), folder: "in".into(), ..Default::default() });
            s.mailbox.add_message(Message { folder: inbox, account: ms, subject: format!("m{i}"), server, ..Default::default() }).unwrap()
        })
        .collect()
}

/// An emptied Deleted Items of tens of thousands, deleted for good at once: each message's
/// delete is queued once, and quickly (it looked through the whole queue for each one).
#[test]
fn many_microsoft_messages_deleted_for_good_go_quickly() {
    let mut s = Session::empty();
    let ms = microsoft(&mut s, "owner@example.org");
    let ids = many_on_microsoft(&mut s, ms, 40_000);
    s.mailbox.queued.push(mailcraft_model::Queued::DeleteMessage { account: ms, id: "M7".into(), kept: None });
    let started = std::time::Instant::now();
    s.run("mail.deletePermanently", &json!({"ids": ids})).unwrap();
    let took = started.elapsed();
    assert!(took.as_millis() < 1_000, "{took:?}");
    let deletes = s.mailbox.queued.iter().filter(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { .. })).count();
    assert_eq!(deletes, 40_000, "M7's was queued already");
}

/// The same on an IMAP account: each message's delete waits in its own place, and quickly.
#[test]
fn many_imap_messages_deleted_for_good_go_quickly() {
    let mut s = Session::empty();
    let read: Vec<Message> = (1..=60_000u32).map(|uid| Message { uid, subject: format!("m{uid}"), ..Default::default() }).collect();
    let a = imap_account(&mut s, "avery@mail.example", read);
    let ids: Vec<Id> = s.mailbox.messages.values().filter(|m| m.account == a).map(|m| m.id).collect();
    let started = std::time::Instant::now();
    s.run("mail.deletePermanently", &json!({"ids": ids})).unwrap();
    let took = started.elapsed();
    assert!(took.as_millis() < 1_000, "{took:?}");
    assert_eq!(s.mailbox.pending.iter().filter(|p| matches!(p, Pending::Delete { .. })).count(), 60_000);
}

/// A message and its copy on its way deleted for good in one go, either way round: nothing is
/// left for the delete of the message on its server to wait for, and it goes.
#[test]
fn a_message_deleted_for_good_with_its_waiting_copy_goes_from_its_server() {
    for copy_first in [false, true] {
        let (mut s, from, to, id) = two_imap();
        let theirs = s.mailbox.folder_of_kind(to, FolderKind::Inbox).unwrap();
        let copy = s.run("mail.copy", &json!({"ids": [id], "folder": theirs})).unwrap()["copies"][0].as_u64().unwrap();
        let ids = if copy_first { [copy, id] } else { [id, copy] };
        s.run("mail.deletePermanently", &json!({"ids": ids})).unwrap();
        assert!(deletes(&s, from), "copy first: {copy_first}");
        assert!(sync_request(&s, to).unwrap().places.is_empty());
    }
}

/// A page of a delta that says tens of thousands of messages deleted for good here left their
/// folder (deleted on a phone too, say) is taken quickly: what each waiting delete keeps is
/// marked in one pass over the queue.
#[test]
fn a_page_leaving_out_many_deletes_for_good_is_taken_quickly() {
    use mailcraft_model::sync::{GraphMailResult, MessagePage};
    let mut s = Session::empty();
    let ms = microsoft(&mut s, "owner@example.org");
    let mut removed = Vec::new();
    for i in 0..40_000u64 {
        let server = Some(ServerMessage { id: format!("D{i}"), folder: "in".into(), ..Default::default() });
        let kept = Box::new(Message { id: 1_000_000 + i, server, ..Default::default() });
        s.mailbox.queued.push(mailcraft_model::Queued::DeleteMessage { account: ms, id: format!("D{i}"), kept: Some(kept) });
        removed.push(format!("D{i}"));
    }
    let pages = vec![MessagePage { folder: "in".into(), removed, ..Default::default() }];
    let result = GraphMailResult { account: ms, email: "owner@example.org".into(), pages, ..Default::default() };
    let started = std::time::Instant::now();
    crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    let took = started.elapsed();
    assert!(took.as_millis() < 1_000, "{took:?}");
    let gone = |q: &mailcraft_model::Queued| match q {
        mailcraft_model::Queued::DeleteMessage { kept: Some(kept), .. } => kept.server.as_ref().is_some_and(|s| s.gone),
        _ => false,
    };
    assert!(s.mailbox.queued.iter().all(gone));
}

/// A run of thousands of pages while tens of thousands of deletes for good wait (an emptied
/// Deleted Items): the waiting deletes are looked through once for the run, not for each page.
#[test]
fn many_pages_while_many_deletes_for_good_wait_are_taken_quickly() {
    use mailcraft_model::sync::{GraphMailResult, MessagePage};
    let mut s = Session::empty();
    let ms = microsoft(&mut s, "owner@example.org");
    for i in 0..40_000u64 {
        let server = Some(ServerMessage { id: format!("D{i}"), folder: "in".into(), ..Default::default() });
        let kept = Box::new(Message { id: 1_000_000 + i, server, ..Default::default() });
        s.mailbox.queued.push(mailcraft_model::Queued::DeleteMessage { account: ms, id: format!("D{i}"), kept: Some(kept) });
    }
    let pages: Vec<MessagePage> = (0..2_000).map(|p| MessagePage { folder: "in".into(), link: format!("L{p}"), ..Default::default() }).collect();
    let result = GraphMailResult { account: ms, email: "owner@example.org".into(), pages, ..Default::default() };
    let started = std::time::Instant::now();
    crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    let took = started.elapsed();
    assert!(took.as_millis() < 1_000, "{took:?}");
}

/// The Microsoft server refused the bytes the copy waits for: it is asked again at the next
/// start, and Check again says they are asked for, not that the copy has them.
#[test]
fn check_again_on_a_copy_whose_bytes_its_microsoft_server_refused_says_they_are_asked_for() {
    use mailcraft_model::sync::GraphFetchResult;
    let dir = folder("ms-refused");
    let path = dir.join("mailbox.mailcraft");
    let (mut s, ms, _, copy) = copy_asking_microsoft(&path);
    let result = GraphFetchResult { account: ms, email: "owner@example.org".into(), failed: vec!["AAkA7".into()], ..Default::default() };
    crate::cmd::graph_mail::apply_graph_fetch(&mut s, result).unwrap();
    let why = s.run("mail.checkAgain", &json!({"id": copy})).unwrap_err().to_string();
    assert!(why.contains("asked for already"), "{why}");
    // A part of it removed here meanwhile: that is what is said.
    s.mailbox.message_mut(copy).unwrap().parts_removed = true;
    let why = s.run("mail.checkAgain", &json!({"id": copy})).unwrap_err().to_string();
    assert!(why.contains("a part was removed"), "{why}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A message of this computer moved into a Microsoft account, then deleted for good before its
/// server had it. An agent's Undo of that delete puts it on its way onto the Microsoft server
/// again (#566), as an agent's own move there would: asked as a send, as `mail.move` there is,
/// naming the server.
#[test]
fn an_agents_undo_that_puts_a_message_back_on_its_way_onto_a_microsoft_server_is_asked_as_a_send() {
    use crate::agent::{Class, Verdict, check};
    let mut s = Session::empty();
    let ms = microsoft(&mut s, "owner@example.org");
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let id = s.mailbox.add_message(Message { folder: here, account: local, subject: "Private".into(), ..Default::default() }).unwrap();
    let theirs = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    let waits = |s: &Session| {
        s.mailbox
            .queued
            .iter()
            .filter(|q| matches!(q, mailcraft_model::Queued::PlaceMessage { account, message, .. } if *message == id && *account == ms))
            .count()
    };
    // An agent's move there is a send.
    let moving = json!({"ids": [id], "folder": theirs});
    assert!(matches!(check(&s, "mail.move", &moving, &[]), Verdict::Ask(Class::Send, _)));
    // The owner moves it there, then thinks better of it and deletes it for good.
    s.run("mail.move", &moving).unwrap();
    assert_eq!(waits(&s), 1);
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert_eq!(waits(&s), 0);
    let verdict = check(&s, "edit.undo", &json!({}), &[]);
    let said = crate::agent::describe(&s, "edit.undo", &json!({}));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(waits(&s), 1, "the undo puts it on its way onto the Microsoft server again");
    assert!(matches!(verdict, Verdict::Ask(Class::Send, _)), "an agent's undo that does is asked as a send: {verdict:?}");
    assert!(said.contains("puts mail back on its way onto the server of owner@example.org"), "{said}");
}

/// The same into an IMAP account, whose queue the undo keeps as the step had it.
#[test]
fn an_agents_undo_that_puts_a_message_back_on_its_way_into_an_imap_account_is_asked_as_a_send() {
    use crate::agent::{Class, Verdict, check};
    let mut s = Session::empty();
    let _ms = microsoft(&mut s, "owner@example.org");
    let imap = imap_account(&mut s, "home@mail.example", vec![]);
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let here = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let id = s.mailbox.add_message(Message { folder: here, account: local, subject: "Private".into(), ..Default::default() }).unwrap();
    let theirs = s.mailbox.folder_of_kind(imap, FolderKind::Inbox).unwrap();
    let waits = |s: &Session| {
        s.mailbox
            .queued
            .iter()
            .filter(|q| matches!(q, mailcraft_model::Queued::PlaceMessage { account, message, .. } if *message == id && *account == imap))
            .count()
    };
    let moving = json!({"ids": [id], "folder": theirs});
    assert!(matches!(check(&s, "mail.move", &moving, &[]), Verdict::Ask(Class::Send, _)));
    s.run("mail.move", &moving).unwrap();
    assert_eq!(waits(&s), 1);
    s.run("mail.deletePermanently", &json!({"ids": [id]})).unwrap();
    assert_eq!(waits(&s), 0);
    let verdict = check(&s, "edit.undo", &json!({}), &[]);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(waits(&s), 1, "the undo puts it on its way onto the IMAP server again");
    assert!(matches!(verdict, Verdict::Ask(Class::Send, _)), "an agent's undo that does is asked as a send: {verdict:?}");
}
