// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Keeping the Favorites of the folder pane the ones Outlook shows, for an account that signs
//! in with Microsoft: read from its server through Microsoft Graph, never written there.
//!
//! The read rides along with the jobs the host does for Graph anyway (the calendar's own job,
//! [`crate::calendar_sync`], and the Graph part of a Send/Receive). It takes two permissions
//! that a sign-in doesn't have until the owner agrees to them on the provider's page, by itself
//! and when the owner chooses to; in an organisation an administrator may have to approve them
//! once.
//!
//! The list is read when the app starts, when the owner asks for a Send/Receive, right after
//! the owner connected it, and otherwise not more often than every quarter of an hour. What
//! comes back is taken in by the engine (`sync.applyFavorites`), and the pane lists what the
//! engine says (`mailcraft_engine::cmd::folder::favorites`). Until there is a list, and whenever
//! the server's can't be had, the favourites kept in the mailbox are shown, untouched. Nothing
//! here ever puts a dialog in front of the owner unasked; what there is to say is a quiet line
//! under the Favorites heading:
//!
//! - the sign-in doesn't cover reading them: the offer, and the server isn't asked again that day;
//! - the owner asked and an administrator has to approve it first: that, with a message to
//!   copy for the administrator and the way to the page that says what to do next
//!   ([`crate::approval`]);
//! - the owner asked and it wasn't to be had (this kind of account has no such permission, the
//!   organisation has shut the door): one plain line saying so;
//! - a read came to nothing: one line with why in a few words, and "Try again".
//!
//! None of these is asked about again that day unless the owner asks.

use std::collections::{HashMap, HashSet};

use mailcraft_engine::cmd::remote;
use mailcraft_model::remote::{FavoritesDownload, FavoritesOutcome, RemoteFavorite};
use mailcraft_model::{Directory, Id};
use serde_json::{Value, json};

use crate::{GraphResult, MailApp, now_ms};

/// How often an account's Favorites are read at most (milliseconds), and for how long the
/// server is left alone after it said the sign-in doesn't cover them.
const EVERY_MS: f64 = 900_000.0;
const DAY_MS: f64 = 86_400_000.0;
/// What a sign-in for the Favorites says when it is not the owner's to turn into a yes: an
/// administrator of the organisation has to approve it first; the organisation doesn't have it
/// at all; this kind of account has no such permission. The host answers a
/// [`crate::SignInRequest`] that has `favorites` with exactly one of these as its
/// error, and the pane shows it (in the interface language).
pub const NEEDS_APPROVAL: &str = "Your organisation's administrator has to approve this once";
pub const REFUSED_BY_ORGANISATION: &str = "Not allowed by your organisation";
pub const REFUSED_FOR_ACCOUNT: &str = "Not available for this account";
/// What the owner is told, once, of a mailbox whose file had them read another way
/// (`Mailbox::take_retired_favorites_choices`): Exchange Web Services, or not at all.
pub const FAVORITES_NOW_GRAPH: &str =
    "Outlook's Favorites are now read through Microsoft Graph. To show MailCraft's own instead, turn them off in File › Options.";

/// What the pane says under the Favorites heading, when there is something to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Note {
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
pub(crate) enum Step {
    /// Copy a message for an administrator, with the address they approve it at
    /// ([`crate::approval`]).
    CopyMessage,
    /// Open the page that says what the owner and an administrator can do
    /// ([`crate::approval`]).
    Help,
}

