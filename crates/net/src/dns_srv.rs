// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a domain's Autodiscover is, by its `_autodiscover._tcp.<domain>` SRV record (RFC 2782;
//! MS-OXDISCO §3.1.5.3), and where its mail servers are, by the SRV records of RFC 6186
//! ([`discover`](crate::discover)), asked of the same DNS-over-HTTPS name servers as
//! [`lookup_mx`](crate::lookup_mx). Only the domain goes out (inside the fixed name of the
//! service), never a mailbox name, and only when the owner has pressed Connect.
//!
//! [`autodiscover_hosts`] asks for the Autodiscover record and one more question of the DNS, the
//! alias of `autodiscover.<domain>`, so that Add Account can tell an address on an Exchange server
//! from one on an IMAP server before anything is sent to any of the hosts named.
//!
//! The answer is a name server's JSON and as hostile as anything from the network: its size is
//! capped by the HTTP layer, the records looked at and the hosts handed back are capped, and a
//! target that isn't a plain host name is passed over.

use mailcraft_proto::ProtoError;
use serde_json::Value;

use crate::NetError;
use crate::dns::{LOOKUP_TIMEOUT, MAX_RECORDS, MAX_RESOLVERS, NO_ERROR, NO_SUCH_DOMAIN, RESOLVERS, domain_name};
use crate::http::{self, Answer, Url};

/// Most hosts handed back.
const MAX_HOSTS: usize = 8;
/// The record type of a service location.
const SRV: u64 = 33;
/// The service Autodiscover is found by, whatever the domain.
const AUTODISCOVER: &str = "_autodiscover._tcp";
/// The record type of an alias.
const CNAME: u64 = 5;
/// The host in the domain that Autodiscover is looked for at, and the first label of the alias
/// that is taken for it.
const ALIAS: &str = "autodiscover";

/// A place an SRV record names: the host (lower case, without the trailing dot) and the port.
#[derive(Clone, PartialEq, Eq)]
pub struct SrvTarget {
    /// The host name.
    pub host: String,
    /// The port the service listens on.
    pub port: u16,
}

/// The host is the organisation's own: not for a log.
impl std::fmt::Debug for SrvTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SrvTarget").field("port", &self.port).finish_non_exhaustive()
    }
}

/// The places that serve Autodiscover for `domain`, best first (lower priority first, then the
/// higher weight): hosts and ports. Empty when the domain has
/// no such record, or says the service isn't offered (a target of `.`). Only the domain is
/// sent, over HTTPS, to a public name server (Cloudflare's; Google's when that one fails).
/// Blocks, for some seconds at most per name server.
pub fn lookup_autodiscover(domain: &str) -> Result<Vec<SrvTarget>, NetError> {
    lookup_autodiscover_at(&RESOLVERS, domain)
}

/// [`lookup_autodiscover`], asking the name servers at `endpoints` instead (`https://` addresses,
/// or `http://` ones on this machine).
pub fn lookup_autodiscover_at(endpoints: &[&str], domain: &str) -> Result<Vec<SrvTarget>, NetError> {
    lookup_srv_at(endpoints, AUTODISCOVER, domain)
}

/// The places that serve `service` (`_imaps._tcp`, say: a label pair without the domain) for
/// `domain`, best first, as [`lookup_autodiscover_at`] gives Autodiscover's.
pub(crate) fn lookup_srv_at(endpoints: &[&str], service: &str, domain: &str) -> Result<Vec<SrvTarget>, NetError> {
    let Some(domain) = domain_name(domain) else {
        let shown: String = domain.chars().filter(|c| !c.is_control()).take(80).collect();
        return Err(NetError::Config(format!("\"{shown}\" isn't a domain that mail could go to").into()));
    };
    let name = format!("{service}.{domain}");
    let mut failure = None;
    for endpoint in endpoints.iter().take(MAX_RESOLVERS) {
        match ask(endpoint, &name) {
            Ok(hosts) => return Ok(hosts),
            Err(e) => {
                log::debug!("a name server couldn't be asked: {e}");
                failure.get_or_insert(e);
            }
        }
    }
    Err(failure.unwrap_or_else(|| NetError::Config("there is no name server to ask".into())))
}

