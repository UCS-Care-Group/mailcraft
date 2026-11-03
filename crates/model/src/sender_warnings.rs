// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Sender warnings**: what MailCraft tells the owner about a message that may not be what it
//! seems, each with its reason. A **warning** when the account's own server says the sender's
//! checks failed, or a name hides another address; a **caution** for a **lookalike** sender or
//! a link whose text names one place and goes to another (and a failed check where the
//! sender's domain asks for nothing to be done about mail that fails); and the quiet **External** tag for
//! mail from outside the **owner's organisation**, which is never a warning by itself.
//!
//! Pure: the rules read a message and the [`Facts`] they are measured against (the owner's own
//! addresses, names and organisation, and the **known correspondents**: the contacts, and
//! whoever mail in Sent Items went to), which the engine gathers and keeps up to date. Nothing
//! here looks anything up, and nothing is stored: the warnings are worked out when a message is
//! read.
//!
//! What a message says is hostile, and so is what it would have the owner believe: every rule is
//! bounded (so many characters of a name, so many links, edit distance only between domains of
//! nearly the same length), and what comes out is data about the message, never its words as
//! instructions.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{Value, json};

use crate::auth_results::{self, Failed, MAX_HEADERS_READ};
use crate::confusables::{as_seen, hides, skeleton};
use crate::{Account, FolderKind, Message, provider};

/// The most characters of a display name that are read.
const MAX_NAME_CHARS: usize = 1_000;
/// The most characters of a name or link text that a warning carries.
const MAX_DETAIL_CHARS: usize = 200;
/// The most links of a message looked at, and the most link warnings it gets.
pub const MAX_LINKS: usize = 1_000;
const MAX_LINK_WARNINGS: usize = 20;
/// The longest link text read as a web address.
const MAX_LINK_TEXT: usize = 2_048;
/// A domain whose name (the part before its suffix: `lumen` of `lumen.example`) is shorter than
/// this is never called a lookalike: short names are one letter from many others.
const MIN_LOOKALIKE_NAME: usize = 5;

/// How much a sender warning weighs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// A quiet mark beside the sender's name (External).
    Tag,
    /// The banner is amber when there are only cautions.
    Caution,
    /// The banner is red.
    Warning,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Tag => "tag",
            Level::Caution => "caution",
            Level::Warning => "warning",
        }
    }
}

/// Why a name hides another address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameReason {
    /// The name carries an address that is not the sender's (`"bank@example.org" <x@evil.example>`).
    AddressInName(String),
    /// It is the name of one of the owner's contacts, on an address that is none of theirs.
    KnownName,
    /// It is the owner's own name, on an address that is not theirs.
    OwnName,
    /// More of it is not shown than is ([`hides`]): padded to slip past what reads it.
    HiddenCharacters,
}

/// One reason a message may not be what it seems.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// The account's own server says the sender's checks failed.
    FailedCheck(Failed),
    /// The sender's name (`name`) hides another address.
    NameHidesAddress { name: String, reason: NameReason },
    /// From outside the owner's organisation: the sender's domain.
    External { domain: String },
    /// The sender's domain (`domain`, written as it goes over the wire) is one character, or one
    /// look-alike character, away from `like`, a known correspondent's.
    LookalikeSender { domain: String, like: String },
    /// A link whose text names `text_host` (empty when its text hides more than it shows,
    /// [`hides`]) and which goes to `host`; `url` is where it goes, in full (an agent is given its
    /// first characters).
    LinkMismatch { text_host: String, host: String, url: String },
}

