// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the account's own mail server says of a message's sender, read from the topmost
//! `Authentication-Results` header (RFC 8601) and nothing deeper.
//!
//! The receiving server puts its own header on top; the ones below it were written before, by
//! whoever, and a sender can write any of them. So only the topmost one is read, and only when
//! the server of the account the message is in wrote it ([`from_own_server`]); on Exchange
//! Online, which writes more of its own above it, the one with its id above the hop at which it
//! took the mail in from the internet ([`exchange_online_results`]). MailCraft makes
//! no checks of its own (no DNS, no DKIM, no SPF): a server that says nothing says nothing.
//!
//! Two answers come of it:
//!
//! - [`vouched`]: the server says the sender's domain passed DMARC, or, on Exchange Online, that
//!   the mail came from inside the owner's own organisation. What the window lets a remembered
//!   sender's pictures come by, and a sender's own picture.
//! - [`failed_check`]: the server says it failed DMARC, or, when the server gives no DMARC
//!   verdict at all, SPF for the sender's domain. A **sender warning**.
//!
//! Everything is bounded: so many headers looked through, so many characters of the value, so
//! many clauses.

use crate::{Account, AccountKind, Message, provider, sender_warnings};

/// How many of a message's headers are looked through for the server's verdict, and how many
/// clauses and characters of it are read.
pub(crate) const MAX_HEADERS_READ: usize = 2_000;
const MAX_TOKENS: usize = 200;
const MAX_HEADER_CHARS: usize = 4_000;
/// The header Exchange writes on every message it delivers, saying whether the sender signed in
/// inside the organisation (`Internal`) or came from outside (`Anonymous`).
pub const AUTH_AS: &str = "X-MS-Exchange-Organization-AuthAs";
/// Most [`AUTH_AS`] headers a message keeps (`mailcraft_mime` keeps them apart from its cap on
/// headers, so that a message padded with others can't push Exchange's own out): one with this
/// many may have had more.
pub const MAX_AUTH_AS: usize = 16;
/// Forget `m`'s [`AUTH_AS`] stamps and its `Authentication-Results` (and
/// `ARC-Authentication-Results`) as it goes onto another account's server by MailCraft's own
/// hand: they are the word of the server it came from, or a sender's own where no server removed
/// them, and no receiving server stamps a copy MailCraft puts there itself, so the copy would be
/// vouched for by them ([`internal`], [`vouched`]).
pub fn forget_stamps(m: &mut Message) {
    m.remove_header(AUTH_AS);
    m.remove_header(RESULTS);
    m.remove_header("ARC-Authentication-Results");
}

/// The header a receiving server writes its verdict in (RFC 8601).
const RESULTS: &str = "Authentication-Results";
/// The authserv-id Exchange Online writes its own `Authentication-Results` with.
const EXCHANGE_ONLINE_ID: &str = "mx.microsoft.com";
/// The names, under this, of the hosts at which Exchange Online takes mail in from the internet.
const EXCHANGE_ONLINE_EDGE: &str = ".mail.protection.outlook.com";
/// The header Exchange writes on every message it delivers, saying whether it came in from
/// outside (`Incoming`) or was sent inside the organisation (`Originating`). `mailcraft_mime`
/// keeps it apart from its cap on headers, as it does [`AUTH_AS`], at most [`MAX_AUTH_AS`] of them.
pub const DIRECTIONALITY: &str = "X-MS-Exchange-Organization-MessageDirectionality";

/// Longest address of a sender that is read, and the longest domain.
const MAX_SENDER_LEN: usize = 320;
const MAX_DOMAIN_LEN: usize = 253;

/// A sender's address as it is compared: ASCII lower case; `None` for what is no address.
pub fn sender(text: &str) -> Option<String> {
    let text = text.trim();
    // ASCII only, folded as ASCII: Unicode lower-casing maps some letters (the Kelvin sign) onto
    // ASCII ones, which would let a lookalike address stand for another.
    let plain = text.is_ascii() && text.len() <= MAX_SENDER_LEN && text.contains('@') && !text.chars().any(|c| c.is_whitespace() || c.is_control());
    (plain && text.len() >= 3).then(|| text.to_ascii_lowercase())
}

