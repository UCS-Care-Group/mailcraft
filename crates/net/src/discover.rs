// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The servers of an address at a domain nobody here knows (#1063), found once the owner has
//! pressed Connect, each step only for what the steps before it didn't find:
//!
//! 1. the domain's mail exchangers ([`lookup_mx`](crate::lookup_mx)), by which a provider may
//!    be known (`provider::by_mx`); nothing more is asked then;
//! 2. the SRV records of RFC 6186 (`_imaps._tcp`, `_submissions._tcp`, and the STARTTLS pair
//!    `_imap._tcp`, `_submission._tcp` behind them), asked of the same DNS-over-HTTPS name
//!    servers: nobody new is told the domain;
//! 3. what the domain publishes for mail programs ([`autoconfig`](crate::autoconfig)), at
//!    `https://autoconfig.<domain>/mail/config-v1.1.xml`, then
//!    `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`: the domain's own servers
//!    are told that somebody is setting its mail up, no more;
//! 4. Mozilla's ISP database, `https://autoconfig.thunderbird.net/v1.1/<domain>`, told the
//!    domain, and then, when the exchangers are in another domain (a hosting company's), that
//!    domain: the one step that tells a third party the domain, so the last.
//!
//! Only the domain goes out, never a mailbox name, and nothing is sent to the servers found:
//! what comes back is shown in Server settings for the owner to confirm, and a host outside
//! the owner's domain is named to them before a password goes there. Implicit TLS is preferred
//! (RFC 8314): a server found only under `_imap._tcp` or `_submission._tcp`, or listed with
//! `STARTTLS`, is used with STARTTLS, and a missing STARTTLS offer is an error at connection
//! time, never plain text; a server listed without encryption is never taken. An SRV target
//! of `.` says the service is not offered; the records read are capped by the lookup, the
//! documents by the HTTP layer and the reader. Everything from outside is hostile and read as
//! such ([`dns_srv`](crate::dns_srv), [`autoconfig`](crate::autoconfig)).

use std::time::{Duration, Instant};

use mailcraft_model::provider::{self, Discovered, FoundServer, Via};
use mailcraft_model::{Security, Server};

use crate::NetError;
use crate::autoconfig;
use crate::dns::{RESOLVERS, domain_name, lookup_mx_at};
use crate::dns_srv::lookup_srv_at;

/// The services of RFC 6186, implicit TLS first (RFC 8314), each with the security it stands for.
const IMAP_SERVICES: [(&str, Security); 2] = [("_imaps._tcp", Security::Tls), ("_imap._tcp", Security::StartTls)];
const SMTP_SERVICES: [(&str, Security); 2] = [("_submissions._tcp", Security::Tls), ("_submission._tcp", Security::StartTls)];
/// How long the whole search may take before the steps left are skipped: a domain whose hosts
/// swallow connections must not keep the owner waiting a minute.
const BUDGET: Duration = Duration::from_secs(30);

/// Where the search looks: the name servers, the addresses of a domain's own autoconfig
/// document (`{domain}` standing for the domain), and the ISP database (the domain goes on the
/// end). The real places are [`PLACES`]; a test names its own.
#[derive(Clone, Copy, Debug)]
pub struct Places<'a> {
    pub resolvers: &'a [&'a str],
    pub autoconfig: &'a [&'a str],
    pub ispdb: Option<&'a str>,
}

/// The real places: public name servers, the domain's own hosts and Mozilla's database.
pub const PLACES: Places<'static> = Places {
    resolvers: &RESOLVERS,
    autoconfig: &["https://autoconfig.{domain}/mail/config-v1.1.xml", "https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"],
    ispdb: Some("https://autoconfig.thunderbird.net/v1.1/"),
};

/// What is known of `domain`'s mail servers, asked for at the real [`PLACES`]: the exchangers
/// alone when they name a provider this program knows, else what the steps above found.
/// Blocks, for some seconds at most per request and half a minute in all.
pub fn discover(domain: &str) -> Result<Discovered, NetError> {
    discover_at(&PLACES, domain)
}

