//! Mail read through Microsoft Graph instead of IMAP, for an account that signs in with Microsoft
//! and whose owner chose that (`account.mailSource`): the switch in the account's Account Settings
//! (`account_setup`), what is said under it, `ui.mailSource` and `ui.allowGraphMail`, and what a
//! Send/Receive of such an account needs (a token for Graph that covers its mail, which Graph's
//! job gets first).
//!
//! The shared mailboxes opened through the account follow it (the owner's decision: an account
//! read through Graph has its shared mailboxes read through Graph too, as `/users/{address}/…`
//! with the owner's token): they have no switch of their own, their Account Settings say whose
//! they follow, and everything here that is about a token or a permission is about the owner's
//! account ([`mailcraft_engine::cmd::shared::signer`]).
//!
//! The permissions it takes (`Mail.ReadWrite`, and `Mail.ReadWrite.Shared` and `Mail.Send.Shared`
//! for the shared mailboxes) are asked for on Microsoft's page only when the owner turns the
//! switch on or says "Allow…", never by itself: a quiet run that finds them missing says so under
//! the switch and stops there.

use std::collections::HashMap;

use mailcraft_model::{Id, MailSource};
use serde_json::{Value, json};

use crate::MailApp;

/// What is said under the switch of an account, while there is something to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Note {
    /// The sign-in doesn't cover the mail through Graph yet: "Allow…".
    Allow,
    /// Microsoft's page didn't give it, and why (the words of [`crate::approval`] when it was an
    /// organisation's no).
    Refused(String),
    /// The switch waits: this many changes made here haven't reached the server.
    Waiting(usize),
}

/// What [`MailApp`] keeps of the accounts whose mail is read through Graph.
#[derive(Default)]
pub struct GraphMail {
    notes: HashMap<Id, Note>,
}

impl Note {
    pub(crate) fn text(&self) -> String {
        match self {
            Note::Allow => tl!("MailCraft needs your permission to read this account's mail through Microsoft Graph.").to_string(),
            Note::Refused(why) => tl!(why).to_string(),
            Note::Waiting(n) => {
                format!(
                    "{n} {}",
                    tl!("change(s) made here haven't reached the server yet. Send/Receive first, or switch anyway and they stay here only.")
                )
            }
        }
    }
}

impl MailApp {
    /// Whether `account`'s mail is read through Microsoft Graph: its owner's choice for a
    /// shared mailbox.
    pub(crate) fn mail_through_graph(&self, account: Id) -> bool {
        mailcraft_engine::cmd::shared::through_graph(&self.session, account)
    }

