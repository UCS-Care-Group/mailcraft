// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing in on the provider's own page (OAuth 2.0): the authorization-code flow with PKCE for
//! an application that keeps no secret (RFC 6749, RFC 7636), coming back through a loopback
//! address (RFC 8252), and trading a refresh token for new tokens.
//!
//! [`sign_in`] hands the host the address of the provider's page to open in the owner's
//! browser, waits on a loopback port for the browser to come back with a code, and trades the
//! code for [`Tokens`]. [`refresh`] gets new ones later without asking the owner again. The
//! access token is what [`sync`](crate::sync()), [`send`](crate::send()) and
//! [`test_account`](crate::test_account) show the mail servers (SASL XOAUTH2).
//!
//! An access token is good for one [`Resource`]: the mail servers, or Microsoft Graph
//! ([`crate::graph`]). One sign-in serves both. The page of a sign-in for mail also asks the
//! owner to agree to what Graph will be asked for ([`Endpoints::consent`]), and the refresh
//! token it brings then gets Graph's tokens as well as mail's ([`refresh_for`]). Should the
//! provider not take the two together, the sign-in is tried once more for mail alone: mail
//! never waits on Graph. Graph can be asked for on its own later ([`sign_in_for`]).
//!
//! Reading the Favorites of Outlook takes two permissions more (`MailboxConfigItem.Read`,
//! `Mail.ReadBasic`). They are never on the page of a sign-in: the owner is asked for them by
//! themselves when he wants those ([`sign_in_for_favorites`]), a no there costs nothing that
//! was had and says whose no it is ([`FavoritesRefusal`]), and tokens that are good for them
//! come from [`refresh_graph_with_favorites`], which falls back to what a sign-in covered
//! before.
//!
//! Reading a shared mailbox (#70) takes one more (`Mail.ReadWrite.Shared`), asked for in the
//! same way: by itself, when the owner has opened one ([`sign_in_for_shared`],
//! [`refresh_graph_shared`]); and sending from one another (`Mail.Send.Shared`), by itself at
//! the first send from one ([`sign_in_for_shared_send`], [`refresh_graph_shared_send`]).
//!
//! When a no to them is the organisation's rather than the owner's, it is told apart by
//! Microsoft's own numbers for it: an administrator has to approve first ([`FavoritesRefusal::Approval`]: [`admin_consent_url`] is the address to hand
//! them), or the door is shut for another reason ([`FavoritesRefusal::Organisation`]).
//! [`Tokens::tenant`] is the directory a sign-in names (the identity token's `tid`), which
//! tells an organisation's account from a personal one.
//!
//! Where an organisation lets only its administrators approve a new application, a sign-in ends
//! on a page of Microsoft's that says so, and it is not certain that anything comes back from
//! it. [`sign_in_explained`] says how a sign-in that brought no tokens ended ([`SignInEnd`]), so
//! that the owner can be told what to do next. What there is to do is for an administrator to
//! approve the application once, for everybody: [`admin_consent`] opens Microsoft's page for
//! that in the browser and waits on a loopback port for its answer (for an owner who is one),
//! and [`admin_consent_url`] is the same page's address for an owner to pass on to one. Either
//! names the application, the directory and the permissions, and nothing of the owner.
//!
//! Nothing here keeps anything: the tokens are the caller's, to hold in memory. The password is
//! typed into the provider's page and never comes this way; the code, the PKCE verifier and the
//! tokens never appear in an error or a log line.
//!
//! What can go wrong: [`NetError::SignInCancelled`] and [`NetError::SignInTimedOut`] for a
//! sign-in that was called off or never finished; `ProtoError::Auth` when the provider said no
//! (access refused, a refresh token that is no longer good: the owner has to sign in again);
//! [`NetError::ConsentNeeded`] when a refresh token that is good gets no token for Graph,
//! because the owner hasn't agreed to that yet; `ProtoError::Rejected` when the provider said
//! "not now"; and the connection and protocol errors of any other server.

use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_charset::base64::URL_SAFE_NO_PAD;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{OAuth, OAuthProvider};
use mailcraft_proto::ProtoError;
use ring::rand::SecureRandom as _;
use serde_json::Value;

use crate::NetError;
use crate::http::{self, Answer, Url};

/// How long the owner has to finish on the provider's page.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How often the wait looks for the browser and at the cancel flag.
const TICK: Duration = Duration::from_millis(25);
/// Largest request taken from the browser, and how long it may take to arrive.
const MAX_REQUEST: usize = 8 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the page for the browser may take to leave, and how long is spent on what a
/// browser still sends after it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
const DRAIN_TIME: Duration = Duration::from_millis(150);
/// Most connections from browsers looked after at once; more are turned away.
const MAX_VISITORS: usize = 16;
/// Tries at finding a port that is free on both loopback addresses.
const MAX_PORT_TRIES: usize = 8;
/// Longest token or code taken or sent.
const MAX_TOKEN: usize = 16 * 1024;
/// How long the token endpoint has to be reached, and to answer.
const TOKEN_TIMEOUT: Duration = Duration::from_secs(20);
/// How long an access token is taken to last when the server doesn't say, and the longest it is believed to.
const DEFAULT_LIFETIME: i64 = 3_600;
const MAX_LIFETIME: i64 = 7 * 24 * 3_600;

/// Longest name or address taken from an identity token.
const MAX_IDENTITY: usize = 254;

const MICROSOFT_LOGIN: &str = "https://login.microsoftonline.com";
/// IMAP and SMTP, a refresh token to go with the access token, and who it is that signed in.
const MICROSOFT_SCOPE: &str =
    "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid profile email";
/// Microsoft Graph: sending mail, reading the calendar and writing to it (events made, moved,
/// changed and deleted here go back to the server), and reading the contacts. A sign-in from
/// before the calendar was written to agreed to reading it only: its refresh token gets no
/// token for this ([`NetError::ConsentNeeded`]) until the owner agrees on the provider's page.
const MICROSOFT_GRAPH: &str =
    "https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read";
/// What Graph is asked for on top of that when it can be had: reading the mailbox's settings,
/// which is where its list of categories and their colours is. Nothing that works without it
/// depends on it: a sign-in that doesn't cover it keeps everything it had ([`refresh_graph`]).
const MICROSOFT_GRAPH_EXTRAS: &str = "https://graph.microsoft.com/MailboxSettings.Read";
/// What keeping a Microsoft account's folders and mail in step through Graph takes (R3): reading
/// and changing them. Asked for only for an account that is kept in step that way
/// ([`Resource::GraphMail`]).
const MICROSOFT_GRAPH_MAIL: &str = "https://graph.microsoft.com/Mail.ReadWrite";
/// And what showing the Favorites of Outlook takes on top of that, when the owner wants them:
/// reading the mailbox's configuration items (of which one holds the list) and reading folders
/// by name. Only ever read. Nothing else depends on them: a sign-in that doesn't cover them
/// keeps everything it had ([`refresh_graph_with_favorites`]), and the owner is asked for them
/// on the provider's page only when he says so ([`sign_in_for_favorites`]).
const MICROSOFT_GRAPH_FAVORITES: &str = "https://graph.microsoft.com/MailboxConfigItem.Read https://graph.microsoft.com/Mail.ReadBasic";
/// What reading a shared mailbox takes (#70, a draft): the folders and mail of the mailboxes the
/// owner was given Full Access to, read and changed. Asked for by itself when the owner opens a
/// shared mailbox and it is first read ([`sign_in_for_shared`], [`refresh_graph_shared`]), never
/// on the page of a sign-in: an account without shared mailboxes is never asked for it.
const MICROSOFT_GRAPH_SHARED: &str = "https://graph.microsoft.com/Mail.ReadWrite.Shared";
/// What sending from a shared mailbox takes (#70): mail sent as it or on its behalf, through its
/// own address. Asked for by itself at the first send from one ([`sign_in_for_shared_send`],
/// [`refresh_graph_shared_send`]), never on the page of a sign-in, and never for a run that has
/// nothing of a shared mailbox's to send.
const MICROSOFT_GRAPH_SHARED_SEND: &str = "https://graph.microsoft.com/Mail.Send.Shared";
/// What reading a shared mailbox's calendars takes (#70): the calendars of the mailboxes the
/// owner was given access to, read and, once writing comes, changed (`Calendars.Read.Shared`
/// alone is never asked for). Asked for by itself the first time the owner shows one of them
/// ([`sign_in_for_shared_calendars`], [`refresh_graph_shared_calendars`]), never on the page of a
/// sign-in: an account without a shared mailbox's calendar shown is never asked for it. It is on
/// the administrator's list too, so that one approval covers it.
const MICROSOFT_GRAPH_SHARED_CALENDARS: &str = "https://graph.microsoft.com/Calendars.ReadWrite.Shared";
/// What keeping the owner's to-dos in step with Microsoft To Do takes (#248): reading and
/// changing the lists and tasks of the signed-in owner. Put to the owner on the page of a new
/// sign-in beside the rest ([`Endpoints::consent`]; a page that won't have it is asked again
/// without), and on the administrator's list. It is **not** part of [`MICROSOFT_GRAPH`]: a sign-in
/// from before it was asked for would get no token at all for calendar and contacts. A job for
/// the to-dos asks for it by itself ([`refresh_graph_tasks`]); a sign-in that doesn't cover it
/// gets [`NetError::ConsentNeeded`], and the window asks the owner ([`sign_in_for_tasks`]).
const MICROSOFT_GRAPH_TASKS: &str = "https://graph.microsoft.com/Tasks.ReadWrite";
/// Whether the page of an ordinary sign-in also puts the Favorites' permissions to the owner,
/// so that one page settles everything. **Off**: Microsoft's permissions reference lists
/// `MailboxConfigItem.Read` as one that takes an administrator's consent in an organisation
/// (its Graph Explorer says the opposite, and the two were not told apart on the real service).
/// Were that so, a page that asks for it would tell everybody who is not an administrator to
/// go and find one before they could sign in to their mail at all. So the page of a sign-in
/// asks for what it always asked for, and these two are asked for by themselves, when the
/// owner chooses to. With this on, a page that is refused is asked once more without them
/// ([`Endpoints::lesser`]); what cannot be undone is a page the owner never comes back from.
const FAVORITES_AT_SIGN_IN: bool = false;
/// Where an administrator approves an application for a whole organisation, in the form the
/// Microsoft identity platform documents ("Admin consent on the Microsoft identity platform":
/// `/{tenant}/v2.0/adminconsent` with `client_id`, `scope` and `redirect_uri`).
const MICROSOFT_ADMIN_CONSENT: &str = "v2.0/adminconsent";
/// The address this application is registered with for coming back from Microsoft's pages.
const REGISTERED_REDIRECT: &str = "http://localhost";
/// What goes with either: a refresh token, and who it is that signed in.
const SIGN_IN_SCOPE: &str = "offline_access openid profile email";

