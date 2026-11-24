// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Other users' mailboxes shared with an account, and mailboxes everyone shares (RFC 2342
//! namespaces, as Dovecot names them: `shared/alice/INBOX`, `Public/Team`): each in its own
//! subtree under the account, never one more `INBOX` at the top, and never taken for this
//! account's Inbox, Sent Items, Deleted Items or Archive.

use mailcraft_model::sync::ListedFolder;
use mailcraft_model::{AccountKind, Server};

use super::*;
use crate::fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};

const PASSWORD: &str = "hunter2";

fn mailbox(raw: &str, flags: &str, messages: &[&str]) -> FakeMailbox {
    let messages = messages
        .iter()
        .zip(1..)
        .map(|(subject, uid)| FakeMessage {
            uid,
            flags: String::new(),
            internal_date: " 1-Jul-2003 09:52:37 +0000".into(),
            body: format!("From: Bob <bob@example.org>\r\nSubject: {subject}\r\nMessage-ID: <{uid}.{subject}@example.org>\r\n\r\nHi.\r\n")
                .into_bytes(),
            claimed_size: None,
        })
        .collect();
    FakeMailbox { raw: raw.into(), flags: flags.into(), uid_validity: 7, messages, refuse_open: false, next_uid: 0 }
}

/// A Dovecot account with `/` between the parts of a name: its own Inbox, Sent (named, not
/// flagged) and Archiv; Alice's folders shared with it under `shared/`, her Sent, Trash and
/// Archive flagged as such, with the folders above them listed as folders that can't be
/// opened; Bob's Inbox with nothing listed above it; and a public folder.
fn dovecot(delimiter: char) -> ImapWorld {
    let d = delimiter;
    let (own, shared, public) = if d == '/' { ("", "shared/", "Public/") } else { ("INBOX.", "shared.", "Public.") };
    let mailboxes = vec![
        mailbox("INBOX", "\\HasChildren", &["Mine"]),
        mailbox(&format!("{own}Sent"), "\\HasNoChildren", &[]),
        mailbox(&format!("{own}Archiv"), "\\HasNoChildren", &[]),
        mailbox("shared", "\\Noselect \\HasChildren", &[]),
        mailbox(&format!("shared{d}alice"), "\\Noselect \\HasChildren", &[]),
        mailbox(&format!("shared{d}alice{d}INBOX"), "\\HasNoChildren", &["For Alice"]),
        mailbox(&format!("shared{d}alice{d}Archiv"), "\\HasNoChildren \\Archive", &[]),
        mailbox(&format!("shared{d}alice{d}Sent"), "\\HasNoChildren \\Sent", &[]),
        mailbox(&format!("shared{d}alice{d}Trash"), "\\HasNoChildren \\Trash", &[]),
        mailbox(&format!("shared{d}bob{d}INBOX"), "\\HasNoChildren", &["For Bob"]),
        mailbox(&format!("Public{d}Team"), "\\HasNoChildren", &["For everyone"]),
    ];
    ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        mailboxes,
        delimiter: Some(d),
        namespace: Some((own.into(), d.to_string())),
        namespace_others: format!("((\"{shared}\" \"{d}\")) ((\"{public}\" \"{d}\"))"),
        ..Default::default()
    }
}

fn request(imap: &FakeImap) -> SyncRequest {
    let account = Account {
        id: 7,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        username: "ada".into(),
        ..Default::default()
    };
    SyncRequest { account, password: PASSWORD.into(), ..Default::default() }
}

/// Each listed folder as (path, parent, name, kind).
fn tree(listed: &[ListedFolder]) -> Vec<(&str, &str, &str, FolderKind)> {
    listed.iter().map(|f| (f.remote.as_str(), f.parent.as_str(), f.name.as_str(), f.kind)).collect()
}

/// The tree is whole: every parent is listed, no name has a part of the path in it, and no two
/// folders share a path, or a parent and a name.
fn assert_whole(listed: &[ListedFolder], delimiter: char) {
    let paths: HashSet<&str> = listed.iter().map(|f| f.remote.as_str()).collect();
    assert_eq!(paths.len(), listed.len(), "a path twice");
    let mut names: HashSet<(&str, String)> = HashSet::new();
    for f in listed {
        assert!(f.parent.is_empty() || paths.contains(f.parent.as_str()), "{} has no parent {}", f.remote, f.parent);
        assert!(!f.name.contains(delimiter), "{}", f.name);
        assert!(names.insert((f.parent.as_str(), f.name.to_lowercase())), "two of {} in {:?}", f.name, f.parent);
    }
}