impl Step {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Step::CopyMessage => "copyMessage",
            Step::Help => "help",
        }
    }

    /// What the link says.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Step::CopyMessage => "Copy a message for my administrator",
            Step::Help => "What to do next",
        }
    }

    /// And what it does, for whoever asks before clicking.
    pub(crate) fn about(self) -> &'static str {
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
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Note::Offer => "offer",
            Note::Approval => "approval",
            Note::Refused(_) => "refused",
            Note::Failed(_) => "failed",
        }
    }

    /// What the line says (to be put into the interface language).
    pub(crate) fn text(&self) -> &str {
        match self {
            Note::Offer => "Show Outlook's Favorites",
            Note::Approval => NEEDS_APPROVAL,
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
    /// The accounts whose sign-in has a token that covers Outlook's Favorites, or whose owner
    /// took the offer to show them.
    connected: HashSet<Id>,
    /// The accounts whose Favorites are read at the next chance, whatever the clock says: the
    /// owner asked.
    due: HashSet<Id>,
    /// Which kind of Microsoft account each is, as a sign-in of this run said, and for an
    /// organisation's the directory it named (for the link an administrator is sent).
    directories: HashMap<Id, Directory>,
    tenants: HashMap<Id, String>,
}

impl MailApp {
    /// Whether `account` signs in with Microsoft.
    pub(crate) fn is_microsoft(&self, account: Id) -> bool {
        let microsoft = |a: &mailcraft_model::Account| a.oauth.as_ref().is_some_and(|o| o.provider == mailcraft_model::OAuthProvider::Microsoft);
        self.session.mailbox.account(account).is_some_and(microsoft)
    }

    /// Whether the Favorites of `account` are read from its server: it signs in with Microsoft,
    /// and the owner hasn't asked to see the mailbox's own everywhere.
    fn reads_from_server(&self, account: Id) -> bool {
        self.is_microsoft(account) && !self.session.prefs.own_favorites
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
    /// which kind of account it is. Kept for the run, and not written into the mailbox.
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
        self.reads_from_server(account) && self.favorites_wanted(account, now, asked_for)
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
                self.favorites.connected.insert(account);
                self.favorites.uncovered.remove(&account);
            }
            Some(false) => {
                self.favorites.uncovered.insert(account);
            }
            None => {}
        }
        let Some(download) = result.favorites.take() else { return };
        let why = std::mem::take(&mut result.favorites_error);
        // (Asked for before the owner turned them off: nobody's answer any more.)
        if !self.reads_from_server(account) {
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
                if self.favorites.connected.contains(&account) || matches!(why, NEEDS_APPROVAL | REFUSED_BY_ORGANISATION | REFUSED_FOR_ACCOUNT) {
                    // (What the owner was told when he asked stays said.)
                    if !(matches!(self.favorites_note_of(account), Some(Note::Refused(_) | Note::Approval)) && why.trim().is_empty()) {
                        let by = match why {
                            REFUSED_BY_ORGANISATION | REFUSED_FOR_ACCOUNT => why,
                            _ => {
                                if self.directory(account) == Directory::Personal {
                                    REFUSED_FOR_ACCOUNT
                                } else {
                                    NEEDS_APPROVAL
                                }
                            }
                        };
                        self.favorites_refused(account, by);
                    }
                } else if !matches!(self.favorites_note_of(account), Some(Note::Refused(_) | Note::Approval)) {
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

    /// What there is to say of the Favorites of `account` under the heading, if anything.
    pub(crate) fn favorites_note_of(&self, account: Id) -> Option<Note> {
        if !self.reads_from_server(account) {
            return None;
        }
        self.favorites.notes.get(&account).cloned()
    }

    /// What the pane says under the Favorites heading: the first account that has something to
    /// say, and what. Nothing while the owner would rather see the mailbox's own favourites.
    pub(crate) fn favorites_note(&self) -> Option<(Id, Note)> {
        let accounts = self.session.mailbox.accounts.iter().map(|a| a.id);
        accounts.filter_map(|id| self.favorites_note_of(id).map(|note| (id, note))).next()
    }

    /// The links under the line that `note` is for `account`: what the owner can do about it.
    pub(crate) fn favorites_steps(&self, note: &Note) -> Vec<Step> {
        match note {
            Note::Approval => vec![Step::CopyMessage, Step::Help],
            Note::Offer | Note::Refused(_) | Note::Failed(_) => Vec::new(),
        }
    }

    /// The account whose Favorites could be Outlook's if the owner connected them: the first
    /// that has the offer standing (File › Options offers it too).
    pub(crate) fn favorites_offer(&self) -> Option<Id> {
        let accounts = self.session.mailbox.accounts.iter().map(|a| a.id);
        accounts.into_iter().find(|id| matches!(self.favorites_note_of(*id), Some(Note::Offer | Note::Refused(_))))
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
        self.calendar.checked.remove(&account);
        self.calendar.failures.remove(&account);
    }

    /// What was remembered of an account that is gone.
    pub(crate) fn forget_favorites(&mut self, account: Id) {
        let kept = &mut self.favorites;
        kept.asked.remove(&account);
        kept.missing.remove(&account);
        kept.notes.remove(&account);
        kept.uncovered.remove(&account);
        kept.connected.remove(&account);
        kept.due.remove(&account);
        kept.directories.remove(&account);
        kept.tenants.remove(&account);
    }

    /// The sign-in for the Favorites of `account` came back with tokens that are good for
    /// them: nothing stands in the way any more, and they are read at once.
    pub(crate) fn favorites_connected(&mut self, account: Id) {
        self.favorites.connected.insert(account);
        self.favorites.notes.remove(&account);
        self.favorites.uncovered.remove(&account);
        self.favorites_due(account);
    }

    /// The sign-in for the Favorites of `account` came to nothing: `error` is why. When the no
    /// is not the owner's to change ([`NEEDS_APPROVAL`], [`REFUSED_BY_ORGANISATION`],
    /// [`REFUSED_FOR_ACCOUNT`]) the pane says so, and nothing is asked again that day; anything
    /// else (the owner said no, the page was closed) goes to the status bar, and the offer stands.
    pub(crate) fn favorites_refused(&mut self, account: Id, error: &str) {
        let note = match error {
            NEEDS_APPROVAL => Note::Approval,
            REFUSED_BY_ORGANISATION | REFUSED_FOR_ACCOUNT => Note::Refused(error.to_string()),
            _ => {
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
        self.status(tl!(error));
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
        let steps: Vec<&str> = note.as_ref().map(|note| self.favorites_steps(note)).unwrap_or_default().into_iter().map(Step::code).collect();
        json!({
            "account": account,
            "from": if from_server { "server" } else { "mailbox" },
            "own": self.session.prefs.own_favorites,
            "directory": self.directory(account).code(),
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

    /// What an administrator's approval is asked for, for `account`: its sign-in settings (as
    /// the account has them) and its organisation's directory when that is known (else empty).
    pub(crate) fn approval_of(&self, account: Id) -> Result<(mailcraft_model::OAuth, String), String> {
        let Some(owner) = self.session.mailbox.account(account) else { return Err(tl!("No account signs in with Microsoft").to_string()) };
        let Some(oauth) = owner.oauth.clone() else { return Err(tl!("This account doesn't sign in with Microsoft").to_string()) };
        // The organisation's own directory, when a sign-in of this run named it or the account's
        // settings do; otherwise the administrator's sign-in says which it is.
        let named = oauth.tenant.trim();
        let settings = (Directory::of_tenant(named) == Directory::Organisation && !named.eq_ignore_ascii_case("organizations")).then_some(named);
        let tenant = self.favorites.tenants.get(&account).map(String::as_str).or(settings).unwrap_or("").to_string();
        Ok((oauth, tenant))
    }

    /// The address of Microsoft's page where an administrator approves this program, for the
    /// sign-in settings `oauth` (with the application id filled in) and the directory `tenant`
    /// (empty: whichever the administrator is of). A directory the host won't make an address of
    /// is left out of it.
    pub(crate) fn admin_link(&self, oauth: &mailcraft_model::OAuth, tenant: &str) -> Result<String, String> {
        let unable = || tl!("This build can't make a link for an administrator.").to_string();
        let make = self.services.admin_consent_link.as_ref().ok_or_else(unable)?;
        let link = make(oauth, tenant).or_else(|| if tenant.is_empty() { None } else { make(oauth, "") });
        link.ok_or_else(unable)
    }

    /// The address an administrator of the organisation of `account` approves this program at.
    fn admin_consent_link(&self, account: Id) -> Result<String, String> {
        let (oauth, tenant) = self.approval_of(account)?;
        self.admin_link(&self.effective_oauth(oauth), &tenant)
    }

    /// `ui.copyAdminConsentLink`: put the address an administrator approves this program at on
    /// the clipboard, for the owner to pass on. Nothing is opened and nothing is sent. (The
    /// window's own links copy a whole message with it: `ui.copyAdminMessage`.)
    fn copy_admin_consent_link(&mut self, p: &Value) -> Result<Value, String> {
        let account = self.favorites_account(p)?;
        let link = self.admin_consent_link(account)?;
        if let Some(ctx) = &self.ctx {
            ctx.copy_text(link.clone());
        }
        self.status(tl!("The link is on the clipboard: send it to your organisation's administrator"));
        Ok(json!({"account": account, "url": link}))
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
                    self.favorites_refused(account, by);
                    return Ok(self.favorites_state(account));
                }
                // (…or that an administrator has to approve it first.)
                "approval" => {
                    self.favorites_refused(account, NEEDS_APPROVAL);
                    return Ok(self.favorites_state(account));
                }
                _ => {}
            }
            let outcome = match outcome {
                "read" => FavoritesOutcome::Read,
                "missing" => FavoritesOutcome::Missing,
                "notPermitted" => FavoritesOutcome::NotPermitted,
                "failed" => FavoritesOutcome::Failed,
                _ => return Err(tl!("`answer.outcome` is read, missing, notPermitted, refused, approval or failed").to_string()),
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
        if self.services.graph.is_none() {
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
/// them, and the way to connect them when its sign-in doesn't cover reading them yet. Nothing is
/// shown while no account signs in with Microsoft.
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

/// Do what a link under a quiet line says ([`Step`]).
pub(crate) fn step_taken(app: &mut MailApp, account: Id, step: Step) {
    let done = match step {
        Step::CopyMessage => app.run("ui.copyAdminMessage", json!({"account": account})),
        Step::Help => app.run("ui.approvalHelp", json!({"account": account})),
    };
    if let Err(e) = done {
        app.status(e);
    }
}

/// The `ui.*` commands of the Favorites; `None` for any other.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.connectFavorites" => app.connect_favorites(p),
        "ui.favorites" => app.favorites_command(p),
        "ui.copyAdminConsentLink" => app.copy_admin_consent_link(p),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