/// A domain as it is compared: what follows the `@` of an address (or the whole of `text`, with
/// or without an `@` in front), in ASCII lower case; `None` for what is no domain (a name with a
/// letter outside ASCII is written the way it goes over the wire, `xn--…`, to be one).
pub fn domain(text: &str) -> Option<String> {
    let text = text.trim();
    let name = text.rsplit_once('@').map_or(text, |(_, name)| name).trim_end_matches('.');
    let plain = !name.is_empty()
        && name.len() <= MAX_DOMAIN_LEN
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        && !name.starts_with(['.', '-'])
        && !name.contains("..");
    plain.then(|| name.to_ascii_lowercase())
}

/// The authserv-id (RFC 8601) an `Authentication-Results` value starts with: the first word before
/// the first `;`, after comments, ignoring an optional version number. `None` when the value
/// starts with a result instead (as Exchange Online did before it wrote one), or the id is no host name.
fn authserv_id(value: &str) -> Option<String> {
    let first = first_word(value)?;
    if first.contains('=') { None } else { domain(&first) }
}

/// Whether an `Authentication-Results` value starts with a result (`spf=pass …`) and no
/// authserv-id, as Exchange Online writes it for consumer mailboxes and did for others before.
fn without_id(value: &str) -> bool {
    first_word(value).is_some_and(|first| first.contains('='))
}

