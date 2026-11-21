// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a server's mailboxes are for: special-use flags (RFC 6154) first, well-known names second.

use std::collections::HashMap;

use mailcraft_model::FolderKind;
use mailcraft_proto::imap::Mailbox;

/// Whether the mailbox can be opened at all.
pub(crate) fn selectable(mailbox: &Mailbox) -> bool {
    !mailbox.has_flag("\\Noselect") && !mailbox.has_flag("\\NonExistent")
}

/// Whether the mailbox only shows messages that live in other mailboxes (Gmail's "All Mail",
/// "Starred" and "Important"): downloading it would bring every message twice.
fn is_view(mailbox: &Mailbox) -> bool {
    ["\\All", "\\Flagged", "\\Important"].iter().any(|flag| mailbox.has_flag(flag))
}

pub(crate) fn is_inbox(mailbox: &Mailbox) -> bool {
    mailbox.raw.eq_ignore_ascii_case("INBOX")
}

/// The kind the server says the mailbox is.
fn kind_from_flags(mailbox: &Mailbox) -> Option<FolderKind> {
    [
        ("\\Sent", FolderKind::Sent),
        ("\\Drafts", FolderKind::Drafts),
        ("\\Trash", FolderKind::Deleted),
        ("\\Junk", FolderKind::Junk),
        ("\\Archive", FolderKind::Archive),
    ]
    .into_iter()
    .find(|(flag, _)| mailbox.has_flag(flag))
    .map(|(_, kind)| kind)
}

/// The kind a mailbox's name suggests, at the top level or directly under the Inbox (where
/// some servers keep everything).
fn kind_from_name(mailbox: &Mailbox) -> Option<FolderKind> {
    let name = mailbox.name.trim();
    let under_inbox = mailbox.delimiter.and_then(|d| {
        let (first, rest) = name.split_once(d)?;
        first.eq_ignore_ascii_case("INBOX").then_some(rest)
    });
    match under_inbox.unwrap_or(name).to_lowercase().as_str() {
        "sent" | "sent items" | "sent messages" | "sent mail" => Some(FolderKind::Sent),
        "drafts" | "draft" => Some(FolderKind::Drafts),
        "trash" | "deleted items" | "deleted messages" => Some(FolderKind::Deleted),
        "junk" | "spam" | "junk email" | "junk e-mail" => Some(FolderKind::Junk),
        "archive" | "archives" => Some(FolderKind::Archive),
        _ => None,
    }
}

/// The name a mailbox is known by here: decoded, and `INBOX` however the server spells it.
pub(crate) fn remote_name(mailbox: &Mailbox) -> String {
    if is_inbox(mailbox) { "INBOX".to_string() } else { mailbox.name.clone() }
}

