// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The accounts in one place (#775): the rows of the list, the default account, and what an
//! account of each kind and sign-in can do, written out.

use crate::*;

fn oauth(provider: OAuthProvider, directory: Directory) -> Option<OAuth> {
    Some(OAuth { provider, directory, ..Default::default() })
}

fn microsoft(directory: Directory) -> Option<OAuth> {
    oauth(OAuthProvider::Microsoft, directory)
}

/// An account at `email` of `kind`, signed in with `oauth`.
fn account(email: &str, kind: AccountKind, oauth: Option<OAuth>) -> Account {
    Account { email: email.into(), kind, oauth, ..Default::default() }
}

/// Every check of `accounts.rs` that answers yes or no, by name.
type Check = (&'static str, fn(&AccountRow<'_>) -> bool);
const CHECKS: [Check; 19] = [
    ("has_server", |a| a.has_server()),
    ("sends_mail", |a| a.sends_mail()),
    ("is_graph", |a| a.is_graph()),
    ("signs_in_with_microsoft", |a| a.signs_in_with_microsoft()),
    ("is_personal_microsoft", |a| a.is_personal_microsoft()),
    ("keeps_calendar", |a| a.keeps_calendar()),
    ("keeps_categories", |a| a.keeps_categories()),
    ("syncs_categories", |a| a.syncs_categories()),
    ("keeps_working_hours", |a| a.keeps_working_hours()),
    ("keeps_rules", |a| a.keeps_rules()),
    ("keeps_auto_replies", |a| a.keeps_auto_replies()),
    ("keeps_favorites", |a| a.keeps_favorites()),
    ("opens_shared_mailboxes", |a| a.opens_shared_mailboxes()),
    ("opens_calendars", |a| a.opens_calendars()),
    ("has_offline_window", |a| a.has_offline_window()),
    ("checks_inbox_first", |a| a.checks_inbox_first()),
    ("searches_server", |a| a.searches_server()),
    ("keeps_changes_for_server", |a| a.keeps_changes_for_server()),
    ("has_signatures", |a| a.has_signatures()),
];

/// The checks that say yes of `row`, in the order above.
fn can(row: &AccountRow<'_>) -> Vec<&'static str> {
    CHECKS.iter().filter(|(_, check)| check(row)).map(|(name, _)| *name).collect()
}

/// The checks that say yes of `account`, alone in a mailbox.
fn alone(account: Account) -> Vec<&'static str> {
    let mut mb = Mailbox::default();
    let id = mb.add_account(account);
    can(&mb.account_row(id).unwrap())
}

/// The kinds of account.
const KINDS: [AccountKind; 6] =
    [AccountKind::Local, AccountKind::Imap, AccountKind::Microsoft, AccountKind::Jmap, AccountKind::Exchange, AccountKind::Unknown];

/// What an account of each kind can do, with no sign-in with Microsoft.
#[test]
fn each_kind_of_account_can_do_what_the_table_says() {
    let expected: [&[&str]; KINDS.len()] = [
        // On this computer.
        &["has_signatures"],
        // IMAP.
        &["has_server", "sends_mail", "has_signatures"],
        // Microsoft. (It signs in with Microsoft: one read from a file may not say so.)
        &[
            "has_server",
            "sends_mail",
            "is_graph",
            "syncs_categories",
            "keeps_rules",
            "opens_shared_mailboxes",
            "has_offline_window",
            "checks_inbox_first",
            "searches_server",
            "keeps_changes_for_server",
            "has_signatures",
        ],
        // JMAP.
        &["has_server", "sends_mail", "has_signatures"],
        // An Exchange server.
        &["has_server", "sends_mail", "has_offline_window", "keeps_changes_for_server", "has_signatures"],
        // A kind this version doesn't know.
        &[],
    ];
    for (kind, expected) in KINDS.into_iter().zip(expected) {
        assert_eq!(alone(account("ada@example.org", kind, None)), expected, "{kind:?}");
    }
}