/// Microsoft's numbers (`AADSTS…`) for a no that is about what was asked for rather than about
/// who asked: scopes of more than one resource together, a scope that isn't one, a scope or a
/// resource the application or the directory doesn't have.
const SCOPE_CODES: [u64; 8] = [28_000, 28_002, 28_003, 70_011, 650_053, 650_057, 65_005, 500_011];
/// And for a no that is the owner's to turn into a yes on the sign-in page: nobody has agreed to
/// what was asked for, the owner said no, or an administrator has to agree.
const CONSENT_CODES: [u64; 3] = [65_001, 65_004, 90_094];
/// And for a no that is not the owner's to turn into a yes, but an administrator's: the
/// organisation keeps the permission for its administrators to give. Microsoft's error
/// reference names them `AdminConsentRequired` (90094), `AdminConsentRequiredRequestAccess`
/// (90095: the same, where the organisation lets its people ask for it) and, on its page about
/// consent troubles, `AdminConsentRequiredDueToRiskyApp` (900941); the same page gives 90093
/// beside 90094 for "permissions that you aren't authorized to grant". Once an administrator
/// has approved, the same question gets a yes.
const APPROVAL_CODES: [u64; 4] = [90_093, 90_094, 90_095, 900_941];
/// And for a no that was given on the page itself: `UserDeclinedConsent` (65004). Microsoft's
/// page about consent troubles says the same number is what comes back from the page that told
/// somebody an administrator has to approve first, when they leave it by its way back to the
/// application: so it does not tell the owner's own no from the organisation's.
const DECLINED_CODES: [u64; 1] = [65_004];
/// And for a no that an approval of these permissions would not turn into a yes: the
/// organisation doesn't let this person use the application (50105, `EntitlementGrantsNotFound`),
/// its Conditional Access refuses the token (53003, `BlockedByConditionalAccess`), it has
/// switched the application off (7000112, `UnauthorizedClientApplicationDisabled`), or it
/// doesn't have the service that was asked for (650052).
const BLOCKED_CODES: [u64; 4] = [50_105, 53_003, 650_052, 7_000_112];
/// And for a no that is about the kind of account: one that can't be used with what was asked
/// for (500207).
const ACCOUNT_CODES: [u64; 1] = [500_207];
/// Most numbers looked at in one answer.
const MAX_CODES: usize = 16;

/// What a token is for. An access token is good for one of these only, so each has its own; one
/// sign-in, and the refresh token it brings, gets both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resource {
    /// The mail servers: IMAP and SMTP (SASL XOAUTH2).
    Mail,
    /// Microsoft Graph ([`crate::graph`]): sending mail where SMTP is switched off, the calendar
    /// and the contacts.
    Graph,
    /// Microsoft Graph with the folders and mail of a Microsoft account read and written through
    /// it (`Mail.ReadWrite`, R3): only for an account kept in step through Graph.
    GraphMail,
}

/// Where a provider signs people in, and what it is asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Endpoints {
    /// The page the owner signs in on.
    pub authorize: String,
    /// Where a code or a refresh token is traded for tokens.
    pub token: String,
    /// What the tokens are to be good for, separated by spaces.
    pub scope: String,
    /// What else the owner is asked to agree to on the sign-in page, separated by spaces, for
    /// tokens that are asked for later ([`refresh_for`]); empty for nothing. Only the page is
    /// shown these: the tokens of the sign-in itself are for `scope`. When the provider refuses
    /// the two together, the sign-in is tried once more without them.
    pub consent: String,
    /// What the owner is asked to agree to instead when the page won't have `consent` (the
    /// provider has no such permission for this kind of account, or the owner may not give it):
    /// less than `consent`, and known to be had. Empty for nothing in between: the sign-in is
    /// then tried without any of it, as above.
    pub lesser: String,
}

impl Endpoints {
    /// The provider's own endpoints for these settings, for a sign-in to mail:
    /// [`Endpoints::for_resource`] with [`Resource::Mail`].
    pub fn of(oauth: &OAuth) -> Endpoints {
        Endpoints::for_resource(oauth, Resource::Mail)
    }

    /// The provider's own endpoints for these settings, and what is asked of them for `resource`.
    ///
    /// Microsoft: `https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize` and
    /// `…/token`. For mail: IMAP, SMTP, a refresh token and the name and address of whoever
    /// signs in, with Graph's scopes for the owner to agree to on the same page
    /// ([`Endpoints::consent`]). For Graph: sending mail, reading and writing the calendar,
    /// reading the contacts, a refresh token and who signs in. An empty tenant is `common`. (A tenant that can't be
    /// one is refused by [`sign_in`] and [`refresh`]; here it reads as `common` too, so that
    /// nothing unchecked ends up in an address.)
    pub fn for_resource(oauth: &OAuth, resource: Resource) -> Endpoints {
        match oauth.provider {
            OAuthProvider::Microsoft => {
                let tenant = tenant(oauth).unwrap_or("common");
                // The optional extras are only ever put to the owner on the page, beside what the
                // tokens are for: a provider that won't have them there is asked again without
                // them ([`sign_in`]), so that asking for more never costs what was to be had.
                let (scope, consent) = match resource {
                    Resource::Mail => (MICROSOFT_SCOPE.to_string(), format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_EXTRAS}")),
                    Resource::Graph => (format!("{MICROSOFT_GRAPH} {SIGN_IN_SCOPE}"), MICROSOFT_GRAPH_EXTRAS.to_string()),
                    Resource::GraphMail => (format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_MAIL} {SIGN_IN_SCOPE}"), MICROSOFT_GRAPH_EXTRAS.to_string()),
                };
                // (With the Favorites' permissions on the same page, were that wanted: what was
                // asked for before them is then what a refused page falls back to.)
                let (consent, lesser) =
                    if FAVORITES_AT_SIGN_IN { (format!("{consent} {MICROSOFT_GRAPH_FAVORITES}"), consent) } else { (consent, String::new()) };
                Endpoints {
                    authorize: format!("{MICROSOFT_LOGIN}/{tenant}/oauth2/v2.0/authorize"),
                    token: format!("{MICROSOFT_LOGIN}/{tenant}/oauth2/v2.0/token"),
                    scope,
                    consent,
                    lesser,
                }
            }
        }
    }
}

/// Sign the owner in on the provider's page and bring back the tokens for the mail servers.
///
/// `open` is given the address of the page and opens it in the owner's browser (it must not
/// wait for the sign-in). `login_hint` (the account's address) fills in who is signing in.
/// Blocks for as long as the owner takes, five minutes at most; setting `cancel` ends the wait
/// within a fraction of a second.
///
/// The page also asks the owner to agree to what Microsoft Graph is asked for later, so that
/// [`refresh_for`] gets Graph's tokens with the refresh token this brings. A provider that
/// won't have the two together is asked once more for mail alone (`open` is then given a second
/// address); Graph is then for [`sign_in_for`] to ask for.
///
/// `oauth.client_id` must be set: the host fills in its own application id before calling.
pub fn sign_in(oauth: &OAuth, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Tokens, NetError> {
    sign_in_for(oauth, login_hint, open, cancel, Resource::Mail)
}

/// [`sign_in`] for the tokens of `resource`. With [`Resource::Graph`] the owner is asked, on the
/// provider's page, to agree to what Graph is asked for and nothing else: for an account that
/// signed in before any of that was asked, or whose provider wouldn't take it with mail's.
///
/// The access token that comes back is for `resource` only. The refresh token is as good as the
/// one of any other sign-in of the same owner: keep the newest. Who signed in comes back too
/// ([`Tokens::email`]): the owner picks the account on the page, and it may not be the one the
/// tokens were wanted for.
pub fn sign_in_for(oauth: &OAuth, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool, resource: Resource) -> Result<Tokens, NetError> {
    sign_in_at(&Endpoints::for_resource(oauth, resource), oauth, login_hint, open, cancel)
}

/// [`sign_in`] at endpoints other than the provider's usual ones, or asking for something else
/// (with [`Endpoints::consent`] empty, nothing but the tokens' own scopes is put to the owner).
/// The addresses must be `https://` ones, or `http://` ones on this machine.
pub fn sign_in_at(endpoints: &Endpoints, oauth: &OAuth, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Tokens, NetError> {
    sign_in_within(endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT)
}

/// [`sign_in_at`], saying how a sign-in that brought no tokens ended ([`SignInEnd`]): what the
/// owner is told depends on whose no it was, and on whether anything came back at all.
pub fn sign_in_explained(
    endpoints: &Endpoints,
    oauth: &OAuth,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
) -> Result<Tokens, SignInEnd> {
    sign_in_refused(endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT).map_err(SignInEnd::from)
}

/// How a sign-in on the provider's page ended without tokens ([`sign_in_explained`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInEnd {
    /// The organisation the account belongs to lets only an administrator approve this
    /// application, and none has: the page came back saying so (Microsoft's own numbers for
    /// it). Signing in again changes nothing until one has ([`admin_consent`],
    /// [`admin_consent_url`]).
    Approval,
    /// The page came back with a no that was given on it. That is the owner's own no; and it is
    /// also what Microsoft sends when the owner leaves the page that said an administrator has
    /// to approve first, so the two can't be told apart here.
    Declined,
    /// Nothing came back in the time that is waited: the page was closed, or left open.
    TimedOut,
    /// Called off from here.
    Cancelled,
    /// Anything else, in the provider's words or as it went wrong.
    Other(NetError),
}

impl From<Refused> for SignInEnd {
    fn from(refused: Refused) -> SignInEnd {
        match (refused.policy, &refused.error) {
            (Policy::Approval, _) => SignInEnd::Approval,
            (_, NetError::SignInTimedOut) => SignInEnd::TimedOut,
            (_, NetError::SignInCancelled) => SignInEnd::Cancelled,
            (Policy::None, _) if refused.declined => SignInEnd::Declined,
            _ => SignInEnd::Other(refused.error),
        }
    }
}

/// [`sign_in_at`], waiting no longer than `patience` for the owner (each time, should the
/// sign-in be tried a second time).
fn sign_in_within(
    endpoints: &Endpoints,
    oauth: &OAuth,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    patience: Duration,
) -> Result<Tokens, NetError> {
    sign_in_refused(endpoints, oauth, login_hint, open, cancel, patience).map_err(|refused| refused.error)
}

/// [`sign_in_within`], with what a no was about and whose it was.
fn sign_in_refused(
    endpoints: &Endpoints,
    oauth: &OAuth,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    patience: Duration,
) -> Result<Tokens, Refused> {
    let (mut consent, lesser) = (endpoints.consent.trim(), endpoints.lesser.trim());
    let mut outcome = attempt(endpoints, consent, oauth, login_hint, open, cancel, patience);
    // The page won't have all it was asked for (the provider has no such permission for this
    // kind of account, or it is not the owner's to give), and there is less that is known to be
    // had: once more, for that.
    let unwilling = |refused: &Refused| matches!(refused.about, About::Scope | About::Consent);
    if !lesser.is_empty() && lesser != consent && outcome.as_ref().is_err_and(unwilling) {
        log::debug!("the sign-in was refused for what it asked for; asking again for what it asked for before");
        consent = lesser;
        outcome = attempt(endpoints, consent, oauth, login_hint, open, cancel, patience);
    }
    match outcome {
        // The provider won't have what was asked for, and more was asked for than the tokens
        // need: once more, for the tokens alone.
        Err(refused) if refused.about == About::Scope && !consent.is_empty() => {
            log::debug!("the sign-in was refused for what it asked for; asking again without the extra permissions");
            attempt(endpoints, "", oauth, login_hint, open, cancel, patience)
        }
        outcome => outcome,
    }
}

/// Why the owner's yes to reading the Favorites of Outlook wasn't had ([`sign_in_for_favorites`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FavoritesRefusal {
    /// The organisation the account belongs to doesn't let its people give this permission
    /// themselves: an administrator has to approve it, once for everybody
    /// ([`admin_consent_url`] is where). Asking again before that changes nothing.
    Approval,
    /// The organisation doesn't have it for this person at all (the application is switched off
    /// there, or kept from them, or its rules for signing in refuse it): an approval of the
    /// permission would not change that.
    Organisation,
    /// The provider has no such permission for this kind of account.
    Account,
    /// Anything else: the owner said no or closed the page, the sign-in was called off or took
    /// too long, the provider couldn't be reached.
    Other(NetError),
}

