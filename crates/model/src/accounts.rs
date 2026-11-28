// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One place to ask which accounts there are, in what order, which one is the default and what
//! each can do (#775).
//!
//! - [`Mailbox::account_rows`] lists them as the folder pane does, [`Mailbox::default_account_row`]
//!   is the default one, and [`Mailbox::account_row`] is the one with an id. They give
//!   [`AccountRow`]s.
//! - What an account can do is a method of [`Account`], one for each thing, worked out from what
//!   the account holds: its kind, its sign-in, whether it is a shared mailbox. What depends on
//!   another account too (a shared mailbox's owner) is a method of [`AccountRow`].
//!
//! New code that needs a list, an account or a yes or no asks here, and doesn't look at
//! [`Account::kind`] or [`Account::oauth`] to find out:
//!
//! ```
//! # let mailbox = mailcraft_model::Mailbox::default();
//! # let in_view: Option<mailcraft_model::AccountRow<'_>> = None;
//! // A list.
//! let todo = mailbox.account_rows().into_iter().filter(|a| a.keeps_tasks());
//! // Is there one.
//! let any = mailbox.account_rows().iter().any(|a| a.opens_shared_mailboxes());
//! // One that can: the one in view if it can, else the first down the folder pane.
//! let sender = in_view.filter(|a| a.sends_mail()).or_else(|| mailbox.account_rows().into_iter().find(|a| a.sends_mail()));
//! # let _ = (todo.count(), any, sender.is_some());
//! ```

use std::ops::Deref;

use crate::{Account, AccountKind, Id, Mailbox, OAuthProvider, mailbox_address};

/// What an account can do, and what is kept with its server: one check for each. Those that were
/// here before are beside the fields they read ([`Account::is_shared`], [`Account::keeps_tasks`],
/// [`Account::keeps_contacts`], [`Account::on_imap_with_microsoft`], [`Account::sends_as`]).
impl Account {
    /// Whether it is kept in step with a server ([`AccountKind::has_server`]).
    pub fn has_server(&self) -> bool {
        self.kind.has_server()
    }

    /// Whether a Send/Receive sends the mail that waits in its Outbox ([`AccountKind::sends_mail`]).
    pub fn sends_mail(&self) -> bool {
        self.kind.sends_mail()
    }

    /// Whether only its mail to keep offline is kept in full, and older mail as headers and a
    /// preview ([`AccountKind::has_offline_window`]).
    pub fn has_offline_window(&self) -> bool {
        self.kind.has_offline_window()
    }

    /// Whether its mail is kept in step through Microsoft Graph: a Microsoft account, a shared
    /// mailbox too. Not an IMAP account that signs in with Microsoft
    /// ([`Account::signs_in_with_microsoft`]), whose mail is kept in step by IMAP.
    pub fn is_graph(&self) -> bool {
        self.kind == AccountKind::Microsoft
    }

    /// Whether it has a sign-in with Microsoft, whatever its kind: that of a Microsoft account, or
    /// of an IMAP account of Outlook.com's or Microsoft 365's. None yet, or one with another
    /// provider, doesn't count. The shared mailboxes the app makes have none of their own: their
    /// owner's ([`AccountRow::owner`]) reads them.
    pub fn signs_in_with_microsoft(&self) -> bool {
        self.oauth.as_ref().is_some_and(|o| o.provider == OAuthProvider::Microsoft)
    }

    /// Whether it signs in with a personal Microsoft account (Outlook.com), which has no shared
    /// mailboxes and no calendars of other people's ([`crate::OAuth::is_personal`]).
    pub fn is_personal_microsoft(&self) -> bool {
        self.signs_in_with_microsoft() && self.oauth.as_ref().is_some_and(|o| o.is_personal(&self.email))
    }