/// The first word of a value before its first `;`, outside comments.
fn first_word(value: &str) -> Option<String> {
    let mut plain = String::new();
    let mut depth = 0usize;
    for c in value.chars().take(MAX_HEADER_CHARS) {
        match c {
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => break,
            c if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    plain.split_whitespace().next().map(str::to_string)
}

/// The host a `Received` value says took the message in: the word after `by`, outside comments.
fn received_by(value: &str) -> Option<String> {
    let mut plain = String::new();
    let mut depth = 0usize;
    for c in value.chars().take(MAX_HEADER_CHARS) {
        match c {
            '(' => {
                depth = depth.saturating_add(1);
                plain.push(' ');
            }
            ')' => {
                depth = depth.saturating_sub(1);
                plain.push(' ');
            }
            c if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let mut words = plain.split_whitespace();
    words.by_ref().find(|word| word.eq_ignore_ascii_case("by"))?;
    domain(words.next()?.trim_end_matches(';'))
}

/// Whether a `Received` header's value is the hop at which Exchange Online took the mail in from
/// the internet ([`EXCHANGE_ONLINE_EDGE`]): such a hop stays on the message (`crate::headers`).
pub(crate) fn is_exchange_online_edge(value: &str) -> bool {
    received_by(value).is_some_and(|host| host.ends_with(EXCHANGE_ONLINE_EDGE))
}

/// The `Authentication-Results` headers Exchange Online wrote of `m` as it took it in from the
/// internet, its own first and the ones below it after: the topmost header with its authserv-id
/// ([`EXCHANGE_ONLINE_ID`]), or with none (it starts with a result), that lies above the topmost
/// `Received` by one of its edge hosts ([`EXCHANGE_ONLINE_EDGE`]): what is below that hop came
/// with the message, and anyone can write it. Its place is what makes it Exchange's, not its id.
/// For a verdict that vouches (`for_failure` false) Exchange must also say the mail came in from
/// outside (every [`DIRECTIONALITY`] header says `Incoming`, and there is one); a failure is read
/// without that word, since a forged one can only add a warning. `None` otherwise.
fn exchange_online_results(m: &Message, for_failure: bool) -> Option<Vec<&str>> {
    let headers: Vec<&(String, String)> = m.headers.iter().take(MAX_HEADERS_READ).collect();
    if !for_failure {
        let mut incoming = false;
        for (_, value) in headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case(DIRECTIONALITY)) {
            if !value.trim().eq_ignore_ascii_case("Incoming") {
                return None;
            }
            incoming = true;
        }
        if !incoming {
            return None;
        }
    }
    let edge = headers.iter().position(|(name, value)| name.eq_ignore_ascii_case("Received") && is_exchange_online_edge(value))?;
    let own = headers.get(..edge)?.iter().position(|(name, value)| {
        name.eq_ignore_ascii_case(RESULTS) && (without_id(value) || authserv_id(value).is_some_and(|id| id == EXCHANGE_ONLINE_ID))
    })?;
    Some(headers.get(own..)?.iter().filter(|(name, _)| name.eq_ignore_ascii_case(RESULTS)).map(|(_, value)| value.as_str()).collect())
}

/// Whether the topmost `Authentication-Results` header of `m` was written by `account`'s own
/// server, for an account not on Exchange Online ([`exchange_online_results`]): one whose
/// authserv-id is a known id of its IMAP host's provider (`provider::authserv_ids`), or the IMAP
/// host's domain one label up, or a name under that. Anything else is not known to be the
/// account's server's.
fn from_own_server(account: &Account, value: &str) -> bool {
    let (Some(host), Some(id)) = (incoming_host(account), authserv_id(value)) else { return false };
    if provider::authserv_ids(&host).contains(&id.as_str()) {
        return true;
    }
    let Some((_, rest)) = host.split_once('.') else { return false };
    rest.split('.').count() >= 2 && !provider::is_public_suffix(rest) && (id == rest || id.strip_suffix(rest).is_some_and(|head| head.ends_with('.')))
}

/// Whether `account`'s mail is on Exchange Online: a Microsoft account, or one whose IMAP host is
/// Microsoft's.
fn on_exchange_online(account: &Account) -> bool {
    account.kind == AccountKind::Microsoft || incoming_host(account).as_deref().is_some_and(provider::is_microsoft_imap_host)
}

/// `account`'s incoming server's host, in lower case and without a trailing dot.
fn incoming_host(account: &Account) -> Option<String> {
    account.incoming.as_ref().map(|server| server.host.trim().trim_end_matches('.').to_ascii_lowercase())
}

/// Whether Exchange Online says `m` came from inside the owner's own organisation: the account is
/// on Exchange Online and has its own domain (not a consumer one such as `outlook.com`), `m` is on
/// the server, every [`AUTH_AS`] header of it says `Internal` (there is at least one, and fewer
/// than [`MAX_AUTH_AS`]) and was read by this version ([`Message::fresh_stamps`]), and the sender's
/// domain is the account's or under it ([`judged_for`]). Exchange's header firewall removes that header from mail arriving from outside and Exchange
/// writes its own, so a sender can't write it; mail between colleagues gets no DMARC verdict (`dmarc=none`), and often no
/// `Authentication-Results` at all. The cross-tenant header is not read: the sending tenant
/// writes it, and it travels on to other organisations.
fn internal(account: Option<&Account>, m: &Message) -> bool {
    let Some(account) = account.filter(|account| on_exchange_online(account)) else { return false };
    let Some(own) = domain(&account.email).filter(|own| !provider::is_mail_provider(own)) else { return false };
    if !m.fresh_stamps || (m.uid == 0 && m.server.is_none()) {
        return false;
    }
    let mut stamps = 0usize;
    for value in m.header_values(AUTH_AS) {
        if !value.trim().eq_ignore_ascii_case("Internal") {
            return false;
        }
        stamps += 1;
    }
    // As many as are kept may mean one was left out, and it may have been Exchange's.
    (1..MAX_AUTH_AS).contains(&stamps)
        && sender(&m.from.email).and_then(|address| domain(&address)).is_some_and(|sender_domain| judged_for(&own, &sender_domain))
}

/// The clauses of an `Authentication-Results` value (RFC 8601), each as its words: split at the
/// `;` that is neither in a comment (nested, with `\` quoting) nor in a quoted string, a comment
/// being a gap between words and a quoted string staying whole inside its word. What a sender
/// can put into a value (a `reason="…"`, a `(comment)`) therefore stays inside its own word.
/// The words of a comment that is not nested and follows a clause's first word are kept too,
/// each with a `(` before it (`(p=NONE`), so that no property is ever read from one. A last
/// clause cut off at [`MAX_HEADER_CHARS`], or left inside a quoted string or a comment, is
/// dropped: its last word may be the start of a longer one (`header.d=shop.example` of
/// `shop.example.evil.example`). With the clauses comes whether the whole value was read: not
/// when it was cut off, left open, or longer than [`MAX_TOKENS`] clauses.
fn clauses(value: &str) -> (Vec<Vec<String>>, bool) {
    let (mut out, mut words, mut word) = (Vec::new(), Vec::new(), String::new());
    let mut comment = String::new();
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    let mut chars = value.chars();
    for c in chars.by_ref().take(MAX_HEADER_CHARS) {
        if escaped {
            escaped = false;
            if quoted {
                word.push(c);
            }
            continue;
        }
        match c {
            '\\' if depth > 0 || quoted => escaped = true,
            _ if depth > 0 => {
                if matches!(c, '(' | ')' | ' ' | '\t' | '\r' | '\n') {
                    if !comment.is_empty() && !words.is_empty() {
                        words.push(format!("({comment}"));
                    }
                    comment.clear();
                } else if depth == 1 {
                    comment.push(c);
                }
                match c {
                    '(' => depth = depth.saturating_add(1),
                    ')' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            '"' => {
                quoted = !quoted;
                word.push(c);
            }
            _ if quoted => word.push(c),
            '(' => {
                depth = 1;
                comment.clear();
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            ';' | ' ' | '\t' | '\r' | '\n' => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                if c == ';' {
                    out.push(std::mem::take(&mut words));
                    if out.len() >= MAX_TOKENS {
                        return (out, false);
                    }
                }
            }
            _ => word.push(c),
        }
    }
    if chars.next().is_some() || quoted || depth > 0 {
        return (out, false);
    }
    if !word.is_empty() {
        words.push(word);
    }
    out.push(words);
    (out, true)
}

/// The `Authentication-Results` headers of `m`, the server's own first, when `m` is on the server
/// (it has a UID there, or the server's own record of it: mail brought in from a file has the
/// file's word and nothing else) and the server of `account` wrote one: on Exchange Online the
/// one [`exchange_online_results`] finds, elsewhere the topmost, when [`from_own_server`] says it
/// is the server's: the receiving server puts its own on top, and the ones below were written
/// before it. `None` otherwise: nothing to go by.
fn own_results<'a>(account: Option<&Account>, m: &'a Message, for_failure: bool) -> Option<Vec<&'a str>> {
    if m.uid == 0 && m.server.is_none() {
        return None;
    }
    let account = account?;
    if on_exchange_online(account) {
        return exchange_online_results(m, for_failure);
    }
    let results: Vec<&str> = m.header_values(RESULTS).collect();
    from_own_server(account, results.first()?).then_some(results)
}

/// The clauses of the topmost `Authentication-Results` header of `m`, as [`own_results`] has it.
fn own_clauses(account: Option<&Account>, m: &Message) -> Option<Vec<Vec<String>>> {
    let results = own_results(account, m, true)?;
    Some(clauses(results.first()?).0)
}

/// The value of the property `key` (`header.from`, `smtp.mailfrom`) in a clause's words, with any
/// quotes taken off.
fn property<'a>(words: &'a [String], key: &str) -> Option<&'a str> {
    words.iter().skip(1).find_map(|word| word.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.trim_matches('"')))
}