/// The shapes of a real Dovecot listing (the names invented): one namespace of other users,
/// `shared/`, whose users are named by address; their folders carry no special-use flags but
/// are called Sent, Trash, Drafts or the like in German, in modified UTF-7, some with spaces;
/// a shared Inbox with folders inside is listed twice; subtrees go seven levels down; and the
/// account's own Inbox comes last.
fn real_dovecot() -> ImapWorld {
    let carol = "shared/carol@example.org";
    let erin = "shared/erin@example.org";
    let mailboxes = vec![
        mailbox("Archiv", "\\HasChildren", &[]),
        mailbox("Archiv/2018-Privat", "\\HasNoChildren", &[]),
        mailbox("Verk&AOQ-ufer", "\\HasNoChildren", &[]),
        mailbox("Trash", "\\HasNoChildren \\UnMarked \\Trash", &[]),
        mailbox("Sent", "\\HasNoChildren \\UnMarked \\Sent", &[]),
        mailbox("Junk", "\\HasNoChildren \\UnMarked \\Junk", &[]),
        mailbox("Drafts", "\\HasNoChildren \\UnMarked \\Drafts", &[]),
        mailbox("shared", "\\Noselect \\HasChildren", &[]),
        mailbox(carol, "\\Noselect \\HasChildren", &[]),
        mailbox(&format!("{carol}/Trash"), "\\HasNoChildren \\UnMarked", &[]),
        mailbox(&format!("{carol}/Sent"), "\\HasNoChildren \\UnMarked", &[]),
        mailbox(&format!("{carol}/Drafts"), "\\HasNoChildren \\UnMarked", &[]),
        mailbox(&format!("{carol}/Archiv"), "\\HasNoChildren", &[]),
        mailbox(&format!("{carol}/INBOX"), "\\HasNoChildren", &["For Carol"]),
        mailbox("shared/dave@example.org", "\\Noselect \\HasChildren", &[]),
        mailbox("shared/dave@example.org/INBOX", "\\HasNoChildren", &["For Dave"]),
        mailbox(erin, "\\Noselect \\HasChildren", &[]),
        mailbox(&format!("{erin}/INBOX"), "\\HasChildren", &["For Erin"]),
        mailbox(&format!("{erin}/INBOX"), "\\HasChildren", &["For Erin"]),
        mailbox(&format!("{erin}/INBOX/Zeiterfassung"), "\\HasNoChildren \\Marked", &[]),
        mailbox(&format!("{erin}/Gesendete Elemente"), "\\HasNoChildren", &[]),
        mailbox(&format!("{erin}/Gel&APY-schte Elemente"), "\\HasNoChildren", &[]),
        mailbox(&format!("{erin}/Junk-E-Mail"), "\\HasNoChildren", &[]),
        mailbox(&format!("{erin}/Postausgang"), "\\HasNoChildren", &[]),
        mailbox(&format!("{erin}/Entw&APw-rfe"), "\\HasNoChildren", &[]),
        mailbox(&format!("{erin}/Team-Archiv"), "\\HasChildren", &[]),
        mailbox(&format!("{erin}/Team-Archiv/Gesendete Elemente"), "\\HasNoChildren", &[]),
        mailbox(&format!("{erin}/A/B/C/D/E"), "\\HasNoChildren", &["Deep"]),
        mailbox("INBOX", "\\HasNoChildren", &["Mine"]),
    ];
    ImapWorld {
        user: "ada".into(),
        password: PASSWORD.into(),
        mailboxes,
        delimiter: Some('/'),
        namespace: Some((String::new(), "/".into())),
        namespace_others: "((\"shared/\" \"/\")) NIL".into(),
        ..Default::default()
    }
}