    /// Whether its sign-in with Microsoft keeps what its owner's mailbox holds beside mail: not a
    /// shared mailbox, whose owner's sign-in reads it.
    fn own_microsoft_sign_in(&self) -> bool {
        self.signs_in_with_microsoft() && !self.is_shared()
    }

    /// Whether its categories are read from its server: they come down with its calendar. Whether
    /// changes made here to the list go up too is [`Account::syncs_categories`].
    pub fn keeps_categories(&self) -> bool {
        self.own_microsoft_sign_in()
    }

    /// Whether its own list of categories is kept in step with its server both ways (#661): one
    /// whose mail is kept in step through Graph, not a shared mailbox, which uses its owner's
    /// list. Whether its sign-in covers changes is [`Mailbox::category_writes`].
    pub fn syncs_categories(&self) -> bool {
        self.is_graph() && !self.is_shared()
    }

    /// Whether the working hours its mailbox has set are read from its server.
    pub fn keeps_working_hours(&self) -> bool {
        self.own_microsoft_sign_in()
    }

    /// Whether its server has automatic replies of its owner's to set: not a shared mailbox's, nor
    /// those of an account kept in step over JMAP or with an Exchange server.
    pub fn keeps_auto_replies(&self) -> bool {
        self.own_microsoft_sign_in() && !matches!(self.kind, AccountKind::Jmap | AccountKind::Exchange)
    }

    /// Whether the Favorites of its owner's Outlook can be read from its server. Whether they are
    /// is the owner's choice too.
    pub fn keeps_favorites(&self) -> bool {
        self.signs_in_with_microsoft()
    }

    /// Whether a shared mailbox may be opened under it: an account kept in step through Graph, of
    /// a work or school (a personal Microsoft account has none), and no shared mailbox itself.
    pub fn opens_shared_mailboxes(&self) -> bool {
        // (Personal whoever it signs in with, as `Mailbox::add_shared_mailbox` refuses it.)
        self.is_graph() && !self.is_shared() && self.oauth.as_ref().is_none_or(|o| !o.is_personal(&self.email))
    }

    /// Whether another person's calendar may be opened by address under it: it signs in with
    /// Microsoft, at a work or school, and is no shared mailbox. Its calendar is read through
    /// Graph whatever its mail is kept by.
    pub fn opens_calendars(&self) -> bool {
        self.own_microsoft_sign_in() && !self.is_personal_microsoft()
    }

    /// Whether its Inbox is brought down before the rest of a Send/Receive, and looked at by
    /// itself between two ([`Account::is_graph`]).
    pub fn checks_inbox_first(&self) -> bool {
        self.is_graph()
    }

    /// Whether its signatures are chosen here: an account of a kind this version doesn't know
    /// keeps its own, as its file has them.
    pub fn has_signatures(&self) -> bool {
        self.kind != AccountKind::Unknown
    }

    /// Whether `address` is its own address: the spaces round it and the letter case don't count,
    /// and an empty one is nobody's. Its other sender addresses aren't its own
    /// ([`Account::sends_as`] counts them).
    pub fn has_address(&self, address: &str) -> bool {
        let address = address.trim();
        !address.is_empty() && address.eq_ignore_ascii_case(self.email.trim())
    }
}

/// An account of the mailbox, as [`Mailbox::account_rows`], [`Mailbox::default_account_row`] and
/// [`Mailbox::account_row`] give it: the account itself (a row is used as an [`Account`]), and the
/// mailbox it is in, for what depends on another account. Nothing is copied from the account. (What
/// is asked of the mailbox goes by the account's id: until [`Mailbox::repair`] gives them their own,
/// two accounts with one id answer as the first. [`AccountRow::is_default`] tells them apart.)
#[derive(Clone, Copy)]
pub struct AccountRow<'a> {
    account: &'a Account,
    mailbox: &'a Mailbox,
}

impl<'a> AccountRow<'a> {
    /// The row of `account`, which is one of `mailbox`'s.
    fn new(mailbox: &'a Mailbox, account: &'a Account) -> Self {
        AccountRow { account, mailbox }
    }