/// [`discover`], looking at `places` instead (`https://` addresses, or `http://` ones on this
/// machine).
pub fn discover_at(places: &Places<'_>, domain: &str) -> Result<Discovered, NetError> {
    let started = Instant::now();
    // The exchangers come first: a name that isn't a domain's is refused here, before anything
    // goes out, and a name server that can't be reached ends the search.
    let exchangers = lookup_mx_at(places.resolvers, domain)?;
    if exchangers.iter().any(|host| provider::by_mx(host).is_some()) {
        return Ok(Discovered::from_exchangers(exchangers));
    }
    let Some(domain) = domain_name(domain) else { return Ok(Discovered::from_exchangers(exchangers)) };
    let mut found = Discovered::from_exchangers(exchangers);
    found.incoming = srv(places.resolvers, &domain, &IMAP_SERVICES);
    found.outgoing = srv(places.resolvers, &domain, &SMTP_SERVICES);
    let mut documents: Vec<(String, Via)> =
        places.autoconfig.iter().map(|template| (template.replace("{domain}", &domain), Via::Autoconfig)).collect();
    if let Some(ispdb) = places.ispdb {
        documents.push((format!("{ispdb}{domain}"), Via::Ispdb));
        // The exchangers' own domain, when it is another: a hosting company's, which the
        // database may know by its name.
        if let Some(hoster) = found.exchangers.first().and_then(|host| base_domain(host)).filter(|hoster| *hoster != domain) {
            documents.push((format!("{ispdb}{hoster}"), Via::Ispdb));
        }
    }
    for (url, via) in documents {
        if found.incoming.is_some() && found.outgoing.is_some() {
            break;
        }
        // What is left of the search's time is all a fetch gets, for connecting and for the answer.
        let left = BUDGET.saturating_sub(started.elapsed());
        if left.is_zero() {
            log::debug!("the search for the servers took too long: the {} step is skipped", via.code());
            break;
        }
        match autoconfig::fetch(&url, autoconfig::PATIENCE.min(left)) {
            Ok(Some(config)) => take(&mut found, config, via),
            Ok(None) => {}
            Err(e) => log::debug!("a settings document couldn't be fetched ({}): {e}", via.code()),
        }
    }
    Ok(found)
}

/// Fill what is still missing from a document's servers. Its user name is its incoming
/// server's and goes along only when that server is the one taken: an account has one user
/// name, the one it reads its mail with. With an incoming server from anywhere else (the SRV
/// records name none, RFC 6186), or from a document that says none, the address stays the
/// user name, whatever a document says of its outgoing server.
fn take(found: &mut Discovered, config: autoconfig::ClientConfig, via: Via) {
    if found.incoming.is_none()
        && let Some(server) = config.incoming
    {
        found.incoming = Some(FoundServer { server, via });
        found.username = config.username;
    }
    if found.outgoing.is_none()
        && let Some(server) = config.outgoing
    {
        found.outgoing = Some(FoundServer { server, via });
    }
}

/// The best place one of `services` is offered at, the first service that has one winning.
/// A lookup that fails is a lookup that found nothing: the search goes on.
fn srv(endpoints: &[&str], domain: &str, services: &[(&str, Security)]) -> Option<FoundServer> {
    for (service, security) in services {
        match lookup_srv_at(endpoints, service, domain) {
            Ok(targets) => {
                // (A port of 0 is no port to connect to.)
                if let Some(target) = targets.into_iter().find(|target| target.port != 0) {
                    return Some(FoundServer { server: Server { host: target.host, port: target.port, security: *security }, via: Via::Srv });
                }
            }
            Err(e) => log::debug!("the {service} record couldn't be looked up: {e}"),
        }
    }
    None
}

/// The domain a host is somebody's name under: its last two labels, or three under a suffix
/// anybody may have a name under (`co.uk`): `hosting.example` for `mx1.hosting.example`.
fn base_domain(host: &str) -> Option<String> {
    let labels: Vec<&str> = host.split('.').collect();
    let last = |n: usize| labels.get(labels.len().checked_sub(n)?..).map(|tail| tail.join("."));
    let two = last(2)?;
    let base = if provider::is_public_suffix(&two) { last(3)? } else { two };
    provider::is_domain(&base).then_some(base)
}

#[cfg(test)]
#[path = "discover_tests.rs"]
mod discover_tests;
