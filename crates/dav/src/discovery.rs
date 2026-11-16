// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding an account's calendar and contacts servers (CalDAV, RFC 4791; CardDAV, RFC 6352) from
//! an e-mail address, and deciding where the owner's password may go, as a state machine that the
//! host drives (the shape of `mailcraft_ews`'s Autodiscover).
//!
//! The machine does no I/O. It says what to do next ([`Discovery::step`]): perform this HTTP
//! request, look this SRV and TXT name up, ask the owner about this host, or here is the end of it
//! ([`Outcome`]). The host does that and tells the machine what came of it
//! ([`Discovery::response`], [`Discovery::srv`], [`Discovery::agree`], [`Discovery::decline`]).
//!
//! The rules (spec C3):
//!
//! - **Where it looks**, in order, for each of calendars and contacts: the address the owner typed
//!   and nothing else ([`Discovery::typed`]); else, for a known provider (iCloud, Fastmail:
//!   `mailcraft_model::provider::dav_by_domain`), the provider's own address and nothing else;
//!   else, for the domain of the address (only the domain goes out, in the SRV and TXT names,
//!   RFC 6764 §3–4): the SRV record of `_caldavs._tcp` / `_carddavs._tcp` and its TXT `path=`
//!   (`_caldav._tcp`, the unencrypted one, is never asked); then `https://<domain>/.well-known/caldav`
//!   (`carddav`), the same on the account's mail server ([`Discovery::mail_host`]), and the SRV
//!   targets, at the TXT path or at the well-known address (RFC 6764 §5).
//! - **Every probe goes without credentials first**: a `PROPFIND` of Depth 0 for the current user's
//!   principal. A host counts as a calendar or contacts server only when it answers 207 with a
//!   `DAV:` multistatus, or 401 **and** a `DAV` header naming `calendar-access` (`addressbook` for
//!   contacts), asked of it with `OPTIONS` when the 401 did not say. A bare 401 is never evidence:
//!   a mail filter's web front answers it to everything, and the password would go to it.
//! - **Redirects** (301, 302, 303, 307, 308) are followed one at a time, at most
//!   [`MAX_REDIRECTS`] in a row, and only to `https://` (plain `http` only between loopback
//!   addresses, for tests). One to another host is followed **without credentials**; and when that
//!   host is outside the owner's mail domain and is nobody's (see below), the owner is asked
//!   **before the first request to it**, and a no sends nothing at all. A host given as a number
//!   (`192.168.1.1`, `[fd00::1]`) is never followed unless the owner typed it.
//! - **The password** (the [`Secret`] the machine was made with) goes only in answer to a host's
//!   own 401 offering Basic, only to a host that is the owner's: the one typed (which a bare Basic
//!   401 is enough for, a proxy that signs everybody in first: what answers must then be a DAV
//!   multistatus, or the search ends there with [`Failure::NotDavServer`]), the account's mail
//!   server (at 443), one the domain's SRV record names, the known provider's own DAV hosts (for
//!   that provider's accounts only), or one the owner agreed to when the machine asked
//!   ([`Step::NeedsYes`], naming the host whole). Any other host asks first: outside the owner's
//!   domain before the first request, inside it before the password (the domain's own name is
//!   often a web site somebody else runs). A no sends nothing there, and it is not asked again. Digest and the rest are not offered the
//!   password; nor is any host over plain `http`.
//! - **Never to Microsoft or Google.** A Microsoft host (`is_exchange_online`,
//!   `is_microsoft_domain`) ends the search with [`Outcome::Microsoft`] (the Microsoft account
//!   signs in on Microsoft's page, ADR 0002) and a Google host with [`Outcome::Google`] (signing in
//!   with Google, C11); nothing is sent there.
//! - **A refused password ends the search at once** ([`Failure::SignInRefused`]): trying other
//!   hosts with a wrong password is how an account gets locked. What was found before a search is
//!   stopped (by that, by Microsoft's or Google's host, by the request cap) is still given back.
//!   A chain of redirects that does not end fails that one place only.
//! - **Then** `current-user-principal`, the `calendar-home-set` or `addressbook-home-set` of it, and
//!   Depth 1 on the home for the collections (RFC 4791 §6.2.1, RFC 6352 §7.1.1). A home or a
//!   principal on another host (iCloud's) is another origin: asked without credentials first and
//!   under these same rules. Plain resources in a home (SOGo's `personal.ics`) are passed over.
//! - Every address a server names is resolved against the one it came from and is on its origin
//!   ([`crate::href::Origin`]) or is another origin, which is a new host under these rules; nothing
//!   is sent with a credential to an origin that has not answered a challenge and been allowed.
//! - Every answer is hostile: XML through [`crate::xml`], statuses through [`crate::status`], at most
//!   [`MAX_REQUESTS`] requests, redirects and SRV hosts capped.
//!
//! Nothing here is logged, and the [`Debug`](std::fmt::Debug) of everything that holds a host, an
//! address or a name shows nothing of it: the names of an organisation's servers are its own.

use std::collections::VecDeque;
use std::fmt;

use mailcraft_link::WebLink;
use mailcraft_model::provider::{self, DavPreset};
use mailcraft_model::{is_exchange_online, is_google_host, is_microsoft_domain, is_within};

use crate::answer::{Collections, Entry, Multistatus};
use crate::href::{HrefError, Origin};
use crate::http::{EndpointError, HttpExchange, HttpRequest, HttpResponse, MAX_ADDRESS, Secret, TransportError, check_endpoint};
use crate::request::{Endpoint, Kind};
use crate::status;

/// Most redirects followed in a row.
pub const MAX_REDIRECTS: u8 = 5;
/// Most requests one search makes, calendars and contacts together.
pub const MAX_REQUESTS: u8 = 48;
/// Most hosts taken from an SRV record.
const MAX_SRV_HOSTS: usize = 3;
/// Most origins held as the owner's.
const MAX_TRUSTED: usize = 32;
/// Longest host name kept.
const MAX_HOST: usize = 253;
/// Longest TXT path kept.
const MAX_PATH: usize = 256;
/// The largest answer to a probe, a principal or a home question; a listing of collections is
/// given more.
const SMALL_ANSWER: usize = 256 * 1024;
const LISTING_ANSWER: usize = 4 * 1024 * 1024;

