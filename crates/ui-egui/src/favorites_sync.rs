//! Keeping the Favorites of the folder pane the ones Outlook shows, for an account that signs
//! in with Microsoft: read from its server, never written there.
//!
//! There are two ways of reading them, and for an account of an organisation (work or school)
//! its owner chooses which (`ui.favoritesSource`, which sets `account.favoritesSource`): when
//! the account is added, from the line under the Favorites heading, or in File › Options.
//!
//! - **Microsoft Graph** (also what an account does that never chose, as before there was a
//!   choice, and the only way for a personal Microsoft account). The read rides along with the
//!   jobs the host does for Graph anyway (the calendar's own job, [`crate::calendar_sync`], and
//!   the Graph part of a Send/Receive). It takes two permissions that a sign-in doesn't have
//!   until the owner agrees to them on the provider's page, by itself and when the owner
//!   chooses to; in an organisation an administrator may have to approve them once.
//! - **Exchange Web Services**, a job of its own ([`crate::Services::ews`]). It needs no
//!   administrator where the organisation still allows it, and Microsoft switches it off on
//!   1 April 2027: from that day on by the session's clock it is not offered, and an account
//!   set to it says so and offers Graph.
//! - **Not now**: nothing is read, and the favourites marked in MailCraft are the account's.
//!
//! Whichever it is, the list is read when the app starts, when the owner asks for a
//! Send/Receive, right after the owner connected it, and otherwise not more often than every
//! quarter of an hour. What comes back is taken in by the engine (`sync.applyFavorites`), and
//! the pane lists what the engine says (`mailcraft_engine::cmd::folder::favorites`). Until
//! there is a list, and whenever the server's can't be had, the favourites kept in the mailbox
//! are shown, untouched. Nothing here ever puts a dialog in front of the owner unasked; what
//! there is to say is a quiet line under the Favorites heading:
//!
//! - the sign-in doesn't cover reading them: the offer, and the server isn't asked again that day;
//! - the owner asked and an administrator has to approve it first: that, with a message to
//!   copy for the administrator, the way to the page that says what to do next
//!   ([`crate::approval`]) and the offer to read them the other way;
//! - the organisation doesn't allow Exchange Web Services: that, and the offer to use Graph;
//! - the owner asked and it wasn't to be had (this kind of account has no such permission, the
//!   organisation has shut the door): one plain line saying so;
//! - a read came to nothing: one line with why in a few words, and "Try again".
//!
//! None of these is asked about again that day unless the owner asks.

pub mod choice;

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;

use mailcraft_engine::cmd::remote;
use mailcraft_model::remote::{FavoritesDownload, FavoritesOutcome, RemoteFavorite};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Directory, FavoritesSource, Id};
use serde_json::{Value, json};

use crate::{EwsRequest, EwsResult, GraphResult, MailApp, now_ms};

/// How often an account's Favorites are read at most (milliseconds), and for how long the
/// server is left alone after it said the sign-in doesn't cover them.
const EVERY_MS: f64 = 900_000.0;
const DAY_MS: f64 = 86_400_000.0;
/// How often at most the system's credential store is asked for a sign-in that a read through
/// Exchange Web Services waits for (milliseconds).
const LOOK_MS: f64 = 5_000.0;

/// What a sign-in for the Favorites says when it is not the owner's to turn into a yes: an
/// administrator of the organisation has to approve it first; the organisation doesn't have it
/// at all; this kind of account has no such permission. The host answers a
/// [`crate::SignInRequest`] that has `favorites` or `ews` with exactly one of these as its
/// error, and the pane shows it (in the interface language).
pub const NEEDS_APPROVAL: &str = tn!("Your organisation's administrator has to approve this once");
pub const REFUSED_BY_ORGANISATION: &str = tn!("Not allowed by your organisation");
pub const REFUSED_FOR_ACCOUNT: &str = tn!("Not available for this account");
/// What the pane says of Exchange Web Services when the organisation doesn't allow it, and
/// when Microsoft has switched it off.
pub const EWS_NOT_ALLOWED: &str = tn!("Your organisation doesn't allow Exchange Web Services");
pub const EWS_SWITCHED_OFF: &str = tn!("Microsoft has switched Exchange Web Services off");

/// What the pane says under the Favorites heading, when there is something to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Note {
    /// Outlook's Favorites are not shown, and could be: the offer to show them.
    Offer,
    /// The owner asked, and an administrator of the organisation has to approve it first, for
    /// the way that was asked for.
    Approval(FavoritesSource),
    /// The organisation doesn't allow Exchange Web Services.
    Blocked,
    /// Microsoft has switched Exchange Web Services off.
    Ended,
    /// The owner asked, and they are not to be had: why, in a line.
    Refused(String),
    /// The last read came to nothing: why, in a few plain words.
    Failed(String),
}

/// A link under a quiet line: what the owner can do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Copy a message for an administrator, with the address they approve it at
    /// ([`crate::approval`]).
    CopyMessage,
    /// Open the page that says what the owner and an administrator can do
    /// ([`crate::approval`]).
    Help,
    /// Read the Favorites the other way instead.
    Use(FavoritesSource),
}

