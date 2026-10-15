//! A shared mailbox opened with its owner's sign-in: over IMAP as the shared mailbox (XOAUTH2
//! with its address as the user and the owner's token), and sent from through SMTP signed in as
//! the owner, with the shared mailbox's address as the sender.

use super::*;

/// The shared mailbox all of these open.
const SHARED: &str = "support@example.org";

/// The servers: the incoming one knows the shared mailbox by the owner's token, the outgoing
/// one knows the owner.
fn shared_servers() -> (FakeImap, FakeSmtp) {
    let imap = Fake::imap(ImapWorld {
        user: SHARED.into(),
        token: TOKEN.into(),
        mailboxes: vec![
            mailbox("INBOX", "\\HasNoChildren", 11, vec![message(1, "", "Printer on fire"), message(2, "\\Seen", "Refund")]),
            mailbox("Sent Items", "\\HasNoChildren \\Sent", 21, Vec::new()),
            mailbox("Escalations", "\\HasNoChildren", 31, vec![message(4, "", "Call back")]),
        ],
        ..Default::default()
    });
    let smtp = Fake::smtp(SmtpWorld { user: "ada@example.org".into(), token: TOKEN.into(), ..smtp_world() });
    (imap, smtp)
}

/// What the engine asks of a run for the shared mailbox (`mailcraft_engine::cmd::account::sync_request`):
/// the shared mailbox as an account that signs in with its owner's Microsoft sign-in, under its
/// own address, with the owner's name for the outgoing server and the owner's token.
fn shared_request(imap: &FakeImap, smtp: &FakeSmtp) -> SyncRequest {
    let owner = oauth_account(Some(imap), Some(smtp));
    let account = Account {
        id: 8,
        name: SHARED.into(),
        email: SHARED.into(),
        display_name: "Support".into(),
        username: SHARED.into(),
        shared_of: Some(owner.id),
        ..owner.clone()
    };
    SyncRequest { account, access_token: TOKEN.into(), folders: local_folders(), send_login: owner.email.clone(), ..Default::default() }
}

/// The user and the token a server was shown in XOAUTH2's envelope, from the lines it received.
fn signed_in_as(log: &[String]) -> Option<(String, String)> {
    log.iter().filter_map(|line| line.rsplit(' ').next()).map(crate::fake::bearer).find(|(user, _)| !user.is_empty())
}

#[test]
fn a_shared_mailbox_is_read_with_its_address_and_the_owners_token() {
    let (imap, smtp) = shared_servers();
    let request = shared_request(&imap, &smtp);
    let result = sync(&request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    // The answer is the shared mailbox's, and its folders are its own.
    assert_eq!((result.account, result.email.as_str()), (8, SHARED));
    let inbox = result.folders.iter().find(|f| f.remote == "INBOX").expect("the Inbox came down");
    let subjects: Vec<&str> = inbox.new.iter().map(|m| m.subject.as_str()).collect();
    assert_eq!(subjects, ["Printer on fire", "Refund"]);
    assert!(result.listed.iter().any(|f| f.remote == "Escalations"));
    // The server was shown the shared address and the owner's token, in XOAUTH2's envelope.
    let log = imap.world().log.clone();
    assert!(!log.iter().any(|line| line.contains(TOKEN)), "the token travels in its envelope only");
    assert_eq!(signed_in_as(&log), Some((SHARED.to_string(), TOKEN.to_string())), "{log:?}");
    // Nothing was sent, so the outgoing server wasn't asked.
    assert_eq!(smtp.world().connections, 0);
}

#[test]
fn mail_from_a_shared_mailbox_goes_out_signed_in_as_the_owner() {
    let (imap, smtp) = shared_servers();
    let mut request = shared_request(&imap, &smtp);
    let from = Address::new("Support", SHARED);
    request.outbox = vec![Message { from: from.clone(), ..hello() }];
    let result = sync(&request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.sent, [9]);
    let world = smtp.world();
    // Signed in as the owner, with the owner's token...
    assert_eq!(world.sign_ins, 1);
    assert_eq!(signed_in_as(&world.log), Some(("ada@example.org".to_string(), TOKEN.to_string())), "{:?}", world.log);
    // ...and the message went from the shared mailbox, envelope and header alike.
    let delivery = world.delivered.first().expect("delivered");
    assert_eq!(delivery.from, SHARED);
    assert_eq!(delivery.recipients, ["bob@example.org"]);
    assert!(delivery.data.contains("From: Support <support@example.org>"), "{}", delivery.data);
}

#[test]
fn a_server_that_refuses_is_said_plainly_and_without_the_token() {
    let (imap, smtp) = shared_servers();
    // No Full Access: the incoming server won't take the owner's token for the shared mailbox.
    imap.world().user = "somebody-else@example.org".into();
    // No Send As: the outgoing server won't take the shared address as the sender.
    smtp.world().refuse_from = vec![SHARED.into()];
    let mut request = shared_request(&imap, &smtp);
    request.outbox = vec![Message { from: Address::new("Support", SHARED), ..hello() }];
    let result = sync(&request);
    assert_eq!(result.errors.len(), 2, "{:?}", result.errors);
    let (send, open) = (&result.errors[0], &result.errors[1]);
    assert!(send.starts_with("Couldn't send \"Hello\" from support@example.org: the server didn't let your account send as that mailbox."), "{send}");
    assert!(send.contains("Send As") && send.contains("5.7.60"), "{send}");
    assert!(open.starts_with("Couldn't open the shared mailbox support@example.org: the server didn't let your account in."), "{open}");
    assert!(open.contains("Full Access"), "{open}");
    assert!(result.sent.is_empty() && result.folders.is_empty());
    // The message is held for the owner, with why, rather than tried every few minutes.
    assert_eq!(result.held.len(), 1, "{:?}", result.held);
    assert_eq!(result.held[0].message, 9);
    assert!(result.held[0].why.starts_with("The server didn't let your account send as support@example.org:"), "{}", result.held[0].why);
    assert!(!format!("{result:?} {request:?}").contains(TOKEN));
    assert!(smtp.world().delivered.is_empty());
}

#[test]
fn another_accounts_refusal_keeps_its_usual_words() {
    let (imap, smtp) = shared_servers();
    let mut request = shared_request(&imap, &smtp);
    // An account of its own, refused the same way: no word of shared mailboxes.
    request.account.shared_of = None;
    request.send_login.clear();
    imap.world().user = "somebody-else@example.org".into();
    let result = sync(&request);
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors[0].starts_with("Couldn't check for new mail: sign-in failed"), "{:?}", result.errors);
    // And a recipient a shared mailbox's server won't take is that, not a matter of Send As.
    smtp.world().refuse = vec!["bob@example.org".into()];
    let mut request = shared_request(&imap, &smtp);
    request.outbox = vec![hello()];
    let result = sync(&request);
    assert!(result.errors.iter().any(|e| e.starts_with("Couldn't send \"Hello\": ")), "{:?}", result.errors);
    assert!(result.held.is_empty(), "a recipient refused holds nothing");
}