/// The method and result a clause starts with (`dmarc`, `pass`), in lower case.
fn result(words: &[String]) -> Option<(String, String)> {
    let (method, result) = words.first()?.split_once('=')?;
    Some((method.to_ascii_lowercase(), result.to_ascii_lowercase()))
}

/// Whether the server that received `m` vouches that it came from the domain it says it did:
/// `m` is on the server; the topmost `Authentication-Results` header, written by the server of
/// the account `m` is in ([`own_results`]), has a clause whose result is `dmarc=pass` (or
/// Exchange Online's `compauth=pass`) and that clause names the `header.from` it judged, which
/// is the sender's domain or one above it ([`judged_for`]). When the server gave no DMARC verdict
/// ([`dmarc_verdict`]) in that header or in another with the same authserv-id (a DMARC filter
/// may write its own beside a DKIM filter's), a clause whose result is `dkim=pass` counts
/// instead, by the signer's `header.d` (or, when the clause names none, the domain of its
/// `header.i`, which the verifier checks is `header.d` or under it: RFC 6376, 6.1.1) under the
/// same rule: a signature of the sender's domain that passes is what makes DMARC pass (RFC 7489,
/// 3.1.1 and 4.2). A verdict the server gave, `fail` included, is never overruled by a signature.
/// Only the clause's own words are read, not what a comment or quoted string in the header says.
/// A header the sender could have written is not read. The sender's address must be plain ASCII.
/// Mail Exchange Online says came from inside the owner's own organisation ([`internal`]) is
/// vouched for too, unless the server says it failed its checks ([`failed_check`]).
pub fn vouched(account: Option<&Account>, m: &Message) -> bool {
    if internal(account, m) {
        // An organisation's connector may call a partner's mail internal: the server's word wins,
        // and a word cut short may have hidden a failure.
        return failed_check(account, m).is_none()
            && own_results(account, m, true).is_none_or(|results| results.first().is_none_or(|v| clauses(v).1));
    }
    let Some(results) = own_results(account, m, false) else { return false };
    let Some((value, below)) = results.split_first() else { return false };
    let Some(sender_domain) = sender(&m.from.email).and_then(|address| domain(&address)) else { return false };
    let id = authserv_id(value);
    // A header below with the server's id may be forged, but it can only take the signature away.
    let verdict_given = dmarc_verdict(value) || below.iter().any(|other| authserv_id(other) == id && dmarc_verdict(other));
    let (parsed, _) = clauses(value);
    parsed.iter().any(|words| {
        let Some(result) = words.first() else { return false };
        let names = |wanted: &str| words.iter().any(|word| word.split_once('=').is_some_and(|(key, _)| key.eq_ignore_ascii_case(wanted)));
        let property = if result.eq_ignore_ascii_case("dmarc=pass") || result.eq_ignore_ascii_case("compauth=pass") {
            "header.from"
        } else if !verdict_given && result.eq_ignore_ascii_case("dkim=pass") {
            if names("header.d") { "header.d" } else { "header.i" }
        } else {
            return false;
        };
        words.iter().skip(1).any(|word| {
            word.split_once('=').is_some_and(|(key, name)| {
                key.eq_ignore_ascii_case(property) && domain(name.trim_matches('"')).is_some_and(|judged| judged_for(&judged, &sender_domain))
            })
        })
    })
}

