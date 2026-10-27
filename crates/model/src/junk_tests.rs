// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::*;

#[test]
fn entries_are_addresses_or_domains_in_lower_case() {
    assert_eq!(junk_entry("  Ada@Example.ORG "), Some("ada@example.org".into()));
    assert_eq!(junk_entry("<ada@example.org>"), Some("ada@example.org".into()));
    assert_eq!(junk_entry("@Example.org"), Some("@example.org".into()));
    assert_eq!(junk_entry("example.org"), Some("@example.org".into()));
    assert_eq!(junk_entry("*@example.org"), Some("@example.org".into()));
    assert_eq!(junk_entry("example.org."), Some("@example.org".into()));
    assert_eq!(junk_entry("bücher.example"), Some("@bücher.example".into()));
    for bad in [
        "",
        "   ",
        "@",
        "ada@",
        "ada",
        "a@@example.org",
        "a b@example.org",
        "ada@-x.example",
        "a@b..example",
        "x\u{0}@example.org",
        "a,b@example.org",
    ] {
        assert_eq!(junk_entry(bad), None, "{bad:?}");
    }
    assert_eq!(junk_entry(&format!("a@{}.example", "x".repeat(300))), None);
    assert_eq!(junk_entry(&"\u{202e}é\n@".repeat(100_000)), None);
}

#[test]
fn an_address_matches_itself_and_a_domain_its_own_mail_only() {
    let mut l = JunkLists::default();
    assert_eq!(l.verdict("ada@example.org"), None);
    assert!(matches!(l.add(JunkList::Blocked, "spam@example.net"), Added::Added(_)));
    assert!(matches!(l.add(JunkList::Blocked, "@offers.example"), Added::Added(_)));
    assert_eq!(l.verdict("SPAM@Example.NET"), Some(JunkList::Blocked));
    assert_eq!(l.verdict("other@example.net"), None);
    assert_eq!(l.verdict("anyone@offers.example"), Some(JunkList::Blocked));
    // Not a subdomain, nor a domain that ends the same way.
    assert_eq!(l.verdict("anyone@mail.offers.example"), None);
    assert_eq!(l.verdict("anyone@myoffers.example"), None);
    assert_eq!(l.verdict(""), None);
    assert_eq!(l.verdict("no-at-sign"), None);
    // Safe wins over blocked.
    assert!(matches!(l.add(JunkList::Safe, "friend@offers.example"), Added::Added(_)));
    assert_eq!(l.verdict("friend@offers.example"), Some(JunkList::Safe));
    assert_eq!(l.verdict("stranger@offers.example"), Some(JunkList::Blocked));
}

#[test]
fn an_entry_is_on_one_list_and_a_list_has_a_limit() {
    let mut l = JunkLists::default();
    assert_eq!(l.add(JunkList::Safe, "Ada@example.org"), Added::Added("ada@example.org".into()));
    assert_eq!(l.add(JunkList::Safe, "ada@example.org"), Added::There("ada@example.org".into()));
    assert_eq!(l.add(JunkList::Blocked, "ada@example.org"), Added::Added("ada@example.org".into()));
    assert!(l.safe.is_empty());
    assert_eq!(l.add(JunkList::Blocked, "not an address"), Added::Invalid);
    assert!(l.remove(JunkList::Blocked, " ADA@example.org "));
    assert!(!l.remove(JunkList::Blocked, "ada@example.org"));
    for i in 0..MAX_JUNK_ENTRIES {
        assert!(matches!(l.add(JunkList::Blocked, &format!("s{i}@example.org")), Added::Added(_)));
    }
    assert_eq!(l.add(JunkList::Blocked, "one-more@example.org"), Added::Full);
    assert_eq!(l.blocked.len(), MAX_JUNK_ENTRIES);
}