    /// The account itself, for as long as the mailbox is borrowed.
    pub fn account(self) -> &'a Account {
        self.account
    }

    /// The account whose sign-in reads the shared mailbox, when it is one ([`Mailbox::owner_of`]).
    pub fn owner(&self) -> Option<AccountRow<'a>> {
        self.mailbox.owner_of(self.id).map(|owner| AccountRow::new(self.mailbox, owner))
    }

    /// The account whose Safe Senders and Blocked Senders, and whose categories, its mail goes by:
    /// itself, or for a shared mailbox the account it names ([`Mailbox::junk_owner`]), which may be
    /// gone: [`AccountRow::owner`] is `None` then.
    pub fn junk_owner(&self) -> Id {
        self.mailbox.junk_owner(self.id)
    }

    /// Whether its server keeps its rules ([`Mailbox::keeps_rules`]).
    pub fn keeps_rules(&self) -> bool {
        self.mailbox.keeps_rules(self.id)
    }

    /// Whether the changes made in it are queued for its server to make
    /// ([`Mailbox::keeps_changes_for_server`]).
    pub fn keeps_changes_for_server(&self) -> bool {
        self.mailbox.keeps_changes_for_server(self.id)
    }

    /// Whether its calendar is kept with Microsoft: it signs in with Microsoft, or it is a shared
    /// mailbox with a calendar the owner ticked (#70), read with its owner's sign-in.
    pub fn keeps_calendar(&self) -> bool {
        let ticked = || self.mailbox.calendars.iter().any(|c| c.origin.account == self.id && c.visible);
        let shared = || self.owner().is_some_and(|owner| owner.oauth.is_some()) && ticked();
        self.signs_in_with_microsoft() || shared()
    }

    /// Whether its mail can be searched on its server: an account kept in step through Graph, and
    /// a shared mailbox only while its owner is here and its address is one.
    pub fn searches_server(&self) -> bool {
        self.is_graph() && (!self.is_shared() || (self.owner().is_some() && mailbox_address(&self.email).is_some()))
    }

    /// Whether it is the default account ([`Mailbox::default_account`]).
    pub fn is_default(&self) -> bool {
        // (The account in its place, not its id: until [`Mailbox::repair`] gives them their own,
        // two accounts of a file may have one id.)
        self.mailbox.default_account().is_some_and(|default| std::ptr::eq(default, self.account))
    }
}

impl Deref for AccountRow<'_> {
    type Target = Account;

    fn deref(&self) -> &Account {
        self.account
    }
}

impl std::fmt::Debug for AccountRow<'_> {
    /// The account, without the mailbox it is in.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AccountRow").field(&self.account).finish()
    }
}

impl Mailbox {
    /// The accounts in the order the folder pane lists them ([`Mailbox::accounts_in_order`]), as
    /// rows. Each account is listed once, whatever it is or can do: a caller filters the rows.
    /// (Where two accounts have one id, until [`Mailbox::repair`] gives them their own, one may be
    /// missing or listed twice.)
    pub fn account_rows(&self) -> Vec<AccountRow<'_>> {
        self.accounts_in_order().into_iter().map(|a| AccountRow::new(self, a)).collect()
    }

    /// The default account ([`Mailbox::default_account`]) as a row: the first added, until one can
    /// be chosen. Whatever it is: code that needs an account that can do something takes the first
    /// of [`Mailbox::account_rows`] that can.
    pub fn default_account_row(&self) -> Option<AccountRow<'_>> {
        self.default_account().map(|a| AccountRow::new(self, a))
    }

    /// The account `id` as a row: [`Mailbox::account`], for a check that depends on another account.
    pub fn account_row(&self, id: Id) -> Option<AccountRow<'_>> {
        self.account(id).map(|a| AccountRow::new(self, a))
    }
}