/// Whether the `Authentication-Results` value `value` gives a DMARC verdict: a `dmarc` (or
/// `compauth`) clause of any result, or a part not read ([`clauses`]) where one may stand.
fn dmarc_verdict(value: &str) -> bool {
    let (parsed, whole) = clauses(value);
    !whole
        || parsed.iter().any(|words| {
            words.first().is_some_and(|result| {
                let method = result.split(['/', '=']).next().unwrap_or_default();
                method.eq_ignore_ascii_case("dmarc") || method.eq_ignore_ascii_case("compauth")
            })
        })
}

/// Which check the server says the sender failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failed {
    Dmarc,
    /// DMARC, for a domain whose policy (`p=none`) asks for nothing to be done about mail that
    /// fails: a caution, not a warning.
    DmarcPolicyNone,
    /// SPF for the sender's domain, when the server gave no DMARC verdict.
    Spf,
}

impl Failed {
    /// The method's name as RFC 8601 writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Failed::Dmarc | Failed::DmarcPolicyNone => "dmarc",
            Failed::Spf => "spf",
        }
    }
}

/// Whether the account's own server says the sender of `m` failed its checks: its topmost
/// `Authentication-Results` ([`own_results`]) has `dmarc=fail` for the sender's domain (a clause
/// that names no `header.from` judged the `From:` it had, which is the sender's), or, when it has
/// no `dmarc` clause at all, `spf=fail` for a `smtp.mailfrom` at the sender's domain or one under
/// the domain it judged; a DMARC failure for a domain whose policy is `p=none` is
/// [`Failed::DmarcPolicyNone`]. A server that says nothing, or says it of another domain, gives `None`,
/// and so does Exchange Online's `compauth=pass` for the sender, which is its last word.
pub fn failed_check(account: Option<&Account>, m: &Message) -> Option<Failed> {
    let clauses = own_clauses(account, m)?;
    // Unlike `vouched`, any address is read: a sender whose name is written in other letters
    // (`itd\u{435}sk@lumen.example`) must not slip by for it. Domains compare as letters, with
    // `xn--` labels read back into them, so `header.from` may be written either way.
    let sender_domain = sender_warnings::sender_domain(&m.from.email)?;
    let judged =
        |words: &[String], key: &str| property(words, key).map(|v| sender_warnings::sender_domain(v).or_else(|| sender_warnings::normal_domain(v)));
    let of_sender = |words: &[String]| judged(words, "header.from").is_none_or(|j| j.is_some_and(|j| judged_for(&j, &sender_domain)));
    // Exchange Online's composite verdict overrides a DMARC failure it chose to let through.
    if clauses.iter().any(|words| result(words).is_some_and(|(method, outcome)| method == "compauth" && outcome == "pass") && of_sender(words)) {
        return None;
    }
    let mut dmarc_said = false;
    for words in &clauses {
        let Some((method, outcome)) = result(words) else { continue };
        if method != "dmarc" {
            continue;
        }
        dmarc_said = true;
        if outcome == "fail" && of_sender(words) {
            return Some(if policy_none(words) { Failed::DmarcPolicyNone } else { Failed::Dmarc });
        }
    }
    if dmarc_said {
        return None;
    }
    let spf_fails = clauses.iter().any(|words| {
        result(words).is_some_and(|(method, outcome)| method == "spf" && outcome == "fail")
            && judged(words, "smtp.mailfrom").flatten().is_some_and(|j| j == sender_domain || judged_for(&sender_domain, &j))
    });
    spf_fails.then_some(Failed::Spf)
}

