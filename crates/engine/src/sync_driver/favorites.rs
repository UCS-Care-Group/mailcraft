// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Outlook's Favorites read from the server (for an account that signs in with Microsoft), the
//! decisions and the state, with no I/O: when an account's Favorites are due, whether the token
//! in hand is good for them, what the owner is told under the Favorites heading and which links
//! come with it, and which kind of account each is. A front end hands the host the read when
//! [`FavoritesDriver::wanted`] says so and feeds what came back to
//! [`FavoritesDriver::took_in`]. All of it is for the run: nothing here is saved.

use std::collections::{HashMap, HashSet};

use mailcraft_model::remote::FavoritesOutcome;
use mailcraft_model::{Directory, Id};

use crate::Session;

/// What a sign-in for the Favorites says when it is not the owner's to turn into a yes: an
/// administrator of the organisation has to approve it first; the organisation doesn't have it
/// at all; this kind of account has no such permission. The host answers a sign-in request that
/// has `favorites` with exactly one of these as its error, and the pane shows it (in the
/// interface language).
pub const NEEDS_APPROVAL: &str = "Your organisation's administrator has to approve this once";
/// The organisation doesn't have the permission at all.
pub const REFUSED_BY_ORGANISATION: &str = "Not allowed by your organisation";
/// This kind of account has no such permission.
pub const REFUSED_FOR_ACCOUNT: &str = "Not available for this account";
/// What the owner is told, once, of a mailbox whose file had them read another way
/// (`Mailbox::take_retired_favorites_choices`): Exchange Web Services, or not at all.
pub const FAVORITES_NOW_GRAPH: &str =
    "Outlook's Favorites are now read through Microsoft Graph. To show MailCraft's own instead, turn them off in File › Options.";

/// What the pane says under the Favorites heading, when there is something to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// Outlook's Favorites are not shown, and could be: the offer to show them.
    Offer,
    /// The owner asked, and an administrator of the organisation has to approve it first.
    Approval,
    /// The owner asked, and they are not to be had: why, in a line.
    Refused(String),
    /// The last read came to nothing: why, in a few plain words.
    Failed(String),
}

/// A link under a quiet line: what the owner can do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Copy a message for an administrator, with the address they approve it at.
    CopyMessage,
    /// Open the page that says what the owner and an administrator can do.
    Help,
}

impl Step {
    /// What the control channel calls it.
    pub fn code(self) -> &'static str {
        match self {
            Step::CopyMessage => "copyMessage",
            Step::Help => "help",
        }
    }

    /// What the link says (to be put into the interface language).
    pub fn label(self) -> &'static str {
        match self {
            Step::CopyMessage => "Copy a message for my administrator",
            Step::Help => "What to do next",
        }
    }

    /// And what it does, for whoever asks before clicking.
    pub fn about(self) -> &'static str {
        match self {
            Step::CopyMessage => {
                "Copies a short message with the address of Microsoft's page where an administrator of your organisation approves MailCraft, once for everybody. Nothing is sent: paste it into mail or chat yourself."
            }
            Step::Help => {
                "Says what you and your organisation's administrator can do, and lets an administrator approve MailCraft on Microsoft's page from here."
            }
        }
    }
}

impl Note {
    /// What the control channel calls it.
    pub fn code(&self) -> &'static str {
        match self {
            Note::Offer => "offer",
            Note::Approval => "approval",
            Note::Refused(_) => "refused",
            Note::Failed(_) => "failed",
        }
    }

    /// What the line says (to be put into the interface language).
    pub fn text(&self) -> &str {
        match self {
            Note::Offer => "Show Outlook's Favorites",
            Note::Approval => NEEDS_APPROVAL,
            Note::Refused(why) | Note::Failed(why) => why,
        }
    }

    /// The links under the line: what the owner can do about it.
    pub fn steps(&self) -> Vec<Step> {
        match self {
            Note::Approval => vec![Step::CopyMessage, Step::Help],
            Note::Offer | Note::Refused(_) | Note::Failed(_) => Vec::new(),
        }
    }
}

/// What the driver remembers of keeping the Favorites current.
#[derive(Debug, Default)]
pub struct FavoritesDriver {
    /// When each account's Favorites were last asked for (milliseconds).
    asked: HashMap<Id, f64>,
    /// The accounts whose sign-in was found not to cover reading them, and when (milliseconds).
    pub missing: HashMap<Id, f64>,
    /// What there is to say under the heading, account by account.
    pub notes: HashMap<Id, Note>,
    /// The accounts whose Graph token in memory is known not to be good for reading them (it
    /// was asked for without those permissions): a job that wants them gets a new one.
    uncovered: HashSet<Id>,
    /// The accounts whose sign-in has a token that covers Outlook's Favorites, or whose owner
    /// took the offer to show them.
    pub connected: HashSet<Id>,
    /// The accounts whose Favorites are read at the next chance, whatever the clock says: the
    /// owner asked.
    due: HashSet<Id>,
    /// Which kind of Microsoft account each is, as a sign-in of this run said, and for an
    /// organisation's the directory it named (for the link an administrator is sent).
    pub directories: HashMap<Id, Directory>,
    /// The directory an organisation's sign-in named.
    pub tenants: HashMap<Id, String>,
}