/// The hosts a domain's DNS names for Autodiscover, best first, without anything being asked of
/// them: the targets of its `_autodiscover._tcp` SRV record, then the host `autodiscover.<domain>`
/// is an alias of, when that host is called `autodiscover` too (`autodiscover.mail.example.net`,
/// as a hoster or Microsoft 365 has it; an alias that a wildcard gives every name in the domain
/// says nothing). Empty when the domain names none; an error only when neither question could be
/// answered. Only the domain is sent, as for [`lookup_autodiscover`]. Blocks, for some seconds at
/// most per question and name server.
pub fn autodiscover_hosts(domain: &str) -> Result<Vec<String>, NetError> {
    autodiscover_hosts_at(&RESOLVERS, domain)
}

/// [`autodiscover_hosts`], asking the name servers at `endpoints` instead.
pub fn autodiscover_hosts_at(endpoints: &[&str], domain: &str) -> Result<Vec<String>, NetError> {
    let found = autodiscover_targets_at(endpoints, domain)?;
    let mut srv: Vec<String> = Vec::new();
    for host in found.srv.into_iter().map(|target| target.host) {
        if !srv.contains(&host) {
            srv.push(host);
        }
    }
    // (The SRV record's targets leave room for an alias that names a host of its own: the alias
    // may be the one that says the domain is Microsoft 365's, however many targets a record
    // names. One the record names already takes none.)
    let own = |alias: &String| !srv.iter().take(MAX_HOSTS).any(|host| host == alias);
    let srv_room = MAX_HOSTS.saturating_sub(usize::from(found.aliases.iter().any(own)));
    let mut hosts: Vec<String> = srv.into_iter().take(srv_room).collect();
    for host in found.aliases {
        if hosts.len() < MAX_HOSTS && !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    Ok(hosts)
}

/// The places [`autodiscover_hosts`] names, kept apart by what named them: the SRV record's
/// targets with the ports it gives, and the hosts `autodiscover.<domain>` is an alias of.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AutodiscoverTargets {
    /// The SRV record's targets, best first.
    pub srv: Vec<SrvTarget>,
    /// The alias's hosts (never `autodiscover.<domain>` itself), asked at 443. Not an SRV
    /// record's word: the search asks the owner before the sign-in goes to one, in the domain or
    /// out of it.
    pub aliases: Vec<String>,
}

/// The hosts are the organisation's own: not for a log.
impl std::fmt::Debug for AutodiscoverTargets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutodiscoverTargets").field("srv", &self.srv.len()).field("aliases", &self.aliases.len()).finish()
    }
}

/// The places [`autodiscover_hosts`] names, by what named them ([`AutodiscoverTargets`]). The
/// search for an Exchange server asks the aliases' hosts after the SRV record's targets, in the
/// place of MS-OXDISCO's plain-HTTP redirect (§3.1.5.4), which is never taken: a domain whose SRV
/// record is only in its own network's DNS (a split DNS) still names its hoster's Autodiscover by
/// the alias, whose host has the certificate that `autodiscover.<domain>` itself hasn't. The alias
/// step is specified in `docs/autodiscover.md`.
pub fn autodiscover_targets(domain: &str) -> Result<AutodiscoverTargets, NetError> {
    autodiscover_targets_at(&RESOLVERS, domain)
}

/// [`autodiscover_targets`], asking the name servers at `endpoints` instead.
pub fn autodiscover_targets_at(endpoints: &[&str], domain: &str) -> Result<AutodiscoverTargets, NetError> {
    let srv = lookup_autodiscover_at(endpoints, domain);
    let Some(domain) = domain_name(domain) else { return srv.map(|_| AutodiscoverTargets::default()) };
    let name = format!("{ALIAS}.{domain}");
    let mut alias = None;
    for endpoint in endpoints.iter().take(MAX_RESOLVERS) {
        match question(endpoint, &name, "CNAME").and_then(|answer| aliases(&answer)) {
            Ok(found) => {
                alias = Some(found);
                break;
            }
            Err(e) => log::debug!("a name server couldn't be asked: {e}"),
        }
    }
    let (srv, alias) = match (srv, alias) {
        (Err(e), None) => return Err(e),
        (srv, alias) => (srv.unwrap_or_default(), alias.unwrap_or_default()),
    };
    let mut found = AutodiscoverTargets::default();
    for target in srv {
        if found.srv.len() < MAX_HOSTS && !found.srv.contains(&target) {
            found.srv.push(target);
        }
    }
    // (An alias of the name to itself names nothing new.)
    for host in alias {
        if found.aliases.len() < MAX_HOSTS && host != name && !found.aliases.contains(&host) {
            found.aliases.push(host);
        }
    }
    Ok(found)
}

