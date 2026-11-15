// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sign-ins of a program that comes back through a redirect strategy of its own choosing
//! (a phone's custom scheme), in the words that program has something to say about.
//!
//! Two things live here, both generic over [`RedirectStrategy`] and both the same flow as the
//! desktop's (PKCE, the state, the trade of the code): a sign-in that says how it ended
//! ([`Ending`], which tells Conditional Access's no from an administrator's), and the sign-ins
//! for the permissions that are asked for by themselves and never with a sign-in
//! ([`Permission`]): the To Do lists, reading shared mailboxes and sending from them.

use super::{
    AtomicBool, Duration, Endpoints, FavoritesRefusal, NetError, OAuth, OAuthProvider, Policy, RedirectStrategy, Refused, Resource, SIGN_IN_TIMEOUT,
    Tokens, attempt_via, shared_scope, shared_send_scope, sign_in_refused_via, tasks_scope,
};

/// A permission that is asked for by itself, after the program has asked the owner, and never on
/// the page of a sign-in (so that a sign-in never waits on an administrator).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Reading and changing the owner's Microsoft To Do lists and tasks (`Tasks.ReadWrite`).
    Tasks,
    /// Reading and changing the shared mailboxes the owner was given Full Access to
    /// (`Mail.ReadWrite.Shared`).
    SharedMail,
    /// Sending from a shared mailbox, as it or on its behalf (`Mail.Send.Shared`), on top of
    /// reading it.
    SharedSend,
}

/// The provider's endpoints for the sign-in that asks for `permission` and nothing but what the
/// tokens are for: nothing is put to the owner beside it.
pub fn permission_endpoints(oauth: &OAuth, permission: Permission) -> Endpoints {
    let (resource, scope) = match permission {
        Permission::Tasks => (Resource::Graph, tasks_scope()),
        Permission::SharedMail => (Resource::GraphMail, shared_scope()),
        Permission::SharedSend => (Resource::GraphMail, shared_send_scope()),
    };
    Endpoints { scope, consent: String::new(), lesser: String::new(), ..Endpoints::for_resource(oauth, resource) }
}

/// Ask the owner, on the provider's page at `endpoints` (see [`permission_endpoints`]), for one
/// permission, coming back the way `via` says. A no says whose it is ([`FavoritesRefusal`]);
/// nothing is tried a second time. Microsoft only: Google has no such permissions.
pub fn sign_in_for_permission_at(
    endpoints: &Endpoints,
    oauth: &OAuth,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    via: &RedirectStrategy,
) -> Result<Tokens, FavoritesRefusal> {
    permission_within(endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT, via)
}

fn permission_within(
    endpoints: &Endpoints,
    oauth: &OAuth,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    patience: Duration,
    via: &RedirectStrategy,
) -> Result<Tokens, FavoritesRefusal> {
    if oauth.provider == OAuthProvider::Google {
        return Err(FavoritesRefusal::Other(NetError::Config("Google sign-in has no such permissions".into())));
    }
    attempt_via(endpoints, "", oauth, login_hint, open, cancel, patience, via).map_err(|refused| match (refused.policy, refused.about) {
        (Policy::Approval, _) => FavoritesRefusal::Approval,
        (Policy::Blocked, _) => FavoritesRefusal::Organisation,
        (Policy::Account, _) | (Policy::None, super::About::Scope) => FavoritesRefusal::Account,
        (Policy::None, _) => FavoritesRefusal::Other(refused.error),
    })
}

/// How a sign-in on the provider's page ended without tokens, in the ways a program has
/// something of its own to say about ([`sign_in_ending_via`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ending {
    /// An administrator of the organisation has to approve the application first.
    Approval,
    /// The page came back with a no that was given on it (also what Microsoft sends when the
    /// owner leaves the page that said an administrator has to approve first).
    Declined,
    /// Nothing came back in the time that is waited.
    TimedOut,
    /// Called off from here.
    Cancelled,
    /// The organisation's rules (Conditional Access, an application switched off, no entitlement)
    /// refuse this person, approved or not: nothing the owner or an approval changes.
    Blocked,
    /// Anything else, in the provider's words or as it went wrong.
    Other(NetError),
}

impl From<Refused> for Ending {
    fn from(refused: Refused) -> Ending {
        match (refused.policy, &refused.error) {
            (Policy::Approval, _) => Ending::Approval,
            (Policy::Blocked, _) => Ending::Blocked,
            (_, NetError::SignInTimedOut) => Ending::TimedOut,
            (_, NetError::SignInCancelled) => Ending::Cancelled,
            (Policy::None, _) if refused.declined => Ending::Declined,
            _ => Ending::Other(refused.error),
        }
    }
}

/// A sign-in at `endpoints` coming back the way `via` says, saying how it ended when it brought
/// no tokens. Microsoft only.
pub fn sign_in_ending_via(
    endpoints: &Endpoints,
    oauth: &OAuth,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    via: &RedirectStrategy,
) -> Result<Tokens, Ending> {
    if oauth.provider == OAuthProvider::Google {
        return Err(Ending::Other(NetError::Config("Google sign-in comes back through a loopback port only".into())));
    }
    sign_in_refused_via(endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT, via).map_err(Ending::from)
}