/// The real listing's shapes: everyone else's folders in `shared` ▸ address ▸ their tree, only
/// the account's own folders do the standard jobs, and a mailbox listed twice is one folder.
#[test]
fn a_real_dovecot_listing_makes_one_tree() {
    let imap = Fake::imap(real_dovecot());
    let result = sync(&request(&imap));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_whole(&result.listed, '/');
    let tree = tree(&result.listed);
    use FolderKind::*;
    let standard: Vec<(&str, FolderKind)> = tree.iter().filter(|f| f.3 != Custom).map(|f| (f.0, f.3)).collect();
    assert_eq!(standard, [("INBOX", Inbox), ("Sent", Sent), ("Drafts", Drafts), ("Trash", Deleted), ("Junk", Junk)]);
    let erin = "shared/erin@example.org";
    for want in [
        ("shared", "", "shared"),
        ("shared/carol@example.org", "shared", "carol@example.org"),
        ("shared/carol@example.org/Sent", "shared/carol@example.org", "Sent"),
        ("shared/carol@example.org/Archiv", "shared/carol@example.org", "Archiv"),
        ("shared/dave@example.org/INBOX", "shared/dave@example.org", "INBOX"),
        (&format!("{erin}/INBOX/Zeiterfassung"), &format!("{erin}/INBOX"), "Zeiterfassung"),
        (&format!("{erin}/Gelöschte Elemente"), erin, "Gelöschte Elemente"),
        (&format!("{erin}/Team-Archiv/Gesendete Elemente"), &format!("{erin}/Team-Archiv"), "Gesendete Elemente"),
        // Seven levels down, with nothing listed between: the folders between are made.
        (&format!("{erin}/A"), erin, "A"),
        (&format!("{erin}/A/B/C/D"), &format!("{erin}/A/B/C"), "D"),
        (&format!("{erin}/A/B/C/D/E"), &format!("{erin}/A/B/C/D"), "E"),
        ("Archiv/2018-Privat", "Archiv", "2018-Privat"),
        ("Verkäufer", "", "Verkäufer"),
    ] {
        assert!(tree.iter().any(|f| (f.0, f.1, f.2) == want), "{want:?} in {tree:?}");
    }
    // Erin's Inbox, listed twice, is one folder, read once.
    let erins_inbox = format!("{erin}/INBOX");
    assert_eq!(tree.iter().filter(|f| f.0 == erins_inbox).count(), 1);
    assert_eq!(result.folders.iter().filter(|f| f.remote == erins_inbox).map(|f| f.new.len()).collect::<Vec<_>>(), [1]);
    // The account's own Inbox, listed last, is the one read as the Inbox.
    let inbox: Vec<&str> = result.folders.iter().filter(|f| f.kind == Inbox).map(|f| f.remote.as_str()).collect();
    assert_eq!(inbox, ["INBOX"]);
}

/// The owner's report: several `INBOX` and `Archiv` side by side at the top of the account.
/// Now they are where the server keeps them, under `shared` ▸ the user, and only the account's
/// own folders do the jobs of its standard folders.
#[test]
fn others_folders_are_a_subtree_and_never_the_accounts_own() {
    let imap = Fake::imap(dovecot('/'));
    let result = sync(&request(&imap));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_whole(&result.listed, '/');
    let tree = tree(&result.listed);
    use FolderKind::*;
    assert_eq!(
        tree,
        [
            ("INBOX", "", "INBOX", Inbox),
            ("Sent", "", "Sent", Sent),
            ("Archiv", "", "Archiv", Custom),
            // The folders above another's, that can't be opened, come before them as holders.
            ("shared", "", "shared", Custom),
            ("shared/alice", "shared", "alice", Custom),
            ("shared/alice/INBOX", "shared/alice", "INBOX", Custom),
            // Flagged \Archive, \Sent and \Trash: Alice's, not this account's.
            ("shared/alice/Archiv", "shared/alice", "Archiv", Custom),
            ("shared/alice/Sent", "shared/alice", "Sent", Custom),
            ("shared/alice/Trash", "shared/alice", "Trash", Custom),
            // Nothing listed above Bob's Inbox: the folder for him is made all the same.
            ("shared/bob", "shared", "bob", Custom),
            ("shared/bob/INBOX", "shared/bob", "INBOX", Custom),
            ("Public", "", "Public", Custom),
            ("Public/Team", "Public", "Team", Custom),
        ]
    );
    // Their mail is read as any folder's is; the holders, which can't be opened, are not.
    let read: Vec<(&str, usize)> = result.folders.iter().map(|f| (f.remote.as_str(), f.new.len())).collect();
    for (remote, new) in [("INBOX", 1), ("shared/alice/INBOX", 1), ("shared/bob/INBOX", 1), ("Public/Team", 1)] {
        assert!(read.contains(&(remote, new)), "{remote}: {read:?}");
    }
    for holder in ["shared", "shared/alice", "shared/bob", "Public"] {
        assert!(!read.iter().any(|(remote, _)| *remote == holder), "{holder}");
    }
    // Only the account's own Inbox is the Inbox the mail of which is read as the Inbox's.
    let kinds: Vec<(&str, FolderKind)> = result.folders.iter().filter(|f| f.kind != Custom).map(|f| (f.remote.as_str(), f.kind)).collect();
    assert_eq!(kinds, [("INBOX", Inbox), ("Sent", Sent)]);
}