/// What an administrator of an organisation is asked to approve, once for everybody in it:
/// everything this program asks an owner for, so that one approval covers what is asked later
/// too. That is what a sign-in's page asks for (IMAP, SMTP, a refresh token and who signs in;
/// Microsoft Graph's sending, calendar, contacts, mailbox settings and, for an account kept in step
/// through Graph, its mail), what reading the Favorites of Outlook through Graph takes, and what
/// reading a shared mailbox and sending from one take, what reading its calendars takes, and what
/// keeping the to-dos in step with Microsoft To Do takes.
///
/// A sign-in asks for no more than is here: a page that asked for one permission an
/// administrator had not approved would send the owner to find one again.
fn admin_scope() -> String {
    let who: Vec<&str> = SIGN_IN_SCOPE.split(' ').collect();
    let mail = MICROSOFT_SCOPE.split(' ').filter(|scope| !who.contains(scope));
    let all = mail.chain([
        MICROSOFT_GRAPH,
        MICROSOFT_GRAPH_EXTRAS,
        MICROSOFT_GRAPH_MAIL,
        MICROSOFT_GRAPH_FAVORITES,
        MICROSOFT_GRAPH_SHARED,
        MICROSOFT_GRAPH_SHARED_SEND,
        MICROSOFT_GRAPH_SHARED_CALENDARS,
        MICROSOFT_GRAPH_TASKS,
        SIGN_IN_SCOPE,
    ]);
    all.collect::<Vec<_>>().join(" ")
}

/// The permissions of `scope` service by service (what stands before a permission's own name
/// says whose it is), each with the ones that belong to no service (who signs in): for a
/// provider that won't have the permissions of several services on one page.
fn apart(scope: &str) -> Vec<String> {
    let service = |permission: &str| permission.rsplit_once('/').map(|(service, _)| service.to_string());
    let (of_services, of_nobody): (Vec<&str>, Vec<&str>) = scope.split(' ').filter(|p| !p.is_empty()).partition(|p| service(p).is_some());
    let mut services: Vec<String> = Vec::new();
    for found in of_services.iter().filter_map(|permission| service(permission)) {
        if !services.contains(&found) {
            services.push(found);
        }
    }
    let one = |wanted: &String| {
        let own = of_services.iter().copied().filter(|permission| service(permission).as_ref() == Some(wanted));
        own.chain(of_nobody.iter().copied()).collect::<Vec<_>>().join(" ")
    };
    services.iter().map(one).collect()
}

/// Microsoft's admin-consent page for the directory `tenant`, in the form the Microsoft
/// identity platform documents (`https://login.microsoftonline.com/{tenant}/v2.0/adminconsent`).
///
/// `tenant` is the organisation's directory when it is known (the id an identity token names,
/// or a domain of its own, which the documentation takes as well); empty for `organizations`,
/// which lets the administrator's own sign-in say which it is. (`common` and the directory of
/// personal accounts have no administrator: they read as `organizations` too, as the
/// documentation asks.) What can't be part of an address is refused.
fn admin_consent_page(oauth: &OAuth, tenant: &str) -> Result<String, NetError> {
    let named = OAuth { tenant: tenant.trim().to_string(), ..oauth.clone() };
    let tenant = match self::tenant(&named)? {
        tenant if mailcraft_model::Directory::of_tenant(tenant) == mailcraft_model::Directory::Organisation => tenant,
        _ => "organizations",
    };
    Ok(format!("{MICROSOFT_LOGIN}/{tenant}/{MICROSOFT_ADMIN_CONSENT}"))
}

/// The address of Microsoft's page where an administrator of an organisation approves this
/// application, once for everybody in it, for everything the program asks an owner for. For
/// the owner to pass on to an administrator: nothing here opens it. [`admin_consent`] is for an owner who is one.
///
/// `tenant` is the organisation's directory when it is known, else empty (as for
/// [`admin_consent`]). The address names the application, the directory, the permissions and
/// where the page goes afterwards (the address the application is registered with, on the
/// administrator's own machine, where nothing is waiting: the approval is given before the
/// browser goes there). Nothing of the owner and no secret is in it.
pub fn admin_consent_url(oauth: &OAuth, tenant: &str) -> Result<String, NetError> {
    let client_id = client_id(oauth)?;
    let scope = admin_scope();
    let fields = [("client_id", client_id), ("scope", scope.as_str()), ("redirect_uri", REGISTERED_REDIRECT)];
    Ok(format!("{}?{}", admin_consent_page(oauth, tenant)?, http::form(&fields)))
}

/// Why an administrator's approval wasn't had ([`admin_consent`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdminRefusal {
    /// Whoever signed in on Microsoft's page may not approve for the organisation: the page
    /// came back with Microsoft's own numbers for "an administrator has to approve". With it,
    /// what Microsoft said, in a line.
    NotAdministrator(String),
    /// The page came back without the approval: it was declined there, or left by its way
    /// back. With it, what Microsoft said, in a line.
    Declined(String),
    /// Nothing came back in the time that is waited: the page was closed, or left open.
    TimedOut,
    /// Called off from here.
    Cancelled,
    /// Anything else, in Microsoft's words or as it went wrong.
    Other(NetError),
}

impl From<Refused> for AdminRefusal {
    fn from(refused: Refused) -> AdminRefusal {
        // What the provider said, without what this program puts before it.
        let words = |error: &NetError| match error {
            NetError::Proto(ProtoError::Auth(text) | ProtoError::Rejected(text)) => text.clone(),
            other => other.to_string(),
        };
        match (refused.policy, &refused.error) {
            (Policy::Approval, said) => AdminRefusal::NotAdministrator(words(said)),
            (_, NetError::SignInTimedOut) => AdminRefusal::TimedOut,
            (_, NetError::SignInCancelled) => AdminRefusal::Cancelled,
            (Policy::None, said) if refused.declined => AdminRefusal::Declined(words(said)),
            _ => AdminRefusal::Other(refused.error),
        }
    }
}

/// Have an administrator approve this application for a whole organisation, on Microsoft's
/// admin-consent page in the owner's browser: for an owner who is an administrator, or has one
/// beside them. `open` is given the address of the page (it must not wait); the page comes back
/// to a loopback port of this machine with a yes or a no and nothing else, and `Ok` is the yes.
/// Blocks for as long as that takes, five minutes at most; setting `cancel` ends the wait.
///
/// What is asked for is what [`admin_consent_url`] asks for, and `tenant` is as there. No tokens come of it: the account is signed in afterwards, as any other
/// ([`sign_in`]). Should Microsoft not take the permissions of several services on one page, it
/// is asked again service by service (`open` is then given an address for each).
///
/// Nothing of what comes back is believed about who anybody is: the directory the page names
/// is not read at all.
pub fn admin_consent(oauth: &OAuth, tenant: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<(), AdminRefusal> {
    admin_consent_within(oauth, tenant, open, cancel, SIGN_IN_TIMEOUT)
}

/// [`admin_consent`], waiting no longer than `patience` for each page.
fn admin_consent_within(oauth: &OAuth, tenant: &str, open: &dyn Fn(&str), cancel: &AtomicBool, patience: Duration) -> Result<(), AdminRefusal> {
    let all = admin_scope();
    match ask_administrator(oauth, tenant, &all, open, cancel, patience) {
        // The page won't have what it was asked for together: once more, a page for each service.
        Err(refused) if refused.about == About::Scope && apart(&all).len() > 1 => {
            log::debug!("the approval page was refused for what it asked for; asking service by service");
            for part in apart(&all) {
                ask_administrator(oauth, tenant, &part, open, cancel, patience)?;
            }
            Ok(())
        }
        outcome => outcome.map_err(AdminRefusal::from),
    }
}

/// One go at an administrator's approval of `scope`: the page, and the wait for its answer.
fn ask_administrator(oauth: &OAuth, tenant: &str, scope: &str, open: &dyn Fn(&str), cancel: &AtomicBool, patience: Duration) -> Result<(), Refused> {
    let client_id = client_id(oauth)?;
    let page = admin_consent_page(oauth, tenant)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(NetError::SignInCancelled.into());
    }
    let redirect = Redirect::listen()?;
    // As for a sign-in: the registered address, on whichever port is free here.
    let redirect_uri = format!("http://localhost:{}", redirect.port);
    // Only the page that was given the state can send it back: nobody else on this machine can
    // say yes or no in its place.
    let state = random(24)?;
    log::debug!("waiting on port {} for the administrator's approval to come back", redirect.port);
    let fields = [("client_id", client_id), ("scope", scope), ("redirect_uri", redirect_uri.as_str()), ("state", state.as_str())];
    open(&format!("{page}?{}", http::form(&fields)));
    redirect.wait_for(&state, cancel, patience, judge_consent)
}