/// Whether `text` is a host or domain name with a dot in it: labels of letters, digits and
/// hyphens, none empty, none over 63, none starting or ending with a hyphen, and a last label that
/// is not all digits (an address such as `10.0.0.1` is no name, RFC 3696 §2: an SRV record names a
/// host, and nothing here is sent to an address a record gave).
fn is_name(text: &str) -> bool {
    text.len() <= MAX_HOST
        && text.contains('.')
        && !text.rsplit('.').next().is_some_and(|last| last.bytes().all(|b| b.is_ascii_digit()))
        && text.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// The domain of an e-mail address: one `@`, a local part without space, control character or
/// angle bracket, a domain that is a name.
fn domain_of(address: &str) -> Option<String> {
    let (local, domain) = address.trim().rsplit_once('@')?;
    let mut domain = domain.trim_end_matches('.').to_ascii_lowercase();
    // A name in other letters is written as its ASCII form (Punycode, RFC 3492), as an address on
    // the wire has it, by the link crate's own reader: no host name that it refuses is looked for.
    if !domain.is_ascii() {
        if !idn_ok(&domain) {
            return None;
        }
        let shown = WebLink::parse(&format!("https://{domain}/"))?;
        domain = shown.host().to_owned();
        // (What it was written as must be what it reads back as.)
        if !domain.split('.').all(round_trips) {
            return None;
        }
    }
    let local_ok =
        !local.is_empty() && local.len() <= 64 && !local.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '<' | '>' | '@'));
    (local_ok && is_name(&domain)).then_some(domain)
}

/// Whether a domain in other letters is one that can be written in ASCII without being another
/// name: nothing that ends a host name (`/ ? # : @ \\` or a space), none of the forms a name
/// normalises (Unicode's compatibility forms: full- and half-width, letter-like, enclosed and
/// mathematical letters, ligatures, and combining marks, which must be in their composed form).
// shortcut: the common compatibility forms, not the whole of UTS 46's table (none is in the tree);
// what is not NFC or is not covered here is refused, never looked up under another name.
fn idn_ok(domain: &str) -> bool {
    domain.chars().all(|c| {
        let n = u32::from(c);
        !(c.is_control()
            || c.is_whitespace()
            || matches!(c, '/' | '?' | '#' | ':' | '@' | '\\')
            || matches!(n, 0xaa | 0xb2 | 0xb3 | 0xb9 | 0xba)
            || (0x300..=0x36f).contains(&n)
            || (0x3200..=0x33ff).contains(&n)
            || (0x2000..=0x2bff).contains(&n)
            || (0xfb00..=0xfb4f).contains(&n)
            || (0xfe00..=0xfe6f).contains(&n)
            || (0xff00..=0xffef).contains(&n)
            || (0x1d400..=0x1d7ff).contains(&n))
    })
}

/// Whether a label that is Punycode (`xn--…`) decodes to letters that encode back to the same label.
fn round_trips(label: &str) -> bool {
    match label.strip_prefix("xn--") {
        None => true,
        Some(code) => mailcraft_link::punycode::decode(code).is_some_and(|chars| {
            chars.iter().all(|c| c.is_alphanumeric() || *c == '-') && mailcraft_link::punycode::encode(&chars).as_deref() == Some(code)
        }),
    }
}

/// An address the owner gave that is not an e-mail address a domain can be taken from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressError;

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("that is not an e-mail address a server can be looked for from; type the server's address instead")
    }
}

impl std::error::Error for AddressError {}

/// What the owner is asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentReason {
    /// The host is in the owner's own mail domain but is neither the mail server nor one the
    /// domain's SRV record names: it may be a web site rather than a calendar server.
    OwnDomain,
    /// The search was sent to the host (a redirect, a principal or a home it named): it is not
    /// one of the owner's.
    Elsewhere,
}

/// A question for the owner: may the password go to `host`? ("Your calendar server is at
/// `dav.example.net`, which is not your mail server. Send your password there?") The host asks,
/// naming it whole, and calls [`Discovery::agree`] or [`Discovery::decline`].
#[derive(Clone, PartialEq, Eq)]
pub struct Consent {
    host: String,
    reason: ConsentReason,
}

impl Consent {
    /// The host, with its port when that is not the scheme's (lower case).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Why it is asked.
    pub fn reason(&self) -> ConsentReason {
        self.reason
    }
}

impl fmt::Debug for Consent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Consent").field("reason", &self.reason).finish()
    }
}

/// Where one kind of collection was found: the homes that answered, and what they hold.
///
/// A principal may name several homes, and on other hosts than its own (some providers keep the
/// home on a shard). The collections of all of them are in one list; **each [`Entry::href`] is
/// absolute and keeps the origin of the home it was listed in**, so a consumer builds the endpoint
/// and the password's grant for each collection's own origin (the password goes per origin, in
/// answer to that origin's challenge and the owner's yes) and never borrows [`Located::home`]'s.
#[derive(Clone, PartialEq, Eq)]
pub struct Located {
    home: String,
    homes: Vec<String>,
    collections: Collections,
}

impl Located {
    /// The address of the first home that answered, absolute and normalised. Not the place of every
    /// collection: see [`Located::homes`] and each collection's own address.
    pub fn home(&self) -> &str {
        &self.home
    }

    /// Every home that answered (the first is [`Located::home`]), each absolute and normalised.
    pub fn homes(&self) -> &[String] {
        &self.homes
    }

    /// The collections in them (the calendars that hold events, or the address books).
    pub fn collections(&self) -> &Collections {
        &self.collections
    }
}

impl fmt::Debug for Located {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Located")
            .field("calendars", &self.collections.calendars.len())
            .field("address_books", &self.collections.address_books.len())
            .finish()
    }
}

/// What a search found. At least one of the two.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Discovered {
    calendars: Option<Located>,
    contacts: Option<Located>,
}

impl Discovered {
    /// The calendar home and its calendars, when calendars were looked for and found.
    pub fn calendars(&self) -> Option<&Located> {
        self.calendars.as_ref()
    }

    /// The address book home and its address books, when contacts were looked for and found.
    pub fn contacts(&self) -> Option<&Located> {
        self.contacts.as_ref()
    }
}

impl fmt::Debug for Discovered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Discovered").field("calendars", &self.calendars).field("contacts", &self.contacts).finish()
    }
}

