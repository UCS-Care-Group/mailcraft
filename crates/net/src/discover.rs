// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The servers of an address at a domain nobody here knows (#1063), found once the owner has
//! pressed Connect: first the domain's mail exchangers ([`lookup_mx`](crate::lookup_mx)), by
//! which a provider may be known; when they say nothing, the SRV records of RFC 6186
//! (`_imaps._tcp`, `_submissions._tcp`, and the STARTTLS pair `_imap._tcp`, `_submission._tcp`
//! behind them), asked of the same DNS-over-HTTPS name servers.
//!
//! Only the domain goes out, never a mailbox name, and nothing is sent to the servers found:
//! what comes back is shown in Server settings for the owner to confirm, and a host outside
//! the owner's domain is named to them before a password goes there. Implicit TLS is preferred
//! (RFC 8314): a server found only under `_imap._tcp` or `_submission._tcp` is used with
//! STARTTLS, and a missing STARTTLS offer is an error at connection time, never plain text.
//! A target of `.` says the service is not offered; the records read are capped by the
//! lookup. Everything from the name server is hostile and read as such ([`dns_srv`](crate::dns_srv)).

use mailcraft_model::provider::{self, Discovered, FoundServer, Via};
use mailcraft_model::{Security, Server};

use crate::NetError;
use crate::dns::{RESOLVERS, lookup_mx_at};
use crate::dns_srv::lookup_srv_at;

/// The services of RFC 6186, implicit TLS first (RFC 8314), each with the security it stands for.
const IMAP_SERVICES: [(&str, Security); 2] = [("_imaps._tcp", Security::Tls), ("_imap._tcp", Security::StartTls)];
const SMTP_SERVICES: [(&str, Security); 2] = [("_submissions._tcp", Security::Tls), ("_submission._tcp", Security::StartTls)];

/// What is known of `domain`'s mail servers, asked for over HTTPS of a public name server
/// (Cloudflare's; Google's when that one fails). The exchangers alone when they name a provider
/// this program knows (`provider::by_mx`): nothing more is asked then. Blocks, for some
/// seconds at most per request.
pub fn discover(domain: &str) -> Result<Discovered, NetError> {
    discover_at(&RESOLVERS, domain)
}

/// [`discover`], asking the name servers at `endpoints` instead (`https://` addresses, or
/// `http://` ones on this machine).
pub fn discover_at(endpoints: &[&str], domain: &str) -> Result<Discovered, NetError> {
    // The exchangers come first: a name that isn't a domain's is refused here, before anything
    // goes out, and a name server that can't be reached ends the search.
    let exchangers = lookup_mx_at(endpoints, domain)?;
    if exchangers.iter().any(|host| provider::by_mx(host).is_some()) {
        return Ok(Discovered::from_exchangers(exchangers));
    }
    let mut found = Discovered::from_exchangers(exchangers);
    found.incoming = srv(endpoints, domain, &IMAP_SERVICES);
    found.outgoing = srv(endpoints, domain, &SMTP_SERVICES);
    Ok(found)
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

#[cfg(test)]
#[path = "discover_tests.rs"]
mod discover_tests;