/// Ask the owner, on the provider's page, to let this program read the Favorites of Outlook
/// (the mailbox's configuration items, and its folders by name), beside what Microsoft Graph was
/// asked for before. The tokens that come back are Graph's, for all of it. Asked for by itself
/// and never with an ordinary sign-in: a page that is refused here costs nothing that was had.
///
/// Nothing is tried a second time: a no is the answer, and it says whose it is
/// ([`FavoritesRefusal`]).
pub fn sign_in_for_favorites(oauth: &OAuth, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Tokens, FavoritesRefusal> {
    let plain = Endpoints::for_resource(oauth, Resource::Graph);
    let scope = format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_EXTRAS} {MICROSOFT_GRAPH_FAVORITES} {SIGN_IN_SCOPE}");
    let endpoints = Endpoints { scope, consent: String::new(), lesser: String::new(), ..plain };
    sign_in_for_favorites_within(&endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT)
}

/// [`sign_in_for_favorites`] at `endpoints`, waiting no longer than `patience` for the owner.
fn sign_in_for_favorites_within(
    endpoints: &Endpoints,
    oauth: &OAuth,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    patience: Duration,
) -> Result<Tokens, FavoritesRefusal> {
    attempt(endpoints, "", oauth, login_hint, open, cancel, patience).map_err(|refused| match (refused.policy, refused.about) {
        (Policy::Approval, _) => FavoritesRefusal::Approval,
        (Policy::Blocked, _) => FavoritesRefusal::Organisation,
        (Policy::Account, _) | (Policy::None, About::Scope) => FavoritesRefusal::Account,
        (Policy::None, _) => FavoritesRefusal::Other(refused.error),
    })
}

/// What the tokens of an account's Graph run for a shared mailbox are asked for (#70): what its
/// own mail takes, and the shared mailboxes' on top.
fn shared_scope() -> String {
    format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_MAIL} {MICROSOFT_GRAPH_SHARED} {SIGN_IN_SCOPE}")
}

/// Ask the owner, on the provider's page, to let this program read the shared mailboxes he was
/// given Full Access to (`Mail.ReadWrite.Shared`), beside what Microsoft Graph was asked for
/// before. Only when he has opened one (#70), and never with an ordinary sign-in. The tokens
/// that come back are Graph's, for all of it; a no says whose it is, in the same kinds as the
/// Favorites' ([`FavoritesRefusal`]). Nothing is tried a second time.
pub fn sign_in_for_shared(oauth: &OAuth, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Tokens, FavoritesRefusal> {
    let plain = Endpoints::for_resource(oauth, Resource::GraphMail);
    let endpoints = Endpoints { scope: shared_scope(), consent: String::new(), lesser: String::new(), ..plain };
    sign_in_for_favorites_within(&endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT)
}

/// [`refresh_graph_mail`] for a Graph run of a shared mailbox (#70): the tokens are asked for
/// with `Mail.ReadWrite.Shared` too, and for nothing less: a sign-in that doesn't cover it gets
/// [`NetError::ConsentNeeded`], and the owner is asked on the provider's page
/// ([`sign_in_for_shared`]). The owner's own runs are not touched by it.
pub fn refresh_graph_shared(oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    refresh_graph_shared_at(&Endpoints::for_resource(oauth, Resource::GraphMail), oauth, refresh_token)
}

/// [`refresh_graph_shared`] at endpoints other than the provider's usual ones (their scope is
/// replaced by what a shared mailbox takes).
pub fn refresh_graph_shared_at(endpoints: &Endpoints, oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    let endpoints = Endpoints { scope: shared_scope(), ..endpoints.clone() };
    let tokens = refresh_for_at(&endpoints, oauth, refresh_token, Resource::GraphMail)?;
    Ok(GraphTokens { tokens, extras_missing: true, covers_favorites: false })
}

/// What the tokens of a shared mailbox's run are asked for when it has mail to send (#70): what
/// reading it takes, and sending from it on top.
fn shared_send_scope() -> String {
    format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_MAIL} {MICROSOFT_GRAPH_SHARED} {MICROSOFT_GRAPH_SHARED_SEND} {SIGN_IN_SCOPE}")
}

/// Ask the owner, on the provider's page, to let this program send from the shared mailboxes he
/// may send as or on behalf of (`Mail.Send.Shared`), beside what reading them takes. Only at the
/// first send from one (#70), after the window has asked him, and never with an ordinary
/// sign-in. A no says whose it is ([`FavoritesRefusal`]); nothing is tried a second time.
pub fn sign_in_for_shared_send(oauth: &OAuth, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Tokens, FavoritesRefusal> {
    let plain = Endpoints::for_resource(oauth, Resource::GraphMail);
    let endpoints = Endpoints { scope: shared_send_scope(), consent: String::new(), lesser: String::new(), ..plain };
    sign_in_for_favorites_within(&endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT)
}

/// [`refresh_graph_shared`] for a run of a shared mailbox that has mail to send (#70): the tokens
/// are asked for with `Mail.Send.Shared` too, and for nothing less. A sign-in that doesn't cover
/// it gets [`NetError::ConsentNeeded`]: nothing is sent, and the window asks the owner
/// ([`sign_in_for_shared_send`]).
pub fn refresh_graph_shared_send(oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    refresh_graph_shared_send_at(&Endpoints::for_resource(oauth, Resource::GraphMail), oauth, refresh_token)
}

/// [`refresh_graph_shared_send`] at endpoints other than the provider's usual ones (their scope
/// is replaced by what sending from a shared mailbox takes).
pub fn refresh_graph_shared_send_at(endpoints: &Endpoints, oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    let endpoints = Endpoints { scope: shared_send_scope(), ..endpoints.clone() };
    let tokens = refresh_for_at(&endpoints, oauth, refresh_token, Resource::GraphMail)?;
    Ok(GraphTokens { tokens, extras_missing: true, covers_favorites: false })
}

/// What the tokens of a Graph run for a shared mailbox's calendars are asked for (#70): what
/// reading a shared mailbox takes, and its calendars on top.
fn shared_calendar_scope() -> String {
    format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_MAIL} {MICROSOFT_GRAPH_SHARED} {MICROSOFT_GRAPH_SHARED_CALENDARS} {SIGN_IN_SCOPE}")
}

/// Ask the owner, on the provider's page, to let this program read the calendars of the shared
/// mailboxes he was given access to (`Calendars.ReadWrite.Shared`), beside what reading their mail
/// takes. Only when he first shows one of them (#70), after the window has asked him, and never
/// with an ordinary sign-in. A no says whose it is ([`FavoritesRefusal`]); nothing is tried a
/// second time.
pub fn sign_in_for_shared_calendars(oauth: &OAuth, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Tokens, FavoritesRefusal> {
    let plain = Endpoints::for_resource(oauth, Resource::GraphMail);
    let endpoints = Endpoints { scope: shared_calendar_scope(), consent: String::new(), lesser: String::new(), ..plain };
    sign_in_for_favorites_within(&endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT)
}

/// [`refresh_graph_shared`] for a Graph run of a shared mailbox's calendars (#70): the tokens are
/// asked for with `Calendars.ReadWrite.Shared` too, and for nothing less. A sign-in that doesn't
/// cover it gets [`NetError::ConsentNeeded`]: nothing of the calendars is read, and the window
/// asks the owner ([`sign_in_for_shared_calendars`]).
pub fn refresh_graph_shared_calendars(oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    refresh_graph_shared_calendars_at(&Endpoints::for_resource(oauth, Resource::GraphMail), oauth, refresh_token)
}

/// [`refresh_graph_shared_calendars`] at endpoints other than the provider's usual ones (their
/// scope is replaced by what a shared mailbox's calendars take).
pub fn refresh_graph_shared_calendars_at(endpoints: &Endpoints, oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    let endpoints = Endpoints { scope: shared_calendar_scope(), ..endpoints.clone() };
    let tokens = refresh_for_at(&endpoints, oauth, refresh_token, Resource::GraphMail)?;
    Ok(GraphTokens { tokens, extras_missing: true, covers_favorites: false })
}

/// What the tokens of a Graph run for a shared mailbox's calendars are asked for when it has
/// something to send: a meeting answered, called off or forwarded, or a meeting with people
/// invited (#70). What its calendars take, and sending from the mailbox on top.
fn shared_calendar_send_scope() -> String {
    format!(
        "{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_MAIL} {MICROSOFT_GRAPH_SHARED} {MICROSOFT_GRAPH_SHARED_CALENDARS} {MICROSOFT_GRAPH_SHARED_SEND} {SIGN_IN_SCOPE}"
    )
}

/// [`refresh_graph_shared_calendars`] for a run that has something of a shared mailbox's calendar
/// to send (#70): the tokens are asked for with `Mail.Send.Shared` too, and for nothing less. A
/// sign-in that doesn't cover it gets [`NetError::ConsentNeeded`]: nothing is sent, and the window
/// asks the owner as it does for mail ([`sign_in_for_shared_send`]).
pub fn refresh_graph_shared_calendars_send(oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    refresh_graph_shared_calendars_send_at(&Endpoints::for_resource(oauth, Resource::GraphMail), oauth, refresh_token)
}

/// [`refresh_graph_shared_calendars_send`] at endpoints other than the provider's usual ones
/// (their scope is replaced).
pub fn refresh_graph_shared_calendars_send_at(endpoints: &Endpoints, oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    let endpoints = Endpoints { scope: shared_calendar_send_scope(), ..endpoints.clone() };
    let tokens = refresh_for_at(&endpoints, oauth, refresh_token, Resource::GraphMail)?;
    Ok(GraphTokens { tokens, extras_missing: true, covers_favorites: false })
}

/// What the tokens of a calendar job for an account with calendars opened by address are asked
/// for (#70, Open Shared Calendar): what its calendar takes, and `Calendars.ReadWrite.Shared` on
/// top, the same permission as a shared mailbox's calendars take. Not the mail permissions of
/// shared mailboxes: an account that only opened somebody's calendar is never asked for those.
fn opened_calendars_scope() -> String {
    format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_SHARED_CALENDARS} {SIGN_IN_SCOPE}")
}