/// Why nothing was found. Each is a sentence the owner can act on, and each offers typing the
/// server's address; none holds a word a server wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Failure {
    /// Nobody answered at the places looked at.
    Unreachable,
    /// There are servers, but no calendar or contacts service at the places looked at.
    NotFound,
    /// A host answered 401 without saying it is a calendar or contacts server (a bare 401), and
    /// the password was not sent.
    NotDavServer,
    /// A secure connection could not be set up: the certificate, for example.
    SecureConnection,
    /// An answer that cannot be read.
    Unreadable,
    /// A redirect to an address that is not `https://`, or no address at all.
    InsecureRedirect,
    /// The search was sent to a host given as a number (`192.168.1.1`, `[fd00::1]`), which a
    /// server's word is never taken for; only an address the owner typed may be one.
    PrivateAddress,
    /// The server or the organisation refused (403).
    ServerRefused,
    /// The server answered without the current user's principal.
    NoPrincipal,
    /// The principal names no calendar home (or address book home).
    NoHome,
    /// The server offers to sign in only in ways that are not built (Digest, Kerberos), and not
    /// with a user name and password over TLS (Basic).
    SignInNotSupported,
    /// The owner did not agree to a host.
    Declined,
    /// The user name or password was not accepted (401). Nothing else was asked.
    SignInRefused,
    /// More redirects in a row than [`MAX_REDIRECTS`].
    TooManyRedirects,
}

impl Failure {
    /// Which of two reasons says more: the one kept when several steps failed.
    fn rank(self) -> u8 {
        match self {
            Self::Unreachable => 0,
            Self::NotFound => 1,
            // (A web site that loops says less than a server that answered.)
            Self::TooManyRedirects => 2,
            Self::NotDavServer => 3,
            Self::SecureConnection => 4,
            Self::Unreadable => 5,
            Self::InsecureRedirect => 6,
            Self::PrivateAddress => 6,
            Self::ServerRefused => 7,
            Self::SignInNotSupported => 8,
            Self::NoPrincipal => 9,
            Self::NoHome => 10,
            Self::Declined => 11,
            Self::SignInRefused => 12,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unreachable => "No server answered at the usual places for this address. Check the network connection, or type the server's address.",
            Self::NotFound => "No calendar or contacts server was found for this address. Type the server's address.",
            Self::NotDavServer => "A server answered that asked for a password but did not say it keeps calendars or contacts, so no password was sent. Type the server's address.",
            Self::SecureConnection => "A secure connection to the server could not be set up, for instance because its certificate is not trusted. Check the address, or type the server's address.",
            Self::Unreadable => "The server's answer could not be read. Type the server's address.",
            Self::InsecureRedirect => "The server sent the setup somewhere that is not encrypted, which is never followed. Type the server's address.",
            Self::PrivateAddress => "The search was sent to a server given by its number, which is not followed unless you typed it. Type the server's address.",
            Self::ServerRefused => "The server or the organisation would not give its calendars or contacts. Ask your administrator for the server's address and type it.",
            Self::NoPrincipal => "The server did not say who you are on it. Type the server's address.",
            Self::NoHome => "The server did not say where your calendars or contacts are. Type the server's address.",
            Self::SignInNotSupported => "The server wants a way of signing in that this program cannot do yet. Ask your administrator whether it can take a user name and password over an encrypted connection.",
            Self::Declined => "You did not agree to send your password to the server the search led to. Type the server's address.",
            Self::SignInRefused => "The server did not accept the user name and password. Check them, or use a different user name and password for calendars and contacts.",
            Self::TooManyRedirects => "The search was sent from one server to another too many times. Type the server's address.",
        })
    }
}

impl std::error::Error for Failure {}

/// How a search ended.
// (Returned once, at the end of a search: the size of the found variant is no cost.)
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Found calendars, contacts, or both.
    Found(Discovered),
    /// The address, or a host the search led to, is Microsoft's: offer the Microsoft account. No
    /// password was sent there.
    Microsoft,
    /// The address, or a host the search led to, is Google's: offer to sign in with Google. No
    /// password was sent there.
    Google,
    /// Nothing found, and why.
    Failed(Failure),
}

/// The name of the SRV (and TXT) record to look up (`_caldavs._tcp.example.org`). Its `Debug`
/// shows nothing: the domain is the owner's.
#[derive(Clone, PartialEq, Eq)]
pub struct SrvName(String);

impl SrvName {
    /// The name to look up.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SrvName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SrvName(<redacted>)")
    }
}

/// What the host's look-up of a [`SrvName`] found: the targets of the SRV record (host and port),
/// best first, and the `path=` of the TXT record beside it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SrvAnswer {
    /// The SRV targets, best first.
    pub targets: Vec<(String, u16)>,
    /// The TXT `path=` value.
    pub path: Option<String>,
}

impl fmt::Debug for SrvAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SrvAnswer").field("targets", &self.targets.len()).field("path", &self.path.is_some()).finish()
    }
}

/// What to do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Perform this request, then call [`Discovery::response`] with what came of it. It follows
    /// no redirect itself.
    Request(HttpRequest),
    /// Look up the SRV record with this name and the TXT record beside it, then call
    /// [`Discovery::srv`] with what they say, or `None` if the lookup failed. Only the domain
    /// goes out.
    LookupSrv(SrvName),
    /// Ask the owner, then call [`Discovery::agree`] or [`Discovery::decline`].
    NeedsYes(Consent),
    /// Finished.
    Done(Outcome),
}

/// What the machine asks of the server: which of the four questions, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// The probe: the current user's principal.
    Probe,
    /// The principal's calendar home or address book home.
    Homes,
    /// The home's collections.
    Collections,
}

/// The request in flight, and what it is for. (No `Debug`: it holds an address.)
#[derive(Clone, PartialEq, Eq)]
struct Ask {
    stage: Stage,
    url: String,
    /// The origin's `scheme://host[:port]`.
    origin: String,
    /// The request carried the password.
    authed: bool,
    /// The request is the `OPTIONS` that asks for the `DAV` header of a 401.
    options: bool,
    /// The 401 that led to the `OPTIONS` offered Basic.
    basic: bool,
    /// The owner is asked before the request is made (a host outside the mail domain that is
    /// nobody's), not before the password goes.
    before: bool,
}

/// What the machine waits for.
#[derive(Clone, PartialEq, Eq)]
enum Waiting {
    Idle,
    Request(HttpRequest, Ask),
    Srv(SrvName),
    Consent(Consent, Ask),
    Done(Outcome),
}

/// A place to ask for one kind of collection.
#[derive(Clone, PartialEq, Eq)]
enum Target {
    Probe(String),
    Srv,
}

/// The search for one kind of collection.
struct Job {
    kind: Kind,
    queue: VecDeque<Target>,
    /// Redirects followed in a row.
    chain: u8,
}

/// The listing of a kind's homes: every home the principal names, and the plain containers one
/// level down, read one at a time; what each brings is merged.
struct Listing {
    /// Addresses still to list, with their depth below a home (0 a home, 1 a container in it).
    queue: VecDeque<(String, u8)>,
    /// The depth of the listing that is being asked.
    level: u8,
    /// Addresses queued so far, so that none is asked twice.
    seen: Vec<String>,
    /// Container listings queued beyond the homes.
    extra: u8,
    /// The homes that answered, the first first.
    homes: Vec<String>,
    /// What the listings brought.
    collections: Collections,
}