/// The mailboxes to synchronise, each with what it is for, the Inbox first. Mailboxes that
/// can't be opened and views of other mailboxes are left out.
///
/// Every standard kind is given to one mailbox at most: the one a local folder of that kind is
/// already `tied` to (so a folder doesn't change its job because the server grew another
/// candidate), else the one the server flags, else the first with a well-known name.
pub(crate) fn plan<'a>(mailboxes: &'a [Mailbox], tied: &[(&str, FolderKind)]) -> Vec<(&'a Mailbox, FolderKind)> {
    let mut plan: Vec<(&Mailbox, FolderKind)> = mailboxes.iter().filter(|m| selectable(m) && !is_view(m)).map(|m| (m, FolderKind::Custom)).collect();
    let mut taken: Vec<FolderKind> = vec![FolderKind::Custom, FolderKind::Outbox];
    let mut give = |guess: &dyn Fn(&Mailbox) -> Option<FolderKind>| {
        for (mailbox, kind) in &mut plan {
            if *kind == FolderKind::Custom
                && let Some(found) = guess(mailbox)
                && !taken.contains(&found)
            {
                *kind = found;
                taken.push(found);
            }
        }
    };
    give(&|m| is_inbox(m).then_some(FolderKind::Inbox));
    // Looked up by name, the first tie of a name being the one: thousands of mailboxes each
    // searching thousands of ties would be millions of comparisons (#991).
    let mut ties: HashMap<&str, FolderKind> = HashMap::with_capacity(tied.len());
    for (remote, kind) in tied {
        ties.entry(*remote).or_insert(*kind);
    }
    give(&|m| ties.get(remote_name(m).as_str()).copied());
    give(&kind_from_flags);
    give(&kind_from_name);
    // The Inbox first, then the other standard folders, then the rest, each group in the server's order.
    plan.sort_by_key(|(_, kind)| match kind {
        FolderKind::Inbox => 0,
        FolderKind::Custom => 2,
        _ => 1,
    });
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox(name: &str, flags: &[&str]) -> Mailbox {
        Mailbox { name: name.into(), raw: name.into(), delimiter: Some('/'), flags: flags.iter().map(|f| f.to_string()).collect() }
    }

    fn kinds(mailboxes: &[Mailbox]) -> Vec<(String, FolderKind)> {
        kinds_with(mailboxes, &[])
    }

    fn kinds_with(mailboxes: &[Mailbox], tied: &[(&str, FolderKind)]) -> Vec<(String, FolderKind)> {
        plan(mailboxes, tied).into_iter().map(|(m, k)| (m.name.clone(), k)).collect()
    }

    fn one(name: &str, flags: &[&str]) -> FolderKind {
        kinds(&[mailbox(name, flags)]).first().map_or(FolderKind::Outbox, |(_, k)| *k)
    }

    #[test]
    fn flags_say_what_a_mailbox_is_for() {
        assert_eq!(one("Gesendet", &["\\HasNoChildren", "\\Sent"]), FolderKind::Sent);
        assert_eq!(one("Entwürfe", &["\\drafts"]), FolderKind::Drafts);
        assert_eq!(one("Papierkorb", &["\\Trash"]), FolderKind::Deleted);
        assert_eq!(one("Werbung", &["\\JUNK"]), FolderKind::Junk);
        assert_eq!(one("Ablage", &["\\Archive"]), FolderKind::Archive);
        assert_eq!(one("Projects", &["\\HasChildren"]), FolderKind::Custom);
    }

    #[test]
    fn well_known_names_say_it_too() {
        for (name, kind) in [
            ("INBOX", FolderKind::Inbox),
            ("inbox", FolderKind::Inbox),
            ("Sent", FolderKind::Sent),
            ("Sent Items", FolderKind::Sent),
            ("Sent Messages", FolderKind::Sent),
            ("INBOX/Sent", FolderKind::Sent),
            ("Drafts", FolderKind::Drafts),
            ("Trash", FolderKind::Deleted),
            ("Deleted Items", FolderKind::Deleted),
            ("Deleted Messages", FolderKind::Deleted),
            ("Junk", FolderKind::Junk),
            ("SPAM", FolderKind::Junk),
            ("Junk Email", FolderKind::Junk),
            ("Archive", FolderKind::Archive),
            ("Sent/2024", FolderKind::Custom),
            ("Projects/Sent", FolderKind::Custom),
            ("Sentinel", FolderKind::Custom),
            ("Outbox", FolderKind::Custom),
            ("", FolderKind::Custom),
        ] {
            assert_eq!(one(name, &[]), kind, "{name}");
        }
        // A server that separates with dots.
        let dotted = Mailbox { name: "INBOX.Trash".into(), raw: "INBOX.Trash".into(), delimiter: Some('.'), flags: Vec::new() };
        assert_eq!(plan(&[dotted], &[])[0].1, FolderKind::Deleted);
    }

    #[test]
    fn a_folder_keeps_the_job_it_already_has() {
        let list = [mailbox("Sent Items", &[]), mailbox("Sent", &[]), mailbox("Verzonden", &["\\Sent"]), mailbox("Spam", &[]), mailbox("inbox", &[])];
        // With nothing tied yet, the flag decides.
        assert_eq!(kinds(&list)[1], ("Verzonden".to_string(), FolderKind::Sent));
        // The local Sent folder is tied to "Sent": it stays the Sent folder, whatever else turns up.
        let tied = [
            ("Sent", FolderKind::Sent),
            ("Spam", FolderKind::Custom),
            ("Gone", FolderKind::Junk),
            ("Verzonden", FolderKind::Inbox),
            ("INBOX", FolderKind::Inbox),
        ];
        assert_eq!(
            kinds_with(&list, &tied),
            [
                ("inbox".to_string(), FolderKind::Inbox),
                ("Sent".to_string(), FolderKind::Sent),
                // A tie as an ordinary folder says nothing; the name still does.
                ("Spam".to_string(), FolderKind::Junk),
                ("Sent Items".to_string(), FolderKind::Custom),
                ("Verzonden".to_string(), FolderKind::Custom),
            ]
        );
        assert_eq!(remote_name(&list[4]), "INBOX");
        assert_eq!(remote_name(&list[0]), "Sent Items");
    }

    #[test]
    fn unusable_mailboxes_and_views_are_left_out() {
        let list = [
            mailbox("[Gmail]", &["\\Noselect", "\\HasChildren"]),
            mailbox("[Gmail]/All Mail", &["\\All"]),
            mailbox("[Gmail]/Starred", &["\\Flagged"]),
            mailbox("[Gmail]/Important", &["\\Important"]),
            mailbox("Gone", &["\\NonExistent"]),
            mailbox("[Gmail]/Sent Mail", &["\\Sent"]),
        ];
        assert_eq!(kinds(&list), [("[Gmail]/Sent Mail".to_string(), FolderKind::Sent)]);
    }

    #[test]
    fn each_kind_is_given_once_and_the_inbox_comes_first() {
        let list = [
            mailbox("Sent", &[]),
            mailbox("Notes", &[]),
            mailbox("Sent Items", &[]),
            mailbox("Verzonden", &["\\Sent"]),
            mailbox("Trash", &[]),
            mailbox("INBOX", &[]),
            mailbox("Inbox", &[]),
        ];
        assert_eq!(
            kinds(&list),
            [
                ("INBOX".to_string(), FolderKind::Inbox),
                // The flag beats the names, whatever the order.
                ("Verzonden".to_string(), FolderKind::Sent),
                ("Trash".to_string(), FolderKind::Deleted),
                ("Sent".to_string(), FolderKind::Custom),
                ("Notes".to_string(), FolderKind::Custom),
                ("Sent Items".to_string(), FolderKind::Custom),
                ("Inbox".to_string(), FolderKind::Custom),
            ]
        );
    }

    proptest::proptest! {
        #[test]
        fn any_listing_gives_each_standard_kind_once(list in proptest::collection::vec(("[a-zA-Z /.]{0,12}", proptest::collection::vec("\\\\(Sent|Trash|Junk|All|Noselect|Drafts|Archive|x)", 0..3)), 0..12)) {
            let list: Vec<Mailbox> = list.iter().map(|(name, flags)| Mailbox { name: name.clone(), raw: name.clone(), delimiter: Some('/'), flags: flags.clone() }).collect();
            let tied = [("Sent", FolderKind::Sent), ("a", FolderKind::Junk), ("", FolderKind::Deleted), ("b", FolderKind::Outbox)];
            let plan = plan(&list, if list.len().is_multiple_of(2) { &tied[..] } else { &[][..] });
            for kind in FolderKind::STANDARD {
                proptest::prop_assert!(plan.iter().filter(|(_, k)| *k == kind).count() <= 1);
            }
            proptest::prop_assert!(plan.len() <= list.len());
        }
    }
}
