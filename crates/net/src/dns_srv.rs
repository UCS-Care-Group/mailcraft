// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a domain's Autodiscover is, by its `_autodiscover._tcp.<domain>` SRV record (RFC 2782;
//! the third place MS-OXDSCLI looks), asked of the same DNS-over-HTTPS name servers as
//! [`lookup_mx`](crate::lookup_mx). Only the domain goes out (inside the fixed name of the
//! service), never a mailbox name, and only when the owner has pressed Connect for an Exchange
//! server account.
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
/// The service asked after, whatever the domain.
const SERVICE: &str = "_autodiscover._tcp";

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
    let Some(domain) = domain_name(domain) else {
        let shown: String = domain.chars().filter(|c| !c.is_control()).take(80).collect();
        return Err(NetError::Config(format!("\"{shown}\" isn't a domain that mail could go to")));
    };
    let name = format!("{SERVICE}.{domain}");
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

fn ask(endpoint: &str, name: &str) -> Result<Vec<SrvTarget>, NetError> {
    let endpoint = endpoint.trim();
    let question = http::form(&[("name", name), ("type", "SRV")]);
    let url = Url::parse(&format!("{endpoint}{}{question}", if endpoint.contains('?') { '&' } else { '?' }))?;
    targets(&http::get(&url, &[("Accept", "application/dns-json")], LOOKUP_TIMEOUT)?)
}

/// The targets of a name server's answer: `{"Status":0,"Answer":[{"type":33,"data":"10 60 443 mail.example.org."}]}`.
/// Whatever else is in there, and whatever isn't a host name, is passed over.
pub(crate) fn targets(answer: &Answer) -> Result<Vec<SrvTarget>, NetError> {
    let nonsense = |text: String| NetError::Proto(ProtoError::Protocol(text));
    if answer.status != 200 {
        return Err(nonsense(format!("the name server answered with status {}", answer.status)));
    }
    let json = serde_json::from_slice::<Value>(&answer.body).ok().filter(Value::is_object);
    let status = json.as_ref().and_then(|json| json.get("Status")).and_then(Value::as_u64);
    let (Some(json), Some(status)) = (&json, status) else { return Err(nonsense("the name server's answer isn't the JSON it should be".into())) };
    match status {
        NO_ERROR => {}
        NO_SUCH_DOMAIN => return Ok(Vec::new()),
        other => return Err(NetError::Proto(ProtoError::Rejected(format!("the name server couldn't look the domain up (code {other})")))),
    }
    let records = json.get("Answer").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    // (priority, weight, target)
    let mut found: Vec<(u16, u16, SrvTarget)> = Vec::new();
    for record in records.iter().take(MAX_RECORDS) {
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
