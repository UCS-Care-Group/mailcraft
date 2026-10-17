//! Shared mailboxes in the window (#70, a draft): read through Microsoft Graph with the sign-in
//! of the account they are opened through, at their own address.
//!
//! A shared mailbox's Send/Receive is its folders and mail alone, what was changed in it here going
//! up first, to its own address, with its drafts and its Outbox: no calendar, no contacts, no
//! Favorites. Mail written in it is from its address, and goes through it. The first time
//! something waits to be sent from one, the window asks the owner whether to go ahead, and then
//! Microsoft's page asks him for `Mail.Send.Shared`, by itself; until then nothing of it is sent
//! (it waits), and a no holds it in its Outbox with why. Its token is
//! renewed for the run with `Mail.ReadWrite.Shared`; when the owner's sign-in doesn't cover that
//! yet, he is asked for it on Microsoft's page, by itself, when he asked for the run (adding a
//! shared mailbox does), and never with a sign-in of any other kind. The refresh token that
//! comes back is the owner's, kept as his; the access token is the shared mailbox's, in memory,
//! for fetching its messages whole. A server that won't open the mailbox for the owner says so,
//! by Graph's code: the mailbox shows No access, the mail already here stays, and it is not read
//! again by a run that started by itself until the owner asks.
//!
//! Add Shared Mailbox checks the address at once (S1): the mailbox is added, its Inbox read by
//! its first run, and it is kept only when that worked; otherwise it is removed again and the
//! dialog says why. Only while shared mailboxes are being tested
//! ([`mailcraft_engine::Session::shared_mailboxes`]).

use mailcraft_model::sync::SharedRefusal;
use mailcraft_model::{Account, AccountKind, Directory, FolderKind, Id};
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::{GraphRequest, GraphResult, MailApp, Tokens};

/// Whether `a` is an account a shared mailbox may be opened under: a Microsoft account kept in
/// step through Graph, of a work or school (not a personal Microsoft account, which has none),
/// and no shared mailbox itself.
pub(crate) fn can_open_shared(a: &Account) -> bool {
    a.kind == AccountKind::Microsoft && !a.is_shared() && a.oauth.as_ref().is_none_or(|oauth| oauth.directory != Directory::Personal)
}

/// What the owner is told when Microsoft won't open the shared mailbox at `address` (S1).
fn refused_words(why: SharedRefusal, address: &str) -> String {
    let words = match why {
        SharedRefusal::NoAccess => tl!("You don't have access to {address}. If access was just given, try again in an hour."),
        SharedRefusal::NoMailbox => tl!("There's no mailbox at {address}."),
        SharedRefusal::NotThroughGraph => tl!("The mailbox at {address} can't be reached through Microsoft Graph."),
    };
    crate::i18n::fmt(words, &[("address", address)])
}

impl MailApp {
    /// A Send/Receive of the shared mailbox `account`: its folders and mail, with its owner's
    /// sign-in.
    pub(crate) fn start_shared_mail(&mut self, account: Id, quiet: bool) -> Result<Value, String> {
        let mailbox = self.session.mailbox.account(account).cloned().ok_or_else(|| tl!("The account is gone").to_string())?;
        let owner = self
            .session
            .mailbox
            .owner_of(account)
            .cloned()
            .ok_or_else(|| tl!("This shared mailbox's account is gone: remove the shared mailbox and add it again").to_string())?;
        if !self.session.shared_mailboxes {
            return Err(tl!(mailcraft_engine::cmd::shared::SWITCHED_OFF).to_string());
        }
        if quiet && self.shared_refused.contains_key(&account) {
            return Err(tl!("Microsoft won't open this shared mailbox for you: Send/Receive tries again").to_string());
        }
        let oauth = owner.oauth.clone().ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        let address = owner.email.clone();
        let kept = self.tokens.get(&owner.id).or(self.graph_tokens.get(&owner.id)).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", &address)).unwrap_or_default();
        if refresh.is_empty() {
            // Being added: the owner's own sign-in comes first, by itself.
            if self.shared_checking.contains(&account) {
                return Err(crate::i18n::fmt(tl!("Sign in to {owner} first (Send/Receive), then add the shared mailbox."), &[("owner", &address)]));
            }
            if quiet {
                return Err(format!("{} {address}", tl!("Use Send/Receive to sign in to")));
            }
            // The owner's own sign-in first; the shared mailbox is read after it, when asked.
            self.begin_sign_in(owner.id, oauth, &address, String::new(), false, true)?;
            return Ok(json!({"pending": "signIn", "account": owner.id}));
        }
        let mail = mailcraft_engine::cmd::graph_mail::graph_mail_request(&self.session, account).map_err(|e| e.to_string())?;
        let (pieces, taken) = std::sync::mpsc::sync_channel(8);
        let ask = GraphRequest {
            account: mailbox,
            oauth: self.effective_oauth(oauth),
            refresh,
            // (Renewed for the run, with what reading a shared mailbox takes.)
            access: String::new(),
            outbox: Vec::new(),
            directory: false,
            calendar: None,
            changes: None,
            asks: Vec::new(),
            settings: false,
            favorites: false,
            favorites_scopes: false,
            mail: Some(mail),
            mail_pieces: Some(pieces),
            cancel: self.services.sync_cancel.clone(),
        };
        self.resume_fetch();
        let Some(graph) = &self.services.graph else { return Err(tl!("This build can't connect to Microsoft").to_string()) };
        self.services.sync_cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        self.graph_pieces = Some(taken);
        self.graph_problems.clear();
        // Asked for: the owner wants to know whether the server opens it now.
        if !quiet {
            self.shared_refused.remove(&account);
        }
        self.graph_rx = Some((account, quiet, false, graph(ask)));
        self.session.sync_status = tl!("Sending and receiving…").to_string();
        Ok(json!({"started": true, "account": account, "graph": true, "shared": true}))
    }