impl Step {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Step::CopyMessage => "copyMessage",
            Step::Help => "help",
            Step::Use(FavoritesSource::Ews) => "useEws",
            Step::Use(_) => "useGraph",
        }
    }

    /// What the link says.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Step::CopyMessage => tn!("Copy a message for my administrator"),
            Step::Help => tn!("What to do next"),
            Step::Use(FavoritesSource::Ews) => tn!("Use Exchange Web Services instead"),
            Step::Use(_) => tn!("Use Microsoft Graph instead"),
        }
    }

    /// And what it does, for whoever asks before clicking.
    pub(crate) fn about(self) -> &'static str {
        match self {
            Step::CopyMessage => {
                tn!(
                    "Copies a short message with the address of Microsoft's page where an administrator of your organisation approves MailCraft, once for everybody. Nothing is sent: paste it into mail or chat yourself."
                )
            }
            Step::Help => {
                tn!(
                    "Says what you and your organisation's administrator can do, and lets an administrator approve MailCraft on Microsoft's page from here."
                )
            }
            Step::Use(FavoritesSource::Ews) => {
                tn!(
                    "Reads Outlook's Favorites through Exchange Web Services, an older way that Microsoft switches off on 1 April 2027. It works where your organisation lets you approve it yourself; elsewhere it needs an administrator too."
                )
            }
            Step::Use(_) => {
                tn!("Reads Outlook's Favorites through Microsoft Graph. It lasts; in an organisation an administrator has to approve it once.")
            }
        }
    }
}

impl Note {
    /// What the control channel calls it.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Note::Offer => "offer",
            Note::Approval(_) => "approval",
            Note::Blocked => "blocked",
            Note::Ended => "ended",
            Note::Refused(_) => "refused",
            Note::Failed(_) => "failed",
        }
    }

    /// What the line says (to be put into the interface language).
    pub(crate) fn text(&self) -> &str {
        match self {
            Note::Offer => tn!("Show Outlook's Favorites"),
            Note::Approval(_) => NEEDS_APPROVAL,
            Note::Blocked => EWS_NOT_ALLOWED,
            Note::Ended => EWS_SWITCHED_OFF,
            Note::Refused(why) | Note::Failed(why) => why,
        }
    }
}

/// What the app remembers of keeping the Favorites current. All of it is for the run: nothing
/// here is saved.
#[derive(Default)]
pub(crate) struct FavoritesSync {
    /// When each account's Favorites were last asked for (milliseconds).
    asked: HashMap<Id, f64>,
    /// The accounts whose sign-in was found not to cover reading them, and when (milliseconds).
    missing: HashMap<Id, f64>,
    /// What there is to say under the heading, account by account.
    notes: HashMap<Id, Note>,
    /// The accounts whose Graph token in memory is known not to be good for reading them (it
    /// was asked for without those permissions): a job that wants them gets a new one.
    uncovered: HashSet<Id>,
    /// The accounts whose Favorites are read at the next chance, whatever the clock says: the
    /// owner asked.
    due: HashSet<Id>,
    /// Which kind of Microsoft account each is, as a sign-in of this run said, and for an
    /// organisation's the directory it named (for the link an administrator is sent).
    directories: HashMap<Id, Directory>,
    tenants: HashMap<Id, String>,
    /// Tokens for Exchange Web Services obtained this run, per account. Never saved.
    ews_tokens: HashMap<Id, Tokens>,
    /// A read through Exchange Web Services that the host is busy with.
    ews_rx: Option<(Id, Receiver<EwsResult>)>,
    /// The accounts whose owner chose Exchange Web Services just now: should the sign-in not
    /// cover it, the provider's page is put in front of them, once.
    chose: HashSet<Id>,
    /// When the system's credential store was last asked for an account's sign-in on behalf
    /// of such a read (milliseconds): not at every frame while nobody is signed in.
    looked: HashMap<Id, f64>,
}