    /// Whether `account` may have its mail read through Graph: it signs in with Microsoft, is
    /// connected to a server, and is not a shared mailbox (which has no switch: it is read with
    /// its owner's sign-in, the way the owner's account is).
    pub(crate) fn graph_mail_offered(&self, account: Id) -> bool {
        self.is_microsoft(account)
            && self.session.mailbox.account(account).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Imap && a.shared_of.is_none())
    }

    /// What is said under the switch of `account`, if anything.
    pub(crate) fn graph_mail_note(&self, account: Id) -> Option<&Note> {
        self.graph_mail.notes.get(&account)
    }

    /// `ui.mailSource`: the owner's choice of how `account`'s mail is read, kept with
    /// `account.mailSource`, and what follows at once: Microsoft's page for the permission when
    /// it is Graph, then a Send/Receive either way. Without `source`, how things stand.
    pub(crate) fn mail_source_command(&mut self, p: &Value) -> Result<Value, String> {
        let account = match p.get("account").and_then(Value::as_u64) {
            Some(account) => account,
            None => self
                .session
                .mailbox
                .accounts
                .iter()
                .map(|a| a.id)
                .find(|id| self.graph_mail_offered(*id))
                .ok_or_else(|| tl!("No account signs in with Microsoft").to_string())?,
        };
        let Some(source) = p.get("source").filter(|v| !v.is_null()) else { return Ok(self.graph_mail_state(account)) };
        let source = source.as_str().and_then(MailSource::parse).ok_or_else(|| tl!("`source` is imap or graph").to_string())?;
        let discard = p.get("discard").and_then(Value::as_bool).unwrap_or(false);
        let said = match self.session.run("account.mailSource", &json!({"account": account, "source": source.code(), "discard": discard})) {
            Ok(said) => said,
            Err(e) => {
                // Changes waiting for the server (in the account, or in a shared mailbox it
                // opens) hold the switch: said under it, with the way on. (A shared mailbox has
                // no switch to say it under: the refusal says whose it follows.)
                let waiting = if self.graph_mail_offered(account) { self.unpushed_with_shared(account) } else { 0 };
                if waiting > 0 {
                    self.graph_mail.notes.insert(account, Note::Waiting(waiting));
                }
                return Err(e.to_string());
            }
        };
        self.graph_mail.notes.remove(&account);
        let changed = said.get("changed").and_then(Value::as_bool).unwrap_or(false);
        let mut pending = Value::Null;
        if changed {
            // A token from before may not cover the mail; and what Graph's job last brought is
            // brought again for this way of reading. The shared mailboxes went with the switch.
            self.graph_tokens.remove(&account);
            self.graph_checked.remove(&account);
            for shared in self.session.mailbox.shared_mailboxes(account).iter().map(|a| a.id).collect::<Vec<Id>>() {
                self.graph_mail.notes.remove(&shared);
            }
        }
        if source == MailSource::Graph && (changed || p.get("allow").and_then(Value::as_bool).unwrap_or(false)) {
            match self.allow_graph_mail(&json!({"account": account})) {
                Ok(_) => pending = json!("signIn"),
                // (The choice stands; the line under the switch asks again.)
                Err(e) => {
                    self.graph_mail.notes.insert(account, Note::Allow);
                    self.status(e);
                }
            }
        } else if changed && !self.syncing() && self.graph_rx.is_none() {
            match self.start_sync(&json!({"account": account})) {
                Ok(_) => pending = json!("sync"),
                Err(e) => self.status(e),
            }
        }
        let mut state = self.graph_mail_state(account);
        state["pending"] = pending;
        state["discarded"] = said.get("discarded").cloned().unwrap_or(json!(0));
        Ok(state)
    }

    /// `ui.allowGraphMail`: sign in on Microsoft's page for reading `account`'s mail through Graph
    /// (beside what Graph was asked for before). The next Send/Receive uses what comes back. For
    /// a shared mailbox, its owner signs in: the permission is the owner's.
    pub(crate) fn allow_graph_mail(&mut self, p: &Value) -> Result<Value, String> {
        let account = p.get("account").and_then(Value::as_u64).ok_or_else(|| tl!("Which account?").to_string())?;
        let account = mailcraft_engine::cmd::shared::signer(&self.session, account);
        if !self.mail_through_graph(account) {
            return Err(tl!("This account's mail is read over IMAP").to_string());
        }
        let Some(owner) = self.session.mailbox.account(account) else { return Err(tl!("No account signs in with Microsoft").to_string()) };
        let Some(oauth) = owner.oauth.clone() else { return Err(tl!("This account doesn't sign in with Microsoft").to_string()) };
        let email = owner.email.clone();
        self.begin_sign_in(account, oauth, &email, String::new(), false, true)?;
        Ok(json!({"pending": "signIn", "account": account}))
    }

    /// The owner allowed it on Microsoft's page: nothing more to say, and the mail comes.
    pub(crate) fn graph_mail_allowed(&mut self, account: Id) {
        self.graph_mail.notes.remove(&account);
        self.graph_consent_needed.remove(&account);
    }

    /// Microsoft's page didn't give it: why, under the switch.
    pub(crate) fn graph_mail_refused(&mut self, account: Id, why: &str) {
        self.graph_mail.notes.insert(account, Note::Refused(why.to_string()));
        self.status(format!("{} {why}", tl!("Mail through Microsoft Graph wasn't allowed:")));
    }

    /// A token for Graph of `account` was asked for and the sign-in doesn't cover its mail (for
    /// a shared mailbox, its owner's sign-in: said under the owner's switch).
    pub(crate) fn graph_mail_not_allowed(&mut self, account: Id, quiet: bool) {
        let account = mailcraft_engine::cmd::shared::signer(&self.session, account);
        let first = self.graph_mail.notes.insert(account, Note::Allow).is_none();
        if first && !quiet {
            self.status(tl!(
                "This account's mail is read through Microsoft Graph, which needs your permission: File › Info › Account Settings › Allow…"
            ));
        }
    }

    /// Why a Send/Receive of `account` can't go yet: its mail is read through Graph and there is
    /// no token for it. Noted under the switch (its owner's, for a shared mailbox).
    pub(crate) fn graph_mail_unready(&mut self, account: Id) -> String {
        let account = mailcraft_engine::cmd::shared::signer(&self.session, account);
        self.graph_mail.notes.entry(account).or_insert(Note::Allow);
        tl!("This account's mail is read through Microsoft Graph, which needs your permission: File › Info › Account Settings › Allow…").to_string()
    }

    /// What waits to reach the server from `account` and the shared mailboxes it opens, which a
    /// switch of the account's would leave behind.
    fn unpushed_with_shared(&self, account: Id) -> usize {
        let shared = self.session.mailbox.shared_mailboxes(account);
        std::iter::once(account)
            .chain(shared.iter().map(|a| a.id))
            .map(|a| mailcraft_engine::cmd::account::unpushed(&self.session, a))
            .fold(0usize, usize::saturating_add)
    }

    fn graph_mail_state(&self, account: Id) -> Value {
        // (A shared mailbox's is its owner's.)
        let source = self.session.mailbox.mail_source_of(account);
        let follows = self.session.mailbox.account(account).and_then(|a| a.shared_of).filter(|owner| *owner != account);
        let note = self.graph_mail_note(account);
        let line = note.map(|note| match note {
            Note::Allow => "allow",
            Note::Refused(_) => "refused",
            Note::Waiting(_) => "waiting",
        });
        json!({
            "account": account,
            "source": source.code(),
            "follows": follows,
            "offered": self.graph_mail_offered(account),
            "line": line,
            "text": note.map(Note::text),
        })
    }
}

/// The `ui.*` commands of mail through Graph; `None` for any other.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.mailSource" => app.mail_source_command(p),
        "ui.allowGraphMail" => app.allow_graph_mail(p),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
