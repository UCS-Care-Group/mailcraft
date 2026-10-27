// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where well-known mail providers keep their servers and how their owners sign in, worked out
//! from an address's domain or from the domain's mail exchangers.
//!
//! Plain data: nothing is looked up here. A host with sockets asks the DNS for the exchangers and
//! passes the answer in, which is how an address at a company's own domain is recognised as
//! (say) Microsoft 365. The server names are the ones each provider publishes.

use crate::{OAuthProvider, Security, Server};

/// What an account at a provider usually takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preset {
    /// The provider's name (`Microsoft 365`); empty for a guess.
    pub service: &'static str,
    /// Set when the owner signs in on the provider's own page instead of with a password.
    pub oauth: Option<OAuthProvider>,
    pub incoming: Server,
    pub outgoing: Server,
    /// Something the owner should know before connecting; may be empty.
    pub note: &'static str,
}

const APP_PASSWORD: &str = "This provider wants an app password (made in the account's security settings), not the everyday one.";
const MICROSOFT: &str = "Microsoft accounts sign in on Microsoft's own page; no password is typed here.";

fn server(host: &str, port: u16, security: Security) -> Server {
    Server { host: host.to_string(), port, security }
}

fn with_password(service: &'static str, imap: &str, smtp: (&str, u16, Security), note: &'static str) -> Preset {
    Preset { service, oauth: None, incoming: server(imap, 993, Security::Tls), outgoing: server(smtp.0, smtp.1, smtp.2), note }
}

/// Outlook.com (`consumer`) or Microsoft 365 work and school mail.
pub fn microsoft(consumer: bool) -> Preset {
    // Outlook.com sends through its own host; work and school mail through Office 365's.
    let (service, smtp) = if consumer { ("Outlook.com", "smtp-mail.outlook.com") } else { ("Microsoft 365", "smtp.office365.com") };
    Preset {
        service,
        oauth: Some(OAuthProvider::Microsoft),
        incoming: server("outlook.office365.com", 993, Security::Tls),
        outgoing: server(smtp, 587, Security::StartTls),
        note: MICROSOFT,
    }
}

/// Whether `host` is the IMAP server of Microsoft's own mail (Exchange Online, Outlook.com).
pub fn is_microsoft_imap_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    matches!(host.as_str(), "outlook.office365.com" | "outlook.office.com" | "imap-mail.outlook.com")
}

/// The `Authentication-Results` authserv-ids (RFC 8601) that a provider's own servers write,
/// by the IMAP host of an account there; empty where none is known for certain.
///
/// Only what the provider publishes: Google documents `mx.google.com` as the authserv-id of the
/// header Gmail adds (Gmail help, "Trace an email with its full headers"), and Gmail and Google
/// Workspace share `imap.gmail.com`. Other providers are left out until their ids are
/// documented: for them (and for an unknown host) the caller falls back to the IMAP host's own
/// domain, or asks the owner.
pub fn authserv_ids(imap_host: &str) -> &'static [&'static str] {
    match imap_host.trim().trim_end_matches('.').to_ascii_lowercase().as_str() {
        "imap.gmail.com" => &["mx.google.com"],
        _ => &[],
    }
}

/// A host name fit to look up or connect to: letters, digits, dots and dashes, with a dot in it.
pub fn is_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|label| !label.is_empty() && label.len() <= 63 && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// The second level under a country's domain that is a suffix of its own (`co.uk`, `com.au`).
const COUNTRY_SECOND_LEVELS: &[&str] = &["co", "com", "net", "org", "gov", "edu", "ac", "or", "ne", "go", "me"];

/// Hosting platforms that give each customer a name under their own (`contoso.sharepoint.com`).
const HOSTING_PLATFORMS: &[&str] = &[
    "onmicrosoft.com",
    "sharepoint.com",
    "azurewebsites.net",
    "cloudapp.net",
    "github.io",
    "gitlab.io",
    "blogspot.com",
    "wordpress.com",
    "herokuapp.com",
    "netlify.app",
    "vercel.app",
    "pages.dev",
    "web.app",
    "firebaseapp.com",
    "appspot.com",
    // Help desks and shops that send their customers' mail from a name of their own.
    "zendesk.com",
    "freshdesk.com",
    "atlassian.net",
    "service-now.com",
    "myshopify.com",
];

/// Whether `domain` (in lower case) is a suffix anybody may have a name of their own under: a
/// country's second level (`co.uk`) or a hosting platform (`github.io`). A name under it is
/// somebody's; the suffix is nobody's.
// shortcut: the common ones, not the Public Suffix List; take that list if sites or servers come out wrong.
pub fn is_public_suffix(domain: &str) -> bool {
    HOSTING_PLATFORMS.contains(&domain)
        || domain
            .split_once('.')
            .is_some_and(|(second, top)| top.len() == 2 && top.bytes().all(|b| b.is_ascii_lowercase()) && COUNTRY_SECOND_LEVELS.contains(&second))
}