impl MailApp {
    /// Whether `account` signs in with Microsoft.
    pub(crate) fn is_microsoft(&self, account: Id) -> bool {
        let microsoft = |a: &mailcraft_model::Account| a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft);
        self.session.mailbox.account(account).is_some_and(microsoft)
    }

    /// What the owner chose for the Favorites of `account` ([`FavoritesSource::Unset`]: never).
    pub(crate) fn favorites_source(&self, account: Id) -> FavoritesSource {
        self.session.mailbox.account(account).map(|a| a.favorites_source).unwrap_or_default()
    }

    /// Which way the Favorites of `account` are read today, when they are: Microsoft Graph for
    /// an account that chose it or never chose, Exchange Web Services for one that chose it
    /// while it exists. `None` for one that keeps its own, and while the owner would rather see
    /// the mailbox's own everywhere.
    fn reads_by(&self, account: Id) -> Option<FavoritesSource> {
        if !self.is_microsoft(account) || self.session.prefs.own_favorites {
            return None;
        }
        match self.favorites_source(account) {
            FavoritesSource::None => None,
            FavoritesSource::Ews if mailcraft_engine::cmd::account::ews_ended(&self.session) => None,
            FavoritesSource::Ews => Some(FavoritesSource::Ews),
            FavoritesSource::Unset | FavoritesSource::Graph => Some(FavoritesSource::Graph),
        }
    }

    /// Which kind of Microsoft account `account` is: what was written down with it; else what a
    /// sign-in of this run said (the directory an identity token names); else what its sign-in
    /// settings say; else what its address says (Outlook.com and its kin are personal).
    pub(crate) fn directory(&self, account: Id) -> Directory {
        let Some(owner) = self.session.mailbox.account(account) else { return Directory::Unknown };
        let Some(oauth) = &owner.oauth else { return Directory::Unknown };
        if oauth.directory != Directory::Unknown {
            return oauth.directory;
        }
        if let Some(said) = self.favorites.directories.get(&account) {
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

    /// A sign-in of `account` named the directory `tenant` (an identity token's): that says
    /// which kind of account it is. Kept for the run; it goes into the mailbox only with a choice.
    pub(crate) fn note_directory(&mut self, account: Id, tenant: &str) {
        let tenant = tenant.trim();
        let kind = Directory::of_tenant(tenant);
        if account == 0 || kind == Directory::Unknown {
            return;
        }
        self.favorites.directories.insert(account, kind);
        if kind == Directory::Organisation && tenant.len() <= 64 {
            self.favorites.tenants.insert(account, tenant.to_string());
        } else {
            self.favorites.tenants.remove(&account);
        }
    }

    /// Whether the owner of `account` has a choice of how its Favorites in Outlook are read: an
    /// account of an organisation that signs in with Microsoft. (A personal one has Microsoft
    /// Graph and nothing else, and one that nobody knows the kind of is treated as that.)
    pub(crate) fn favorites_choice(&self, account: Id) -> bool {
        self.is_microsoft(account) && self.directory(account) == Directory::Organisation
    }

    /// What the owner of `account` may choose today, by what a command calls each.
    fn favorites_offered(&self, account: Id) -> Vec<&'static str> {
        mailcraft_engine::cmd::account::favorites_sources_offered(&self.session, self.directory(account))
    }

    /// Whether the Favorites of `account` are due at `now`: the owner asked (`asked_for`, or
    /// earlier), or a quarter of an hour has gone by, and the server didn't say today that the
    /// sign-in doesn't cover them. Notes that they were asked for when they are.
    fn favorites_wanted(&mut self, account: Id, now: f64, asked_for: bool) -> bool {
        let read = self.favorites_are_due(account, now, asked_for);
        if read {
            self.favorites.asked.insert(account, now);
            self.favorites.due.remove(&account);
        }
        read
    }

    /// [`favorites_wanted`](Self::favorites_wanted), without noting anything.
    fn favorites_are_due(&self, account: Id, now: f64, asked_for: bool) -> bool {
        let owner = self.favorites.due.contains(&account);
        // Told that the sign-in doesn't cover them: not again that day, unless the owner asks.
        let refused = self.favorites.missing.get(&account).is_some_and(|at| (0.0..DAY_MS).contains(&(now - at)));
        let due = owner || asked_for || self.favorites.asked.get(&account).is_none_or(|at| now - at >= EVERY_MS || now < *at);
        due && (owner || !refused)
    }

    /// Whether a Graph job for `account` that starts at `now` reads the Favorites (and so asks
    /// for a token that is good for reading them, when it has to ask for one: a job that
    /// doesn't read them asks for nothing it didn't always ask for). `asked_for` when the owner
    /// asked for the job (a Send/Receive). Notes that they were asked for when they are.
    pub(crate) fn favorites_ask(&mut self, account: Id, now: f64, asked_for: bool) -> bool {
        match self.reads_by(account) {
            Some(FavoritesSource::Graph) => self.favorites_wanted(account, now, asked_for),
            // Read by a job of its own ([`favorites_tick_at`](Self::favorites_tick_at)), which
            // the owner's Send/Receive makes due.
            Some(_) => {
                if asked_for {
                    self.favorites.due.insert(account);
                }
                false
            }
            None => false,
        }
    }

    /// The Graph token to hand a job for `account`: the one in memory when it is still good,
    /// unless the job reads the Favorites and this one is known not to be good for that. A job
    /// that will have to ask for a token, and not for one that is good for the Favorites, leaves
    /// the account with one that isn't: that is noted here.
    pub(crate) fn graph_access_for(&mut self, account: Id, favorites: bool) -> String {
        let access = self.graph_access(account, favorites);
        if access.is_empty() && !favorites {
            self.favorites.uncovered.insert(account);
        }
        access
    }

    /// [`graph_access_for`](Self::graph_access_for), without noting anything.
    pub(crate) fn graph_access(&self, account: Id, favorites: bool) -> String {
        if favorites && self.favorites.uncovered.contains(&account) {
            return String::new();
        }
        self.graph_tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone()).unwrap_or_default()
    }

    /// A Graph token for `account` came from somewhere that didn't ask for the Favorites'
    /// permissions with it (a check of the calendar connection, "Connect calendar and contacts").
    pub(crate) fn favorites_token_is_plain(&mut self, account: Id) {
        self.favorites.uncovered.insert(account);
    }

    /// Take in what a Graph job said of the Favorites of `account`: whether its token covers
    /// them, and the list, or why there is none. Never a dialog, never the problems report:
    /// what there is to say is said under the Favorites heading.
    pub(crate) fn take_in_favorites(&mut self, account: Id, result: &mut GraphResult) {
        match result.favorites_covered {
            Some(true) => {
                self.favorites.uncovered.remove(&account);
            }
            Some(false) => {
                self.favorites.uncovered.insert(account);
            }
            None => {}
        }
        let Some(download) = result.favorites.take() else { return };
        let why = std::mem::take(&mut result.favorites_error);
        // (Asked for before the owner chose another way: nobody's answer any more.)
        if self.reads_by(account) != Some(FavoritesSource::Graph) {
            return;
        }
        self.apply_favorites_download(account, &download, &why);
    }

    /// [`take_in_favorites`](Self::take_in_favorites) for one download; `why` is what went
    /// wrong, in a few plain words, when the read failed.
    fn apply_favorites_download(&mut self, account: Id, download: &FavoritesDownload, why: &str) {
        // (By the clock of the look that asked, which is the one the next look goes by.)
        let now = self.favorites.asked.get(&account).copied().unwrap_or_else(now_ms);
        match download.outcome {
            FavoritesOutcome::Read | FavoritesOutcome::Missing => {
                self.favorites.missing.remove(&account);
                self.favorites.notes.remove(&account);
            }
            FavoritesOutcome::NotPermitted => {
                self.favorites.missing.insert(account, now);
                self.favorites.uncovered.insert(account);
                // (What the owner was told when he asked stays said.)
                if !matches!(self.favorites_note_of(account), Some(Note::Refused(_) | Note::Approval(_) | Note::Blocked)) {
                    self.favorites.notes.insert(account, Note::Offer);
                }
            }
            FavoritesOutcome::Failed => {
                let why = if why.trim().is_empty() { "Couldn't read the list" } else { why.trim() };
                self.favorites.notes.insert(account, Note::Failed(why.chars().filter(|c| !c.is_control()).take(60).collect()));
            }
        }
        match remote::apply_favorites(&mut self.session, download) {
            // Counts, never a name: what the log may hold.
            Ok(answer) => log::info!(
                "favorites, account {account}: {:?}; {} folders listed here, {} of the server's are not folders here, {} entries were no folders, {} named none",
                download.outcome,
                answer["favorites"].as_array().map_or(0, Vec::len),
                answer["absent"].as_u64().unwrap_or(0),
                download.skipped,
                download.unresolved
            ),
            // (For another account than the mailbox has now: nobody's.)
            Err(_) => log::debug!("favorites, account {account}: the answer was not taken in"),
        }
    }

    /// The server's list of `account` is not to be had: what was kept of one goes, and the
    /// favourites marked here are what the pane shows. Says nothing under the heading.
    fn drop_server_favorites(&mut self, account: Id) {
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return };
        let gone = FavoritesDownload { outcome: FavoritesOutcome::NotPermitted, ..Default::default() }.for_account(&owner);
        let _ = remote::apply_favorites(&mut self.session, &gone);
    }

    /// The way the Favorites of `account` are read, or would be: Exchange Web Services for an
    /// account set to it, Microsoft Graph for any other.
    fn favorites_way(&self, account: Id) -> FavoritesSource {
        if self.favorites_source(account) == FavoritesSource::Ews { FavoritesSource::Ews } else { FavoritesSource::Graph }
    }

    /// What there is to say of the Favorites of `account` under the heading, if anything.
    pub(crate) fn favorites_note_of(&self, account: Id) -> Option<Note> {
        if !self.is_microsoft(account) || self.session.prefs.own_favorites {
            return None;
        }
        match self.favorites_source(account) {
            // "Not now" is not "never": the offer stands, and nobody is asked for it.
            FavoritesSource::None => Some(Note::Offer),
            FavoritesSource::Ews if mailcraft_engine::cmd::account::ews_ended(&self.session) => Some(Note::Ended),
            // (What was said of a way the account no longer takes is not said of the other.)
            _ => {
                let way = self.favorites_way(account);
                let stands = |note: &&Note| match note {
                    Note::Blocked | Note::Ended => way == FavoritesSource::Ews,
                    Note::Approval(of) => *of == way,
                    Note::Offer | Note::Refused(_) | Note::Failed(_) => true,
                };
                self.favorites.notes.get(&account).filter(stands).cloned()
            }
        }
    }

    /// What the pane says under the Favorites heading: the first account that has something to
    /// say, and what. Nothing while the owner would rather see the mailbox's own favourites.
    pub(crate) fn favorites_note(&self) -> Option<(Id, Note)> {
        let accounts = self.session.mailbox.accounts.iter().map(|a| a.id);
        accounts.filter_map(|id| self.favorites_note_of(id).map(|note| (id, note))).next()
    }

    /// The links under the line that `note` is for `account`: what the owner can do about it.
    pub(crate) fn favorites_steps(&self, account: Id, note: &Note) -> Vec<Step> {
        let offered = self.favorites_offered(account);
        let other = |source: FavoritesSource| source.code().filter(|code| offered.contains(code)).map(|_| Step::Use(source));
        match note {
            Note::Approval(FavoritesSource::Ews) => {
                [Some(Step::CopyMessage), Some(Step::Help), other(FavoritesSource::Graph)].into_iter().flatten().collect()
            }
            // (The other way only where there is a choice: a personal account has none.)
            Note::Approval(_) => {
                let ews = other(FavoritesSource::Ews).filter(|_| self.favorites_choice(account));
                [Some(Step::CopyMessage), Some(Step::Help), ews].into_iter().flatten().collect()
            }
            Note::Blocked | Note::Ended => vec![Step::Use(FavoritesSource::Graph)],
            Note::Offer | Note::Refused(_) | Note::Failed(_) => Vec::new(),
        }
    }

    /// The account whose Favorites could be Outlook's if the owner connected them: the first
    /// that has the offer standing (File › Options offers it too).
    pub(crate) fn favorites_offer(&self) -> Option<Id> {
        let accounts = self.session.mailbox.accounts.iter().map(|a| a.id);
        accounts.filter(|id| !self.favorites_choice(*id)).find(|id| matches!(self.favorites_note_of(*id), Some(Note::Offer | Note::Refused(_))))
    }

    /// The Favorites of `account` are read at the next chance, whatever the clock says and
    /// whatever was said before: the owner asked. The calendar's own job carries the read
    /// through Microsoft Graph, so it is made due too.
    pub(crate) fn favorites_due(&mut self, account: Id) {
        if !self.is_microsoft(account) {
            return;
        }
        self.favorites.due.insert(account);
        self.favorites.missing.remove(&account);
        if self.reads_by(account) != Some(FavoritesSource::Ews) {
            self.calendar.checked.remove(&account);
            self.calendar.failures.remove(&account);
        }
    }

    /// What was remembered of an account that is gone.
    pub(crate) fn forget_favorites(&mut self, account: Id) {
        let kept = &mut self.favorites;
        kept.asked.remove(&account);
        kept.missing.remove(&account);
        kept.notes.remove(&account);
        kept.uncovered.remove(&account);
        kept.due.remove(&account);
        kept.directories.remove(&account);
        kept.tenants.remove(&account);
        kept.ews_tokens.remove(&account);
        kept.chose.remove(&account);
        kept.looked.remove(&account);
        if kept.ews_rx.as_ref().is_some_and(|(whose, _)| *whose == account) {
            kept.ews_rx = None;
        }
    }

    /// The sign-in for the Favorites of `account` came back with tokens that are good for
    /// them: nothing stands in the way any more, and they are read at once.
    pub(crate) fn favorites_connected(&mut self, account: Id) {
        self.favorites.notes.remove(&account);
        self.favorites.uncovered.remove(&account);
        self.favorites_due(account);
    }

    /// The owner agreed, on the provider's page, to Exchange Web Services for `account`: the
    /// token is kept for the run, and the Favorites are read at once.
    pub(crate) fn favorites_ews_connected(&mut self, account: Id, tokens: Tokens) {
        self.favorites.ews_tokens.insert(account, tokens);
        self.favorites.notes.remove(&account);
        self.favorites.chose.remove(&account);
        self.favorites_due(account);
    }

    /// The sign-in for the Favorites of `account` came to nothing: `error` is why. When the no
    /// is not the owner's to change ([`NEEDS_APPROVAL`], [`REFUSED_BY_ORGANISATION`],
    /// [`REFUSED_FOR_ACCOUNT`]) the pane says so, and nothing is asked again that day; anything
    /// else (the owner said no, the page was closed) goes to the status bar, and the offer stands.
    pub(crate) fn favorites_refused(&mut self, account: Id, error: &str) {
        self.favorites_refused_for(account, error, FavoritesSource::Graph);
    }

    /// [`favorites_refused`](Self::favorites_refused) for a sign-in that asked for the way
    /// `source` of reading them (Microsoft Graph, or Exchange Web Services). A no to a way the
    /// owner has turned from meanwhile is nobody's any more.
    pub(crate) fn favorites_refused_for(&mut self, account: Id, error: &str, source: FavoritesSource) {
        if source != self.favorites_way(account) {
            return;
        }
        let note = match error {
            NEEDS_APPROVAL => Note::Approval(source),
            REFUSED_BY_ORGANISATION | REFUSED_FOR_ACCOUNT => Note::Refused(error.to_string()),
            _ => {
                // (Asked through Exchange Web Services, there is no list until it is agreed to.)
                if source == FavoritesSource::Ews && !self.favorites.notes.contains_key(&account) {
                    self.favorites.notes.insert(account, Note::Offer);
                }
                self.status(error.to_string());
                return;
            }
        };
        // (Only an organisation has an administrator to ask: an account that is told to find
        // one is an organisation's, whatever else was known of it.)
        if error == NEEDS_APPROVAL && self.directory(account) == Directory::Unknown {
            self.favorites.directories.insert(account, Directory::Organisation);
        }
        self.favorites.notes.insert(account, note);
        self.favorites.missing.insert(account, now_ms());
        if source == FavoritesSource::Ews {
            self.drop_server_favorites(account);
        }
        self.status(tl!(error));
    }

    // ---- Exchange Web Services: a job of its own ----

    /// A token for Exchange Web Services that is still good, for `account`; else empty.
    fn ews_access(&self, account: Id) -> String {
        self.favorites.ews_tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone()).unwrap_or_default()
    }

    /// Whether a read through Exchange Web Services is in flight.
    pub fn favorites_reading(&self) -> bool {
        self.favorites.ews_rx.is_some()
    }

    /// Look whether an account that reads its Favorites through Exchange Web Services is due,
    /// and start the read when it is. Called every frame.
    pub(crate) fn favorites_tick(&mut self, ctx: &egui::Context) {
        self.favorites_tick_at(now_ms());
        if self.favorites.ews_rx.is_some() {
            // An answer is on its way (the host wakes the window when it is there, too).
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// [`favorites_tick`](Self::favorites_tick) at `now` (milliseconds). Whether a read was
    /// started. (Public for a host's tests, which have no window.)
    pub fn favorites_tick_at(&mut self, now: f64) -> bool {
        if !self.auto_sync || self.services.ews.is_none() || self.favorites.ews_rx.is_some() || self.signing_in.is_some() {
            return false;
        }
        let accounts: Vec<Id> = self.session.mailbox.accounts.iter().map(|a| a.id).collect();
        for account in accounts {
            if self.favorites_source(account) != FavoritesSource::Ews || !self.is_microsoft(account) {
                continue;
            }
            // Switched off by Microsoft: nothing is asked any more, and what was kept goes.
            if mailcraft_engine::cmd::account::ews_ended(&self.session) {
                if mailcraft_engine::cmd::folder::favorites_from_server(&self.session, account) {
                    self.drop_server_favorites(account);
                }
                continue;
            }
            if self.reads_by(account) != Some(FavoritesSource::Ews) {
                continue;
            }
            // (This is looked at every frame: nothing further is, until a read is due.)
            if !self.favorites_are_due(account, now, false) {
                continue;
            }
            let Some(owner) = self.session.mailbox.account(account).cloned() else { continue };
            let Some(oauth) = owner.oauth.clone() else { continue };
            let access = self.ews_access(account);
            let kept = self.tokens.get(&account).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
            let refresh = match kept {
                Some(refresh) => refresh,
                None if !access.is_empty() => String::new(),
                // What the system remembers of the sign-in, asked for every few seconds at most.
                None => {
                    if self.favorites.looked.get(&account).is_some_and(|at| (0.0..LOOK_MS).contains(&(now - at))) {
                        continue;
                    }
                    self.favorites.looked.insert(account, now);
                    self.remembered("refresh", &owner.email).unwrap_or_default()
                }
            };
            // Nobody is signed in yet: Send/Receive does that, and this waits for it.
            if access.is_empty() && refresh.is_empty() {
                continue;
            }
            self.favorites_wanted(account, now, false);
            let ask = EwsRequest { account: owner, oauth: self.effective_oauth(oauth), refresh, access };
            let Some(ews) = &self.services.ews else { return false };
            self.favorites.ews_rx = Some((account, ews(ask)));
            return true;
        }
        false
    }

    /// The read through Exchange Web Services answered: keep its token, and take in the list,
    /// or say in a line why there is none. (Public for a host's tests.)
    pub fn poll_favorites(&mut self) {
        let Some((account, rx)) = &self.favorites.ews_rx else { return };
        let account = *account;
        let mut result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => EwsResult {
                account,
                favorites: Some(FavoritesDownload { outcome: FavoritesOutcome::Failed, ..Default::default() }),
                ..Default::default()
            },
        };
        self.favorites.ews_rx = None;
        if result.token_refused {
            self.favorites.ews_tokens.remove(&account);
        }
        let address = self.session.mailbox.account(account).map(|a| a.email.clone()).unwrap_or_default();
        if let Some(tokens) = result.tokens.take() {
            self.note_directory(account, &tokens.tenant);
            // The newest refresh token is the account's, whatever it was asked for.
            if !tokens.refresh.is_empty() {
                self.remember("refresh", &address, &tokens.refresh);
                if let Some(mail) = self.tokens.get_mut(&account) {
                    mail.refresh = tokens.refresh.clone();
                }
            }
            let now = mailcraft_time::now();
            self.favorites.ews_tokens.insert(account, Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens });
        }
        // (Asked for before the owner chose another way: nobody's answer any more.)
        if self.reads_by(account) != Some(FavoritesSource::Ews) {
            self.favorites.chose.remove(&account);
            return;
        }
        let now = self.favorites.asked.get(&account).copied().unwrap_or_else(now_ms);
        if result.blocked {
            log::info!("favorites, account {account}: the organisation doesn't allow Exchange Web Services");
            self.favorites.chose.remove(&account);
            self.favorites.notes.insert(account, Note::Blocked);
            self.favorites.missing.insert(account, now);
            self.drop_server_favorites(account);
            return;
        }
        if result.consent_needed {
            self.favorites.missing.insert(account, now);
            self.drop_server_favorites(account);
            // The owner chose this just now: the provider's page, once. Nobody else is ever
            // sent there for it.
            if self.favorites.chose.remove(&account) {
                match self.connect_ews(account) {
                    Ok(()) => return,
                    Err(e) => self.status(e),
                }
            }
            if !matches!(self.favorites.notes.get(&account), Some(Note::Refused(_) | Note::Approval(_))) {
                self.favorites.notes.insert(account, Note::Offer);
            }
            return;
        }
        // (The token was no longer taken: the next look gets another, and says what there is.)
        if result.token_refused && result.favorites.is_none() {
            self.favorites.asked.remove(&account);
            return;
        }
        self.favorites.chose.remove(&account);
        let Some(download) = result.favorites.take() else { return };
        let why = std::mem::take(&mut result.favorites_error);
        self.apply_favorites_download(account, &download, &why);
    }

    /// Have the owner agree, on Microsoft's page, to Exchange Web Services for `account`, and
    /// to nothing else.
    fn connect_ews(&mut self, account: Id) -> Result<(), String> {
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return Err(tl!("No account signs in with Microsoft").to_string()) };
        let Some(oauth) = owner.oauth.clone() else { return Err(tl!("This account doesn't sign in with Microsoft").to_string()) };
        self.ews_next = true;
        let begun = self.begin_sign_in(account, oauth, &owner.email, String::new(), false, false);
        self.ews_next = false;
        begun
    }

    // ---- commands ----

    /// The Microsoft account a favourites command is for: the one named, else the first.
    pub(crate) fn favorites_account(&self, p: &Value) -> Result<Id, String> {
        let named = p.get("account").and_then(Value::as_u64);
        let first = || self.session.mailbox.accounts.iter().map(|a| a.id).find(|id| self.is_microsoft(*id));
        match named.or_else(first) {
            Some(id) if self.is_microsoft(id) => Ok(id),
            Some(_) => Err(tl!("This account doesn't sign in with Microsoft").to_string()),
            None => Err(tl!("No account signs in with Microsoft").to_string()),
        }
    }

    /// What the control channel is told of the Favorites of `account`.
    fn favorites_state(&self, account: Id) -> Value {
        let from_server = mailcraft_engine::cmd::folder::favorites_from_server(&self.session, account);
        let note = self.favorites_note_of(account);
        let steps: Vec<&str> =
            note.as_ref().map(|note| self.favorites_steps(account, note)).unwrap_or_default().into_iter().map(Step::code).collect();
        json!({
            "account": account,
            "from": if from_server { "server" } else { "mailbox" },
            "own": self.session.prefs.own_favorites,
            "source": self.favorites_source(account).code(),
            "directory": self.directory(account).code(),
            "choice": self.favorites_choice(account),
            "offered": self.favorites_offered(account),
            "line": note.as_ref().map(Note::code),
            "text": note.as_ref().map(|note| tl!(note.text()).to_string()).unwrap_or_default(),
            "steps": steps,
            "favorites": mailcraft_engine::cmd::folder::favorites(&self.session),
        })
    }

    /// `ui.connectFavorites`: have the owner agree, on Microsoft's page, to this program reading
    /// the Favorites of Outlook. Nothing else is asked for there that wasn't agreed to before.
    fn connect_favorites(&mut self, p: &Value) -> Result<Value, String> {
        let account = self.favorites_account(p)?;
        let Some(owner) = self.session.mailbox.account(account).cloned() else { return Err(tl!("No account signs in with Microsoft").to_string()) };
        let Some(oauth) = owner.oauth.clone() else { return Err(tl!("This account doesn't sign in with Microsoft").to_string()) };
        self.favorites_next = true;
        let begun = self.begin_sign_in(account, oauth, &owner.email, String::new(), false, true);
        self.favorites_next = false;
        begun?;
        Ok(json!({"pending": "signIn", "account": account}))
    }

    /// `ui.favoritesSource`: the owner's choice of how the Favorites of Outlook are read for an
    /// account (`account.favoritesSource` keeps it), and what follows from it at once: Microsoft's
    /// page for Graph's two permissions, a read through Exchange Web Services (with Microsoft's
    /// page for it only should the sign-in not cover it), or nothing. Without `source`, how
    /// things stand.
    fn favorites_source_command(&mut self, p: &Value) -> Result<Value, String> {
        let account = self.favorites_account(p)?;
        let source = match p.get("source") {
            None | Some(Value::Null) => return Ok(self.favorites_state(account)),
            Some(source) => source.as_str().and_then(FavoritesSource::parse).ok_or_else(|| tl!("`source` is graph, ews or none").to_string())?,
        };
        let before = self.favorites_source(account);
        let had_list = mailcraft_engine::cmd::folder::favorites_from_server(&self.session, account);
        // Which kind of account it is goes into the mailbox with the choice.
        let mut params = json!({"account": account, "source": source.code()});
        if let Some(kind) = self.directory(account).code() {
            params["directory"] = json!(kind);
        }
        self.session.run("account.favoritesSource", &params).map_err(|e| e.to_string())?;
        // What was said of the way it was before is not said of this one.
        self.favorites.notes.remove(&account);
        self.favorites.missing.remove(&account);
        self.favorites.chose.remove(&account);
        let mut pending = Value::Null;
        match source {
            FavoritesSource::Graph => {
                // It works already (the list is Graph's): nothing to agree to, only to read.
                let works = had_list && matches!(before, FavoritesSource::Unset | FavoritesSource::Graph);
                if works {
                    self.favorites_due(account);
                    pending = json!("read");
                } else {
                    match self.connect_favorites(&json!({"account": account})) {
                        Ok(_) => pending = json!("signIn"),
                        // (The choice stands; the offer under the heading asks again.)
                        Err(e) => {
                            self.favorites.notes.insert(account, Note::Offer);
                            self.status(e);
                        }
                    }
                }
            }
            FavoritesSource::Ews => {
                if self.services.ews.is_some() {
                    self.favorites.chose.insert(account);
                    self.favorites_due(account);
                    pending = json!("read");
                } else {
                    self.status(tl!("This build can't reach the account's server."));
                }
            }
            FavoritesSource::None | FavoritesSource::Unset => {}
        }
        let mut state = self.favorites_state(account);
        state["pending"] = pending;
        Ok(state)
    }

    /// What an administrator's approval is asked for, for `account`: its sign-in settings (as
    /// the account has them), its organisation's directory when that is known (else empty), and
    /// whether Exchange Web Services is asked for with the rest (`Ews`: its Favorites are, or
    /// were to be, read that way).
    pub(crate) fn approval_of(&self, account: Id) -> Result<(mailcraft_model::OAuth, String, FavoritesSource), String> {
        let Some(owner) = self.session.mailbox.account(account) else { return Err(tl!("No account signs in with Microsoft").to_string()) };
        let Some(oauth) = owner.oauth.clone() else { return Err(tl!("This account doesn't sign in with Microsoft").to_string()) };
        let source = match self.favorites.notes.get(&account) {
            Some(Note::Approval(source)) => *source,
            _ if self.favorites_source(account) == FavoritesSource::Ews => FavoritesSource::Ews,
            _ => FavoritesSource::Graph,
        };
        // The organisation's own directory, when a sign-in of this run named it or the account's
        // settings do; otherwise the administrator's sign-in says which it is.
        let named = oauth.tenant.trim();
        let settings = (Directory::of_tenant(named) == Directory::Organisation && !named.eq_ignore_ascii_case("organizations")).then_some(named);
        let tenant = self.favorites.tenants.get(&account).map(String::as_str).or(settings).unwrap_or("").to_string();
        Ok((oauth, tenant, source))
    }

    /// The address of Microsoft's page where an administrator approves this program, for the
    /// sign-in settings `oauth` (with the application id filled in), the directory `tenant`
    /// (empty: whichever the administrator is of) and with Exchange Web Services or without
    /// (`source`). A directory the host won't make an address of is left out of it.
    pub(crate) fn admin_link(&self, oauth: &mailcraft_model::OAuth, tenant: &str, source: FavoritesSource) -> Result<String, String> {
        let unable = || tl!("This build can't make a link for an administrator.").to_string();
        let make = self.services.admin_consent_link.as_ref().ok_or_else(unable)?;
        let link = make(oauth, tenant, source).or_else(|| if tenant.is_empty() { None } else { make(oauth, "", source) });
        link.ok_or_else(unable)
    }

    /// The address an administrator of the organisation of `account` approves this program at,
    /// with Exchange Web Services in it where its Favorites are (or were to be) read that way.
    fn admin_consent_link(&self, account: Id) -> Result<(String, FavoritesSource), String> {
        let (oauth, tenant, source) = self.approval_of(account)?;
        let link = self.admin_link(&self.effective_oauth(oauth), &tenant, source)?;
        Ok((link, source))
    }

    /// `ui.copyAdminConsentLink`: put the address an administrator approves this program at on
    /// the clipboard, for the owner to pass on. Nothing is opened and nothing is sent. (The
    /// window's own links copy a whole message with it: `ui.copyAdminMessage`.)
    fn copy_admin_consent_link(&mut self, p: &Value) -> Result<Value, String> {
        let account = self.favorites_account(p)?;
        let (link, source) = self.admin_consent_link(account)?;
        if let Some(ctx) = &self.ctx {
            ctx.copy_text(link.clone());
        }
        self.status(tl!("The link is on the clipboard: send it to your organisation's administrator"));
        Ok(json!({"account": account, "source": source.code(), "url": link}))
    }

    /// `ui.favorites`: what the folder pane has of an account's Favorites in Outlook. With
    /// `"refresh": true` they are read again at the next chance ("Try again"). With `answer`,
    /// nothing is asked of anybody: it is taken in as what the host brought (for a script, and
    /// for looking at the pane in each of its states).
    fn favorites_command(&mut self, p: &Value) -> Result<Value, String> {
        let account = self.favorites_account(p)?;
        if let Some(answer) = p.get("answer") {
            let outcome = answer.get("outcome").and_then(Value::as_str).unwrap_or("");
            let why = answer.get("why").and_then(Value::as_str).unwrap_or("");
            match outcome {
                // (The owner asked on the provider's page and was told no.)
                "refused" => {
                    let by = if why == REFUSED_FOR_ACCOUNT { REFUSED_FOR_ACCOUNT } else { REFUSED_BY_ORGANISATION };
                    self.favorites_refused_for(account, by, self.favorites_way(account));
                    return Ok(self.favorites_state(account));
                }
                // (…or that an administrator has to approve it first.)
                "approval" => {
                    self.favorites_refused_for(account, NEEDS_APPROVAL, self.favorites_way(account));
                    return Ok(self.favorites_state(account));
                }
                // (The organisation doesn't allow Exchange Web Services.)
                "blocked" => {
                    self.favorites.notes.insert(account, Note::Blocked);
                    self.favorites.missing.insert(account, now_ms());
                    self.drop_server_favorites(account);
                    return Ok(self.favorites_state(account));
                }
                _ => {}
            }
            let outcome = match outcome {
                "read" => FavoritesOutcome::Read,
                "missing" => FavoritesOutcome::Missing,
                "notPermitted" => FavoritesOutcome::NotPermitted,
                "failed" => FavoritesOutcome::Failed,
                _ => return Err(tl!("`answer.outcome` is read, missing, notPermitted, refused, approval, blocked or failed").to_string()),
            };
            let folders: Vec<RemoteFavorite> = match answer.get("folders") {
                None | Some(Value::Null) => Vec::new(),
                Some(folders) => {
                    serde_json::from_value(folders.clone()).map_err(|_| tl!("`answer.folders` is a list of {path, kind}").to_string())?
                }
            };
            let Some(owner) = self.session.mailbox.account(account).cloned() else {
                return Err(tl!("No account signs in with Microsoft").to_string());
            };
            let download = FavoritesDownload { outcome, folders, ..Default::default() }.for_account(&owner);
            self.apply_favorites_download(account, &download, why);
            return Ok(self.favorites_state(account));
        }
        if p.get("refresh").and_then(Value::as_bool) != Some(true) {
            return Ok(self.favorites_state(account));
        }
        if self.session.prefs.own_favorites {
            return Err(tl!("Outlook's Favorites are switched off in File › Options").to_string());
        }
        let reads = self.reads_by(account);
        let reachable = match reads {
            Some(FavoritesSource::Ews) => self.services.ews.is_some(),
            _ => self.services.graph.is_some(),
        };
        if !reachable {
            return Err(tl!("This build can't reach the account's server.").to_string());
        }
        // What the last read said is not said any longer: the next one says what there is.
        if matches!(self.favorites.notes.get(&account), Some(Note::Failed(_))) {
            self.favorites.notes.remove(&account);
        }
        self.favorites_due(account);
        Ok(self.favorites_state(account))
    }
}