/// The same with dots between the parts of a name and the account's own folders under `INBOX.`.
#[test]
fn others_folders_are_a_subtree_with_dots_too() {
    let imap = Fake::imap(dovecot('.'));
    let result = sync(&request(&imap));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_whole(&result.listed, '.');
    let tree = tree(&result.listed);
    use FolderKind::*;
    assert!(tree.contains(&("INBOX.Sent", "INBOX", "Sent", Sent)), "{tree:?}");
    assert!(tree.contains(&("shared", "", "shared", Custom)), "{tree:?}");
    assert!(tree.contains(&("shared.alice", "shared", "alice", Custom)), "{tree:?}");
    assert!(tree.contains(&("shared.alice.INBOX", "shared.alice", "INBOX", Custom)), "{tree:?}");
    assert!(tree.contains(&("shared.alice.Sent", "shared.alice", "Sent", Custom)), "{tree:?}");
    assert!(tree.contains(&("shared.bob.INBOX", "shared.bob", "INBOX", Custom)), "{tree:?}");
    assert!(tree.contains(&("Public.Team", "Public", "Team", Custom)), "{tree:?}");
    assert_eq!(tree.iter().filter(|(_, _, _, kind)| *kind == Inbox).count(), 1);
}

/// What can be told without a NAMESPACE answer is as before: a server that doesn't name its
/// namespaces has nothing of anyone else's told apart, and nothing is made above its folders.
#[test]
fn without_namespaces_nothing_is_made_above() {
    let mut world = dovecot('/');
    world.namespace = None;
    let imap = Fake::imap(world);
    let result = sync(&request(&imap));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(!result.listed.iter().any(|f| f.remote == "shared" || f.remote == "shared/bob"));
}

/// Hostile namespaces and names: a namespace with no prefix takes in nothing, `INBOX` is never
/// another's, and of a path deeper than the holders made the highest is at the top; nothing panics.
#[test]
fn odd_namespaces_and_deep_paths_are_bounded() {
    use mailcraft_proto::imap::{Namespace, Namespaces};
    let one = |prefix: &str| Namespace { prefix: prefix.into(), delimiter: Some('/') };
    let others = Others::of(&Namespaces { personal: vec![one("")], other_users: vec![one(""), one("INBOX/")], shared: vec![one("x/")] });
    let deep = format!("x/{}leaf", "a/".repeat(100));
    let names = ["INBOX", "INBOX/Sub", "Sent", deep.as_str(), "x/", "x//", "x"];
    let mailboxes: Vec<Mailbox> =
        names.iter().map(|name| Mailbox { name: (*name).into(), raw: (*name).into(), delimiter: Some('/'), flags: vec!["\\Sent".into()] }).collect();
    let plan = planned(&mailboxes, &[], false, &others);
    let kinds: Vec<(&str, FolderKind)> = plan.iter().map(|(m, k)| (m.name.as_str(), *k)).collect();
    // The account's own Inbox, and the first of its own flagged Sent; nothing of `x/`.
    assert_eq!(kinds.first(), Some(&("INBOX", FolderKind::Inbox)));
    assert!(kinds.contains(&("Sent", FolderKind::Sent)), "{kinds:?}");
    assert!(kinds.iter().filter(|(name, _)| name.starts_with('x')).all(|(_, kind)| *kind == FolderKind::Custom));
    let listed = listed(&plan, &others);
    assert!(listed.len() <= names.len() + MAX_HOLDER_LEVELS + 1, "{}", listed.len());
    let leaf = listed.iter().find(|f| f.remote == deep);
    assert_eq!(leaf.map(|f| f.name.as_str()), Some("leaf"));
    assert!(leaf.is_some_and(|f| listed.iter().any(|holder| holder.remote == f.parent)));
    // Every parent named is one of those listed, or the top.
    assert!(listed.iter().all(|f| f.parent.is_empty() || listed.iter().any(|p| p.remote == f.parent)));
}