/// Whether `domain` (in lower case) is a hosting platform's or a name its customers have under
/// it: what it serves there, its icon among the rest, is the platform's as much as theirs.
pub fn on_hosting_platform(domain: &str) -> bool {
    HOSTING_PLATFORMS.iter().any(|platform| domain.strip_suffix(platform).is_some_and(|head| head.is_empty() || head.ends_with('.')))
}

/// Mail anybody can have an address at, besides the providers [`by_domain`] knows; and Apple's
/// relay for hidden addresses (`privaterelay.appleid.com`).
const MAIL_PROVIDERS: &[&str] = &[
    "proton.me",
    "protonmail.com",
    "pm.me",
    "tutanota.com",
    "tuta.io",
    "posteo.de",
    "mailbox.org",
    "hey.com",
    "duck.com",
    "appleid.com",
    "web.de",
    "t-online.de",
    "freenet.de",
    "orange.fr",
    "free.fr",
    "laposte.net",
    "libero.it",
    "seznam.cz",
    "wp.pl",
    "mail.ru",
    "yandex.ru",
    "yandex.com",
    "qq.com",
    "163.com",
    "126.com",
    "naver.com",
    "comcast.net",
    "att.net",
    "verizon.net",
    "btinternet.com",
    "email.com",
];

/// Whether `domain` (in lower case) is a mail provider's, where anybody can have an address:
/// its icon says nothing of the sender.
// shortcut: the providers of most addresses, not every one; add one when its logo turns up as a sender's picture.
pub fn is_mail_provider(domain: &str) -> bool {
    MAIL_PROVIDERS.contains(&domain) || by_domain(domain).is_some()
}

/// Mail Microsoft hosts for anyone: Outlook.com and its older names, in every country
/// (`outlook.com`, `hotmail.co.uk`, `live.com.au`; not `live.somecompany.example`).
fn is_microsoft_consumer(domain: &str) -> bool {
    let mut labels = domain.split('.');
    let first = labels.next().unwrap_or_default();
    let rest: Vec<&str> = labels.collect();
    matches!(first, "outlook" | "hotmail" | "live" | "msn") && (1..=2).contains(&rest.len()) && rest.iter().all(|l| (2..=3).contains(&l.len()))
}

/// From the address's domain alone (`gmail.com`). `None` for a domain that isn't a provider's own.
pub fn by_domain(domain: &str) -> Option<Preset> {
    let domain = domain.trim().to_ascii_lowercase();
    if !is_domain(&domain) {
        return None;
    }
    if is_microsoft_consumer(&domain) {
        return Some(microsoft(true));
    }
    let (tls, starttls) = (Security::Tls, Security::StartTls);
    Some(match domain.as_str() {
        "gmail.com" | "googlemail.com" => with_password("Gmail", "imap.gmail.com", ("smtp.gmail.com", 465, tls), APP_PASSWORD),
        "icloud.com" | "me.com" | "mac.com" => with_password("iCloud", "imap.mail.me.com", ("smtp.mail.me.com", 587, starttls), APP_PASSWORD),
        "fastmail.com" | "fastmail.fm" => with_password("Fastmail", "imap.fastmail.com", ("smtp.fastmail.com", 465, tls), APP_PASSWORD),
        "aol.com" => with_password("AOL", "imap.aol.com", ("smtp.aol.com", 465, tls), APP_PASSWORD),
        "zoho.com" | "zohomail.com" => with_password("Zoho", "imap.zoho.com", ("smtp.zoho.com", 465, tls), ""),
        "gmx.com" | "gmx.us" => with_password("GMX", "imap.gmx.com", ("mail.gmx.com", 587, starttls), ""),
        "gmx.net" | "gmx.de" | "gmx.at" | "gmx.ch" => with_password("GMX", "imap.gmx.net", ("mail.gmx.net", 587, starttls), ""),
        "mail.com" => with_password("mail.com", "imap.mail.com", ("smtp.mail.com", 587, starttls), ""),
        d if d == "ymail.com" || d == "rocketmail.com" || d.starts_with("yahoo.") => {
            with_password("Yahoo", "imap.mail.yahoo.com", ("smtp.mail.yahoo.com", 465, tls), APP_PASSWORD)
        }
        _ => return None,
    })
}

/// From one of the domain's mail exchangers (`example-org.mail.protection.outlook.com`): who
/// hosts the mail of a domain that isn't the provider's own.
pub fn by_mx(host: &str) -> Option<Preset> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if !is_domain(&host) {
        return None;
    }
    // The host itself or anything under it, never a look-alike (`evilgoogle.com`).
    let under = |parent: &str| host == parent || host.ends_with(&format!(".{parent}"));
    let (tls, starttls) = (Security::Tls, Security::StartTls);
    if under("olc.protection.outlook.com") {
        return Some(microsoft(true));
    }
    if under("mail.protection.outlook.com") || under("mx.microsoft") {
        return Some(microsoft(false));
    }
    if under("google.com") || under("googlemail.com") {
        return Some(with_password("Google Workspace", "imap.gmail.com", ("smtp.gmail.com", 465, tls), APP_PASSWORD));
    }
    if under("yahoodns.net") {
        return Some(with_password("Yahoo", "imap.mail.yahoo.com", ("smtp.mail.yahoo.com", 465, tls), APP_PASSWORD));
    }
    if under("mail.icloud.com") {
        return Some(with_password("iCloud", "imap.mail.me.com", ("smtp.mail.me.com", 587, starttls), APP_PASSWORD));
    }
    if under("messagingengine.com") {
        return Some(with_password("Fastmail", "imap.fastmail.com", ("smtp.fastmail.com", 465, tls), APP_PASSWORD));
    }
    if under("zoho.com") {
        return Some(with_password("Zoho", "imap.zoho.com", ("smtp.zoho.com", 465, tls), ""));
    }
    None
}

