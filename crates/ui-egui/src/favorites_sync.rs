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

use mailcraft_engine::cmd::remote;
use mailcraft_model::remote::{FavoritesDownload, FavoritesOutcome, RemoteFavorite};
use mailcraft_model::{Directory, Id};
use serde_json::{Value, json};

use crate::{GraphResult, MailApp, now_ms};

// How often an account's Favorites are read at most, and for how long the server is left alone
// after it said the sign-in doesn't cover them, is the engine's ([`mailcraft_engine::sync_driver`]).
// What a sign-in for the Favorites says when it is not the owner's to turn into a yes, the line
// under the Favorites heading with its links, and what is remembered of keeping the Favorites
// current are the engine's ([`mailcraft_engine::sync_driver::favorites`]).
pub use mailcraft_engine::sync_driver::{FAVORITES_NOW_GRAPH, NEEDS_APPROVAL, REFUSED_BY_ORGANISATION, REFUSED_FOR_ACCOUNT};
pub(crate) use mailcraft_engine::sync_driver::{Note, Step};

impl MailApp {
    /// Whether `account` signs in with Microsoft.
    pub(crate) fn is_microsoft(&self, account: Id) -> bool {
        self.session.is_microsoft(account)
    }

    /// Which kind of Microsoft account `account` is ([`mailcraft_engine::Session::account_directory`]).
    pub(crate) fn directory(&self, account: Id) -> Directory {
        self.session.account_directory(account)
    }

    /// A sign-in of `account` named the directory `tenant` (an identity token's): that says
    /// which kind of account it is. Kept for the run, and not written into the mailbox.
    pub(crate) fn note_directory(&mut self, account: Id, tenant: &str) {
        self.session.driver.favorites.note_directory(account, tenant);
    }

    /// Whether a Graph job for `account` that starts at `now` reads the Favorites (and so asks
    /// for a token that is good for reading them, when it has to ask for one: a job that
    /// doesn't read them asks for nothing it didn't always ask for). `asked_for` when the owner
    /// asked for the job (a Send/Receive). Notes that they were asked for when they are.
    pub(crate) fn favorites_ask(&mut self, account: Id, now: f64, asked_for: bool) -> bool {
        self.session.reads_favorites_from_server(account) && self.session.driver.favorites.wanted(account, now, asked_for)
    }

    /// The Graph token to hand a job for `account`: the one in memory when it is still good,
    /// unless the job reads the Favorites and this one is known not to be good for that. A job
    /// that will have to ask for a token, and not for one that is good for the Favorites, leaves
    /// the account with one that isn't: that is noted here.
    pub(crate) fn graph_access_for(&mut self, account: Id, favorites: bool) -> String {
        let access = self.graph_access(account, favorites);
        if access.is_empty() && !favorites {
            self.session.driver.favorites.token_covers(account, false);
        }
        access
    }

    /// [`graph_access_for`](Self::graph_access_for), without noting anything.
    pub(crate) fn graph_access(&self, account: Id, favorites: bool) -> String {
        if favorites && self.session.driver.favorites.is_uncovered(account) {
            return String::new();
        }
        self.graph_tokens.get(&account).filter(|t| t.fresh(mailcraft_time::now())).map(|t| t.access.clone()).unwrap_or_default()
    }

    /// A Graph token for `account` came from somewhere that didn't ask for the Favorites'
    /// permissions with it (a check of the calendar connection, "Connect calendar and contacts").
    pub(crate) fn favorites_token_is_plain(&mut self, account: Id) {
        self.session.driver.favorites.token_covers(account, false);
    }

    /// Take in what a Graph job said of the Favorites of `account`: whether its token covers
    /// them, and the list, or why there is none. Never a dialog, never the problems report:
    /// what there is to say is said under the Favorites heading.
    pub(crate) fn take_in_favorites(&mut self, account: Id, result: &mut GraphResult) {
        self.session.driver.favorites.job_said(account, result.favorites_covered);
        let Some(download) = result.favorites.take() else { return };
        let why = std::mem::take(&mut result.favorites_error);
        // (Asked for before the owner turned them off: nobody's answer any more.)
        if !self.session.reads_favorites_from_server(account) {
            return;
        }
        self.apply_favorites_download(account, &download, &why);
    }

    /// [`take_in_favorites`](Self::take_in_favorites) for one download; `why` is what went
    /// wrong, in a few plain words, when the read failed.
    fn apply_favorites_download(&mut self, account: Id, download: &FavoritesDownload, why: &str) {
        let directory = self.directory(account);
        if let Some(by) = self.session.driver.favorites.took_in(account, download.outcome, why, directory, now_ms()) {
            self.favorites_refused(account, &by);
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
        self.session.favorites_note_of(account)
    }

    /// What the pane says under the Favorites heading: the first account that has something to
    /// say, and what. Nothing while the owner would rather see the mailbox's own favourites.
    pub(crate) fn favorites_note(&self) -> Option<(Id, Note)> {
        self.session.favorites_note()
    }

    /// The links under the line that `note` is for `account`: what the owner can do about it.
    pub(crate) fn favorites_steps(&self, note: &Note) -> Vec<Step> {
        note.steps()
    }

    /// The account whose Favorites could be Outlook's if the owner connected them: the first
    /// that has the offer standing (File › Options offers it too).
    pub(crate) fn favorites_offer(&self) -> Option<Id> {
        self.session.favorites_offer()
    }

    /// The Favorites of `account` are read at the next chance, whatever the clock says and
    /// whatever was said before: the owner asked. The calendar's own job carries the read
    /// through Microsoft Graph, so it is made due too.
    pub(crate) fn favorites_due(&mut self, account: Id) {
        if !self.is_microsoft(account) {
            return;
        }
        self.session.driver.favorites.owner_asked(account);
        self.session.driver.calendar.book.forget_checked(account);
        self.session.driver.calendar.book.forget_failures(account);
    }

    /// What was remembered of an account that is gone.
    pub(crate) fn forget_favorites(&mut self, account: Id) {
        self.session.driver.favorites.forget(account);
    }

    /// The sign-in for the Favorites of `account` came back with tokens that are good for
    /// them: nothing stands in the way any more, and they are read at once.
    pub(crate) fn favorites_connected(&mut self, account: Id) {
        self.session.driver.favorites.signed_in(account);
        self.favorites_due(account);
    }

    /// The sign-in for the Favorites of `account` came to nothing: `error` is why. When the no
    /// is not the owner's to change ([`NEEDS_APPROVAL`], [`REFUSED_BY_ORGANISATION`],
    /// [`REFUSED_FOR_ACCOUNT`]) the pane says so, and nothing is asked again that day; anything
    /// else (the owner said no, the page was closed) goes to the status bar, and the offer stands.
    pub(crate) fn favorites_refused(&mut self, account: Id, error: &str) {
        let directory = self.directory(account);
        if self.session.driver.favorites.refused(account, error, directory, now_ms()) {
            self.status(tl!(error));
        } else {
            self.status(error.to_string());
        }
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
        let tenant = self.session.driver.favorites.tenants.get(&account).map(String::as_str).or(settings).unwrap_or("").to_string();
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
        if matches!(self.session.driver.favorites.notes.get(&account), Some(Note::Failed(_))) {
            self.session.driver.favorites.notes.remove(&account);
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