#[test]
fn a_file_s_lists_are_tidied_when_repaired() {
    let mut mb = Mailbox::default();
    mb.junk.safe = vec!["Ada@Example.org".into(), "ada@example.org".into(), "rubbish".into()];
    mb.junk.blocked = (0..MAX_JUNK_ENTRIES + 20).map(|i| format!("s{i}@example.org")).chain(["ada@example.org".to_string()]).collect();
    let fixed = mb.repair();
    assert!(fixed.iter().any(|f| f.contains("Safe and Blocked")), "{fixed:?}");
    assert_eq!(mb.junk.safe, ["ada@example.org"]);
    assert_eq!(mb.junk.blocked.len(), MAX_JUNK_ENTRIES);
    assert!(!mb.junk.repair());
    // Kept in the file, and read back.
    let json = serde_json::to_string(&mb).unwrap();
    let back: Mailbox = serde_json::from_str(&json).unwrap();
    assert_eq!(back.junk, mb.junk);
    // A mailbox without lists writes nothing of them.
    assert!(!serde_json::to_string(&Mailbox::default()).unwrap().contains("junk"));
}

#[test]
fn arriving_mail_is_sorted_by_the_lists() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let account = mb.accounts[0].id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let junk = mb.folder_of_kind(account, FolderKind::Junk).unwrap();
    let mail = |folder, from: &str| Message { folder, from: Address::new("", from), subject: "Hello".into(), ..Default::default() };
    let blocked = mb.add_message(mail(inbox, "spam@example.net")).unwrap();
    let safe = mb.add_message(mail(junk, "friend@example.com")).unwrap();
    // No lists: nothing moves.
    assert!(!mb.sort_junk(blocked) && !mb.sort_junk(safe));
    mb.junk.add(JunkList::Blocked, "spam@example.net");
    mb.junk.add(JunkList::Safe, "@example.com");
    assert!(mb.sort_junk(blocked));
    assert_eq!(mb.message(blocked).unwrap().folder, junk);
    assert!(mb.sort_junk(safe));
    assert_eq!(mb.message(safe).unwrap().folder, inbox);
    // Only from the Inbox to Junk and back: a blocked sender's mail elsewhere stays.
    let sent = mb.folder_of_kind(account, FolderKind::Sent).unwrap();
    let elsewhere = mb.add_message(mail(sent, "spam@example.net")).unwrap();
    assert!(!mb.sort_junk(elsewhere));
    assert!(!mb.sort_junk(9_999_999));
}

#[test]
fn an_international_domain_is_the_same_in_its_own_letters_and_in_xn_form() {
    assert_eq!(junk_key("Ada@Bücher.Example"), "ada@xn--bcher-kva.example");
    assert_eq!(junk_key("@xn--bcher-kva.example"), "@xn--bcher-kva.example");
    let mut l = JunkLists::default();
    assert!(matches!(l.add(JunkList::Blocked, "@bücher.example"), Added::Added(_)));
    assert_eq!(l.verdict("shop@xn--bcher-kva.example"), Some(JunkList::Blocked));
    assert_eq!(l.verdict("shop@BÜCHER.example"), Some(JunkList::Blocked));
    // A fully qualified name's one trailing dot is the same domain.
    assert_eq!(junk_key("Ada@Example.org."), "ada@example.org");
    assert_eq!(l.verdict("shop@bücher.example."), Some(JunkList::Blocked));
    // One entry, whichever way it is written, on one list.
    assert!(matches!(l.add(JunkList::Blocked, "@xn--bcher-kva.example"), Added::There(_)));
    assert!(matches!(l.add(JunkList::Safe, "@XN--BCHER-KVA.example"), Added::Added(_)));
    assert!(l.blocked.is_empty());
    assert!(l.remove(JunkList::Safe, "@bücher.example"));
    // And when a file has both, repair keeps one.
    l.safe = vec!["@bücher.example".into(), "@xn--bcher-kva.example".into()];
    l.blocked = vec!["@xn--bcher-kva.example".into()];
    assert!(l.repair());
    assert_eq!((l.safe.len(), l.blocked.len()), (1, 0));
}

#[test]
fn a_shared_mailbox_s_mail_is_never_sorted() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let account = mb.accounts[0].id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mb.junk.add(JunkList::Blocked, "@vendor.example");
    let id = mb.add_message(Message { folder: inbox, from: Address::new("", "sales@vendor.example"), ..Default::default() }).unwrap();
    mb.accounts[0].shared_of = Some(9_999);
    assert!(!mb.sort_junk(id));
    assert_eq!(mb.message(id).unwrap().folder, inbox);
    mb.accounts[0].shared_of = None;
    assert!(mb.sort_junk(id));
}