/// From the provider's name, for an owner who says whose the account is (`microsoft365`,
/// `outlook.com`, `google`, `yahoo`, `icloud`, `fastmail`).
pub fn by_name(name: &str) -> Option<Preset> {
    let name: String = name.trim().to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    match name.as_str() {
        "microsoft" | "microsoft365" | "office365" | "m365" | "exchange" => Some(microsoft(false)),
        "outlook" | "outlookcom" | "hotmail" | "live" => Some(microsoft(true)),
        "google" | "gmail" | "googleworkspace" => by_domain("gmail.com"),
        "yahoo" => by_domain("yahoo.com"),
        "icloud" | "apple" => by_domain("icloud.com"),
        "fastmail" => by_domain("fastmail.com"),
        "aol" => by_domain("aol.com"),
        "zoho" => by_domain("zoho.com"),
        _ => None,
    }
}

/// The usual names at a domain nobody here has heard of, for the owner to correct.
pub fn guess(domain: &str) -> Preset {
    let domain = domain.trim().to_ascii_lowercase();
    Preset {
        service: "",
        oauth: None,
        incoming: server(&format!("imap.{domain}"), 993, Security::Tls),
        outgoing: server(&format!("smtp.{domain}"), 587, Security::StartTls),
        note: "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_providers_own_domains() {
        for domain in ["outlook.com", "Hotmail.co.uk", "live.com.au", "msn.com", " outlook.de "] {
            let p = by_domain(domain).unwrap();
            assert_eq!(
                (p.service, p.oauth, p.outgoing.host.as_str()),
                ("Outlook.com", Some(OAuthProvider::Microsoft), "smtp-mail.outlook.com"),
                "{domain}"
            );
        }
        assert_eq!(by_domain("gmail.com").unwrap().incoming, server("imap.gmail.com", 993, Security::Tls));
        assert_eq!(by_domain("yahoo.co.uk").unwrap().service, "Yahoo");
        assert_eq!(by_domain("gmx.net").unwrap().outgoing, server("mail.gmx.net", 587, Security::StartTls));
        // Someone else's domain that merely starts like one of them, and things that aren't domains.
        for domain in ["live.somecompany.example", "outlook.example.org", "hotmail", "lumen.example", "", "a b.com", "evil/../x.com"] {
            assert_eq!(by_domain(domain), None, "{domain}");
        }
    }

    #[test]
    fn a_companys_domain_is_known_by_who_takes_its_mail() {
        let work = by_mx("example-org.mail.protection.outlook.com.").unwrap();
        assert_eq!((work.service, work.oauth), ("Microsoft 365", Some(OAuthProvider::Microsoft)));
        assert_eq!(
            (work.incoming.host.as_str(), work.outgoing.host.as_str(), work.outgoing.port),
            ("outlook.office365.com", "smtp.office365.com", 587)
        );
        assert_eq!(by_mx("example-org.h-v1.mx.microsoft").unwrap().service, "Microsoft 365");
        assert_eq!(by_mx("OUTLOOK-COM.olc.protection.outlook.com").unwrap().service, "Outlook.com");
        assert_eq!(by_mx("aspmx.l.google.com").unwrap().service, "Google Workspace");
        assert_eq!(by_mx("in1-smtp.messagingengine.com").unwrap().service, "Fastmail");
        assert_eq!(by_mx("mx01.mail.icloud.com").unwrap().service, "iCloud");
        // A look-alike is not the provider, nor is a host that isn't one.
        for host in ["evilgoogle.com", "mail.protection.outlook.com.evil.example", "mx.lumen.example", "", "a b", "x/../mx.microsoft"] {
            assert_eq!(by_mx(host), None, "{host}");
        }
    }

    #[test]
    fn a_provider_by_name() {
        assert_eq!(by_name(" Microsoft 365 ").unwrap().service, "Microsoft 365");
        assert_eq!(by_name("Outlook.com").unwrap().service, "Outlook.com");
        assert_eq!(by_name("google").unwrap().incoming.host, "imap.gmail.com");
        assert_eq!(by_name("iCloud").unwrap().service, "iCloud");
        assert_eq!(by_name("nobody"), None);
        assert_eq!(by_name(""), None);
    }

    #[test]
    fn a_guess_uses_the_usual_names() {
        let p = guess("Lumen.Example");
        assert_eq!((p.service, p.incoming.host.as_str(), p.outgoing.host.as_str()), ("", "imap.lumen.example", "smtp.lumen.example"));
        assert!(is_domain("mail.lumen.example") && !is_domain("localhost") && !is_domain(&format!("{}.example", "a".repeat(64))));
    }
}
