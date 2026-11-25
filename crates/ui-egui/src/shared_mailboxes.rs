// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared mailboxes in the window (#70, a draft): read through Microsoft Graph with the sign-in
//! of the account they are opened through, at their own address.
//!
//! A shared mailbox's Send/Receive is its folders and mail alone, what was changed in it here going
//! up first, to its own address, with its drafts and its Outbox: no calendar, no contacts, no
//! Favorites. Mail written in it is from its address, and goes through it, with a token that
//! covers sending from shared mailboxes (`Mail.Send.Shared`); its token for the run is renewed
//! with `Mail.ReadWrite.Shared`. A sign-in's page asks for both with everything else. When an
//! older sign-in doesn't cover them, the one page for it ([`crate::server_sync`]) asks: at once,
//! without a question, when the owner asked for the mailbox to be read (Send/Receive, or adding
//! it); with the one question first when something waits to be sent from it and the owner asked
//! for the run or pressed Send in it. Until then nothing of it is sent (it waits), and a no holds
//! it in its Outbox with why; mail that waits for that starts no run by itself, and a run that
//! starts by itself leaves it where it is (#155). The refresh token that comes back is the
//! owner's, kept as his; the access token is the shared mailbox's, in memory, for fetching its
//! messages whole. A server that won't open the mailbox for the owner says so,
//! by Graph's code: the mailbox shows No access, the mail already here stays, and it is not read
//! again by a run that started by itself until the owner asks.
//!
//! Add Shared Mailbox checks the address at once (S1): the mailbox is added, its Inbox read by
//! its first run, and it is kept only when that worked; otherwise it is removed again and the
//! dialog says why. It is kept as soon as the run's first piece (the folders) shows the Inbox
//! read: the dialog doesn't wait for the rest of the mailbox, which comes down behind it. A
//! Send/Receive under way when Add is pressed is called off for it (it loses nothing, and goes
//! on after, as it was asked for), instead of the new mailbox being refused for it; so is a
//! sign-in left waiting in the browser. Once the new mailbox is kept, the rest of it waits for
//! the runs in line: its own run is called off in turn and goes on after them (#274).

use mailcraft_engine::sync_driver::Pressed;
use mailcraft_model::sync::SharedRefusal;
use mailcraft_model::{Account, AccountKind, FolderKind, Id};
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::{GraphRequest, GraphResult, MailApp, Tokens};