/// [`sign_in_for_shared_calendars`] for an account that opened somebody's calendar by address:
/// the same one permission, asked for by itself and never with a sign-in, without what reading a
/// shared mailbox's mail takes.
pub fn sign_in_for_opened_calendars(oauth: &OAuth, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Tokens, FavoritesRefusal> {
    let plain = Endpoints::for_resource(oauth, Resource::Graph);
    let endpoints = Endpoints { scope: opened_calendars_scope(), consent: String::new(), lesser: String::new(), ..plain };
    sign_in_for_favorites_within(&endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT)
}

/// [`refresh_graph_shared_calendars`] for a calendar job of an account with calendars opened by
/// address (#70): asked for with `Calendars.ReadWrite.Shared` too, and for nothing less. A sign-in
/// that doesn't cover it gets [`NetError::ConsentNeeded`], and the job goes on with the account's
/// own calendars while the owner is asked ([`sign_in_for_opened_calendars`]).
pub fn refresh_graph_opened_calendars(oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    refresh_graph_opened_calendars_at(&Endpoints::for_resource(oauth, Resource::Graph), oauth, refresh_token)
}

/// [`refresh_graph_opened_calendars`] at endpoints other than the provider's usual ones.
pub fn refresh_graph_opened_calendars_at(endpoints: &Endpoints, oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    let endpoints = Endpoints { scope: opened_calendars_scope(), ..endpoints.clone() };
    let tokens = refresh_for_at(&endpoints, oauth, refresh_token, Resource::Graph)?;
    Ok(GraphTokens { tokens, extras_missing: true, covers_favorites: false })
}

/// What the tokens of a to-do job are asked for (#248): what Graph is asked for, and the lists
/// and tasks of the owner on top.
fn tasks_scope() -> String {
    format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_TASKS} {SIGN_IN_SCOPE}")
}

/// Ask the owner, on the provider's page, to let this program read and change the to-dos of the
/// signed-in owner (`Tasks.ReadWrite`). Only when a to-do job found the sign-in doesn't cover
/// it, after the window has asked him, and never with an ordinary sign-in of an account that
/// was made before it was asked for. A no says whose it is ([`FavoritesRefusal`]); nothing is
/// tried a second time.
pub fn sign_in_for_tasks(oauth: &OAuth, login_hint: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Tokens, FavoritesRefusal> {
    let plain = Endpoints::for_resource(oauth, Resource::Graph);
    let endpoints = Endpoints { scope: tasks_scope(), consent: String::new(), lesser: String::new(), ..plain };
    sign_in_for_favorites_within(&endpoints, oauth, login_hint, open, cancel, SIGN_IN_TIMEOUT)
}

/// [`refresh_graph`] for a to-do job (#248): the tokens are asked for with `Tasks.ReadWrite`,
/// and for nothing less. A sign-in that doesn't cover it gets [`NetError::ConsentNeeded`]: the
/// to-dos are neither read nor sent, everything else goes on, and the window asks the owner
/// ([`sign_in_for_tasks`]).
pub fn refresh_graph_tasks(oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    refresh_graph_tasks_at(&Endpoints::for_resource(oauth, Resource::Graph), oauth, refresh_token)
}

/// [`refresh_graph_tasks`] at endpoints other than the provider's usual ones.
pub fn refresh_graph_tasks_at(endpoints: &Endpoints, oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    let endpoints = Endpoints { scope: tasks_scope(), ..endpoints.clone() };
    let tokens = refresh_for_at(&endpoints, oauth, refresh_token, Resource::Graph)?;
    Ok(GraphTokens { tokens, extras_missing: true, covers_favorites: false })
}

/// One go at signing in: the page (asked for the tokens' scopes and for `consent`), the wait for
/// the browser, and the code traded in for tokens (for the tokens' scopes only: a token request
/// names one resource).
fn attempt(
    endpoints: &Endpoints,
    consent: &str,
    oauth: &OAuth,
    login_hint: &str,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    patience: Duration,
) -> Result<Tokens, Refused> {
    let client_id = client_id(oauth)?;
    tenant(oauth)?;
    // Both addresses are checked before the owner is sent anywhere.
    Url::parse(&endpoints.authorize)?;
    let token_endpoint = Url::parse(&endpoints.token)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(NetError::SignInCancelled.into());
    }
    let redirect = Redirect::listen()?;
    // As it is registered with the provider (`http://localhost`), which ignores the port of a
    // loopback address but compares the rest exactly: no path, no trailing slash.
    let redirect_uri = format!("http://localhost:{}", redirect.port);
    // PKCE: only who knows the verifier can trade the code in, and only its hash goes through the browser.
    let verifier = random(32)?;
    let state = random(24)?;
    log::debug!("waiting on port {} for the sign-in to come back", redirect.port);
    open(&authorize_url(endpoints, consent, client_id, &redirect_uri, &state, &challenge(&verifier), login_hint));
    let code = redirect.wait(&state, cancel, patience)?;
    drop(redirect);
    let fields = [
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("code_verifier", verifier.as_str()),
        ("scope", endpoints.scope.as_str()),
    ];
    trade(&token_endpoint, &fields, "", &[code.as_str(), verifier.as_str()])
}

/// Trade a refresh token for new tokens for the mail servers, without asking the owner anything.
///
/// The tokens that come back carry the refresh token to use next time: a new one when the
/// provider sent one, the one given otherwise. `ProtoError::Auth` means the provider no longer
/// takes it and the owner has to [`sign_in`] again.
pub fn refresh(oauth: &OAuth, refresh_token: &str) -> Result<Tokens, NetError> {
    refresh_for(oauth, refresh_token, Resource::Mail)
}

/// [`refresh`] for the tokens of `resource`: the same refresh token gets an access token for the
/// mail servers or one for Microsoft Graph, whichever is asked for.
///
/// For [`Resource::Graph`], three kinds of failure are told apart:
/// - [`NetError::ConsentNeeded`]: the refresh token is good, but there is no token for Graph
///   until the owner agrees to that on the provider's page ([`sign_in_for`]). Mail is not
///   affected, and signing in to mail again is not the cure.
/// - `ProtoError::Auth` ([`NetError::is_refused_sign_in`]): the provider no longer takes the
///   refresh token for anything; the owner has to [`sign_in`] again.
/// - anything else: the provider couldn't be asked, or couldn't say; the refresh token is as
///   good as it was.
///
/// For [`Resource::Mail`] every refusal is `ProtoError::Auth`, as [`refresh`] has it: signing
/// in again is what puts it right, whatever it was about.
pub fn refresh_for(oauth: &OAuth, refresh_token: &str, resource: Resource) -> Result<Tokens, NetError> {
    refresh_for_at(&Endpoints::for_resource(oauth, resource), oauth, refresh_token, resource)
}

/// [`refresh`] at endpoints other than the provider's usual ones.
pub fn refresh_at(endpoints: &Endpoints, oauth: &OAuth, refresh_token: &str) -> Result<Tokens, NetError> {
    refresh_for_at(endpoints, oauth, refresh_token, Resource::Mail)
}

/// [`refresh_for`] at endpoints other than the provider's usual ones: the tokens are asked for
/// [`Endpoints::scope`], and a refusal is read as one of a token for `resource`.
pub fn refresh_for_at(endpoints: &Endpoints, oauth: &OAuth, refresh_token: &str, resource: Resource) -> Result<Tokens, NetError> {
    let client_id = client_id(oauth)?;
    tenant(oauth)?;
    let token_endpoint = Url::parse(&endpoints.token)?;
    if refresh_token.is_empty() || !usable(refresh_token) {
        return Err(NetError::Proto(ProtoError::Auth("there is no earlier sign-in to renew".into())));
    }
    let fields = [("grant_type", "refresh_token"), ("client_id", client_id), ("refresh_token", refresh_token), ("scope", endpoints.scope.as_str())];
    trade(&token_endpoint, &fields, refresh_token, &[refresh_token]).map_err(|refused| match (resource, refused.about, refused.error) {
        // The sign-in is good and Graph is what it doesn't cover: because nobody agreed to it,
        // or because the provider won't hand it to this application. Asking the owner on the
        // provider's page is how either is put right, or shown for what it is.
        (Resource::Graph | Resource::GraphMail, About::Consent | About::Scope, NetError::Proto(ProtoError::Auth(why))) => {
            // Said in a code alone, this reads like a sign-in that is over, which it isn't.
            NetError::ConsentNeeded(if why == STALE { "the sign-in doesn't cover Microsoft Graph".to_string() } else { why })
        }
        (_, _, error) => error,
    })
}

/// The tokens [`refresh_graph`] got for Microsoft Graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphTokens {
    pub tokens: Tokens,
    /// The sign-in doesn't cover the optional extras (reading the mailbox's settings, for its
    /// list of categories): the tokens are for everything else. Asking the owner on the
    /// provider's page ([`sign_in_for`]) adds them.
    pub extras_missing: bool,
    /// The tokens are also good for reading the Favorites of Outlook: they were asked for with
    /// that ([`refresh_graph_with_favorites`]) and the sign-in covers it. When not, asking the
    /// owner on the provider's page ([`sign_in_for_favorites`]) is what adds it.
    pub covers_favorites: bool,
}