impl Warning {
    /// The stable id agents and scripts read.
    pub fn kind(&self) -> &'static str {
        match self {
            Warning::FailedCheck(_) => "failedCheck",
            Warning::NameHidesAddress { .. } => "nameHidesAddress",
            Warning::External { .. } => "external",
            Warning::LookalikeSender { .. } => "lookalikeSender",
            Warning::LinkMismatch { .. } => "linkMismatch",
        }
    }

    pub fn level(&self) -> Level {
        match self {
            Warning::FailedCheck(Failed::DmarcPolicyNone) => Level::Caution,
            Warning::FailedCheck(_) | Warning::NameHidesAddress { .. } => Level::Warning,
            Warning::LookalikeSender { .. } | Warning::LinkMismatch { .. } => Level::Caution,
            Warning::External { .. } => Level::Tag,
        }
    }

    /// As data: `kind`, `level`, and what it is about.
    pub fn json(&self) -> Value {
        let mut v = json!({"kind": self.kind(), "level": self.level().as_str()});
        let detail = match self {
            Warning::FailedCheck(Failed::DmarcPolicyNone) => json!({"method": "dmarc", "policy": "none"}),
            Warning::FailedCheck(method) => json!({"method": method.as_str()}),
            Warning::NameHidesAddress { name, reason } => match reason {
                NameReason::AddressInName(address) => json!({"name": name, "reason": "addressInName", "address": address}),
                NameReason::KnownName => json!({"name": name, "reason": "knownName"}),
                NameReason::OwnName => json!({"name": name, "reason": "ownName"}),
                NameReason::HiddenCharacters => json!({"name": name, "reason": "hiddenCharacters"}),
            },
            Warning::External { domain } => json!({"domain": domain}),
            Warning::LookalikeSender { domain, like } => json!({"domain": domain, "like": like}),
            Warning::LinkMismatch { text_host, host, url } => json!({"textHost": text_host, "host": host, "url": detail(url)}),
        };
        if let (Some(o), Value::Object(more)) = (v.as_object_mut(), detail) {
            o.extend(more);
        }
        v
    }
}

/// The heaviest level among `warnings`, not counting the External tag; `None` when there is
/// nothing for a banner.
pub fn banner_level(warnings: &[Warning]) -> Option<Level> {
    warnings.iter().map(Warning::level).filter(|l| *l > Level::Tag).max()
}

/// The kinds of `warnings`, once each, in order: what dismissing their banner is remembered as
/// ([`dismissed`]).
pub fn kinds(warnings: &[Warning]) -> Vec<String> {
    let mut out: Vec<String> = warnings.iter().map(|w| w.kind().to_string()).collect();
    out.sort();
    out.dedup();
    out
}

/// Whether the banner for `warnings` was dismissed: there is one, and every kind in it is among
/// the `dismissed` kinds kept with the message. A kind that wasn't there then shows it again.
pub fn dismissed(warnings: &[Warning], dismissed: &[String]) -> bool {
    banner_level(warnings).is_some() && warnings.iter().all(|w| dismissed.iter().any(|k| k == w.kind()))
}

/// What the warnings are measured against. Built by [`FactsBuilder`].
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// The owner's own addresses, in lower case: every account's and shared mailbox's.
    own: HashSet<String>,
    /// The domains of the owner's organisation: of those addresses, other than public mail
    /// providers'. Empty for an owner with only personal accounts.
    organisation: HashSet<String>,
    /// People by their name ([`name_key`]): the addresses that are theirs, and whether the name
    /// is the owner's own.
    people: HashMap<String, (HashSet<String>, bool)>,
    /// The known correspondents' and the owner's own registrable domains ([`registrable`]).
    domains: HashSet<String>,
    /// The same by skeleton, and by length in characters (for the edit distance).
    by_skeleton: HashMap<String, String>,
    by_len: BTreeMap<usize, Vec<(Vec<char>, String)>>,
}

impl Facts {
    /// Whether `address` (any case) is one of the owner's own.
    pub fn is_own(&self, address: &str) -> bool {
        self.own.contains(&address.trim().to_lowercase())
    }

    /// The owner's organisation's domains.
    pub fn organisation(&self) -> impl Iterator<Item = &str> {
        self.organisation.iter().map(String::as_str)
    }
}

/// Gathers [`Facts`]: the owner's accounts, then every known correspondent.
#[derive(Default)]
pub struct FactsBuilder {
    facts: Facts,
    known_domains: HashSet<String>,
}