/// File › Options: whether the folder pane shows Outlook's Favorites for an account that has
/// them; for an account of an organisation, how they are read; and for any other, the way to
/// connect them when its sign-in doesn't cover reading them yet. Nothing is shown while no
/// account signs in with Microsoft.
pub(crate) fn options(app: &mut MailApp, ui: &mut egui::Ui, t: &crate::theme::Tokens) {
    let ids: Vec<Id> = app.session.mailbox.accounts.iter().map(|a| a.id).collect();
    if !ids.iter().any(|id| app.is_microsoft(*id)) {
        return;
    }
    let mut outlook = !app.session.prefs.own_favorites;
    let about = tl!(
        "For an account that signs in with Microsoft: the folders you keep under Favorites in Outlook, in the same order. They are read from the server and never changed there. Without this, the favourites you mark in MailCraft are shown."
    );
    let resp = ui.checkbox(&mut outlook, tl!("Show the Favorites from Outlook in the folder pane")).on_hover_text(about);
    app.hit.add("favoritesOption", 0, resp.rect);
    if resp.changed() {
        let _ = app.run("view.favorites", json!({"own": !outlook}));
    }
    if !outlook {
        return;
    }
    // An account of an organisation: how its Favorites are read, account by account.
    let choosing: Vec<Id> = ids.iter().copied().filter(|id| app.favorites_choice(*id)).collect();
    for account in choosing {
        choice::options_row(app, ui, t, account);
    }
    // The sign-in doesn't cover reading them yet: the same offer as under the Favorites heading.
    if let Some(account) = app.favorites_offer() {
        let said = match app.favorites_note_of(account) {
            Some(Note::Refused(why)) => tl!(&why).to_string(),
            _ => String::new(),
        };
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            let link = ui.link(tl!("Connect Outlook's Favorites")).on_hover_text(tl!(
                "Sign in with Microsoft once more to let MailCraft read the folders you keep under Favorites in Outlook. They are only read, never changed, and everything else stays as it is."
            ));
            app.hit.add("favoritesConnect", account, link.rect);
            if link.clicked()
                && let Err(e) = app.run("ui.connectFavorites", json!({"account": account}))
            {
                app.status(e);
            }
            if !said.is_empty() {
                ui.label(egui::RichText::new(said).color(t.text_dim));
            }
        });
    }
}

/// The `ui.*` commands of the Favorites; `None` for any other.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.connectFavorites" => app.connect_favorites(p),
        "ui.favorites" => app.favorites_command(p),
        "ui.favoritesSource" => app.favorites_source_command(p),
        "ui.copyAdminConsentLink" => app.copy_admin_consent_link(p),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