/// [`refresh_for`] with [`Resource::Graph`], for a sign-in that may be older than what Graph is
/// asked for today: nothing that worked stops working.
///
/// The tokens are asked for everything first. When the provider won't hand that out with this
/// refresh token, they are asked for without the optional extras, which is what the owner may
/// have agreed to before those were asked for; that they are missing comes back with the tokens.
///
/// A provider may say "this sign-in doesn't cover that" in the words it has for "this sign-in is
/// over" (a bare `invalid_grant`). The two are told apart by asking for mail with the same
/// refresh token: when that still works, the sign-in is good and Graph is what it doesn't cover
/// ([`NetError::ConsentNeeded`]: the owner is to be asked on the provider's page, and mail goes
/// on meanwhile); when it doesn't, the sign-in is over (`ProtoError::Auth`).
pub fn refresh_graph(oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    let plain = Endpoints::for_resource(oauth, Resource::Graph);
    let all = Endpoints { scope: format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_EXTRAS} {SIGN_IN_SCOPE}"), ..plain.clone() };
    refresh_graph_at(&all, &plain, &Endpoints::of(oauth), oauth, refresh_token)
}

/// [`refresh_graph`] for an account whose folders and mail are kept in step through Graph
/// ([`Resource::GraphMail`]): the tokens are asked for with `Mail.ReadWrite` too. A sign-in that
/// doesn't cover it gets [`NetError::ConsentNeeded`], and the owner is asked on the provider's
/// page ([`sign_in_for`] with [`Resource::GraphMail`]); nothing is asked without it, since the
/// account's mail can't be had any other way.
pub fn refresh_graph_mail(oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    let plain = Endpoints::for_resource(oauth, Resource::GraphMail);
    let all = Endpoints { scope: format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_MAIL} {MICROSOFT_GRAPH_EXTRAS} {SIGN_IN_SCOPE}"), ..plain.clone() };
    refresh_graph_at(&all, &plain, &Endpoints::of(oauth), oauth, refresh_token)
}

/// [`refresh_graph`] at endpoints other than the provider's usual ones: `all` asks for
/// everything, `plain` for everything but the optional extras, `mail` for the mail servers.
pub fn refresh_graph_at(all: &Endpoints, plain: &Endpoints, mail: &Endpoints, oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    // A no that could be about what was asked for rather than about who asked.
    let about_scope = |error: &NetError| error.is_consent_needed() || error.is_refused_sign_in();
    let first = match refresh_for_at(all, oauth, refresh_token, Resource::Graph) {
        Ok(tokens) => return Ok(GraphTokens { tokens, extras_missing: false, covers_favorites: false }),
        Err(error) if about_scope(&error) && plain.scope != all.scope => error,
        Err(error) => return Err(error),
    };
    log::debug!("the sign-in doesn't cover everything Microsoft Graph is asked for; asking without the extras");
    match refresh_for_at(plain, oauth, refresh_token, Resource::Graph) {
        Ok(tokens) => Ok(GraphTokens { tokens, extras_missing: true, covers_favorites: false }),
        Err(error) if error.is_consent_needed() => Err(error),
        // "No longer good": for anything at all, or only for Graph?
        Err(error) if error.is_refused_sign_in() => match refresh_for_at(mail, oauth, refresh_token, Resource::Mail) {
            Ok(_) => Err(NetError::ConsentNeeded("the sign-in doesn't cover Microsoft Graph".into())),
            Err(_) => Err(error),
        },
        // The provider couldn't say this time: nothing is concluded from the first no.
        Err(error) => {
            log::debug!("asking without the extras came to nothing (the first answer was: {first})");
            Err(error)
        }
    }
}

/// [`refresh_graph`], asking first for tokens that are also good for reading the Favorites of
/// Outlook ([`GraphTokens::covers_favorites`]). A sign-in that doesn't cover that (the owner
/// was never asked, or said no) gets its tokens exactly as [`refresh_graph`] gets them: nothing
/// that worked stops working, and nothing but that one question is put a second time.
pub fn refresh_graph_with_favorites(oauth: &OAuth, refresh_token: &str) -> Result<GraphTokens, NetError> {
    let plain = Endpoints::for_resource(oauth, Resource::Graph);
    let all = Endpoints { scope: format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_EXTRAS} {SIGN_IN_SCOPE}"), ..plain.clone() };
    let most =
        Endpoints { scope: format!("{MICROSOFT_GRAPH} {MICROSOFT_GRAPH_EXTRAS} {MICROSOFT_GRAPH_FAVORITES} {SIGN_IN_SCOPE}"), ..plain.clone() };
    refresh_graph_with_favorites_at(&most, &all, &plain, &Endpoints::of(oauth), oauth, refresh_token)
}

/// [`refresh_graph_with_favorites`] at endpoints other than the provider's usual ones: `most`
/// asks for everything and the Favorites' permissions; the others are [`refresh_graph_at`]'s.
pub fn refresh_graph_with_favorites_at(
    most: &Endpoints,
    all: &Endpoints,
    plain: &Endpoints,
    mail: &Endpoints,
    oauth: &OAuth,
    refresh_token: &str,
) -> Result<GraphTokens, NetError> {
    match refresh_for_at(most, oauth, refresh_token, Resource::Graph) {
        Ok(tokens) => Ok(GraphTokens { tokens, extras_missing: false, covers_favorites: true }),
        // A no that could be about what was asked for rather than about who asked.
        Err(error) if (error.is_consent_needed() || error.is_refused_sign_in()) && most.scope != all.scope => {
            log::debug!("the sign-in doesn't cover the Favorites; asking for what Microsoft Graph was asked for before them");
            refresh_graph_at(all, plain, mail, oauth, refresh_token)
        }
        Err(error) => Err(error),
    }
}

/// What a refusal that says no more than `invalid_grant` is taken to mean.
const STALE: &str = "the earlier sign-in is no longer good";

/// What a provider's no is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum About {
    /// Who asked, or what they showed: the code, the refresh token; or nothing that is known.
    Other,
    /// What was asked for: a scope or a resource the provider won't hand out, or not together.
    Scope,
    /// The owner's say-so: nobody has agreed to what was asked for, or the owner has to be
    /// asked on the provider's page again.
    Consent,
}

/// A sign-in or a renewal that came to nothing: the error to hand on, and what the no was about.
#[derive(Debug, PartialEq)]
struct Refused {
    error: NetError,
    about: About,
    /// Whose no it is, when it is not the owner's to turn into a yes.
    policy: Policy,
    /// The no was given on the provider's page, or the page sent whoever was on it back with
    /// one: `access_denied`, `consent_required`, or Microsoft's number for it.
    declined: bool,
}

/// A no that is not the owner's to turn into a yes: whose it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Policy {
    /// None such: the owner's own no, or nothing that is known.
    None,
    /// An administrator of the organisation has to approve what was asked for
    /// ([`APPROVAL_CODES`]).
    Approval,
    /// The organisation doesn't have it for this person, approved or not ([`BLOCKED_CODES`]).
    Blocked,
    /// This kind of account can't have it ([`ACCOUNT_CODES`]).
    Account,
}

impl From<NetError> for Refused {
    fn from(error: NetError) -> Refused {
        Refused { error, about: About::Other, policy: Policy::None, declined: false }
    }
}

/// Whether a no was given on the provider's page: the error code says so (RFC 6749 §4.1.2.1's
/// `access_denied`; `consent_required`, which is what Microsoft's admin-consent page comes back
/// with), or Microsoft's number for it does.
fn declined(error: &str, description: &str) -> bool {
    let error = error.trim().to_ascii_lowercase();
    matches!(error.as_str(), "access_denied" | "consent_required") || aadsts(description).any(|code| DECLINED_CODES.contains(&code))
}

/// Whose no it is, by Microsoft's numbers for it (listed in `codes`, or named in the
/// description). "Shut" goes before "an administrator has to agree": approving a permission
/// opens nothing that is shut for another reason.
fn by_policy(description: &str, codes: &[u64]) -> Policy {
    let named = |numbers: &[u64]| codes.iter().take(MAX_CODES).copied().chain(aadsts(description)).any(|code| numbers.contains(&code));
    if named(&BLOCKED_CODES) {
        Policy::Blocked
    } else if named(&APPROVAL_CODES) {
        Policy::Approval
    } else if named(&ACCOUNT_CODES) {
        Policy::Account
    } else {
        Policy::None
    }
}

/// The numbers of the `AADSTS…` codes a description names (Microsoft's own error numbers).
fn aadsts(description: &str) -> impl Iterator<Item = u64> + '_ {
    description.match_indices("AADSTS").take(MAX_CODES).filter_map(move |(at, tag)| {
        let digits: String = description.get(at.saturating_add(tag.len())..)?.chars().take_while(char::is_ascii_digit).take(12).collect();
        digits.parse().ok()
    })
}

/// What a no is about, from its error code (RFC 6749 §4.1.2.1 and §5.2) and from what Microsoft
/// adds to it: a `suberror`, and its own numbers, listed (`codes`) or in the description.
fn about(error: &str, suberror: &str, description: &str, codes: &[u64]) -> About {
    let (error, suberror) = (error.trim().to_ascii_lowercase(), suberror.trim().to_ascii_lowercase());
    let named = |numbers: &[u64]| codes.iter().take(MAX_CODES).copied().chain(aadsts(description)).any(|code| numbers.contains(&code));
    if matches!(error.as_str(), "invalid_scope" | "invalid_resource") || named(&SCOPE_CODES) {
        About::Scope
    } else if matches!(error.as_str(), "consent_required" | "interaction_required") || suberror == "consent_required" || named(&CONSENT_CODES) {
        About::Consent
    } else {
        About::Other
    }
}

/// Text from outside for an error message: short, and without control characters.
fn shown(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(60).collect()
}

/// The application (client) id the provider knows this program by.
fn client_id(oauth: &OAuth) -> Result<&str, NetError> {
    let id = oauth.client_id.trim();
    if id.is_empty() {
        return Err(NetError::Config(format!("an application (client) id is needed to sign in with {}", oauth.provider.label())));
    }
    if id.len() > 200 || !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(NetError::Config(format!("\"{}\" can't be an application (client) id", shown(id))));
    }
    Ok(id)
}

/// The directory to sign in to: `common` unless the settings name one. It becomes part of an
/// address, so it is letters, digits, dots and hyphens or it is refused.
fn tenant(oauth: &OAuth) -> Result<&str, NetError> {
    let tenant = oauth.tenant.trim();
    if tenant.is_empty() {
        return Ok("common");
    }
    let plain = tenant.len() <= 253 && tenant.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'));
    let edges = |b: Option<u8>| b.is_some_and(|b| b.is_ascii_alphanumeric());
    if plain && edges(tenant.bytes().next()) && edges(tenant.bytes().next_back()) && !tenant.contains("..") {
        Ok(tenant)
    } else {
        Err(NetError::Config(format!(
            "\"{}\" can't be a {} tenant: use common, consumers, organizations, or a directory's id or domain",
            shown(tenant),
            oauth.provider.label()
        )))
    }
}