fn ask(endpoint: &str, name: &str) -> Result<Vec<SrvTarget>, NetError> {
    targets(&question(endpoint, name, "SRV")?)
}

/// One question of one name server: `name` and the record type.
fn question(endpoint: &str, name: &str, kind: &str) -> Result<Answer, NetError> {
    let endpoint = endpoint.trim();
    let question = http::form(&[("name", name), ("type", kind)]);
    let url = Url::parse(&format!("{endpoint}{}{question}", if endpoint.contains('?') { '&' } else { '?' }))?;
    http::get(&url, &[("Accept", "application/dns-json")], LOOKUP_TIMEOUT)
}

/// The records of a name server's answer, once its status says there are any to look at.
fn records(answer: &Answer) -> Result<Vec<Value>, NetError> {
    let nonsense = |text: String| NetError::Proto(ProtoError::Protocol(text.into()));
    if answer.status != 200 {
        return Err(nonsense(format!("the name server answered with status {}", answer.status)));
    }
    let json = serde_json::from_slice::<Value>(&answer.body).ok().filter(Value::is_object);
    let status = json.as_ref().and_then(|json| json.get("Status")).and_then(Value::as_u64);
    let (Some(json), Some(status)) = (json, status) else { return Err(nonsense("the name server's answer isn't the JSON it should be".into())) };
    match status {
        NO_ERROR => {}
        NO_SUCH_DOMAIN => return Ok(Vec::new()),
        other => return Err(NetError::Proto(ProtoError::Rejected(format!("the name server couldn't look the domain up (code {other})").into()))),
    }
    let mut records = match json {
        Value::Object(mut json) => match json.remove("Answer") {
            Some(Value::Array(records)) => records,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    records.truncate(MAX_RECORDS);
    Ok(records)
}

/// The hosts called `autodiscover` that an answer about an alias names:
/// `{"Status":0,"Answer":[{"type":5,"data":"autodiscover.mail.example.net."}]}`. Whatever else is in
/// there is passed over.
pub(crate) fn aliases(answer: &Answer) -> Result<Vec<String>, NetError> {
    let mut hosts: Vec<String> = Vec::new();
    for record in records(answer)? {
        if record.get("type").and_then(Value::as_u64) != Some(CNAME) {
            continue;
        }
        let Some(host) = record.get("data").and_then(Value::as_str).and_then(domain_name) else { continue };
        if host.split('.').next() == Some(ALIAS) && hosts.len() < MAX_HOSTS && !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    Ok(hosts)
}

/// The targets of a name server's answer: `{"Status":0,"Answer":[{"type":33,"data":"10 60 443 mail.example.org."}]}`.
/// Whatever else is in there, and whatever isn't a host name, is passed over.
pub(crate) fn targets(answer: &Answer) -> Result<Vec<SrvTarget>, NetError> {
    // (priority, weight, target)
    let mut found: Vec<(u16, u16, SrvTarget)> = Vec::new();
    for record in records(answer)? {
        if record.get("type").and_then(Value::as_u64) != Some(SRV) {
            continue;
        }
        // `10 60 443 mail.example.org.`: priority (lower is better), weight, port, target.
        let mut words = record.get("data").and_then(Value::as_str).unwrap_or("").split_whitespace();
        let number = |word: Option<&str>| word.and_then(|n| n.parse::<u16>().ok());
        let (Some(priority), Some(weight), Some(port), Some(host), None) =
            (number(words.next()), number(words.next()), number(words.next()), words.next(), words.next())
        else {
            continue;
        };
        // A target of "." says the service is not offered (RFC 2782).
        if let Some(host) = domain_name(host) {
            found.push((priority, weight, SrvTarget { host, port }));
        }
    }
    found.sort_by_key(|(priority, weight, _)| (*priority, std::cmp::Reverse(*weight)));
    let mut hosts: Vec<SrvTarget> = Vec::new();
    for (_, _, target) in found {
        if hosts.len() < MAX_HOSTS && !hosts.contains(&target) {
            hosts.push(target);
        }
    }
    Ok(hosts)
}

#[cfg(test)]
#[path = "dns_srv_tests.rs"]
mod dns_srv_tests;