/// Most homes read of one kind (a principal may name several, RFC 4791 section 6.2.1, RFC 6352
/// section 7.1.1), and most listings of containers below them.
const MAX_HOMES: usize = 4;
const MAX_CONTAINER_LISTINGS: u8 = 8;

fn kind_index(kind: Kind) -> usize {
    match kind {
        Kind::Calendar => 0,
        Kind::AddressBook => 1,
    }
}

fn well_known(kind: Kind) -> &'static str {
    match kind {
        Kind::Calendar => "/.well-known/caldav",
        Kind::AddressBook => "/.well-known/carddav",
    }
}

/// The search for an account's calendar and contacts servers. See the [module](self).
pub struct Discovery {
    authorization: Secret,
    domain: String,
    mail_host: Option<String>,
    preset: Option<DavPreset>,
    /// The kinds the caller asked for.
    wanted: [bool; 2],
    /// The addresses the owner typed, calendars' and contacts'.
    typed: [Option<String>; 2],
    /// The origins of those addresses.
    typed_origins: Vec<String>,
    /// The request that was answered last was to this machine (a loopback test server): only then
    /// may a redirect lead to this machine too, as [`Origin::foreign`] has it.
    via_loopback: bool,
    /// Origins the password went to without a `DAV` header having said what they are (the host
    /// the owner typed, behind a proxy that asks for a sign-in first): until an answer is a DAV
    /// multistatus, nothing else is believed of them.
    unproven: Vec<String>,
    /// The kinds still to search for, the first being the one in hand.
    todo: VecDeque<Kind>,
    job: Option<Job>,
    /// The homes of the kind in hand being listed.
    listing: Option<Listing>,
    found: Discovered,
    /// Origins that are the owner's: the typed ones, the SRV record's, the ones agreed to.
    trusted: Vec<String>,
    /// Origins that have answered a challenge and may be sent the password.
    signed_in: Vec<String>,
    /// Origins the owner said no to.
    declined: Vec<String>,
    waiting: Waiting,
    requests: u8,
    reason: Option<Failure>,
}

impl fmt::Debug for Discovery {
    /// Nothing: an address, a domain and the password are all the owner's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Discovery").field("requests", &self.requests).finish()
    }
}

impl Discovery {
    /// A search for the calendars (`calendars`) and contacts (`contacts`) of `email`, signing in
    /// with `authorization` ([`Secret::basic`]) when a host that is the owner's asks. Nothing is
    /// sent until the host performs the first [`Step`].
    pub fn new(email: &str, authorization: Secret, calendars: bool, contacts: bool) -> Result<Self, AddressError> {
        let domain = domain_of(email).ok_or(AddressError)?;
        let mut todo = VecDeque::new();
        if calendars {
            todo.push_back(Kind::Calendar);
        }
        if contacts {
            todo.push_back(Kind::AddressBook);
        }
        let waiting = if is_microsoft_domain(&domain) {
            Waiting::Done(Outcome::Microsoft)
        } else if is_google_host(&domain) {
            Waiting::Done(Outcome::Google)
        } else {
            Waiting::Idle
        };
        Ok(Self {
            authorization,
            preset: provider::dav_by_domain(&domain),
            domain,
            mail_host: None,
            wanted: [calendars, contacts],
            typed: [None, None],
            typed_origins: Vec::new(),
            via_loopback: false,
            listing: None,
            unproven: Vec::new(),
            todo,
            job: None,
            found: Discovered::default(),
            trusted: Vec::new(),
            signed_in: Vec::new(),
            declined: Vec::new(),
            waiting,
            requests: 0,
            reason: None,
        })
    }

    /// The account's own mail server (its IMAP host, `imap.example.org`): asked after the domain,
    /// and the password may go to it. A known provider's IMAP host also says whose DAV servers
    /// the account's are. A Microsoft or Google mail server ends the search as such.
    #[must_use]
    pub fn mail_host(mut self, host: &str) -> Self {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if !is_name(&host) || !matches!(self.waiting, Waiting::Idle) {
            return self;
        }
        if is_exchange_online(&host) {
            self.waiting = Waiting::Done(Outcome::Microsoft);
            return self;
        }
        if is_google_host(&host) {
            self.waiting = Waiting::Done(Outcome::Google);
            return self;
        }
        self.preset = self.preset.or_else(|| provider::dav_by_imap_host(&host));
        self.mail_host = Some(host);
        self
    }

    /// The address the owner typed for calendars or for contacts: that address only (and the
    /// redirects it gives), and the host of it is the owner's. When any is typed, only the kinds
    /// typed **and** asked for in [`Discovery::new`] are searched for: an address for a kind that
    /// was not asked for is ignored (and its host is not the owner's for anything).
    pub fn typed(mut self, kind: Kind, url: &str) -> Result<Self, EndpointError> {
        let url = url.trim();
        check_endpoint(url)?;
        let origin = Origin::parse(url)?;
        if !self.wanted.get(kind_index(kind)).copied().unwrap_or(false) {
            return Ok(self);
        }
        let key = origin.base();
        if !self.trusted.contains(&key) && self.trusted.len() < MAX_TRUSTED {
            self.trusted.push(key.clone());
        }
        if !self.typed_origins.contains(&key) && self.typed_origins.len() < MAX_TRUSTED {
            self.typed_origins.push(key);
        }
        if let Some(slot) = self.typed.get_mut(kind_index(kind)) {
            *slot = Some(url.to_owned());
        }
        let typed_kinds: VecDeque<Kind> =
            [Kind::Calendar, Kind::AddressBook].into_iter().filter(|k| self.typed.get(kind_index(*k)).is_some_and(Option::is_some)).collect();
        self.todo = typed_kinds;
        Ok(self)
    }

    /// What to do next. Asked again before the host has answered, it says the same.
    pub fn step(&mut self) -> Step {
        if matches!(self.waiting, Waiting::Idle) {
            self.advance();
        }
        match &self.waiting {
            Waiting::Request(request, _) => Step::Request(request.clone()),
            Waiting::Srv(name) => Step::LookupSrv(name.clone()),
            Waiting::Consent(consent, _) => Step::NeedsYes(consent.clone()),
            Waiting::Done(outcome) => Step::Done(outcome.clone()),
            // (`advance` always leaves the machine waiting for something.)
            Waiting::Idle => Step::Done(Outcome::Failed(Failure::Unreachable)),
        }
    }