impl FavoritesDriver {
    /// Account removal: nothing of these accounts' Favorites is kept.
    pub fn forget_accounts(&mut self, gone: &std::collections::HashSet<Id>) {
        self.asked.retain(|account, _| !gone.contains(account));
        self.missing.retain(|account, _| !gone.contains(account));
        self.notes.retain(|account, _| !gone.contains(account));
        self.uncovered.retain(|account| !gone.contains(account));
        self.connected.retain(|account| !gone.contains(account));
        self.due.retain(|account| !gone.contains(account));
        self.directories.retain(|account, _| !gone.contains(account));
        self.tenants.retain(|account, _| !gone.contains(account));
    }

    /// Whether the Favorites of `account` are due at `now`, without noting anything.
    pub fn are_due(&self, account: Id, now: f64, asked_for: bool) -> bool {
        // Told that the sign-in doesn't cover them: not again that day, unless the owner asks.
        super::favorites_due(self.due.contains(&account), asked_for, self.asked.get(&account).copied(), self.missing.get(&account).copied(), now)
    }

    /// Whether the Favorites of `account` are due at `now`: the owner asked (`asked_for`, or
    /// earlier), or a quarter of an hour has gone by, and the server didn't say today that the
    /// sign-in doesn't cover them. Notes that they were asked for when they are.
    pub fn wanted(&mut self, account: Id, now: f64, asked_for: bool) -> bool {
        let read = self.are_due(account, now, asked_for);
        if read {
            self.asked.insert(account, now);
            self.due.remove(&account);
        }
        read
    }

    /// Whether the Graph token in memory for `account` is known not to be good for the Favorites.
    pub fn is_uncovered(&self, account: Id) -> bool {
        self.uncovered.contains(&account)
    }

    /// The token handed to a job for `account` was not good for the Favorites, or is known to
    /// be good for them (`covered`): noted.
    pub fn token_covers(&mut self, account: Id, covered: bool) {
        if covered {
            self.uncovered.remove(&account);
        } else {
            self.uncovered.insert(account);
        }
    }

    /// A Graph job said whether its token covers the Favorites (`covered`, when it said).
    pub fn job_said(&mut self, account: Id, covered: Option<bool>) {
        match covered {
            Some(true) => {
                self.connected.insert(account);
                self.uncovered.remove(&account);
            }
            Some(false) => {
                self.uncovered.insert(account);
            }
            None => {}
        }
    }

    /// A sign-in of `account` named the directory `tenant` (an identity token's): that says
    /// which kind of account it is. Kept for the run, and not written into the mailbox.
    pub fn note_directory(&mut self, account: Id, tenant: &str) {
        let tenant = tenant.trim();
        let kind = Directory::of_tenant(tenant);
        if account == 0 || kind == Directory::Unknown {
            return;
        }
        self.directories.insert(account, kind);
        if kind == Directory::Organisation && tenant.len() <= 64 {
            self.tenants.insert(account, tenant.to_string());
        } else {
            self.tenants.remove(&account);
        }
    }

    /// The Favorites of `account` are read at the next chance, whatever the clock says and
    /// whatever was said before: the owner asked. (The calendar's own job carries the read
    /// through Microsoft Graph, so the caller makes that due too.)
    pub fn owner_asked(&mut self, account: Id) {
        self.due.insert(account);
        self.missing.remove(&account);
    }

    /// What was remembered of an account that is gone.
    pub fn forget(&mut self, account: Id) {
        self.asked.remove(&account);
        self.missing.remove(&account);
        self.notes.remove(&account);
        self.uncovered.remove(&account);
        self.connected.remove(&account);
        self.due.remove(&account);
        self.directories.remove(&account);
        self.tenants.remove(&account);
    }

    /// The sign-in for the Favorites of `account` came back with tokens that are good for
    /// them: nothing stands in the way any more (the caller has them read at once).
    pub fn signed_in(&mut self, account: Id) {
        self.connected.insert(account);
        self.notes.remove(&account);
        self.uncovered.remove(&account);
    }

    /// The sign-in for the Favorites of `account` came to nothing: `error` is why, `directory`
    /// what kind of account it is. When the no is not the owner's to change ([`NEEDS_APPROVAL`],
    /// [`REFUSED_BY_ORGANISATION`], [`REFUSED_FOR_ACCOUNT`]) the pane says so, nothing is asked
    /// again that day, and `true` comes back; anything else (the owner said no, the page was
    /// closed) is for the status bar only, and the offer stands.
    pub fn refused(&mut self, account: Id, error: &str, directory: Directory, now: f64) -> bool {
        let note = match error {
            NEEDS_APPROVAL => Note::Approval,
            REFUSED_BY_ORGANISATION | REFUSED_FOR_ACCOUNT => Note::Refused(error.to_string()),
            _ => return false,
        };
        // (Only an organisation has an administrator to ask: an account that is told to find
        // one is an organisation's, whatever else was known of it.)
        if error == NEEDS_APPROVAL && directory == Directory::Unknown {
            self.directories.insert(account, Directory::Organisation);
        }
        self.notes.insert(account, note);
        self.missing.insert(account, now);
        true
    }