/// Whether `a` is an account a shared mailbox may be opened under: a Microsoft account kept in
/// step through Graph, of a work or school (not a personal Microsoft account, which has none),
/// and no shared mailbox itself.
pub(crate) fn can_open_shared(a: &Account) -> bool {
    a.kind == AccountKind::Microsoft && !a.is_shared() && a.oauth.as_ref().is_none_or(|oauth| !oauth.is_personal(&a.email))
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

/// A line of the Send/Receive report about the shared mailbox at `address`: which one, and what
/// happens next. A refusal is left alone until the owner asks (File › Info says why); anything
/// else is tried again at the next Send/Receive (#539).
fn shared_problem_words(address: &str, error: &str, refused: bool) -> String {
    let error = error.trim().trim_end_matches('.');
    let words =
        if refused { tl!("{address}: {error}. See File › Info.") } else { tl!("{address}: {error}. It will try again at the next Send/Receive.") };
    crate::i18n::fmt(words, &[("address", address), ("error", error)])
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
        if quiet && self.shared_refused.contains_key(&account) {
            return Err(tl!("Microsoft won't open this shared mailbox for you: Send/Receive tries again").to_string());
        }
        let oauth = owner.oauth.clone().ok_or_else(|| tl!("This account has no sign-in with Microsoft").to_string())?;
        let address = owner.email.clone();
        let kept = self.tokens.get(&owner.id).or(self.graph_tokens.get(&owner.id)).map(|t| t.refresh.clone()).filter(|r| !r.is_empty());
        let refresh = kept.or_else(|| self.remembered("refresh", owner.id)).unwrap_or_default();
        if refresh.is_empty() {
            // Being added: the owner's own sign-in comes first, by itself.
            if self.shared_checking.contains(&account) {
                return Err(crate::i18n::fmt(tl!("Sign in to {owner} first (Send/Receive), then add the shared mailbox."), &[("owner", &address)]));
            }
            if quiet {
                return Err(crate::i18n::fmt(tl!("Use Send/Receive to sign in to {address}"), &[("address", &address)]));
            }
            // The owner's own sign-in first; the shared mailbox is read after it, when asked.
            self.begin_sign_in(owner.id, oauth, &address, String::new(), false, true)?;
            return Ok(json!({"pending": "signIn", "account": owner.id}));
        }
        let mut mail = mailcraft_engine::cmd::graph_mail::graph_mail_request(&self.session, account).map_err(|e| e.to_string())?;
        // Its Outbox waits for the owner's permission (#70): a run nobody asked for leaves it
        // there, and asks Microsoft nothing about it.
        if quiet && self.session.shared_send_waits(account) {
            mail.sends.clear();
        }
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
            // A renewed sign-in that isn't the owner's stops the run before the Outbox or
            // anything of the mail goes.
            signer: Some(owner.clone()),
            contacts: None,
        };
        self.resume_fetch();
        if self.offline() {
            return Err(crate::send_receive::said());
        }
        if self.services.graph.is_none() {
            return Err(tl!("This build can't connect to Microsoft").to_string());
        }
        self.services.sync_cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        // What it sends is marked so, on disk, before it leaves (AU3-26).
        let opening = self.session.openings();
        let rx = self.hand_graph(account, ask)?;
        self.graph_pieces = Some(taken);
        self.graph_problems.clear();
        self.graph_pieces_taken = 0;
        // Asked for: the owner wants to know whether the server opens it now.
        // (And whether its mail may be sent from it now.)
        if !quiet {
            self.shared_refused.remove(&account);
            self.session.driver.sending.allowed(account);
        }
        self.sync_run = Some((account, None, false, quiet));
        self.graph_rx = Some((account, quiet, false, opening, rx));
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
        // (Only the owner's own sign-in: somebody else's is neither kept nor remembered, AU3-10.)
        let tokens = result.tokens.take().filter(|tokens| owner.as_ref().is_some_and(|(owner, _)| !self.signed_in_as_other(*owner, tokens)));
        if let (Some(tokens), Some((owner, _))) = (tokens, owner.as_ref()) {
            if !tokens.refresh.is_empty() {
                self.remember("refresh", *owner, &tokens.refresh);
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
        // Which mailbox, and what happens next (#539): this is the owner's own window, so the
        // address may be here; the log never gets these lines.
        let address = self.session.mailbox.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
        let refused = self.shared_refused.contains_key(&account);
        for line in &mut errors {
            *line = shared_problem_words(&address, line, refused);
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
            let given = owner.as_ref().map(|(owner, _)| *owner).filter(|owner| self.shared_page_given.contains(owner));
            if quiet {
                errors.push(tl!("Reading shared mailboxes needs your permission: use Send/Receive").to_string());
            } else if let Some(owner) = given {
                // The page was answered with the shared permissions already in this session and
                // Microsoft still says they are missing: not put again by itself (File › Info's
                // Ask again is the owner's way), and what waited is held with why.
                errors.extend(self.shared_sync_not_given(owner, None));
            } else {
                match self.begin_shared_sign_in(account) {
                    Ok(()) => asking = true,
                    Err(e) => errors.push(e),
                }
            }
        }
        // Mail waits to go from it, and sending from shared mailboxes isn't allowed yet (#70):
        // asked about when the owner asked for the run or pressed Send in it, else said. Either
        // way it waits for the owner: no run starts again for it by itself.
        let missing = if result.send_consent_needed { self.session.shared_send_missing(account) } else { None };
        if let Some(pressed) = missing {
            if (quiet && !pressed) || asking || self.dialog.is_some() || !self.ask_shared_send(account) {
                errors.push(
                    tl!("Mail in this shared mailbox's Outbox waits: sending from shared mailboxes needs your permission. Use Send/Receive.")
                        .to_string(),
                );
            }
        } else {
            // (What was pressed Send on is gone once the Outbox is empty.)
            self.session.shared_send_none_waits(account);
        }
        errors.dedup();
        // Being added: kept when its Inbox was read, else removed again with the reason (S1).
        if !asking && self.shared_checking.contains(&account) {
            let read = self.shared_inbox_read(account);
            let address = self.session.mailbox.account(account).map(|a| a.email.trim().to_string()).unwrap_or_default();
            let outcome = match (read, self.shared_refused.get(&account)) {
                (true, _) => Ok(()),
                (false, Some(why)) => Err(refused_words(*why, &address)),
                (false, None) => Err(match errors.first() {
                    Some(first) => crate::i18n::fmt(tl!("{address} couldn't be opened: {error}"), &[("address", &address), ("error", first)]),
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
        self.note_problems_from("shared", account, &errors, None, true);
        self.note_pushed(account);
    }

    /// The owner pressed Send on `message` (#70). From a shared mailbox that may not be sent from
    /// yet, the window asks at once; otherwise the run that takes it out asks if it finds so,
    /// though that run starts by itself.
    pub(crate) fn shared_send_pressed_in(&mut self, message: Id) {
        if let Pressed::Ask(account) = self.session.shared_send_pressed_in(message)
            && self.dialog.is_none()
        {
            self.ask_shared_send(account);
        }
    }

    /// The owner asked for the shared mailbox `shared` to be read, and the sign-in of the account
    /// it is opened through doesn't cover shared mailboxes: the one page for an older sign-in, at
    /// once (his own doing), which asks for this with everything else; `shared` is read once it
    /// is answered.
    pub(crate) fn begin_shared_sign_in(&mut self, shared: Id) -> Result<(), String> {
        let owner = self.session.mailbox.owner_of(shared).map(|o| o.id).ok_or_else(|| tl!("This shared mailbox's account is gone").to_string())?;
        self.shared_next = Some(shared);
        let begun = self.begin_sync_sign_in(owner);
        // (A sign-in that couldn't start doesn't leave the mailbox for another one.)
        self.shared_next = None;
        begun?;
        self.session.sync_status = tl!("Allow MailCraft to read shared mailboxes on the page that opened in your browser…").to_string();
        Ok(())
    }

    /// Something waits to be sent from the shared mailbox `shared` and the sign-in of the account
    /// it is opened through doesn't cover that: the one question for an older sign-in, because the
    /// owner asked for the run or pressed Send in it (so even after a Not now). Whether it was
    /// asked.
    pub(crate) fn ask_shared_send(&mut self, shared: Id) -> bool {
        let Some(owner) = self.session.mailbox.owner_of(shared).map(|o| o.id) else { return false };
        self.ask_sync_again(owner)
    }

    /// Whether the Inbox of the shared mailbox `shared` has been read from the server: its
    /// folders came down, so Microsoft opened it for the owner.
    fn shared_inbox_read(&self, shared: Id) -> bool {
        let mb = &self.session.mailbox;
        mb.folder_of_kind(shared, FolderKind::Inbox).and_then(|f| mb.folder(f)).is_some_and(|f| f.server.is_some())
    }

    /// A piece of the first run of a shared mailbox being added came in: once its Inbox has been
    /// read, it is kept and the dialog closes, and the rest of its mail comes down behind it.
    /// (The whole of a big mailbox can take a long while; the dialog doesn't wait for it.) When
    /// a run was called off for it, or others wait their turn, the rest of it waits for them
    /// (#274): its run is called off too, which loses nothing, and goes on after them. `over`:
    /// its last piece is in already.
    pub(crate) fn shared_check_piece(&mut self, shared: Id, over: bool) {
        if !(self.shared_checking.contains(&shared) && !self.shared_refused.contains_key(&shared) && self.shared_inbox_read(shared)) {
            return;
        }
        self.finish_shared_check(shared, Ok(()));
        if over || self.session.driver.round.queue.is_empty() {
            return;
        }
        let quiet = self.graph_rx.as_ref().is_some_and(|(_, quiet, ..)| *quiet);
        self.services.sync_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        if !self.session.driver.round.queue.iter().any(|(a, _)| *a == shared) {
            self.session.driver.round.queue.push_back((shared, quiet));
        }
    }

    /// Add Shared Mailbox: read the shared mailbox `shared`, just added, to see whether it is to
    /// be kept ([`MailApp::finish_shared_check`]). When the run can't start, it is removed again.
    /// A Send/Receive under way (the owner's own, say, which can take long) is called off for it,
    /// which loses nothing: this one starts once that has stopped, and that one goes on after.
    pub(crate) fn begin_shared_check(&mut self, shared: Id) -> Result<(), String> {
        self.shared_checking.insert(shared);
        if self.shared_check_waits() {
            self.call_off_for_shared_check();
        }
        // (A sign-in left waiting in the browser is called off at once.)
        if self.shared_check_waits() {
            self.shared_check_next = Some(shared);
            return Ok(());
        }
        match self.start_sync(&json!({"account": shared})) {
            Ok(_) => Ok(()),
            Err(e) => {
                self.drop_shared_check(shared);
                Err(e)
            }
        }
    }

    /// Whether the first run of a shared mailbox being added has to wait for another to end.
    fn shared_check_waits(&self) -> bool {
        self.sync_rx.is_some() || self.graph_rx.is_some() || self.signing_in.is_some()
    }

    /// Call off the run under way, at its next safe place, and put its account first in line
    /// again, as it was asked for. (A Graph job that isn't bringing mail down can't be called
    /// off: it is short; nor a sign-in being renewed.) A sign-in waiting on a page in the browser
    /// is called off at once (#274): the owner is here, not there, and Send/Receive asks again,
    /// which the owner is told.
    fn call_off_for_shared_check(&mut self) {
        if self.signing_in.as_ref().is_some_and(|s| s.browser)
            && let Some(left) = self.signing_in.take()
        {
            left.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            // Said where Send/Receive's problems are, under the account it was for: the next good
            // run of it clears it.
            let address = self.session.mailbox.account(left.account).map(|a| a.email.clone()).unwrap_or_default();
            let said = crate::i18n::fmt(
                tl!(
                    "The sign-in to {address} waiting in your browser was stopped so the shared mailbox could be read. Send/Receive will ask you to sign in again."
                ),
                &[("address", &address)],
            );
            self.keep_sync_problems(left.account, &[said]);
            return;
        }
        let whose = if self.sync_rx.is_some() {
            // (Its end puts it back in line itself.)
            if let Some((whose, baseline, _, quiet)) = self.sync_run {
                self.sync_run = Some((whose, baseline, true, quiet));
            }
            None
        } else if self.graph_pieces.is_some() {
            self.graph_rx.as_ref().map(|(whose, quiet, ..)| (*whose, *quiet))
        } else {
            return;
        };
        self.services.sync_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some((whose, quiet)) = whose
            && !self.session.driver.round.queue.iter().any(|(a, _)| *a == whose)
        {
            self.session.driver.round.queue.push_front((whose, quiet));
        }
    }

    /// Every frame: the first run of a shared mailbox being added, once the run it waited for
    /// has stopped (that one may have handed over to another leg of its own, which is called off
    /// too).
    pub(crate) fn shared_check_tick(&mut self) {
        let Some(shared) = self.shared_check_next else { return };
        if !self.shared_checking.contains(&shared) {
            self.shared_check_next = None;
            return;
        }
        if self.shared_check_waits() {
            if !self.services.sync_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                self.call_off_for_shared_check();
            }
            return;
        }
        self.shared_check_next = None;
        if let Err(e) = self.start_sync(&json!({"account": shared})) {
            self.finish_shared_check(shared, Err(e));
        }
    }

    /// A shared mailbox being added that isn't to be kept: removed again, with what was kept for
    /// it in memory. (Not undoable by itself: it was never there for the owner.)
    pub(crate) fn drop_shared_check(&mut self, shared: Id) {
        self.shared_checking.remove(&shared);
        if self.shared_check_next == Some(shared) {
            self.shared_check_next = None;
        }
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
                    self.set_dialog(None);
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