    /// What the search is over with: `outcome`, unless a kind was found, which is then what is
    /// given back: a host that stops the search (Microsoft's, Google's, a refused password, the
    /// request cap) stops further asking, never what was found before it.
    fn settle(&self, outcome: Outcome) -> Outcome {
        if matches!(outcome, Outcome::Found(_)) || (self.found.calendars.is_none() && self.found.contacts.is_none()) {
            outcome
        } else {
            Outcome::Found(self.found.clone())
        }
    }

    fn finish(&mut self, outcome: Outcome) {
        // (What the listings brought so far is kept, as a kind found before is.)
        self.commit_listing();
        let outcome = self.settle(outcome);
        self.waiting = Waiting::Done(outcome);
    }

    /// A candidate failed for `why`: remember the most telling reason, and go on to the next place.
    fn fail_candidate(&mut self, why: Failure) {
        if self.reason.is_none_or(|have| why.rank() > have.rank()) {
            self.reason = Some(why);
        }
        if let Some(job) = self.job.as_mut() {
            job.chain = 0;
        }
        self.waiting = Waiting::Idle;
    }

    /// Set `waiting` to the next thing to do.
    fn advance(&mut self) {
        // Each round takes one place or one kind off a list; the rounds are bounded by those and
        // by the requests.
        for _ in 0..usize::from(MAX_REQUESTS).saturating_mul(8) {
            // The homes of a kind come before anything else: one listing at a time.
            if self.listing.is_some() {
                let next = self.listing.as_mut().and_then(|listing| {
                    let (url, level) = listing.queue.pop_front()?;
                    listing.level = level;
                    Some(url)
                });
                match next {
                    Some(url) => {
                        self.issue(Stage::Collections, url, false);
                        if !matches!(self.waiting, Waiting::Idle) {
                            return;
                        }
                    }
                    None => self.commit_listing(),
                }
                continue;
            }
            if self.job.is_none() {
                let Some(kind) = self.todo.pop_front() else {
                    let outcome = if self.found.calendars.is_some() || self.found.contacts.is_some() {
                        Outcome::Found(self.found.clone())
                    } else {
                        Outcome::Failed(self.reason.unwrap_or(Failure::NotFound))
                    };
                    return self.finish(outcome);
                };
                let queue = self.candidates(kind);
                self.job = Some(Job { kind, queue, chain: 0 });
            }
            let Some(target) = self.job.as_mut().and_then(|job| job.queue.pop_front()) else {
                // This kind has no place left to ask.
                self.job = None;
                if self.reason.is_none() {
                    self.reason = Some(Failure::NotFound);
                }
                continue;
            };
            match target {
                Target::Srv => {
                    let Some(kind) = self.job.as_ref().map(|job| job.kind) else { continue };
                    let service = if kind == Kind::Calendar { "_caldavs._tcp" } else { "_carddavs._tcp" };
                    self.waiting = Waiting::Srv(SrvName(format!("{service}.{}", self.domain)));
                    return;
                }
                Target::Probe(url) => {
                    self.via_loopback = false;
                    self.issue(Stage::Probe, url, false);
                    if !matches!(self.waiting, Waiting::Idle) {
                        return;
                    }
                }
            }
        }
        self.finish(Outcome::Failed(self.reason.unwrap_or(Failure::Unreachable)));
    }

    /// The places to ask for `kind`, in order.
    fn candidates(&self, kind: Kind) -> VecDeque<Target> {
        if let Some(Some(url)) = self.typed.get(kind_index(kind)) {
            return VecDeque::from([Target::Probe(url.clone())]);
        }
        if let Some(preset) = self.preset {
            let url = if kind == Kind::Calendar { preset.caldav } else { preset.carddav };
            return VecDeque::from([Target::Probe(url.to_owned())]);
        }
        let mut queue = VecDeque::from([Target::Srv, Target::Probe(format!("https://{}{}", self.domain, well_known(kind)))]);
        if let Some(host) = self.mail_host.as_ref().filter(|host| **host != self.domain) {
            queue.push_back(Target::Probe(format!("https://{host}{}", well_known(kind))));
        }
        queue
    }

    /// Whether the password may go to `origin` without asking: it is the owner's (typed, named by
    /// the domain's SRV record, agreed to), the account's mail server, or the known provider's own
    /// DAV host for this account.
    fn may_sign_in(&self, origin: &Origin) -> bool {
        let secure_default = origin.secure() && origin.port() == 443;
        self.trusted.iter().any(|own| *own == origin.base())
            || (secure_default && self.mail_host.as_deref() == Some(origin.host()))
            || (secure_default && self.preset.is_some_and(|preset| preset.trusts(origin.host())))
    }

    /// Why the owner is asked about `origin`.
    fn reason_for(&self, origin: &Origin) -> ConsentReason {
        if is_within(origin.host(), &self.domain) { ConsentReason::OwnDomain } else { ConsentReason::Elsewhere }
    }

    /// The proof that the password may go to `origin`, which only this machine makes: the origin
    /// has answered a challenge and been allowed ([`Self::sign_in`]), and is `https` (or loopback).
    fn grant(&self, origin: &Origin) -> Option<Grant> {
        (self.signed_in.contains(&origin.base()) && (origin.secure() || origin.is_loopback())).then(|| Grant { origin: origin.clone() })
    }