impl FactsBuilder {
    /// The owner's own accounts and shared mailboxes.
    pub fn accounts<'a>(self, accounts: impl IntoIterator<Item = &'a Account>) -> Self {
        self.accounts_by(accounts.into_iter().map(|a| (a.email.as_str(), a.display_name.as_str(), a.is_shared())))
    }

    /// The same, each as its address, the owner's name on it, and whether it is a shared mailbox.
    pub fn accounts_by<'a>(mut self, accounts: impl IntoIterator<Item = (&'a str, &'a str, bool)>) -> Self {
        for (address, name, shared) in accounts {
            let address = address.trim().to_lowercase();
            let Some(domain) = sender_domain(&address) else { continue };
            self.facts.own.insert(address.clone());
            if !provider::is_mail_provider(&domain) && !provider::is_public_suffix(&domain) {
                self.facts.organisation.insert(domain.clone());
            }
            self.known_domains.insert(domain);
            // A shared mailbox's name ("Support") is the organisation's, not the owner's.
            if !shared && let Some(key) = name_key(name) {
                let entry = self.facts.people.entry(key).or_default();
                entry.0.insert(address);
                entry.1 = true;
            }
        }
        self
    }

    /// A known correspondent: an address the owner keeps as a contact, with the name it goes by
    /// there, or has written to (with no name: a name in Sent Items is the owner's word for
    /// somebody, not theirs, and is not compared).
    pub fn known(&mut self, address: &str, name: &str) {
        let address = address.trim().to_lowercase();
        let Some(domain) = sender_domain(&address) else { return };
        if let Some(key) = name_key(name) {
            self.facts.people.entry(key).or_default().0.insert(address);
        }
        self.known_domains.insert(domain);
    }

    pub fn build(mut self) -> Facts {
        for domain in std::mem::take(&mut self.known_domains) {
            let domain = registrable(&domain).to_string();
            if self.facts.domains.insert(domain.clone()) {
                self.facts.by_skeleton.entry(skeleton(&domain)).or_insert_with(|| domain.clone());
                let chars: Vec<char> = domain.chars().collect();
                self.facts.by_len.entry(chars.len()).or_default().push((chars, domain));
            }
        }
        self.facts
    }
}

/// A name as names are compared: as it reads on the screen ([`as_seen`]), each word without the
/// quotes and punctuation round it and as its [`skeleton`] (so `M\u{430}ya` is `Maya`), the words
/// with letters in them one space apart. `None` for what is no person's name: fewer than two
/// such words (a one-word name is never compared with anybody's), or an address.
pub fn name_key(name: &str) -> Option<String> {
    let seen = as_seen(name, MAX_NAME_CHARS);
    if seen.contains('@') {
        return None;
    }
    let words: Vec<String> = seen
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| word.chars().any(char::is_alphabetic))
        .map(skeleton)
        .collect();
    (words.len() >= 2).then(|| words.join(" "))
}

/// The domain of `address` as domains are compared: lower case, without a dot at the end, each
/// `xn--` label read back into the letters it stands for. `None` for what is no domain.
pub fn sender_domain(address: &str) -> Option<String> {
    let (_, domain) = address.trim().rsplit_once('@')?;
    normal_domain(domain)
}

/// `domain` as domains are compared ([`sender_domain`]).
pub(crate) fn normal_domain(domain: &str) -> Option<String> {
    let domain = domain.trim().trim_end_matches('.');
    if domain.is_empty() || domain.len() > 253 || domain.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '@' | '/' | '\\')) {
        return None;
    }
    let mut labels = Vec::new();
    for label in domain.split('.') {
        if label.is_empty() {
            return None;
        }
        let label = label.to_lowercase();
        let read = label.strip_prefix("xn--").and_then(mailcraft_link::punycode::decode).map(|chars| chars.into_iter().collect::<String>());
        labels.push(read.unwrap_or(label));
    }
    Some(labels.join("."))
}