/// Whether a `dmarc` clause says the domain's policy is `none`: in the server's comment
/// (`(p=NONE sp=NONE dis=NONE)`) or the registered property for it (`policy.dmarc`, or DMARCbis's
/// `policy.published-domain-policy`). Another policy, or none said, is not. Exchange Online's
/// `action=none` says what it did with the message, not the domain's policy, and is not read:
/// that keeps the warning red, which is the safe way to be wrong.
fn policy_none(words: &[String]) -> bool {
    words.iter().skip(1).any(|word| {
        let word = word.to_ascii_lowercase();
        word == "(p=none"
            || word
                .split_once('=')
                .is_some_and(|(key, value)| matches!(key, "policy.dmarc" | "policy.published-domain-policy") && value.trim_matches('"') == "none")
    })
}

/// Whether a verdict on the domain `judged` stands for mail from `sender_domain`: the same name,
/// or a name under it, as Gmail names the organisation's domain (`shop.example` for mail from
/// `news.shop.example`) and as an organisation signs for its names; never a bare suffix such as
/// `co.uk`, which is nobody's organisation.
fn judged_for(judged: &str, sender_domain: &str) -> bool {
    judged == sender_domain
        || (judged.contains('.') && !provider::is_public_suffix(judged) && sender_domain.strip_suffix(judged).is_some_and(|head| head.ends_with('.')))
}