    /// Asks the server at `url` the question of `stage`: without the password unless its origin
    /// has answered a challenge and been allowed. `options` asks for the `DAV` header instead.
    ///
    /// Before anything is sent: Microsoft's and Google's hosts get nothing; a host given as a
    /// number is refused unless the owner typed it; and a host outside the owner's mail domain
    /// that is nobody's (not typed, not the mail server, not the provider's, not named by the
    /// domain's SRV record, not agreed to) is asked about first, and a no sends nothing at all.
    fn issue(&mut self, stage: Stage, url: String, options: bool) {
        let Ok(origin) = Origin::parse(&url) else { return self.fail_candidate(Failure::Unreadable) };
        // (A home the principal names, or a container in it, that is Microsoft's or Google's is
        // that home failing, not the search ending: nothing is sent to it, and the other homes and
        // the other kind go on. Elsewhere it is the account being a Microsoft or Google one.)
        let in_listing = stage == Stage::Collections && self.listing.is_some();
        if is_exchange_online(origin.host()) || is_microsoft_domain(origin.host()) {
            return if in_listing { self.fail_candidate(Failure::NoHome) } else { self.finish(Outcome::Microsoft) };
        }
        if is_google_host(origin.host()) {
            return if in_listing { self.fail_candidate(Failure::NoHome) } else { self.finish(Outcome::Google) };
        }
        let key = origin.base();
        // This machine, and addresses by number, are only the owner's own to type; a redirect or
        // an address a server names may lead to this machine only from a request to this machine
        // (a test server on loopback), as `Origin::foreign` has it.
        let typed = self.typed_origins.contains(&key);
        let this_machine = origin.is_loopback();
        if (this_machine && !typed && !self.via_loopback) || (is_number_host(origin.host()) && !this_machine && !typed) {
            return self.fail_candidate(Failure::PrivateAddress);
        }
        if self.declined.contains(&key) {
            return self.fail_candidate(Failure::Declined);
        }
        if self.requests >= MAX_REQUESTS {
            let why = self.reason.unwrap_or(Failure::TooManyRedirects);
            return self.finish(Outcome::Failed(why));
        }
        if !is_within(origin.host(), &self.domain) && !self.may_sign_in(&origin) && !this_machine {
            let host = label(&origin);
            let ask = Ask { stage, url, origin: key, authed: false, options, basic: false, before: true };
            self.waiting = Waiting::Consent(Consent { host, reason: ConsentReason::Elsewhere }, ask);
            return;
        }
        let grant = if options { None } else { self.grant(&origin) };
        let authed = grant.is_some();
        let endpoint = match grant {
            Some(grant) => Endpoint::granted(&grant, self.authorization.clone()),
            None => Endpoint::anonymous(origin),
        };
        let built = match (options, stage) {
            (true, _) => endpoint.options(&url),
            (false, Stage::Probe) => endpoint.propfind_principal(&url),
            (false, Stage::Homes) => endpoint.propfind_homes(&url),
            (false, Stage::Collections) => endpoint.propfind_collections(&url),
        };
        match built {
            Ok(request) => {
                let limit = if stage == Stage::Collections { LISTING_ANSWER } else { SMALL_ANSWER };
                let ask = Ask { stage, url, origin: key, authed, options, basic: false, before: false };
                self.requests = self.requests.saturating_add(1);
                self.waiting = Waiting::Request(request.with_response_limit(limit), ask);
            }
            Err(_) => self.fail_candidate(Failure::Unreadable),
        }
    }

    /// The host has performed the [`Step::Request`]: this is what came of it. (A call at any other
    /// time is ignored.)
    pub fn response(&mut self, result: Result<HttpResponse, TransportError>) {
        if !matches!(self.waiting, Waiting::Request(..)) {
            return;
        }
        let Waiting::Request(_, ask) = std::mem::replace(&mut self.waiting, Waiting::Idle) else {
            return;
        };
        self.via_loopback = Origin::parse(&ask.url).is_ok_and(|origin| origin.is_loopback());
        match result {
            // The password went to a host that never said what it is, and no answer came: the
            // search is over, so that the other kind cannot send it again.
            Err(TransportError::Certificate) if ask.authed && self.unproven.contains(&ask.origin) => {
                self.finish(Outcome::Failed(Failure::SecureConnection))
            }
            Err(_) if ask.authed && self.unproven.contains(&ask.origin) => self.finish(Outcome::Failed(Failure::Unreachable)),
            Err(TransportError::Certificate) => self.fail_candidate(Failure::SecureConnection),
            Err(_) => self.fail_candidate(Failure::Unreachable),
            Ok(response) if ask.options => self.options_answered(ask, &response),
            // The password went to a host that never said what it is: what answers with it must be
            // a DAV multistatus, or the search is over. No redirect is followed with it, no other
            // place is asked, and nothing is sent to it again.
            Ok(response) if ask.authed && self.unproven.contains(&ask.origin) => match response.status() {
                207 => self.answered(&ask, &response),
                401 => self.finish(Outcome::Failed(Failure::SignInRefused)),
                _ => self.finish(Outcome::Failed(Failure::NotDavServer)),
            },
            Ok(response) => match response.status() {
                // (In the listing phase it is that home's refusal: the others are still asked.)
                401 if ask.authed && ask.stage == Stage::Collections && self.listing.is_some() => self.fail_candidate(Failure::SignInRefused),
                401 if ask.authed => self.finish(Outcome::Failed(Failure::SignInRefused)),
                401 => self.challenged(ask, &response),
                301 | 302 | 303 | 307 | 308 => self.redirected(&ask, &response),
                207 => self.answered(&ask, &response),
                403 => self.fail_candidate(Failure::ServerRefused),
                _ => self.fail_candidate(Failure::NotFound),
            },
        }
    }

    /// The kind of collection in hand.
    fn kind(&self) -> Kind {
        self.job.as_ref().map_or(Kind::Calendar, |job| job.kind)
    }

    /// Whether `response` names the class of server the search is for.
    fn says_dav(&self, response: &HttpResponse) -> bool {
        let class = if self.kind() == Kind::Calendar { "calendar-access" } else { "addressbook" };
        status::dav_classes(response).iter().any(|have| have == class)
    }

    /// A host answered `401` to a request without the password. That is a challenge only from a
    /// calendar or contacts server: its `DAV` header says so, or an `OPTIONS` brings it.
    fn challenged(&mut self, mut ask: Ask, response: &HttpResponse) {
        ask.basic = status::challenges(response).iter().any(|scheme| scheme == "basic");
        if self.says_dav(response) {
            return self.evidence_found(ask);
        }
        // The host the owner typed: they named it, so its Basic challenge is enough for the
        // password to go (a proxy that signs everybody in first answers every request with a
        // bare 401). What answers must then be a DAV multistatus, or it is no calendar server.
        if ask.basic && self.typed_origins.contains(&ask.origin) {
            if !self.unproven.contains(&ask.origin) && self.unproven.len() < MAX_TRUSTED {
                self.unproven.push(ask.origin.clone());
            }
            return self.evidence_found(ask);
        }
        // For any other host, a bare 401 is not evidence. Ask the host what it is, once, without the password.
        let (stage, url, basic) = (ask.stage, ask.url.clone(), ask.basic);
        self.issue(stage, url, true);
        if let Waiting::Request(_, options) = &mut self.waiting {
            options.basic = basic;
        }
    }

    /// The `OPTIONS` that asked a 401's host what it is: it is a calendar or contacts server when
    /// it says so (and says it by answering at all, or by its 401 again).
    fn options_answered(&mut self, mut ask: Ask, response: &HttpResponse) {
        let answered = matches!(response.status(), 200 | 204 | 401);
        if answered && self.says_dav(response) {
            ask.options = false;
            self.evidence_found(ask)
        } else {
            self.fail_candidate(Failure::NotDavServer)
        }
    }