/// What signing in with Microsoft adds: to a Microsoft account of a work or school, and to an
/// IMAP account, whose calendar, contacts and to-dos Microsoft keeps and whose mail it doesn't.
#[test]
fn an_account_that_signs_in_with_microsoft_can_do_what_the_table_says() {
    assert_eq!(
        alone(account("ada@example.org", AccountKind::Microsoft, microsoft(Directory::Organisation))),
        [
            "has_server",
            "sends_mail",
            "is_graph",
            "signs_in_with_microsoft",
            "keeps_calendar",
            "keeps_categories",
            "syncs_categories",
            "keeps_working_hours",
            "keeps_rules",
            "keeps_auto_replies",
            "keeps_favorites",
            "opens_shared_mailboxes",
            "opens_calendars",
            "has_offline_window",
            "checks_inbox_first",
            "searches_server",
            "keeps_changes_for_server",
            "has_signatures",
        ]
    );
    let imap = |directory| alone(account("ada@example.org", AccountKind::Imap, microsoft(directory)));
    assert_eq!(
        imap(Directory::Organisation),
        [
            "has_server",
            "sends_mail",
            "signs_in_with_microsoft",
            "keeps_calendar",
            "keeps_categories",
            "keeps_working_hours",
            "keeps_auto_replies",
            "keeps_favorites",
            "opens_calendars",
            "has_signatures",
        ]
    );
    assert_eq!(
        imap(Directory::Personal),
        [
            "has_server",
            "sends_mail",
            "signs_in_with_microsoft",
            "is_personal_microsoft",
            "keeps_calendar",
            "keeps_categories",
            "keeps_working_hours",
            "keeps_auto_replies",
            "keeps_favorites",
            "has_signatures",
        ]
    );
}

/// Signing in with Google is not signing in with Microsoft, whatever the address is.
#[test]
fn an_account_that_signs_in_with_google_is_an_imap_account() {
    for email in ["ada@gmail.com", "ada@outlook.com"] {
        assert_eq!(
            alone(account(email, AccountKind::Imap, oauth(OAuthProvider::Google, Directory::Unknown))),
            ["has_server", "sends_mail", "has_signatures"],
            "{email}"
        );
    }
}

/// A sign-in with Microsoft that a file gives an account kept in step over JMAP or with an
/// Exchange server: no automatic replies of Microsoft's.
#[test]
fn a_sign_in_with_microsoft_of_another_servers_account_has_no_automatic_replies() {
    for kind in [AccountKind::Jmap, AccountKind::Exchange] {
        let mut mb = Mailbox::default();
        let id = mb.add_account(account("ada@example.org", kind, microsoft(Directory::Organisation)));
        let row = mb.account_row(id).unwrap();
        assert!(row.signs_in_with_microsoft() && !row.keeps_auto_replies(), "{kind:?}");
    }
}

/// A personal Microsoft account is told by the directory that signed it in, by the directory its
/// sign-in settings name, or, when neither says, by its address. It opens nothing of other
/// people's.
#[test]
fn a_personal_microsoft_account_is_told_three_ways() {
    let personal = |email: &str, directory, tenant: &str| {
        let oauth = OAuth { provider: OAuthProvider::Microsoft, directory, tenant: tenant.into(), ..Default::default() };
        let mut mb = Mailbox::default();
        let id = mb.add_account(account(email, AccountKind::Microsoft, Some(oauth)));
        let row = mb.account_row(id).unwrap();
        let is = row.is_personal_microsoft();
        assert_eq!((row.opens_shared_mailboxes(), row.opens_calendars()), (!is, !is), "{email} {directory:?} {tenant}");
        is
    };
    assert!(personal("ada@example.org", Directory::Personal, ""));
    assert!(personal("ada@example.org", Directory::Unknown, "consumers"));
    assert!(personal("ada@outlook.com", Directory::Unknown, "common"));
    assert!(!personal("ada@outlook.com", Directory::Organisation, ""));
    assert!(!personal("ada@outlook.com", Directory::Unknown, "organizations"));
    assert!(!personal("ada@example.org", Directory::Unknown, ""));
}