/// The part of `domain` an organisation registers: its last two labels, or three under a suffix
/// anybody may have a name of their own under (`lumen.co.uk`, `lumen.github.io`).
pub fn registrable(domain: &str) -> &str {
    let from_end = |labels: usize| domain.rmatch_indices('.').nth(labels - 1).and_then(|(dot, _)| domain.get(dot + 1..)).unwrap_or(domain);
    let two = from_end(2);
    if provider::is_public_suffix(two) { from_end(3) } else { two }
}

/// Whether `a` and `b` differ by at most one edit: a character put in, taken out or changed, or
/// two side by side swapped. Linear in their length.
pub fn within_one_edit(a: &[char], b: &[char]) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    match long.len() - short.len() {
        0 => {
            let differ: Vec<usize> = (0..short.len()).filter(|&i| short.get(i) != long.get(i)).take(3).collect();
            match differ.as_slice() {
                [] | [_] => true,
                [i, j] => *j == i + 1 && short.get(*i) == long.get(*j) && short.get(*j) == long.get(*i),
                _ => false,
            }
        }
        1 => {
            let first = (0..short.len()).find(|&i| short.get(i) != long.get(i)).unwrap_or(short.len());
            short.get(first..) == long.get(first + 1..)
        }
        _ => false,
    }
}

/// The known correspondent's domain the sender's `domain` looks like, when it isn't one itself:
/// the same skeleton, or one edit away. Measured between the registrable parts, and never for a
/// public mail provider's domain or a short name.
fn lookalike(facts: &Facts, domain: &str) -> Option<String> {
    let mine = registrable(domain);
    if facts.domains.contains(mine) || provider::is_mail_provider(mine) {
        return None;
    }
    let name = mine.split('.').next().unwrap_or(mine);
    if name.chars().count() < MIN_LOOKALIKE_NAME {
        return None;
    }
    if let Some(like) = facts.by_skeleton.get(&skeleton(mine)).filter(|like| like.as_str() != mine) {
        return Some(like.clone());
    }
    let chars: Vec<char> = mine.chars().collect();
    let len = chars.len();
    facts
        .by_len
        .range(len.saturating_sub(1)..=len.saturating_add(1))
        .flat_map(|(_, domains)| domains)
        .find(|(theirs, like)| like.split('.').next().is_some_and(|n| n.chars().count() >= MIN_LOOKALIKE_NAME) && within_one_edit(&chars, theirs))
        .map(|(_, like)| like.clone())
}

/// `domain` written the way it goes over the wire (`xn--…` for letters outside ASCII), so that a
/// lookalike shows for what it is.
fn wire(domain: &str) -> String {
    mailcraft_link::WebLink::parse(&format!("http://{domain}/")).map_or_else(|| domain.to_string(), |link| link.host().to_string())
}

/// `text` cut to [`MAX_DETAIL_CHARS`].
fn detail(text: &str) -> String {
    text.trim().chars().take(MAX_DETAIL_CHARS).collect()
}