    /// The host is a calendar or contacts server and asks for a password: it goes there when the
    /// host is the owner's, after the owner's yes when it is not.
    fn evidence_found(&mut self, ask: Ask) {
        if !ask.basic {
            // Digest, Negotiate and the like are not offered the password; no word at all is
            // not a challenge.
            return self.fail_candidate(Failure::SignInNotSupported);
        }
        let Ok(origin) = Origin::parse(&ask.url) else { return self.fail_candidate(Failure::Unreadable) };
        if self.declined.contains(&ask.origin) {
            return self.fail_candidate(Failure::Declined);
        }
        if self.may_sign_in(&origin) {
            return self.sign_in(ask);
        }
        if self.requests >= MAX_REQUESTS {
            let why = self.reason.unwrap_or(Failure::TooManyRedirects);
            return self.finish(Outcome::Failed(why));
        }
        let consent = Consent { host: label(&origin), reason: self.reason_for(&origin) };
        self.waiting = Waiting::Consent(consent, ask);
    }

    /// The question again, with the password, to the origin that has asked for it and may have it.
    fn sign_in(&mut self, ask: Ask) {
        if !self.signed_in.contains(&ask.origin) && self.signed_in.len() < MAX_TRUSTED {
            self.signed_in.push(ask.origin.clone());
        }
        if !self.signed_in.contains(&ask.origin) {
            return self.fail_candidate(Failure::Unreadable);
        }
        self.issue(ask.stage, ask.url, false);
    }

    /// A redirect: followed one at a time, to `https://` only; to another host without the password.
    fn redirected(&mut self, ask: &Ask, response: &HttpResponse) {
        let chain = self.job.as_mut().map_or(MAX_REDIRECTS, |job| {
            job.chain = job.chain.saturating_add(1);
            job.chain
        });
        // A chain that does not end is this place's failing (a looping web site), not the search's:
        // the places after it, the mail server and the SRV targets, are still asked.
        if chain > MAX_REDIRECTS {
            return self.fail_candidate(Failure::TooManyRedirects);
        }
        let Ok(origin) = Origin::parse(&ask.url) else { return self.fail_candidate(Failure::Unreadable) };
        let next = response.header("location").and_then(|location| {
            let location = location.trim();
            if location.len() > MAX_ADDRESS {
                return None;
            }
            match origin.resolve(&ask.url, location) {
                Ok(url) => Some(url),
                Err(HrefError::OffOrigin) => origin.foreign(location),
                Err(_) => None,
            }
        });
        match next {
            Some(url) => self.issue(ask.stage, url, false),
            None => self.fail_candidate(Failure::InsecureRedirect),
        }
    }

    /// A 207 answered the question of `ask`.
    fn answered(&mut self, ask: &Ask, response: &HttpResponse) {
        let Ok(origin) = Origin::parse(&ask.url) else { return self.fail_candidate(Failure::Unreadable) };
        let listing = match status::multistatus(response, &origin, &ask.url) {
            Ok(listing) => listing,
            // (A 207 that is no DAV multistatus is no evidence of anything; from a host that had the
            // password unproven it ends the search.)
            Err(_) if self.unproven.contains(&ask.origin) => return self.finish(Outcome::Failed(Failure::NotDavServer)),
            Err(_) => return self.fail_candidate(if ask.stage == Stage::Probe { Failure::NotDavServer } else { Failure::Unreadable }),
        };
        let kind = self.kind();
        // (A DAV multistatus is the proof a host that was never asked what it is lacked.)
        self.unproven.retain(|origin| *origin != ask.origin);
        // (Each question that is answered starts the redirects over.)
        if let Some(job) = self.job.as_mut() {
            job.chain = 0;
        }
        match ask.stage {
            Stage::Probe => {
                let base = listing.entries.iter().find(|entry| entry.is_base).or_else(|| listing.entries.first());
                match base.and_then(|entry| entry.principal.clone().or_else(|| entry.foreign_principal.clone())) {
                    Some(principal) => self.issue(Stage::Homes, principal, false),
                    None => self.fail_candidate(Failure::NoPrincipal),
                }
            }
            Stage::Homes => {
                let base = listing.entries.iter().find(|entry| entry.is_base).or_else(|| listing.entries.first());
                let homes = base.map(|entry| homes_of(entry, kind)).unwrap_or_default();
                if homes.is_empty() {
                    return self.fail_candidate(Failure::NoHome);
                }
                self.listing = Some(Listing {
                    seen: homes.clone(),
                    queue: homes.into_iter().map(|home| (home, 0)).collect(),
                    level: 0,
                    extra: 0,
                    homes: Vec::new(),
                    collections: Collections::default(),
                });
                self.waiting = Waiting::Idle;
            }
            Stage::Collections => self.located(ask, listing),
        }
    }

    /// A listing of a home, or of a plain container in it: what it holds is merged, and the plain
    /// containers of a home are listed too (one level down, no further).
    fn located(&mut self, ask: &Ask, listing: Multistatus) {
        let Some(state) = self.listing.as_mut() else { return };
        if state.level == 0 && !state.homes.contains(&ask.url) {
            state.homes.push(ask.url.clone());
        }
        if state.level == 0 {
            for entry in &listing.entries {
                let plain = !entry.is_base && !entry.is_gone() && entry.is_collection && !entry.is_calendar && !entry.is_addressbook;
                if plain
                    && state.extra < MAX_CONTAINER_LISTINGS
                    && state.seen.len() < MAX_HOMES + usize::from(MAX_CONTAINER_LISTINGS)
                    && !state.seen.contains(&entry.href)
                {
                    state.seen.push(entry.href.clone());
                    state.queue.push_back((entry.href.clone(), 1));
                    state.extra += 1;
                }
            }
        }
        state.collections.merge(Collections::from_listing(listing));
        self.waiting = Waiting::Idle;
    }

    /// The listings are over: what they brought is this kind found (nothing, when none answered).
    fn commit_listing(&mut self) {
        let Some(state) = self.listing.take() else { return };
        let Some(home) = state.homes.first().cloned() else { return };
        let located = Located { home, homes: state.homes, collections: state.collections };
        match self.kind() {
            Kind::Calendar => self.found.calendars = Some(located),
            Kind::AddressBook => self.found.contacts = Some(located),
        }
        self.job = None;
        self.waiting = Waiting::Idle;
    }