/// What is said to open shared mailboxes is what opens one, whatever a file says of its sign-in.
#[test]
fn what_opens_shared_mailboxes_is_what_opens_one() {
    let signed_in = [OAuthProvider::Microsoft, OAuthProvider::Google].map(Some).into_iter().chain([None]);
    for provider in signed_in {
        for kind in KINDS {
            for directory in [Directory::Unknown, Directory::Organisation, Directory::Personal] {
                for email in ["ada@example.org", "ada@outlook.com"] {
                    let mut mb = Mailbox::default();
                    let id = mb.add_account(account(email, kind, provider.and_then(|p| oauth(p, directory))));
                    let said = mb.account_row(id).unwrap().opens_shared_mailboxes();
                    assert_eq!(said, mb.add_shared_mailbox(id, "team@example.org").is_ok(), "{kind:?} {provider:?} {directory:?} {email}");
                }
            }
        }
    }
    let (mut mb, _, shared) = with_shared();
    assert!(!mb.account_row(shared).unwrap().opens_shared_mailboxes() && mb.add_shared_mailbox(shared, "desk@example.org").is_err());
}

/// A work account with a shared mailbox opened through it, and their ids.
fn with_shared() -> (Mailbox, Id, Id) {
    let mut mb = Mailbox::default();
    let owner = mb.add_account(account("ada@example.org", AccountKind::Microsoft, microsoft(Directory::Organisation)));
    let shared = mb.add_shared_mailbox(owner, "team@example.org").unwrap();
    (mb, owner, shared)
}

/// What a shared mailbox can do: its mail goes through Graph, and what its owner's sign-in keeps
/// is its owner's, not its own.
#[test]
fn a_shared_mailbox_can_do_what_the_table_says() {
    let (mb, owner, shared) = with_shared();
    let row = mb.account_row(shared).unwrap();
    assert_eq!(
        can(&row),
        [
            "has_server",
            "sends_mail",
            "is_graph",
            "has_offline_window",
            "checks_inbox_first",
            "searches_server",
            "keeps_changes_for_server",
            "has_signatures"
        ]
    );
    assert_eq!(row.owner().map(|o| o.id), Some(owner));
    assert_eq!(row.junk_owner(), owner);
    let own = mb.account_row(owner).unwrap();
    assert!(own.owner().is_none());
    assert_eq!(own.junk_owner(), owner);
}

/// A shared mailbox whose owner can't be one: nobody's sign-in reads it, and its lists go by the
/// account it names all the same.
#[test]
fn a_shared_mailbox_whose_owner_is_no_owner_is_read_by_nobody() {
    let (mut mb, owner, shared) = with_shared();
    let gone = 424_242;
    mb.account_mut(shared).unwrap().shared_of = Some(gone);
    assert_eq!((mb.account_row(shared).unwrap().owner().is_none(), mb.account_row(shared).unwrap().junk_owner()), (true, gone));
    mb.account_mut(shared).unwrap().shared_of = Some(shared);
    assert_eq!((mb.account_row(shared).unwrap().owner().is_none(), mb.account_row(shared).unwrap().junk_owner()), (true, shared));
    mb.account_mut(shared).unwrap().shared_of = Some(owner);
    for kind in KINDS.into_iter().filter(|kind| *kind != AccountKind::Microsoft) {
        mb.account_mut(owner).unwrap().kind = kind;
        let row = mb.account_row(shared).unwrap();
        assert_eq!((row.owner().is_none(), row.junk_owner()), (true, owner), "{kind:?}");
    }
}

/// A shared mailbox that a file gives a sign-in of its own has what any sign-in with Microsoft
/// brings, and still nothing that is its owner's to have.
#[test]
fn a_shared_mailbox_with_a_sign_in_of_its_own_has_nothing_of_its_owners() {
    let (mut mb, _, shared) = with_shared();
    mb.account_mut(shared).unwrap().oauth = microsoft(Directory::Organisation);
    assert_eq!(
        can(&mb.account_row(shared).unwrap()),
        [
            "has_server",
            "sends_mail",
            "is_graph",
            "signs_in_with_microsoft",
            "keeps_calendar",
            "keeps_favorites",
            "has_offline_window",
            "checks_inbox_first",
            "searches_server",
            "keeps_changes_for_server",
            "has_signatures",
        ]
    );
}