/// The addresses written in a display name (what has an `@` with something on either side and a
/// dot after it), in lower case.
fn addresses_in(name: &str) -> Vec<String> {
    // As it reads: what draws nothing left out, `＠` and `․` as `@` and `.`.
    let chars: Vec<char> = as_seen(name, MAX_NAME_CHARS).chars().collect();
    let local = |c: char| c.is_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-' | '\'');
    let host = |c: char| c.is_alphanumeric() || matches!(c, '.' | '-');
    let mut out = Vec::new();
    for (at, _) in chars.iter().enumerate().filter(|(_, c)| **c == '@') {
        let start = chars.get(..at).map_or(at, |before| before.iter().rposition(|c| !local(*c)).map_or(0, |i| i + 1));
        let end = chars.get(at + 1..).map_or(at + 1, |after| after.iter().position(|c| !host(*c)).map_or(chars.len(), |i| at + 1 + i));
        let (Some(left), Some(right)) = (chars.get(start..at), chars.get(at + 1..end)) else { continue };
        let right: String = right.iter().collect::<String>().trim_end_matches(['.', '-']).to_string();
        if !left.is_empty() && right.contains('.') && !right.starts_with('.') {
            out.push(format!("{}@{}", left.iter().collect::<String>(), right).to_lowercase());
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// Whether the sender's name hides another address: it carries one that isn't the sender's, or
/// it is the name of somebody the owner knows (or the owner's own) on an address none of theirs.
fn name_hides(facts: &Facts, m: &Message) -> Option<Warning> {
    let address = m.from.email.trim().to_lowercase();
    let name = detail(&m.from.name);
    if hides(&m.from.name) {
        return Some(Warning::NameHidesAddress { name, reason: NameReason::HiddenCharacters });
    }
    // A list writes its members' names as "Maya Okafor via Lumen Team": the name is the
    // member's, the address the list's.
    let via = via_list(&as_seen(&m.from.name, MAX_NAME_CHARS));
    let list = m.headers.iter().take(MAX_HEADERS_READ).any(|(h, _)| h.eq_ignore_ascii_case("List-Id") || h.eq_ignore_ascii_case("List-Post"));
    if !(via && list)
        && let Some(other) = addresses_in(&m.from.name).into_iter().find(|a| *a != address)
    {
        return Some(Warning::NameHidesAddress { name, reason: NameReason::AddressInName(other) });
    }
    if via {
        return None;
    }
    let (theirs, own) = facts.people.get(&name_key(&m.from.name)?)?;
    (!theirs.contains(&address)).then_some(Warning::NameHidesAddress { name, reason: if *own { NameReason::OwnName } else { NameReason::KnownName } })
}

/// Whether `name` (as it reads) is a list's for one of its members: "<anything> via <list>".
fn via_list(name: &str) -> bool {
    name.to_lowercase().rsplit_once(" via ").is_some_and(|(member, list)| !member.trim().is_empty() && list.chars().any(char::is_alphanumeric))
}

/// The host a link's text names, when the text is itself a web address or a bare domain
/// (`https://lumen.example/pay`, `www.lumen.example`, `lumen.example/pay`); `None` for words
/// ("Click here"), an e-mail address, and anything else.
fn named_host(text: &str) -> Option<String> {
    // As it reads: `www.lumen\u{200b}.example` is shown as `www.lumen.example`.
    let text = as_seen(text, MAX_LINK_TEXT.saturating_add(1));
    let text = text.trim().trim_end_matches(['.', ',', ';', ':', ')', '>']).trim_start_matches(['(', '<']);
    if text.is_empty() || text.len() > MAX_LINK_TEXT || text.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return mailcraft_link::WebLink::parse(text).map(|link| link.host().to_string());
    }
    if text.contains(['@', ':']) {
        return None;
    }
    // A bare domain: letters, digits, dashes and dots up to the first `/`, with a top-level
    // label of letters only.
    let host = text.split(['/', '?', '#']).next()?;
    let top = host.trim_end_matches('.').rsplit('.').next()?;
    let plain = host.contains('.') && top.chars().count() >= 2 && top.chars().all(char::is_alphabetic) && !host.starts_with(['.', '-']);
    if !plain || host.chars().any(|c| !(c.is_alphanumeric() || matches!(c, '.' | '-'))) {
        return None;
    }
    mailcraft_link::WebLink::parse(&format!("http://{text}")).map(|link| link.host().to_string())
}

/// A host as hosts are compared: without a leading `www.` and a dot at the end.
fn host_key(host: &str) -> &str {
    let host = host.trim_end_matches('.');
    host.strip_prefix("www.").unwrap_or(host)
}

/// Microsoft's Safe Links, which rewrites a link to go through `*.safelinks.protection.outlook.com`
/// with the link it stands for in its `url` parameter. That link is the one compared.
const SAFE_LINKS: &str = "safelinks.protection.outlook.com";

/// Proofpoint's URL Defense, whose links carry the link they stand for, written as Proofpoint
/// describes in public: version 2 at this host, `/v2/url?u=…` with `-` for `%` and `_` for `/`.
const PROOFPOINT_V2: &str = "urldefense.proofpoint.com";
/// Version 3, at this domain: `/v3/__<link>__;…`, a `*` in the link standing for characters
/// given after it.
const PROOFPOINT_V3: &str = "urldefense.com";

/// The other services an organisation may have rewrite every link of its mail to pass through
/// them, whose links don't say where they go (Mimecast's URL Protect), each with the hosts under
/// it. A link through them is not compared at all. Kept short: only services that rewrite a
/// whole organisation's mail and hide where it goes belong here.
const LINK_REWRITERS: [&str; 2] = ["mimecast.com", "mimecastprotect.com"];

/// The most links one link is read as standing for (Safe Links' `url`, given more than once).
const MAX_INNER_LINKS: usize = 4;

/// Whether `host` is `domain` or a host under it (never merely a name that contains it).
fn is_at(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|head| head.ends_with('.'))
}