    /// The host has looked up the SRV record and the TXT record asked for: the targets (host and
    /// port, best first) and the path, or `None` when the lookup failed. Whatever is given is
    /// checked: names only, at most three, a port only if it is 443 or 1024 and above, a path only
    /// if it is a plain absolute path. (A call at any other time is ignored.)
    pub fn srv(&mut self, answer: Option<SrvAnswer>) {
        if !matches!(self.waiting, Waiting::Srv(_)) {
            return;
        }
        self.waiting = Waiting::Idle;
        let Some(kind) = self.job.as_ref().map(|job| job.kind) else { return };
        let answer = answer.unwrap_or_default();
        let path = answer.path.as_deref().filter(|path| plain_path(path)).unwrap_or(well_known(kind)).to_owned();
        let mut named: Vec<(String, u16)> = Vec::new();
        for (host, port) in answer.targets {
            let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
            let port = if port == 0 { 443 } else { port };
            let target = (host, port);
            if is_name(&target.0) && (port == 443 || port >= 1024) && !named.contains(&target) && named.len() < MAX_SRV_HOSTS {
                named.push(target);
            }
        }
        for (host, port) in named {
            let authority = if port == 443 { host } else { format!("{host}:{port}") };
            let url = format!("https://{authority}{path}");
            // The domain's own record names it: the host is the owner's (checked for Microsoft's
            // and Google's when it is asked).
            if let Ok(origin) = Origin::parse(&url) {
                let key = origin.base();
                if !self.trusted.contains(&key) && self.trusted.len() < MAX_TRUSTED {
                    self.trusted.push(key);
                }
            }
            if let Some(job) = self.job.as_mut() {
                job.queue.push_back(Target::Probe(url));
            }
        }
    }

    /// The owner agrees to the host the machine asked about: the password goes there. (A call at
    /// any other time is ignored.)
    pub fn agree(&mut self) {
        if !matches!(self.waiting, Waiting::Consent(..)) {
            return;
        }
        let Waiting::Consent(_, ask) = std::mem::replace(&mut self.waiting, Waiting::Idle) else {
            return;
        };
        if !self.trusted.contains(&ask.origin) && self.trusted.len() < MAX_TRUSTED {
            self.trusted.push(ask.origin.clone());
        }
        if !self.trusted.contains(&ask.origin) {
            return self.fail_candidate(Failure::Declined);
        }
        // Asked before the request: it is made, without the password, and the host's own challenge
        // comes first. Asked before the password: the password goes.
        if ask.before { self.issue(ask.stage, ask.url, ask.options) } else { self.sign_in(ask) }
    }

    /// The owner does not agree: nothing is sent to that host with the password, it is not asked
    /// about again, and the search goes on elsewhere.
    pub fn decline(&mut self) {
        if matches!(self.waiting, Waiting::Consent(..)) {
            let Waiting::Consent(_, ask) = std::mem::replace(&mut self.waiting, Waiting::Idle) else {
                return;
            };
            if self.declined.len() < MAX_TRUSTED {
                self.declined.push(ask.origin);
            }
            self.fail_candidate(Failure::Declined);
        }
    }

    /// Drives the whole search with the host's ways of doing things: `exchange` performs a
    /// request, `srv` looks a name up (the hosts and the path it names, or `None`), and `ask` asks
    /// the owner (true is yes). The host that must wait for a person calls [`step`](Self::step)
    /// itself instead.
    pub fn run<E: HttpExchange>(
        &mut self,
        exchange: &E,
        mut srv: impl FnMut(&SrvName) -> Option<SrvAnswer>,
        mut ask: impl FnMut(&Consent) -> bool,
    ) -> Outcome {
        // Every round is a request, a look-up or a question, and each is bounded.
        for _ in 0..usize::from(MAX_REQUESTS).saturating_mul(8) {
            match self.step() {
                Step::Request(request) => {
                    let result = exchange.exchange(&request);
                    self.response(result);
                }
                Step::LookupSrv(name) => {
                    let answer = srv(&name);
                    self.srv(answer);
                }
                Step::NeedsYes(consent) => {
                    if ask(&consent) {
                        self.agree();
                    } else {
                        self.decline();
                    }
                }
                Step::Done(outcome) => return outcome,
            }
        }
        self.settle(Outcome::Failed(self.reason.unwrap_or(Failure::TooManyRedirects)))
    }
}

/// The host of `origin` as the owner is asked about it: with its port when that is not the scheme's.
fn label(origin: &Origin) -> String {
    if origin.port() == if origin.secure() { 443 } else { 80 } { origin.host().to_owned() } else { format!("{}:{}", origin.host(), origin.port()) }
}

/// Whether `host` is an address by number and not a name: an IPv6 address in brackets, or a name
/// whose last label is all digits or `0x` and hex digits (`10.0.0.1`, `10.0.0.1.`, `2130706433`,
/// `0x7f000001`, `0x7f.0x1`, octal `0177.0.0.1`): what a resolver reads as an address (WHATWG URL
/// "ends in a number"; `crates/link/src/host.rs`). `0xacab.org` and `0x00sec.org` are names.
fn is_number_host(host: &str) -> bool {
    if host.starts_with('[') {
        return true;
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    host.rsplit('.').next().is_some_and(|last| {
        let hex = last.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("0x"))
            && last.get(2..).is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_hexdigit()));
        !last.is_empty() && (hex || last.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// The proof that the password may go to an origin. Only [`Discovery`] makes one (the field is
/// private to this module), when the host has answered its own challenge and has been allowed;
/// [`Endpoint::granted`] takes it, and nothing else pairs a password with an origin a server named.
pub(crate) struct Grant {
    origin: Origin,
}

impl Grant {
    /// The origin the password may go to.
    pub(crate) fn origin(&self) -> &Origin {
        &self.origin
    }
}

/// The homes of `kind` a principal names (it may name several): on its own origin first, then
/// another one's, each once, at most [`MAX_HOMES`].
fn homes_of(entry: &Entry, kind: Kind) -> Vec<String> {
    let (own, other) = match kind {
        Kind::Calendar => (&entry.calendar_homes, &entry.foreign_calendar_homes),
        Kind::AddressBook => (&entry.addressbook_homes, &entry.foreign_addressbook_homes),
    };
    let mut homes: Vec<String> = Vec::new();
    for home in own.iter().chain(other.iter()) {
        if homes.len() < MAX_HOMES && !homes.contains(home) {
            homes.push(home.clone());
        }
    }
    homes
}

/// Whether `path` is an absolute path of plain characters, as a TXT `path=` must be: it is put
/// after a host in an address.
fn plain_path(path: &str) -> bool {
    path.len() <= MAX_PATH
        && path.starts_with('/')
        && !path.starts_with("//")
        && path.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'?' | b'#' | b'\\' | b'"'))
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod discovery_tests;