    /// A shared mailbox's run came back (its pieces are in already): keep the owner's refresh
    /// token as his and the access token for this mailbox, ask for the permission when it is
    /// missing, and say what went wrong.
    pub(crate) fn finish_shared_mail(&mut self, account: Id, quiet: bool, mut result: GraphResult) {
        let owner = self.session.mailbox.owner_of(account).map(|o| (o.id, o.email.clone()));
        let mut errors = std::mem::take(&mut result.errors);
        errors.append(&mut self.graph_problems);
        if result.token_refused {
            self.graph_tokens.remove(&account);
        }
        if let (Some(tokens), Some((owner, address))) = (result.tokens.take(), owner.as_ref()) {
            if !tokens.refresh.is_empty() {
                self.remember("refresh", address, &tokens.refresh);
                if let Some(kept) = self.tokens.get_mut(owner) {
                    kept.refresh = tokens.refresh.clone();
                }
            }
            let now = mailcraft_time::now();
            self.graph_tokens.insert(account, Tokens { expires_at: tokens.expires_at.clamp(now, now.saturating_add(86_400)), ..tokens });
        }
        if let Some(mail) = result.mail.take() {
            if let Some(why) = mail.refused {
                self.shared_refused.insert(account, why);
            }
            match mailcraft_engine::cmd::graph_mail::apply_graph_mail(&mut self.session, mail) {
                Ok(taken) => {
                    let said = taken["errors"].as_array().map(|all| all.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                    errors.extend(said.unwrap_or_default());
                }
                Err(e) => errors.push(e.to_string()),
            }
        }
        // The owner's sign-in was refused (run out, taken back, #124): signing in again is theirs
        // to do, from their own account's Send/Receive.
        if result.sign_in_refused && !result.consent_needed {
            errors.push(
                tl!("The sign-in of the account this shared mailbox is opened through was refused: sign in again with Send/Receive").to_string(),
            );
        }
        let mut asking = false;
        if result.consent_needed {
            self.session.sync_status.clear();
            if quiet {
                errors.push(tl!("Reading shared mailboxes needs your permission: use Send/Receive").to_string());
            } else {
                match self.begin_shared_sign_in(account) {
                    Ok(()) => asking = true,
                    Err(e) => errors.push(e),
                }
            }
        }
        // Mail waits to go from it, and sending from shared mailboxes isn't allowed yet (#70):
        // asked about when the owner asked for the run, else said.
        if result.send_consent_needed
            && self.session.mailbox.account(account).is_some()
            && (quiet || asking || self.dialog.is_some() || !self.ask_shared_send(account))
        {
            errors.push(
                tl!("Mail in this shared mailbox's Outbox waits: sending from shared mailboxes needs your permission. Use Send/Receive.").to_string(),
            );
        }
        errors.dedup();
        // Being added: kept when its Inbox was read, else removed again with the reason (S1).
        if !asking && self.shared_checking.contains(&account) {
            let mb = &self.session.mailbox;
            let read = mb.folder_of_kind(account, FolderKind::Inbox).and_then(|f| mb.folder(f)).is_some_and(|f| f.server.is_some());
            let address = mb.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
            let outcome = match (read, self.shared_refused.get(&account)) {
                (true, _) => Ok(()),
                (false, Some(why)) => Err(refused_words(*why, &address)),
                (false, None) => Err(match errors.first() {
                    Some(first) => format!("{} {first}", crate::i18n::fmt(tl!("{address} couldn't be opened:"), &[("address", &address)])),
                    None => crate::i18n::fmt(tl!("{address} couldn't be opened."), &[("address", &address)]),
                }),
            };
            self.finish_shared_check(account, outcome);
        }
        // Access taken away: the mail already here stays, and what waits here is named (S6).
        if let Some(why) = self.shared_refused.get(&account).copied().filter(|_| self.session.mailbox.account(account).is_some()) {
            let waiting = self.waiting_to_go(account);
            if waiting > 0 {
                errors.push(crate::i18n::fmt(
                    tl!("{why}. {n} changes made in it here wait on this computer."),
                    &[("why", why.said()), ("n", &waiting.to_string())],
                ));
            }
        }
        if let Some(first) = errors.first() {
            self.session.sync_status = first.clone();
        } else if !result.consent_needed {
            self.session.sync_status.clear();
        }
        self.note_sync_problems(account, &errors, true);
    }

    /// Ask the owner of the shared mailbox `shared`, on Microsoft's page, to let this program read
    /// shared mailboxes: by itself, because he wants this one read.
    pub(crate) fn begin_shared_sign_in(&mut self, shared: Id) -> Result<(), String> {
        let owner = self.session.mailbox.owner_of(shared).cloned().ok_or_else(|| tl!("This shared mailbox's account is gone").to_string())?;
        let oauth = owner.oauth.clone().ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        self.shared_next = Some(shared);
        let begun = self.begin_sign_in(owner.id, oauth, &owner.email, String::new(), false, true);
        // (A sign-in that couldn't start doesn't leave the question for another one.)
        self.shared_next = None;
        begun?;
        self.session.sync_status = tl!("Allow MailCraft to read shared mailboxes on the page that opened in your browser…").to_string();
        Ok(())
    }

    /// The owner answered on Microsoft's page about shared mailboxes: with the tokens, the shared
    /// mailbox is read now; without, why not is said (and one being added isn't kept).
    pub(crate) fn shared_signed_in(&mut self, shared: Id, owner: Id, answer: Result<Tokens, String>) {
        self.session.sync_status.clear();
        let address = self.session.mailbox.account(owner).map(|a| a.email.clone()).unwrap_or_default();
        let refused = match answer {
            // The owner picks who signs in on the page: it has to be the account it is opened
            // through. Somebody else's tokens are neither used nor remembered (#103).
            Ok(tokens) if self.signed_in_as_other(owner, &tokens) => Some(self.status_msg.as_ref().map(|(said, _)| said.clone()).unwrap_or_default()),
            Ok(tokens) => {
                if !tokens.refresh.is_empty() {
                    self.remember("refresh", &address, &tokens.refresh);
                    if let Some(kept) = self.tokens.get_mut(&owner) {
                        kept.refresh = tokens.refresh.clone();
                    }
                }
                self.graph_tokens.insert(shared, tokens);
                self.start_sync(&json!({"account": shared})).err()
            }
            Err(e) if e == crate::favorites_sync::NEEDS_APPROVAL => {
                Some(tl!("Your organisation's administrator has to approve reading shared mailboxes once: File › Info").to_string())
            }
            Err(e) => Some(format!("{}: {e}", tl!("Shared mailboxes weren't allowed"))),
        };
        if let Some(said) = refused {
            if self.shared_checking.contains(&shared) {
                self.finish_shared_check(shared, Err(said));
            } else {
                self.status(said);
            }
        }
    }

    /// The first send from the shared mailbox `shared` (#70): ask the owner whether Microsoft's
    /// page may ask him to let MailCraft send from shared mailboxes. Whether it was asked.
    pub(crate) fn ask_shared_send(&mut self, shared: Id) -> bool {
        let Some(address) = self.session.mailbox.account(shared).filter(|a| a.is_shared()).map(|a| a.email.trim().to_string()) else { return false };
        let text = crate::i18n::fmt(
            tl!(
                "To send from {address}, MailCraft needs your permission to send from shared mailboxes. Microsoft's page opens in your browser to ask you, once.\n\nWhether your mail goes as {address} or on its behalf is your organisation's choice. Until you allow it, the mail waits in its Outbox, and nothing is sent from your own address."
            ),
            &[("address", &address)],
        );
        let ask = crate::dialogs::Question {
            title: tl!("Send from a Shared Mailbox").to_string(),
            text,
            action: tl!("Continue").to_string(),
            with: Value::Null,
            more: Vec::new(),
            note: None,
        };
        self.dialog = Some(Dialog::Confirm { command: "ui.allowSharedSend".to_string(), params: json!({"account": shared}), ask });
        true
    }

    /// `ui.allowSharedSend`: Microsoft's page, for sending from shared mailboxes too, because the
    /// owner is sending from `shared` and said go ahead.
    pub(crate) fn begin_shared_send_sign_in(&mut self, shared: Id) -> Result<Value, String> {
        if !self.session.shared_mailboxes {
            return Err(tl!(mailcraft_engine::cmd::shared::SWITCHED_OFF).to_string());
        }
        if !self.session.mailbox.account(shared).is_some_and(Account::is_shared) {
            return Err(tl!("That isn't a shared mailbox").to_string());
        }
        let owner = self.session.mailbox.owner_of(shared).cloned().ok_or_else(|| tl!("This shared mailbox's account is gone").to_string())?;
        let oauth = owner.oauth.clone().ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        (self.shared_next, self.shared_send_next) = (Some(shared), true);
        let begun = self.begin_sign_in(owner.id, oauth, &owner.email, String::new(), false, true);
        (self.shared_next, self.shared_send_next) = (None, false);
        begun?;
        self.session.sync_status = tl!("Allow MailCraft to send from shared mailboxes on the page that opened in your browser…").to_string();
        Ok(json!({"pending": "signIn", "account": owner.id}))
    }

    /// The owner answered on Microsoft's page about sending from shared mailboxes: with the
    /// tokens, what waits in `shared`'s Outbox goes now; without, it is held there with why
    /// (an administrator's approval has its own page), or waits when the page never came up.
    pub(crate) fn shared_send_signed_in(&mut self, shared: Id, owner: Id, answer: Result<Tokens, String>) {
        use crate::approval::NO_BROWSER;
        use crate::favorites_sync::{NEEDS_APPROVAL, REFUSED_BY_ORGANISATION, REFUSED_FOR_ACCOUNT};
        self.session.sync_status.clear();
        let address = self.session.mailbox.account(owner).map(|a| a.email.clone()).unwrap_or_default();
        let why = match answer {
            // (Somebody else's tokens are neither used nor remembered, #103.)
            Ok(tokens) if self.signed_in_as_other(owner, &tokens) => return,
            Ok(tokens) => {
                if !tokens.refresh.is_empty() {
                    self.remember("refresh", &address, &tokens.refresh);
                    if let Some(kept) = self.tokens.get_mut(&owner) {
                        kept.refresh = tokens.refresh.clone();
                    }
                }
                self.graph_tokens.insert(shared, tokens);
                if let Err(e) = self.start_sync(&json!({"account": shared})) {
                    self.status(e);
                }
                return;
            }
            Err(e) if e == NEEDS_APPROVAL => "approval",
            Err(e) if e == REFUSED_BY_ORGANISATION || e == REFUSED_FOR_ACCOUNT => "organisation",
            // The page never came up: nothing was refused, so the mail waits, and the next
            // Send/Receive asks again.
            Err(e) if e == NO_BROWSER => {
                self.status(format!("{}: {e}", tl!("Mail from the shared mailbox waits")));
                return;
            }
            Err(_) => "declined",
        };
        let _ = self.execute("sync.holdSends", json!({"account": shared, "why": why}));
        if why == "approval" {
            // What to do next (#124): an administrator approves once, for everybody.
            let _ = self.run("ui.approvalHelp", json!({"account": owner}));
        }
    }

    /// Add Shared Mailbox: read the shared mailbox `shared`, just added, to see whether it is to
    /// be kept ([`MailApp::finish_shared_check`]). When the run can't start, it is removed again.
    pub(crate) fn begin_shared_check(&mut self, shared: Id) -> Result<(), String> {
        self.shared_checking.insert(shared);
        match self.start_sync(&json!({"account": shared})) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.drop_shared_check(shared);
                Err(e)
            }
        }
    }

    /// A shared mailbox being added that isn't to be kept: removed again, with what was kept for
    /// it in memory. (Not undoable by itself: it was never there for the owner.)
    pub(crate) fn drop_shared_check(&mut self, shared: Id) {
        self.shared_checking.remove(&shared);
        self.shared_refused.remove(&shared);
        self.graph_tokens.remove(&shared);
        if self.session.mailbox.account(shared).is_some_and(Account::is_shared) {
            let _ = self.execute("account.removeSharedMailbox", json!({"id": shared}));
        }
    }

    /// What reading a shared mailbox being added came to: kept and the dialog closed, or removed
    /// again and the reason said in the dialog (or the status bar, when the dialog is gone).
    pub(crate) fn finish_shared_check(&mut self, shared: Id, outcome: Result<(), String>) {
        if !self.shared_checking.contains(&shared) {
            return;
        }
        let ours = matches!(&self.dialog, Some(Dialog::SharedMailbox { checking: Some(id), .. }) if *id == shared);
        match outcome {
            Ok(()) => {
                self.shared_checking.remove(&shared);
                if ours {
                    self.dialog = None;
                }
            }
            Err(said) => {
                self.drop_shared_check(shared);
                self.session.sync_status.clear();
                match &mut self.dialog {
                    Some(Dialog::SharedMailbox { checking, error, .. }) if ours => {
                        *checking = None;
                        *error = said;
                    }
                    _ => self.status(said),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "shared_mailboxes_tests.rs"]
mod tests;