/// The folders made to hold others' count against the folders a listing may have (#1863): a
/// server that lists as many of others' folders as are read, each under paths it doesn't list,
/// gets no more than that many, every one read is there, and none points at a folder not made.
#[test]
fn holders_never_take_a_listing_past_its_cap() {
    use mailcraft_proto::imap::{Namespace, Namespaces};
    let one = |prefix: &str| Namespace { prefix: prefix.into(), delimiter: Some('/') };
    let others = Others::of(&Namespaces { personal: vec![one("")], other_users: vec![one("shared/")], shared: vec![] });
    let own = |name: String| Mailbox { raw: name.clone(), name, delimiter: Some('/'), flags: Vec::new() };
    let check = |made: &[ListedFolder], mailboxes: &[Mailbox]| {
        assert!(made.len() <= MAX_FOLDERS, "{}", made.len());
        let paths: HashSet<&str> = made.iter().map(|f| f.remote.as_str()).collect();
        assert!(mailboxes.iter().take(MAX_FOLDERS).all(|m| paths.contains(m.name.as_str())));
        assert!(made.iter().all(|f| f.parent.is_empty() || paths.contains(f.parent.as_str())));
    };

    // Every one of them deep under paths not listed: no room for any holder.
    let hostile: Vec<Mailbox> = (0..MAX_FOLDERS).map(|n| own(format!("shared/u{n}/a/a/a/INBOX"))).collect();
    let plan = planned(&hostile, &[], false, &others);
    let made = listed(&plan, &others);
    check(&made, &hostile);
    assert_eq!(made.len(), MAX_FOLDERS);
    assert!(made.iter().all(|f| f.parent.is_empty() && f.name == f.remote), "{:?}", made.first());

    // Room for three: the first chain is made whole, the next that doesn't fit is at the top.
    let mut tight = vec![own("shared/alice@example.org/Team/INBOX".into()), own("shared/bob@example.org/INBOX".into())];
    tight.extend((0..MAX_FOLDERS - 5).map(|n| own(format!("F{n}"))));
    let plan = planned(&tight, &[], false, &others);
    let made = listed(&plan, &others);
    check(&made, &tight);
    assert_eq!(made.len(), MAX_FOLDERS);
    let at = |remote: &str| made.iter().find(|f| f.remote == remote).map(|f| (f.parent.as_str(), f.name.as_str()));
    assert_eq!(at("shared/alice@example.org/Team/INBOX"), Some(("shared/alice@example.org/Team", "INBOX")));
    assert_eq!(at("shared/alice@example.org/Team"), Some(("shared/alice@example.org", "Team")));
    assert_eq!(at("shared"), Some(("", "shared")));
    assert_eq!(at("shared/bob@example.org/INBOX"), Some(("", "shared/bob@example.org/INBOX")));
    assert_eq!(at("shared/bob@example.org"), None);
}

/// A mailbox the server lists twice, first as one that can't be opened and then as one that can
/// (#1863): it is one folder, read, with the flags of both listings.
#[test]
fn a_mailbox_listed_closed_and_then_open_is_read() {
    let mut world = dovecot('/');
    let erin = "shared/erin@example.org/INBOX";
    world.mailboxes.push(mailbox(erin, "\\Noselect \\HasNoChildren", &[]));
    world.mailboxes.push(mailbox(erin, "\\HasChildren", &["For Erin"]));
    world.mailboxes.push(mailbox(&format!("{erin}/Team"), "\\HasNoChildren", &[]));
    let imap = Fake::imap(world);
    let result = sync(&request(&imap));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_whole(&result.listed, '/');
    let listed: Vec<&ListedFolder> = result.listed.iter().filter(|f| f.remote == erin).collect();
    assert_eq!(listed.len(), 1, "{:?}", result.listed);
    assert!(listed.iter().all(|f| f.kind == FolderKind::Custom && f.name == "INBOX"));
    assert!(result.listed.iter().any(|f| f.remote == format!("{erin}/Team") && f.parent == erin));
    assert_eq!(result.folders.iter().filter(|f| f.remote == erin).map(|f| f.new.len()).collect::<Vec<_>>(), [1]);
}