/// A shared mailbox's calendar is kept in step while one of its calendars is ticked and the
/// account it is opened through is here and signs in.
#[test]
fn a_shared_mailboxs_calendar_is_kept_once_it_is_ticked() {
    let (mut mb, owner, shared) = with_shared();
    let calendar =
        |account, visible| Calendar { name: "Team".into(), visible, origin: Origin { account, ..Default::default() }, ..Default::default() };
    let kept = |mb: &Mailbox| mb.account_row(shared).unwrap().keeps_calendar();
    assert!(!kept(&mb));
    mb.calendars.push(calendar(owner, true));
    assert!(!kept(&mb), "its owner's calendar is not its own");
    mb.calendars.push(calendar(shared, false));
    assert!(!kept(&mb), "not ticked");
    mb.calendars.push(calendar(shared, true));
    assert!(kept(&mb));
    mb.account_mut(owner).unwrap().oauth = None;
    assert!(!kept(&mb), "nobody signs in for it");
    mb.account_mut(owner).unwrap().oauth = oauth(OAuthProvider::Google, Directory::Unknown);
    assert!(kept(&mb), "its owner's sign-in is all that is asked");
    mb.account_mut(owner).unwrap().oauth = microsoft(Directory::Organisation);
    mb.account_mut(shared).unwrap().shared_of = Some(424_242);
    assert!(!kept(&mb), "its owner is gone");
}

/// A shared mailbox is searched on its server while its owner is here and its address is one.
#[test]
fn a_shared_mailbox_is_searched_on_its_server_through_its_owner() {
    let (mut mb, owner, shared) = with_shared();
    let searched = |mb: &Mailbox| mb.account_row(shared).unwrap().searches_server();
    assert!(searched(&mb));
    mb.account_mut(shared).unwrap().email = "not an address".into();
    assert!(!searched(&mb));
    mb.account_mut(shared).unwrap().email = "team@example.org".into();
    // Opened through itself, through an account that is no Microsoft account, or through none.
    for through in [shared, 424_242] {
        mb.account_mut(shared).unwrap().shared_of = Some(through);
        assert!(!searched(&mb) && mb.account_row(shared).unwrap().owner().is_none(), "{through}");
    }
    mb.account_mut(shared).unwrap().shared_of = Some(owner);
    mb.account_mut(owner).unwrap().kind = AccountKind::Imap;
    assert!(!searched(&mb));
}

/// The checks by an account's id say what the account says, and nothing of an account that isn't
/// here.
#[test]
fn the_checks_by_id_say_what_the_account_says() {
    let mut mb = Mailbox::default();
    let ids = KINDS.map(|kind| mb.add_account(account("ada@example.org", kind, None)));
    for id in ids {
        let a = mb.account(id).unwrap();
        assert_eq!(mb.is_graph(id), a.is_graph(), "{:?}", a.kind);
        let row = mb.account_row(id).unwrap();
        assert_eq!(mb.keeps_rules(id), row.keeps_rules(), "{:?}", a.kind);
        assert_eq!(mb.keeps_changes_for_server(id), row.keeps_changes_for_server(), "{:?}", a.kind);
    }
    assert!(!mb.is_graph(424_242) && !mb.keeps_rules(424_242) && !mb.keeps_changes_for_server(424_242));
    assert!(mb.account_row(424_242).is_none());
}