/// `text` with its `%XX` escapes read back (`+` left as it is), as UTF-8 at most loosely.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b, hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The path and query of `link` (`/v2/url?u=…`), without its fragment.
fn path_of(link: &str) -> &str {
    let rest = link.split_once("://").map_or("", |(_, rest)| rest);
    let path = rest.find('/').and_then(|at| rest.get(at..)).unwrap_or("");
    path.split('#').next().unwrap_or(path)
}

/// The values of the query parameter `key` of `link` (its name in any letter case), as written.
fn query_values<'a>(link: &'a str, key: &'a str) -> impl Iterator<Item = &'a str> {
    let query = path_of(link).split_once('?').map_or("", |(_, query)| query);
    query.split('&').filter_map(move |pair| pair.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v))
}

/// The link a URL Defense link stands for; `None` when it can't be read whole.
fn proofpoint(link: &mailcraft_link::WebLink, host: &str) -> Option<mailcraft_link::WebLink> {
    let path = path_of(link.as_str());
    if is_at(host, PROOFPOINT_V2) {
        if !path.get(..7).is_some_and(|p| p.eq_ignore_ascii_case("/v2/url")) {
            return None;
        }
        let written: String = query_values(link.as_str(), "u")
            .next()?
            .chars()
            .map(|c| match c {
                '-' => '%',
                '_' => '/',
                c => c,
            })
            .collect();
        return mailcraft_link::WebLink::parse(percent_decoded(&written).trim());
    }
    // ponytail: a `*` stands for characters given after `__;`; such a link stays uncompared
    // rather than be half read. Read them if rewritten links with one turn out common.
    let (inner, _) = path.strip_prefix("/v3/__")?.split_once("__;")?;
    if inner.contains('*') {
        return None;
    }
    mailcraft_link::WebLink::parse(inner.trim())
}

/// Where `link` really goes: through Safe Links, each link its `url` stands for (the Safe Links
/// address itself when it stands for none); through URL Defense, the link it stands for; else
/// `link` itself. None for a link through one of the [`LINK_REWRITERS`], or a URL Defense link
/// that can't be read whole: those are not compared.
fn real_links(link: mailcraft_link::WebLink) -> Vec<mailcraft_link::WebLink> {
    if !is_at(&host_key(link.host()).to_ascii_lowercase(), SAFE_LINKS) {
        return unwrapped(link);
    }
    // Safe Links round another rewriter (an organisation behind Proofpoint as well) is
    // unwrapped once more; no deeper.
    let (mut inner, mut named) = (Vec::<mailcraft_link::WebLink>::new(), false);
    for value in query_values(link.as_str(), "url").take(MAX_INNER_LINKS) {
        let Some(target) = mailcraft_link::WebLink::parse(percent_decoded(value).trim()) else { continue };
        named = true;
        for target in unwrapped(target) {
            if !inner.iter().any(|known| known.as_str() == target.as_str()) {
                inner.push(target);
            }
        }
    }
    if named { inner } else { vec![link] }
}