    /// Take in what a read of the Favorites of `account` came to: `why` is what went wrong, in a
    /// few plain words, when it failed; `directory` what kind of account it is; `clock` the
    /// time now. Whether the pane is to say something, and what: `Some(error)` is a refusal to
    /// put in the status bar as [`FavoritesDriver::refused`] says.
    pub fn took_in(&mut self, account: Id, outcome: FavoritesOutcome, why: &str, directory: Directory, clock: f64) -> Option<String> {
        // (By the clock of the look that asked, which is the one the next look goes by.)
        let now = self.asked.get(&account).copied().unwrap_or(clock);
        let mut refusal = None;
        match outcome {
            FavoritesOutcome::Read | FavoritesOutcome::Missing => {
                self.missing.remove(&account);
                self.notes.remove(&account);
            }
            FavoritesOutcome::NotPermitted => {
                self.missing.insert(account, now);
                self.uncovered.insert(account);
                let told = matches!(self.notes.get(&account), Some(Note::Refused(_) | Note::Approval));
                if self.connected.contains(&account) || matches!(why, NEEDS_APPROVAL | REFUSED_BY_ORGANISATION | REFUSED_FOR_ACCOUNT) {
                    // (What the owner was told when he asked stays said.)
                    if !(told && why.trim().is_empty()) {
                        let by = match why {
                            REFUSED_BY_ORGANISATION | REFUSED_FOR_ACCOUNT => why,
                            _ if directory == Directory::Personal => REFUSED_FOR_ACCOUNT,
                            _ => NEEDS_APPROVAL,
                        };
                        refusal = Some(by.to_string());
                    }
                } else if !told {
                    self.notes.insert(account, Note::Offer);
                }
            }
            FavoritesOutcome::Failed => {
                let why = if why.trim().is_empty() { "Couldn't read the list" } else { why.trim() };
                self.notes.insert(account, Note::Failed(why.chars().filter(|c| !c.is_control()).take(60).collect()));
            }
        }
        refusal
    }
}

impl Session {
    /// Whether `account` signs in with Microsoft.
    pub fn is_microsoft(&self, account: Id) -> bool {
        let microsoft = |a: &mailcraft_model::Account| a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft);
        self.mailbox.account(account).is_some_and(microsoft)
    }

    /// Whether the Favorites of `account` are read from its server: it signs in with Microsoft,
    /// and the owner hasn't asked to see the mailbox's own everywhere.
    pub fn reads_favorites_from_server(&self, account: Id) -> bool {
        self.is_microsoft(account) && !self.prefs.own_favorites
    }

    /// Which kind of Microsoft account `account` is: what was written down with it; else what a
    /// sign-in of this run said (the directory an identity token names); else what its sign-in
    /// settings say; else what its address says (Outlook.com and its kin are personal).
    pub fn account_directory(&self, account: Id) -> Directory {
        let Some(owner) = self.mailbox.account(account) else { return Directory::Unknown };
        let Some(oauth) = &owner.oauth else { return Directory::Unknown };
        if oauth.directory != Directory::Unknown {
            return oauth.directory;
        }
        if let Some(said) = self.driver.favorites.directories.get(&account) {
            return *said;
        }
        match Directory::of_tenant(&oauth.tenant) {
            Directory::Unknown => {}
            named => return named,
        }
        let domain = owner.email.rsplit_once('@').map_or("", |(_, domain)| domain);
        let personal = mailcraft_model::provider::by_domain(domain).is_some_and(|preset| preset.service == "Outlook.com");
        if personal { Directory::Personal } else { Directory::Unknown }
    }

    /// What there is to say of the Favorites of `account` under the heading, if anything.
    pub fn favorites_note_of(&self, account: Id) -> Option<Note> {
        if !self.reads_favorites_from_server(account) {
            return None;
        }
        self.driver.favorites.notes.get(&account).cloned()
    }

    /// What the pane says under the Favorites heading: the first account that has something to
    /// say, and what. Nothing while the owner would rather see the mailbox's own favourites.
    pub fn favorites_note(&self) -> Option<(Id, Note)> {
        self.mailbox.accounts.iter().map(|a| a.id).find_map(|id| self.favorites_note_of(id).map(|note| (id, note)))
    }

    /// The account whose Favorites could be Outlook's if the owner connected them: the first
    /// that has the offer standing (File › Options offers it too).
    pub fn favorites_offer(&self) -> Option<Id> {
        self.mailbox.accounts.iter().map(|a| a.id).find(|id| matches!(self.favorites_note_of(*id), Some(Note::Offer | Note::Refused(_))))
    }
}