/// A standard folder an earlier version tied to another's mailbox (#1863): the tie is not
/// followed, that mailbox is an ordinary folder, and the account's own one does the job.
#[test]
fn a_tie_to_anothers_mailbox_is_not_followed() {
    let imap = Fake::imap(dovecot('/'));
    let mut request = request(&imap);
    request.folders = vec![
        FolderState { id: 3, kind: FolderKind::Sent, remote: "shared/alice/Sent".into(), ..Default::default() },
        FolderState { id: 4, kind: FolderKind::Deleted, remote: "shared/alice/Trash".into(), ..Default::default() },
    ];
    let result = sync(&request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let kind = |remote: &str| result.listed.iter().find(|f| f.remote == remote).map(|f| f.kind);
    assert_eq!(kind("shared/alice/Sent"), Some(FolderKind::Custom));
    assert_eq!(kind("shared/alice/Trash"), Some(FolderKind::Custom));
    assert_eq!(kind("Sent"), Some(FolderKind::Sent));
}

/// A server that offers NAMESPACE and refuses it is read as one without NAMESPACE was before
/// (#1863): every folder listed is taken as the account's own, and nothing is said. One whose
/// connection fails at NAMESPACE says so, naming that question, not the list of folders.
#[test]
fn a_refused_or_failed_namespace_question() {
    let mut world = dovecot('/');
    world.refuse = vec!["NAMESPACE".into()];
    let imap = Fake::imap(world);
    let result = sync(&request(&imap));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(!result.listed.iter().any(|f| f.remote == "shared/alice"), "nothing is made above others' folders");
    assert!(result.listed.iter().any(|f| f.remote == "shared/alice/INBOX" && f.parent.is_empty()));
    assert!(result.folders.iter().any(|f| f.remote == "shared/alice/INBOX"));

    let mut world = dovecot('/');
    world.hang_up_on = Some("NAMESPACE".into());
    let imap = Fake::imap(world);
    let result = sync(&request(&imap));
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors.iter().all(|e| e.starts_with("Couldn't ask the server which folders are other people's")), "{:?}", result.errors);
    assert!(result.listed.is_empty());
}

/// A namespace whose prefix doesn't end with its delimiter (RFC 2342 doesn't ask it to): with
/// `shared` and `/`, the account's own `shared-notes` and `shared2/x` are its own, keep their
/// standard jobs and have nothing made above them; `shared` and `shared/bob/x` are another's.
#[test]
fn a_prefix_without_its_delimiter_takes_in_only_what_is_under_it() {
    use mailcraft_proto::imap::{Namespace, Namespaces};
    let others = Others::of(&Namespaces {
        personal: vec![Namespace { prefix: String::new(), delimiter: Some('/') }],
        other_users: vec![Namespace { prefix: "shared".into(), delimiter: Some('/') }],
        shared: vec![Namespace { prefix: "#flat.".into(), delimiter: None }],
    });
    let one = |name: &str, flag: &str| Mailbox { name: name.into(), raw: name.into(), delimiter: Some('/'), flags: vec![flag.into()] };
    let mailboxes = [
        one("INBOX", "\\HasNoChildren"),
        one("shared/bob/Sent", "\\Sent"),
        one("shared-notes", "\\Sent"),
        one("shared2/x", "\\Trash"),
        one("shared", "\\Drafts"),
        one("sharedDrafts", "\\Drafts"),
        one("#flat.Junk", "\\Junk"),
        one("#flatter", "\\Junk"),
    ];
    let has = |name: &str| mailboxes.iter().find(|m| m.raw == name).is_some_and(|m| others.has(m));
    for (name, theirs) in [
        ("INBOX", false),
        ("shared/bob/Sent", true),
        ("shared", true),
        ("shared-notes", false),
        ("shared2/x", false),
        ("sharedDrafts", false),
        ("#flat.Junk", true),
        ("#flatter", false),
    ] {
        assert_eq!(has(name), theirs, "{name}");
    }
    let plan = planned(&mailboxes, &[], false, &others);
    let kind = |name: &str| plan.iter().find(|(m, _)| m.raw == name).map(|(_, kind)| *kind);
    use FolderKind::*;
    assert_eq!(kind("shared/bob/Sent"), Some(Custom));
    assert_eq!(kind("shared-notes"), Some(Sent));
    assert_eq!(kind("shared2/x"), Some(Deleted));
    assert_eq!(kind("shared"), Some(Custom));
    assert_eq!(kind("sharedDrafts"), Some(Drafts));
    assert_eq!(kind("#flat.Junk"), Some(Custom));
    assert_eq!(kind("#flatter"), Some(Junk));
    // `shared/bob` is made to hold Bob's folder; nothing is made above the account's `shared2/x`.
    let listed = listed(&plan, &others);
    let at = |remote: &str| listed.iter().find(|f| f.remote == remote).map(|f| (f.parent.as_str(), f.name.as_str()));
    assert_eq!(at("shared/bob/Sent"), Some(("shared/bob", "Sent")));
    assert_eq!(at("shared/bob"), Some(("shared", "bob")));
    assert_eq!(at("shared2/x"), Some(("", "shared2/x")));
    assert_eq!(at("shared2"), None);
    assert_eq!(at("shared-notes"), Some(("", "shared-notes")));
}