/// `bytes` random bytes as text that is safe in a URL.
fn random(bytes: usize) -> Result<String, NetError> {
    let mut buf = vec![0u8; bytes.min(64)];
    ring::rand::SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| NetError::Config("this computer couldn't make the random numbers a sign-in is protected with".into()))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

/// What the provider is shown of a PKCE verifier: its SHA-256 (the `S256` method of RFC 7636).
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()))
}

/// The address of the provider's page, with everything it needs to send the owner back here.
/// The page is asked for the tokens' scopes and, after them, for `consent`: what the owner
/// agrees to there, a later request for tokens doesn't have to ask about.
fn authorize_url(
    endpoints: &Endpoints,
    consent: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
    login_hint: &str,
) -> String {
    let scope = if consent.trim().is_empty() { endpoints.scope.clone() } else { format!("{} {}", endpoints.scope.trim(), consent.trim()) };
    let mut fields = vec![
        ("client_id", client_id),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri),
        ("response_mode", "query"),
        ("scope", scope.as_str()),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];
    let hint = login_hint.trim();
    if !hint.is_empty() && hint.len() <= 320 && !hint.chars().any(char::is_control) {
        fields.push(("login_hint", hint));
    }
    // The owner picks the account on the page, rather than being signed in as whoever the browser remembers.
    fields.push(("prompt", "select_account"));
    let address = endpoints.authorize.trim();
    format!("{address}{}{}", if address.contains('?') { '&' } else { '?' }, http::form(&fields))
}

/// Whether a token is something that can be sent on: visible ASCII (RFC 6749 §A.12) of a sane length.
fn usable(token: &str) -> bool {
    token.len() <= MAX_TOKEN && token.bytes().all(|b| matches!(b, b' '..=b'~'))
}

/// The error for a provider that said no: `error` is its code (`access_denied`,
/// `invalid_grant`), `description` what it adds for people.
fn refusal(error: &str, description: &str) -> NetError {
    // The first line is the one for people; trace ids and timestamps follow.
    let first = description.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    let code = shown(&http::tidy(error));
    let text = match (first, code.as_str()) {
        ("", "access_denied") => "access was refused on the sign-in page".into(),
        ("", "invalid_grant") => STALE.into(),
        ("", _) => format!("the sign-in server said \"{code}\""),
        _ => http::tidy(first),
    };
    NetError::Proto(match code.as_str() {
        // Not a no: the server can't just now.
        "server_error" | "temporarily_unavailable" => ProtoError::Rejected(text),
        _ => ProtoError::Auth(text),
    })
}

/// POST the form to the token endpoint and read the tokens out of the answer. `secrets` are
/// the parts of the form that must not come back in an error.
fn trade(endpoint: &Url, fields: &[(&str, &str)], earlier_refresh: &str, secrets: &[&str]) -> Result<Tokens, Refused> {
    let answer = http::post_form(endpoint, fields, TOKEN_TIMEOUT).map_err(|e| secrets.iter().fold(e, |e, secret| e.without(secret)))?;
    tokens_from(&answer, earlier_refresh, mailcraft_time::now(), secrets)
}

/// The tokens in a token endpoint's answer (RFC 6749 §5.1), or why there are none (§5.2), with
/// what the no is about. The refresh token stays the earlier one when the answer has no new one.
fn tokens_from(answer: &Answer, earlier_refresh: &str, now: i64, secrets: &[&str]) -> Result<Tokens, Refused> {
    let nonsense = |text: &str| Refused::from(NetError::Proto(ProtoError::Protocol(text.into())));
    let json = serde_json::from_slice::<Value>(&answer.body).ok().filter(Value::is_object);
    let text = |name: &str| json.as_ref().and_then(|json| json.get(name)).and_then(Value::as_str).unwrap_or("");
    // A refusal is one whatever status it comes with.
    if !text("error").is_empty() {
        // A careless server may repeat what it was sent; that is taken out before the text is
        // shortened, as a secret cut in half would no longer be recognised.
        let hide = |text: &str| secrets.iter().fold(text.to_string(), |text, secret| crate::scrub(&text, secret));
        // Microsoft lists its own numbers for the refusal beside the description that names them.
        let listed = json.as_ref().and_then(|json| json.get("error_codes")).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let codes: Vec<u64> = listed.iter().take(MAX_CODES).filter_map(Value::as_u64).collect();
        return Err(Refused {
            error: refusal(&hide(text("error")), &hide(text("error_description"))),
            about: about(text("error"), text("suberror"), text("error_description"), &codes),
            policy: by_policy(text("error_description"), &codes),
            // (The token endpoint is asked by this program, not by a person on a page.)
            declined: false,
        });
    }
    if answer.status != 200 {
        return Err(nonsense(&format!("the sign-in server answered with status {}", answer.status)));
    }
    let Some(json) = &json else { return Err(nonsense("the sign-in server's answer isn't the JSON it should be")) };
    let kind = text("token_type");
    if !kind.is_empty() && !kind.eq_ignore_ascii_case("bearer") {
        return Err(nonsense("the sign-in server sent a kind of token that mail servers don't take"));
    }
    let access = text("access_token");
    if access.is_empty() {
        return Err(nonsense("the sign-in server's answer has no access token"));
    }
    let refresh = match text("refresh_token") {
        "" => earlier_refresh,
        new => new,
    };
    if !usable(access) || !usable(refresh) {
        return Err(nonsense("the sign-in server sent a token that can't be used"));
    }
    // Seconds, as a number or (from some servers) as text.
    let lifetime = match json.get("expires_in") {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|seconds| seconds as i64)),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    let lifetime = lifetime.unwrap_or(DEFAULT_LIFETIME).clamp(0, MAX_LIFETIME);
    Ok(Tokens { access: access.to_string(), refresh: refresh.to_string(), expires_at: now.saturating_add(lifetime), ..identity(text("id_token")) })
}

/// Who signed in, from the identity token that may come with the others (a JWT: three parts in
/// base64, of which the middle one is JSON): the name, the address from `email` or else
/// `preferred_username`, and the directory that signed them in (`tid`). Only these three fields
/// are set, and any of them may be empty.
///
/// The token's signature isn't checked: it came from the token endpoint itself, and what is
/// read only fills in a name the owner can change. It is read as hostile all the same.
fn identity(id_token: &str) -> Tokens {
    let mut parts = id_token.split('.');
    let payload = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(_), Some(payload), Some(_), None) if id_token.len() <= MAX_TOKEN => payload,
        _ => return Tokens::default(),
    };
    let claims = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok().and_then(|json| serde_json::from_slice::<Value>(&json).ok());
    let claim = |name: &str| claims.as_ref().and_then(|claims| claims.get(name)).and_then(Value::as_str).unwrap_or("");
    // An address is one or it is left out: `preferred_username` may be a telephone number.
    let address = |text: &str| {
        let text = text.trim();
        let shaped = text.split_once('@').is_some_and(|(local, domain)| !local.is_empty() && !domain.is_empty() && !domain.contains('@'));
        (shaped && text.len() <= MAX_IDENTITY && !text.chars().any(|c| c.is_whitespace() || unprintable(c))).then(|| text.to_string())
    };
    // A name is one line, of a length a name has.
    let words: Vec<&str> = claim("name").split(|c: char| c.is_whitespace() || unprintable(c)).filter(|word| !word.is_empty()).collect();
    let name: String = words.join(" ").chars().take(MAX_IDENTITY).collect();
    // The directory that signed them in (`tid`: a GUID), which is how an organisation's account
    // is told from a personal one. Anything that isn't shaped like one says nothing.
    let tenant = claim("tid").trim();
    let shaped = tenant.len() == 36 && tenant.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    let tenant = if shaped { tenant.to_ascii_lowercase() } else { String::new() };
    Tokens { name, email: address(claim("email")).or_else(|| address(claim("preferred_username"))).unwrap_or_default(), tenant, ..Default::default() }
}

/// A character with no place in a name or an address: a control character, or one that turns
/// the direction of the text around it.
fn unprintable(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A page for the browser. The text is ours, never the request's.
struct Page {
    status: &'static str,
    title: &'static str,
    text: &'static str,
}

const DONE: Page = Page { status: "200 OK", title: "Back to MailCraft", text: "You can close this tab and return to MailCraft." };
const FAILED: Page = Page {
    status: "200 OK",
    title: "The sign-in didn't go through",
    text: "You can close this tab and return to MailCraft, which says what went wrong.",
};
const NOT_OURS: Page = Page {
    status: "400 Bad Request",
    title: "Not the sign-in MailCraft is waiting for",
    text: "Close this tab and start the sign-in again from MailCraft.",
};
const NOT_FOUND: Page = Page { status: "404 Not Found", title: "Nothing here", text: "This address only brings a sign-in back to MailCraft." };
const NOT_ALLOWED: Page =
    Page { status: "405 Method Not Allowed", title: "Nothing here", text: "This address only brings a sign-in back to MailCraft." };
const TOO_LARGE: Page =
    Page { status: "431 Request Header Fields Too Large", title: "Too much", text: "This address only brings a sign-in back to MailCraft." };
const APPROVED: Page =
    Page { status: "200 OK", title: "MailCraft is approved for your organisation", text: "You can close this tab and return to MailCraft." };
const NOT_APPROVED: Page = Page {
    status: "200 OK",
    title: "The approval didn't go through",
    text: "You can close this tab and return to MailCraft, which says what went wrong.",
};

impl Page {
    /// The whole answer: a page that stands on its own, with no script and nothing to fetch.
    fn response(&self) -> Vec<u8> {
        let body = format!(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<meta name=\"color-scheme\" content=\"light dark\">\n\
             <title>MailCraft</title>\n\
             <style>body{{font:1rem system-ui,sans-serif;max-width:30em;margin:20vh auto;padding:0 1.5em;text-align:center}}</style>\n\
             </head>\n<body>\n<h1>{}</h1>\n<p>{}</p>\n</body>\n</html>\n",
            self.title, self.text
        );
        format!(
            "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
             Referrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n\
             Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nConnection: close\r\n\r\n{body}",
            self.status,
            body.len()
        )
        .into_bytes()
    }
}

/// Whether two texts are the same, taking as long whatever the answer.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |differences, (x, y)| differences | (x ^ y)) == 0
}