#[test]
fn a_shared_mailbox_is_checked_before_it_is_added() {
    let (imap, smtp) = shared_servers();
    let account = shared_request(&imap, &smtp).account;
    // Let in, and its Inbox opens: it can be added.
    assert_eq!(crate::check_shared(&account, TOKEN), Ok(()));
    // A mailbox the account has no Full Access to: the server doesn't let it in.
    let other = Account { username: "finance@example.org".into(), email: "finance@example.org".into(), ..account.clone() };
    match crate::check_shared(&other, TOKEN) {
        Err(crate::SharedCheck::NoAccess(said)) => assert!(!said.contains(TOKEN), "{said}"),
        other => panic!("{other:?}"),
    }
    // Let in, and its Inbox won't open.
    imap.world().mailbox("INBOX").refuse_open = true;
    assert!(matches!(crate::check_shared(&account, TOKEN), Err(crate::SharedCheck::NoAccess(_))));
    // A server that can't be reached is no answer about access.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut unreachable = account.clone();
    if let Some(server) = unreachable.incoming.as_mut() {
        server.port = closed;
    }
    assert!(matches!(crate::check_shared(&unreachable, TOKEN), Err(crate::SharedCheck::Failed(_))));
}

#[test]
fn a_shared_mailboxs_sent_copy_is_put_in_its_own_sent_items_even_where_the_server_files_what_it_sends() {
    // Exchange files what the owner sends as the shared mailbox in the owner's Sent Items: the
    // shared mailbox's copy is put there from here, and said filed only once it is.
    let (imap, smtp) = shared_servers();
    let mut request = shared_request(&imap, &smtp);
    request.outbox = vec![Message { from: Address::new("Support", SHARED), ..hello() }];
    let exchange = Host { files_sent: true, microsoft: true };
    let mut pieces: Vec<SyncResult> = Vec::new();
    sync_as(&request, Limits::in_pieces(), exchange, &AtomicBool::new(false), &mut |piece| pieces.push(piece));
    let mut result = SyncResult::default();
    for piece in &pieces {
        result.absorb(piece.clone());
    }
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.sent, [9]);
    assert_eq!(result.filed, [9]);
    assert!(result.unfiled.is_empty());
    // Said sent before it was filed, and filed only in a piece after the copy was put there.
    let sent_at = pieces.iter().position(|p| p.sent.contains(&9)).unwrap();
    let filed_at = pieces.iter().position(|p| p.filed.contains(&9)).unwrap();
    assert!(sent_at < filed_at, "{sent_at} {filed_at}");
    let world = imap.world();
    let sent_items = world.mailboxes.iter().find(|m| m.raw == "Sent Items").unwrap();
    assert_eq!(sent_items.messages.len(), 1);
    assert!(String::from_utf8_lossy(&sent_items.messages[0].body).contains("From: Support <support@example.org>"));
    assert!(sent_items.messages[0].flags.contains("\\Seen"));
    drop(world);

    // When the server won't take the copy, it isn't said filed, and the message is still sent.
    let (imap, smtp) = shared_servers();
    imap.world().refuse = vec!["APPEND".into()];
    let mut request = shared_request(&imap, &smtp);
    request.outbox = vec![Message { from: Address::new("Support", SHARED), ..hello() }];
    let mut whole = SyncResult::default();
    sync_as(&request, Limits::in_pieces(), exchange, &AtomicBool::new(false), &mut |piece| whole.absorb(piece));
    assert_eq!(whole.sent, [9]);
    assert!(whole.filed.is_empty() && whole.unfiled.is_empty(), "{whole:?}");
    assert!(whole.errors.iter().any(|e| e.starts_with("Couldn't keep a copy of \"Hello\" on the server")), "{:?}", whole.errors);
}