/// The same through a whole synchronisation, the server naming `shared` with no `/` at its end.
#[test]
fn a_server_naming_a_prefix_without_its_delimiter() {
    let mut world = dovecot('/');
    world.namespace_others = "((\"shared\" \"/\")) ((\"Public/\" \"/\"))".into();
    world.mailboxes.push(mailbox("shared-notes", "\\HasNoChildren \\Drafts", &["Note"]));
    world.mailboxes.push(mailbox("shared2", "\\HasChildren", &[]));
    world.mailboxes.push(mailbox("shared2/x", "\\HasNoChildren \\Trash", &[]));
    let imap = Fake::imap(world);
    let result = sync(&request(&imap));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_whole(&result.listed, '/');
    let tree = tree(&result.listed);
    use FolderKind::*;
    assert!(tree.contains(&("shared-notes", "", "shared-notes", Drafts)), "{tree:?}");
    assert!(tree.contains(&("shared2", "", "shared2", Custom)), "{tree:?}");
    assert!(tree.contains(&("shared2/x", "shared2", "x", Deleted)), "{tree:?}");
    assert!(tree.contains(&("shared/bob", "shared", "bob", Custom)), "{tree:?}");
    assert!(tree.contains(&("shared/alice/Sent", "shared/alice", "Sent", Custom)), "{tree:?}");
    assert!(tree.contains(&("Sent", "", "Sent", Sent)), "{tree:?}");
}

/// A part of a path with spaces around it (`shared/u/Team /x`, `shared/u/Team ` not listed):
/// the folder made to hold it is the server's `shared/u/Team `, shown as `Team`, and it holds
/// all that lies under it; one made of spaces only is shown as it is, and nothing panics.
#[test]
fn a_part_with_spaces_around_it_is_held_once() {
    use mailcraft_proto::imap::{Namespace, Namespaces};
    let others = Others::of(&Namespaces {
        personal: vec![Namespace { prefix: String::new(), delimiter: Some('/') }],
        other_users: vec![Namespace { prefix: "shared/".into(), delimiter: Some('/') }],
        shared: Vec::new(),
    });
    let one = |name: &str| Mailbox { name: name.into(), raw: name.into(), delimiter: Some('/'), flags: Vec::new() };
    let mailboxes = [one("INBOX"), one("shared/u/Team /x"), one("shared/u/Team /x/sub"), one("shared/u/Team /y/z"), one("shared/v/  /w")];
    let plan = planned(&mailboxes, &[], false, &others);
    let listed = listed(&plan, &others);
    assert_whole(&listed, '/');
    let at = |remote: &str| listed.iter().find(|f| f.remote == remote).map(|f| (f.parent.as_str(), f.name.as_str()));
    assert_eq!(at("shared/u/Team "), Some(("shared/u", "Team")), "{:?}", tree(&listed));
    assert_eq!(at("shared/u/Team /x"), Some(("shared/u/Team ", "x")));
    assert_eq!(at("shared/u/Team /x/sub"), Some(("shared/u/Team /x", "sub")));
    assert_eq!(at("shared/u/Team /y"), Some(("shared/u/Team ", "y")));
    assert_eq!(at("shared/u/Team /y/z"), Some(("shared/u/Team /y", "z")));
    assert_eq!(at("shared/u"), Some(("shared", "u")));
    assert_eq!(at("shared"), Some(("", "shared")));
    assert_eq!(at("shared/v/  "), Some(("shared/v", "  ")));
    assert_eq!(at("shared/v/  /w"), Some(("shared/v/  ", "w")));
    assert_eq!(listed.iter().filter(|f| f.name == "Team").count(), 1, "{:?}", tree(&listed));
}