/// What a request to the loopback port brings, when it is the one that is waited for: the names
/// and values the provider's page sent back with `state`. Otherwise the page it is answered
/// with, and the wait goes on.
fn brought(request: &[u8], state: &str) -> Result<Vec<(String, String)>, &'static Page> {
    // `GET /?code=…&state=… HTTP/1.1`.
    let line = request.split(|b| *b == b'\n').next().unwrap_or(&[]);
    let line = String::from_utf8_lossy(line);
    let mut words = line.split_ascii_whitespace();
    let (method, target) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    if method != "GET" {
        return Err(&NOT_ALLOWED);
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    // A browser also asks for `/favicon.ico`.
    if path != "/" {
        return Err(&NOT_FOUND);
    }
    let fields = http::pairs(query);
    // Only the page that was given the state can send it back. Anything else on this machine
    // that finds the port can neither slip a code in nor call the sign-in off.
    let sent = fields.iter().find(|(name, _)| name == "state").map_or("", |(_, value)| value.as_str());
    if state.is_empty() || !same(sent, state) {
        return Err(&NOT_OURS);
    }
    Ok(fields)
}

/// The no a provider's page came back with (`error` and what goes with it), when it is one.
fn page_refusal(fields: &[(String, String)]) -> Option<Refused> {
    let field = |name: &str| fields.iter().find(|(n, _)| n == name).map_or("", |(_, value)| value.as_str());
    let (error, description) = (field("error"), field("error_description"));
    if !error.is_empty() {
        // For whoever has to find out why a sign-in ended: the code and Microsoft's numbers for
        // it, never the words, which may name the owner or the organisation.
        let numbers: Vec<String> = aadsts(description).map(|number| number.to_string()).collect();
        log::info!(
            "the provider's page came back with a no: \"{}\"{}",
            shown(&http::tidy(error)),
            if numbers.is_empty() { String::new() } else { format!(" (AADSTS {})", numbers.join(", ")) }
        );
    }
    (!error.is_empty()).then(|| Refused {
        error: refusal(error, description),
        about: about(error, field("error_subcode"), description, &[]),
        policy: by_policy(description, &[]),
        declined: declined(error, description),
    })
}

/// [`judge`] for Microsoft's admin-consent page, which comes back with `admin_consent=True` and,
/// when the approval wasn't given, with an `error` beside it (Microsoft identity platform,
/// "Admin consent protocols"). Whatever else it sends (the directory, the permissions) is not
/// read: the documentation itself warns against believing the directory, and nothing here
/// needs either.
fn judge_consent(request: &[u8], state: &str) -> (&'static Page, Option<Result<(), Refused>>) {
    let fields = match brought(request, state) {
        Ok(fields) => fields,
        Err(page) => return (page, None),
    };
    if let Some(refused) = page_refusal(&fields) {
        return (&NOT_APPROVED, Some(Err(refused)));
    }
    let said = fields.iter().find(|(name, _)| name == "admin_consent").map_or("", |(_, value)| value.as_str());
    if said.trim().eq_ignore_ascii_case("true") {
        (&APPROVED, Some(Ok(())))
    } else {
        let broken = ProtoError::Protocol("the approval page came back without an answer".into());
        (&NOT_APPROVED, Some(Err(NetError::Proto(broken).into())))
    }
}

/// The page a request is answered with and, when it is the one waited for, how the sign-in
/// went: the code, or why there is none. `None` when the wait goes on.
fn judge(request: &[u8], state: &str) -> (&'static Page, Option<Result<String, Refused>>) {
    let fields = match brought(request, state) {
        Ok(fields) => fields,
        Err(page) => return (page, None),
    };
    let field = |name: &str| fields.iter().find(|(n, _)| n == name).map_or("", |(_, value)| value.as_str());
    if let Some(refused) = page_refusal(&fields) {
        return (&FAILED, Some(Err(refused)));
    }
    let code = field("code");
    if code.is_empty() || !usable(code) {
        let broken = ProtoError::Protocol("the sign-in page came back without a code".into());
        return (&FAILED, Some(Err(NetError::Proto(broken).into())));
    }
    (&DONE, Some(Ok(code.to_string())))
}

/// A connection to the loopback port and what it has sent so far.
struct Visitor {
    stream: TcpStream,
    request: Vec<u8>,
    since: Instant,
    /// More of the request is on its way than is taken in.
    unread: bool,
}

enum Arrival {
    /// Not all here yet.
    Waiting,
    /// The request line is in, and the headers, or as much of them as is taken.
    Whole,
    /// Nothing will come of it.
    Gone,
}

impl Visitor {
    /// Take in what has arrived, without waiting for more.
    fn read(&mut self) -> Arrival {
        let mut chunk = [0u8; 1024];
        // Every round takes bytes in or ends, so the request's limit also limits the rounds.
        for _ in 0..=MAX_REQUEST / chunk.len() + 1 {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Arrival::Gone,
                Ok(n) => {
                    self.request.extend_from_slice(chunk.get(..n.min(chunk.len())).unwrap_or(&[]));
                    if self.request.windows(4).any(|w| w == b"\r\n\r\n") || self.request.windows(2).any(|w| w == b"\n\n") {
                        return Arrival::Whole;
                    }
                    if self.request.len() > MAX_REQUEST {
                        self.unread = true;
                        // Headers without end (a browser sends every cookie it holds for
                        // `localhost`) aren't waited for: the first line says all there is to know.
                        if self.request.contains(&b'\n') {
                            return Arrival::Whole;
                        }
                        self.answer(&TOO_LARGE);
                        return Arrival::Gone;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Arrival::Gone,
            }
        }
        // A connection that says nothing (browsers open spares) isn't kept for ever.
        if self.since.elapsed() > REQUEST_TIMEOUT { Arrival::Gone } else { Arrival::Waiting }
    }

    /// Send a page and hang up. A browser that has gone away is no loss.
    fn answer(&mut self, page: &Page) {
        if self.stream.set_nonblocking(false).and_then(|()| self.stream.set_write_timeout(Some(REPLY_TIMEOUT))).is_err() {
            return;
        }
        let response = page.response();
        let mut left = response.as_slice();
        let started = Instant::now();
        // Not `write_all`: that retries an interrupted write for ever.
        while !left.is_empty() && started.elapsed() < REPLY_TIMEOUT {
            match self.stream.write(left) {
                Ok(0) => break,
                Ok(n) => left = left.get(n..).unwrap_or(&[]),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = self.stream.shutdown(Shutdown::Write);
        if self.unread && self.stream.set_read_timeout(Some(TICK)).is_ok() {
            // Hanging up on a browser that is still sending would cost it the page: what it
            // sends is taken off the line for a moment first.
            let started = Instant::now();
            let mut rest = [0u8; 4096];
            while started.elapsed() < DRAIN_TIME {
                match self.stream.read(&mut rest) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
                    Err(_) => break,
                }
            }
        }
    }
}

/// The loopback port the browser is sent back to.
struct Redirect {
    listeners: Vec<TcpListener>,
    port: u16,
}

impl Redirect {
    /// Listen on a free port of 127.0.0.1 and, where there is IPv6, on the same port of `::1`:
    /// a browser may take `localhost` to be either.
    fn listen() -> Result<Redirect, NetError> {
        let failed = || NetError::Config("couldn't open a port on this computer for the sign-in to come back to".into());
        for attempt in 1..=MAX_PORT_TRIES {
            let v4 = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| failed())?;
            let port = v4.local_addr().map_err(|_| failed())?.port();
            let mut listeners = vec![v4];
            match TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
                Ok(v6) => listeners.push(v6),
                // Somebody else has that one, and a browser going there would hand them the code: another port.
                Err(e) if matches!(e.kind(), ErrorKind::AddrInUse | ErrorKind::PermissionDenied) && attempt < MAX_PORT_TRIES => continue,
                // No IPv6 on this machine.
                Err(_) => {}
            }
            // The wait looks at the cancel flag between looks at the port.
            if listeners.iter().any(|listener| listener.set_nonblocking(true).is_err()) {
                return Err(failed());
            }
            return Ok(Redirect { listeners, port });
        }
        Err(failed())
    }

    /// Let in whoever is at the door.
    fn admit(&self, visitors: &mut Vec<Visitor>) {
        for listener in &self.listeners {
            for _ in 0..MAX_VISITORS {
                // Nothing waiting (`WouldBlock`), or a connection that broke before it was taken.
                let Ok((stream, peer)) = listener.accept() else { break };
                // Only this machine can connect to a loopback address; checked all the same.
                if peer.ip().is_loopback() && visitors.len() < MAX_VISITORS && stream.set_nonblocking(true).is_ok() {
                    visitors.push(Visitor { stream, request: Vec::new(), since: Instant::now(), unread: false });
                }
            }
        }
    }

    /// Wait for the browser to come back with the code that goes with `state`, answering what
    /// else turns up (a favicon, a stranger) on the way.
    fn wait(&self, state: &str, cancel: &AtomicBool, patience: Duration) -> Result<String, Refused> {
        self.wait_for(state, cancel, patience, judge)
    }

    /// [`wait`](Self::wait) for whatever `judge` reads out of the request that brings `state` back.
    fn wait_for<T>(
        &self,
        state: &str,
        cancel: &AtomicBool,
        patience: Duration,
        judge: impl Fn(&[u8], &str) -> (&'static Page, Option<Result<T, Refused>>),
    ) -> Result<T, Refused> {
        let started = Instant::now();
        let mut visitors: Vec<Visitor> = Vec::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(NetError::SignInCancelled.into());
            }
            if started.elapsed() >= patience {
                return Err(NetError::SignInTimedOut.into());
            }
            self.admit(&mut visitors);
            let mut outcome = None;
            visitors.retain_mut(|visitor| {
                if outcome.is_some() {
                    return false;
                }
                match visitor.read() {
                    Arrival::Waiting => true,
                    Arrival::Gone => false,
                    Arrival::Whole => {
                        let (page, result) = judge(&visitor.request, state);
                        visitor.answer(page);
                        outcome = result;
                        false
                    }
                }
            });
            if let Some(outcome) = outcome {
                return outcome;
            }
            std::thread::sleep(TICK);
        }
    }
}

#[cfg(test)]
mod tests;
