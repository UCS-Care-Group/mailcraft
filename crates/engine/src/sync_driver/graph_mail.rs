// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whether and how a Send/Receive run of an account kept in step through Microsoft Graph starts,
//! with no I/O: the owner is asked to sign in, or the run goes ahead with the request the mailbox
//! makes. The host adds the token and runs it ((`mailcraft_host::sync`)); a shared mailbox's run
//! is the window's for now.

use mailcraft_model::sync::GraphMailRequest;
use mailcraft_model::{Account, Id, OAuth};

use crate::Session;
use crate::cmd::graph_mail::graph_mail_request;

/// Why a run can't start at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The account is not in the mailbox.
    AccountGone,
    /// The account has no sign-in with Microsoft.
    NoSignIn,
    /// The mailbox couldn't make the request (the account isn't kept in step through Graph); the
    /// reason, in the mailbox's words.
    NotGraph(String),
}

/// What a run of an account asks for first.
#[derive(Debug)]
pub enum MailStart {
    /// There is no sign-in to run with and nobody asked for this run: nothing is put in front of
    /// the owner. `address` names the account.
    Quiet { address: String },
    /// There is no sign-in to run with: the owner signs in on the provider's page.
    SignIn { oauth: OAuth, address: String },
    /// Go ahead.
    Run(Box<MailPlan>),
}

/// What a run is made of: the account, its sign-in settings as the account holds them (the host
/// fills in the application id), the refresh token to ask for a token with, and what the mailbox
/// asks of Graph. The Outbox goes with the run itself, as sends and drafts in `mail`.
pub struct MailPlan {
    pub owner: Account,
    pub oauth: OAuth,
    pub refresh: String,
    pub mail: GraphMailRequest,
}

impl std::fmt::Debug for MailPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailPlan")
            .field("account", &self.owner.email)
            .field("oauth", &self.oauth)
            .field("refresh", &if self.refresh.is_empty() { "" } else { "…" })
            .field("mail", &self.mail)
            .finish()
    }
}

/// Decide how `account`'s run starts, given the refresh token a front end or host found for it
/// (empty: none). `quiet` is a run nobody asked for.
pub fn plan_graph_mail(s: &Session, account: Id, quiet: bool, refresh: String) -> Result<MailStart, Refusal> {
    let owner = s.mailbox.account(account).cloned().ok_or(Refusal::AccountGone)?;
    let oauth = owner.oauth.clone().ok_or(Refusal::NoSignIn)?;
    if refresh.is_empty() {
        let address = owner.email.clone();
        return Ok(if quiet { MailStart::Quiet { address } } else { MailStart::SignIn { oauth, address } });
    }
    let mail = graph_mail_request(s, account).map_err(|e| Refusal::NotGraph(e.to_string()))?;
    Ok(MailStart::Run(Box::new(MailPlan { owner, oauth, refresh, mail })))
}