/// An account's own address is told whatever the letter case and the spaces round it, and an
/// empty one is nobody's. Its other sender addresses aren't its own, and it sends as them too.
#[test]
fn an_accounts_own_address_is_told_whatever_the_case() {
    let mut ada = account(" Ada@Example.org ", AccountKind::Imap, None);
    ada.sender_addresses = vec![SenderAddress { email: "support@example.org".into(), ..Default::default() }];
    for address in ["ada@example.org", "ADA@EXAMPLE.ORG", "  ada@example.org\t"] {
        assert!(ada.has_address(address) && ada.sends_as(address), "{address:?}");
    }
    for address in ["", "  ", "ada", "ada+news@example.org", "ada@example.com"] {
        assert!(!ada.has_address(address) && !ada.sends_as(address), "{address:?}");
    }
    assert!(!ada.has_address("support@example.org") && ada.sends_as("Support@example.org"));
    let nobody = account("  ", AccountKind::Imap, None);
    assert!(!nobody.has_address("") && !nobody.has_address("  ") && !nobody.sends_as(""));
}

/// A row is its account, and the default account is the first added wherever the owner puts it in
/// the folder pane: one row says so, and only one.
#[test]
fn one_row_is_the_default_account() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "a@example.org");
    let a = mb.accounts[0].id;
    let b = mb.add_account(account("b@example.org", AccountKind::Microsoft, microsoft(Directory::Organisation)));
    let shared = mb.add_shared_mailbox(b, "team@example.org").unwrap();
    let defaults = |mb: &Mailbox| mb.account_rows().iter().filter(|row| row.is_default()).map(|row| row.id).collect::<Vec<_>>();
    assert_eq!(defaults(&mb), [a]);
    mb.move_account(a, None).unwrap();
    assert_eq!(mb.account_rows().iter().map(|row| row.id).collect::<Vec<_>>(), [b, shared, a]);
    assert_eq!(defaults(&mb), [a]);
    assert!(mb.default_account_row().is_some_and(|row| row.id == a && row.is_default()));
    assert_eq!(mb.account_row(b).map(|row| (row.email.clone(), row.is_default())), Some(("b@example.org".into(), false)));
    for row in mb.account_rows() {
        assert!(std::ptr::eq(row.account(), mb.account(row.id).unwrap()));
        let shown = format!("{row:?}");
        assert!(mb.accounts.iter().all(|a| shown.contains(&a.email) == (a.id == row.id)), "the account, and not the mailbox: {shown}");
    }
    // Two accounts with one id, as a file may have them: the first added is the default.
    mb.accounts[1].id = a;
    assert_eq!(mb.account_rows().iter().filter(|row| row.is_default()).map(|row| row.email.clone()).collect::<Vec<_>>(), ["a@example.org"]);
    mb.accounts.clear();
    assert!(mb.default_account_row().is_none() && mb.account_rows().is_empty());
}

/// The rows are the accounts of the folder pane's list, in its order, and the default account's
/// row is the default account: after a move, with a shared mailbox whose owner is gone, and with
/// two accounts of one id.
#[test]
fn the_rows_are_the_accounts_in_order() {
    let same = |mb: &Mailbox, when: &str| {
        let (rows, accounts) = (mb.account_rows(), mb.accounts_in_order());
        assert_eq!(rows.len(), accounts.len(), "{when}");
        assert!(rows.iter().zip(&accounts).all(|(row, a)| std::ptr::eq(row.account(), *a)), "{when}");
        match (mb.default_account_row(), mb.default_account()) {
            (Some(row), Some(a)) => assert!(std::ptr::eq(row.account(), a), "{when}"),
            (row, a) => assert!(row.is_none() && a.is_none(), "{when}"),
        }
    };
    same(&Mailbox::default(), "no account");
    let (mut mb, owner, shared) = with_shared();
    let last = mb.add_account(account("b@example.org", AccountKind::Imap, None));
    same(&mb, "as added");
    mb.move_account(last, Some(owner)).unwrap();
    same(&mb, "moved");
    assert_eq!(mb.account_rows().iter().map(|row| row.id).collect::<Vec<_>>(), [last, owner, shared]);
    assert_eq!(mb.default_account_row().map(|row| row.id), Some(owner));
    mb.account_mut(shared).unwrap().shared_of = Some(424_242);
    same(&mb, "its owner is gone");
    mb.accounts[2].id = owner;
    same(&mb, "one id twice");
}