/// Where `link` goes, through a rewriter other than Safe Links: the link URL Defense carries,
/// nothing for one that hides it or can't be read, else `link` itself.
fn unwrapped(link: mailcraft_link::WebLink) -> Vec<mailcraft_link::WebLink> {
    let host = host_key(link.host()).to_ascii_lowercase();
    if LINK_REWRITERS.iter().any(|rewriter| is_at(&host, rewriter)) {
        return Vec::new();
    }
    if is_at(&host, PROOFPOINT_V2) || is_at(&host, PROOFPOINT_V3) {
        return proofpoint(&link, &host).into_iter().collect();
    }
    vec![link]
}

/// Where the link to `href` really goes, as [`link_mismatches`] compares it: none for no web
/// address, or one whose real place can't be read ([`LINK_REWRITERS`]); more than one for a
/// Safe Links address that names several.
pub fn link_targets(href: &str) -> Vec<mailcraft_link::WebLink> {
    mailcraft_link::WebLink::parse(href.trim()).map(real_links).unwrap_or_default()
}

/// The links whose text names one place and which go to another, of `links` (each its text and
/// where it goes). Two hosts of the same registrable domain are one place (`lumen.example` and
/// `pay.lumen.example`). A link that is no web address (`mailto:`, `cid:`) is not one of them; a
/// link rewritten by Safe Links or URL Defense is compared as the link it stands for (each, when
/// Safe Links names several), and one through a service that hides it ([`LINK_REWRITERS`]) not
/// at all.
pub fn link_mismatches<'a>(links: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Warning> {
    let mut out: Vec<Warning> = Vec::new();
    for (text, href) in links.into_iter().take(MAX_LINKS) {
        // A text that hides more than it shows names nothing that can be believed: wherever the
        // link goes, it is said, with the host the text shows when it shows one.
        let hidden = hides(text);
        let Some(named) = named_host(text).or_else(|| hidden.then(String::new)) else { continue };
        for link in link_targets(href) {
            let (named, real) = (host_key(&named), host_key(link.host()));
            if !hidden && (named == real || registrable(named) == registrable(real)) {
                continue;
            }
            let same = |w: &Warning| matches!(w, Warning::LinkMismatch { url, .. } if url == link.as_str());
            if !out.iter().any(same) {
                out.push(Warning::LinkMismatch { text_host: named.to_string(), host: real.to_string(), url: link.as_str().to_string() });
            }
            if out.len() >= MAX_LINK_WARNINGS {
                return out;
            }
        }
    }
    out
}

/// The sender warnings of `m`, in the account `account` and a folder of `kind`, measured against
/// `facts`; with `links` (each link's text and where it goes) when the body was looked at. In
/// order: warnings, cautions, then the External tag. Nothing for mail in Sent Items, Drafts or
/// the Outbox. Mail from the owner's own addresses (a shared mailbox's too) is only checked for a
/// failed check: anybody can write the owner's address on a message, and only the server can say
/// it was not the owner who sent it.
pub fn check<'a>(
    m: &Message,
    account: Option<&Account>,
    kind: Option<FolderKind>,
    facts: &Facts,
    links: Option<impl IntoIterator<Item = (&'a str, &'a str)>>,
) -> Vec<Warning> {
    if matches!(kind, Some(FolderKind::Sent | FolderKind::Drafts | FolderKind::Outbox)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(failed) = auth_results::failed_check(account, m) {
        out.push(Warning::FailedCheck(failed));
    }
    if facts.is_own(&m.from.email) {
        return out;
    }
    out.extend(name_hides(facts, m));
    let domain = sender_domain(&m.from.email);
    if let Some(domain) = &domain
        && let Some(like) = lookalike(facts, domain)
    {
        out.push(Warning::LookalikeSender { domain: wire(registrable(domain)), like: wire(&like) });
    }
    if let Some(links) = links {
        out.extend(link_mismatches(links));
    }
    if let Some(domain) = domain
        && !facts.organisation.is_empty()
        && !facts.organisation.iter().any(|ours| domain == *ours || domain.strip_suffix(ours.as_str()).is_some_and(|head| head.ends_with('.')))
    {
        out.push(Warning::External { domain: wire(&domain) });
    }
    out
}
