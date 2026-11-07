// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whose `Authentication-Results` is believed: on Exchange Online only the one it writes with its
//! own id above the hop where it took the mail in from the internet, for mail it says came in
//! (AU3-V5); and none that MailCraft itself carried onto a server (AU3-11).
//!
//! The headers are shaped as Exchange Online writes them, with invented hosts, addresses and ids.

use crate::auth_results::{AUTH_AS, Failed, failed_check, forget_stamps, vouched};
use crate::{Account, AccountKind, Address, Message, Security, Server};

/// Ada's Microsoft 365 account at Lumen.
fn microsoft() -> Account {
    Account { kind: AccountKind::Microsoft, email: "ada@lumen.example".into(), ..Account::default() }
}

/// The same mailbox reached over IMAP at Microsoft's host.
fn microsoft_imap() -> Account {
    Account {
        kind: AccountKind::Imap,
        email: "ada@lumen.example".into(),
        incoming: Some(Server { host: "outlook.office365.com".into(), port: 993, security: Security::Tls }),
        ..Account::default()
    }
}

fn h(name: &str, value: &str) -> (String, String) {
    (name.into(), value.into())
}

const OWN_PASS: &str = "mx.microsoft.com 1; spf=pass (sender ip is 192.0.2.10) smtp.mailfrom=example.org; dkim=pass (signature was verified) \
                        header.d=example.org;dmarc=pass action=none header.from=example.org;compauth=pass reason=100";
const OWN_FAIL: &str = "mx.microsoft.com 1; spf=fail (sender ip is 198.51.100.7) smtp.mailfrom=example.org; dkim=none (message not signed) \
                        header.d=none;dmarc=fail action=quarantine header.from=example.org;compauth=fail reason=000";
const FORGED_PASS: &str = "mx.microsoft.com 1; dmarc=pass action=none header.from=example.org;compauth=pass reason=100";
const EDGE: &str = "from mail.example.org (192.0.2.10) by AM3PEPF0000A001.mail.protection.outlook.com (10.167.16.1) with Microsoft SMTP Server \
                    id 15.20.1 via Frontend Transport; Tue, 3 Nov 2026 10:00:01 +0000";

/// Exchange's hops inside, topmost first.
fn inside() -> Vec<(String, String)> {
    vec![
        h(
            "Received",
            "from AM9PR01MB1234.eurprd01.prod.outlook.com (2603:10a6::1) by DB9PR01MB5678.eurprd01.prod.outlook.com with HTTPS; Tue, 3 Nov 2026 10:00:03 +0000",
        ),
        h(
            "Received",
            "from AS9PR06CA0001.eurprd06.prod.outlook.com (2603:10a6::2) by AM9PR01MB1234.eurprd01.prod.outlook.com (2603:10a6::1) with Microsoft SMTP Server; Tue, 3 Nov 2026 10:00:02 +0000",
        ),
        h(
            "Received",
            "from AM3PEPF0000A001.eurprd04.prod.outlook.com (2603:10a6::3) by AS9PR06CA0001.outlook.office365.com (2603:10a6::2) with Microsoft SMTP Server; Tue, 3 Nov 2026 10:00:02 +0000",
        ),
    ]
}

/// What the sender wrote, `extra` among it.
fn senders(extra: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out = vec![h("DKIM-Signature", "v=1; a=rsa-sha256; d=example.org; s=s1; b=AAAA"), h("From", "Bank <alerts@example.org>")];
    out.extend(extra);
    out.push(h("Subject", "Your statement"));
    out
}

/// Exchange's stamps, written below the sender's headers.
fn stamps(direction: &str, auth_as: &str) -> Vec<(String, String)> {
    vec![
        h("X-MS-Exchange-Organization-MessageDirectionality", direction),
        h(AUTH_AS, auth_as),
        h("X-MS-Exchange-CrossTenant-FromEntityHeader", "Internet"),
    ]
}

/// Mail from outside as Exchange Online delivers it, its verdict `results` above the edge hop.
fn external(results: &str, below: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut headers = inside();
    headers.push(h("Authentication-Results", results));
    headers.push(h(
        "Received-SPF",
        "Pass (protection.outlook.com: domain of example.org designates 192.0.2.10 as permitted sender) receiver=protection.outlook.com; client-ip=192.0.2.10",
    ));
    headers.push(h("Received", EDGE));
    headers.extend(senders(below));
    headers.extend(stamps("Incoming", "Anonymous"));
    headers
}

/// Mail between colleagues: Exchange's hops inside alone, no verdict, `extra` among the sender's.
fn internal_mail(from: &str, extra: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut headers = inside();
    headers.push(h("From", from));
    headers.extend(extra);
    headers.extend(stamps("Originating", "Internal"));
    headers
}

fn message(from: &str, headers: Vec<(String, String)>) -> Message {
    Message { from: Address::new("", from), uid: 7, headers, fresh_stamps: true, ..Message::default() }
}

#[test]
fn exchange_onlines_own_verdict_above_its_edge_hop_is_believed() {
    for account in [microsoft(), microsoft_imap()] {
        let m = message("alerts@example.org", external(OWN_PASS, Vec::new()));
        assert!(vouched(Some(&account), &m));
        assert_eq!(failed_check(Some(&account), &m), None);
        let m = message("alerts@example.org", external(OWN_FAIL, Vec::new()));
        assert!(!vouched(Some(&account), &m));
        assert_eq!(failed_check(Some(&account), &m), Some(Failed::Dmarc));
        // Mail brought in from a file has nothing to go by.
        let m = Message { uid: 0, ..message("alerts@example.org", external(OWN_PASS, Vec::new())) };
        assert!(!vouched(Some(&account), &m));
    }
}

#[test]
fn internal_mail_is_never_vouched_for_by_an_authentication_results() {
    // A colleague: vouched for by Exchange's own stamp, which no outsider can write.
    let colleague = message("bo@lumen.example", internal_mail("Bo <bo@lumen.example>", Vec::new()));
    assert!(vouched(Some(&microsoft()), &colleague));
    // A colleague's own message, or one through a connector, with a verdict it wrote itself.
    let forged = internal_mail("Bank <alerts@example.org>", vec![h("Authentication-Results", FORGED_PASS)]);
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", forged.clone())));
    // Not even with a hop by Exchange's edge written above it.
    let mut edged = vec![h("Authentication-Results", FORGED_PASS), h("Received", EDGE)];
    edged.extend(forged);
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", edged)));
    // And a forged failure is no warning.
    let fail = internal_mail("Bank <alerts@example.org>", vec![h("Authentication-Results", OWN_FAIL)]);
    assert_eq!(failed_check(Some(&microsoft()), &message("alerts@example.org", fail)), None);
}

#[test]
fn a_verdict_below_the_edge_hop_came_with_the_message() {
    // Exchange says it failed; the sender's own header below says it passed.
    let m = message("alerts@example.org", external(OWN_FAIL, vec![h("Authentication-Results", FORGED_PASS)]));
    assert!(!vouched(Some(&microsoft()), &m));
    assert_eq!(failed_check(Some(&microsoft()), &m), Some(Failed::Dmarc));
    // With no verdict of Exchange's above the hop, the one below is not read.
    let mut headers = inside();
    headers.push(h("Received", EDGE));
    headers.extend(senders(vec![h("Authentication-Results", FORGED_PASS)]));
    headers.extend(stamps("Incoming", "Anonymous"));
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", headers)));
    // Nor is a hop the sender wrote below Exchange's own, with a verdict above it.
    let forged_edge = vec![h("Authentication-Results", FORGED_PASS), h("Received", &EDGE.replace("AM3PEPF0000A001", "FAKE01"))];
    let m = message("alerts@example.org", external(OWN_FAIL, forged_edge));
    assert!(!vouched(Some(&microsoft()), &m));
}

#[test]
fn only_exchanges_own_id_and_its_word_that_the_mail_came_in_count() {
    // Another id above the hop, or none.
    for other in ["dmarc=pass action=none header.from=example.org;compauth=pass reason=100", "mx.lumen.example; dmarc=pass header.from=example.org"] {
        assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", external(other, Vec::new()))), "{other}");
    }
    // No word on where it came from, or the word that it was sent inside, or two words.
    let without = |keep: &dyn Fn(&(String, String)) -> bool| external(OWN_PASS, Vec::new()).into_iter().filter(keep).collect::<Vec<_>>();
    let none = without(&|(name, _)| name != "X-MS-Exchange-Organization-MessageDirectionality");
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", none)));
    for direction in ["Originating", "Incoming, Originating"] {
        let mut headers = without(&|(name, _)| name != "X-MS-Exchange-Organization-MessageDirectionality");
        headers.push(h("X-MS-Exchange-Organization-MessageDirectionality", direction));
        assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", headers)), "{direction}");
    }
    let mut twice = external(OWN_PASS, Vec::new());
    twice.push(h("X-MS-Exchange-Organization-MessageDirectionality", "Originating"));
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", twice)));
    // No edge hop at all.
    let no_edge = without(&|(name, value)| !(name == "Received" && value == EDGE));
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", no_edge)));
    // An edge host named in a comment is no hop by it.
    let commented = external(OWN_PASS, Vec::new())
        .into_iter()
        .map(
            |(name, value)| {
                if value == EDGE { (name, "from x (by A1.mail.protection.outlook.com) by relay.example.net".to_string()) } else { (name, value) }
            },
        )
        .collect();
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", commented)));
}

#[test]
fn headers_in_another_order_are_read_by_where_they_stand() {
    // Exchange's stamps on top instead of at the bottom: the same.
    let mut headers = external(OWN_PASS, Vec::new());
    let stamps: Vec<_> = headers.iter().filter(|(name, _)| name.starts_with("X-MS-")).cloned().collect();
    headers.retain(|(name, _)| !name.starts_with("X-MS-"));
    let mut on_top = stamps;
    on_top.extend(headers.clone());
    assert!(vouched(Some(&microsoft()), &message("alerts@example.org", on_top)));
    // The verdict moved below the edge hop is no longer Exchange's.
    let at = |headers: &[(String, String)], name: &str| headers.iter().position(|(n, _)| n == name).unwrap();
    let mut moved = external(OWN_PASS, Vec::new());
    let verdict = moved.remove(at(&moved, "Authentication-Results"));
    let edge = moved.iter().position(|(_, value)| value == EDGE).unwrap();
    moved.insert(edge + 1, verdict);
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", moved)));
    // The edge hop topmost of all: nothing of Exchange's above it.
    let mut first = external(OWN_PASS, Vec::new());
    let edge = first.remove(first.iter().position(|(_, value)| value == EDGE).unwrap());
    first.insert(0, edge);
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", first)));
}

#[test]
fn a_message_placed_on_a_server_by_mailcraft_forgets_every_verdict_it_carried() {
    let mut m = message("alerts@example.org", external(OWN_PASS, Vec::new()));
    m.headers.push(h("ARC-Authentication-Results", "i=1; mx.microsoft.com 1; dmarc=pass header.from=example.org"));
    m.headers.push(h("authentication-results", FORGED_PASS));
    forget_stamps(&mut m);
    let names: Vec<&str> = m.headers.iter().map(|(name, _)| name.as_str()).collect();
    assert!(names.iter().all(|name| !name.eq_ignore_ascii_case("Authentication-Results")
        && !name.eq_ignore_ascii_case("ARC-Authentication-Results")
        && !name.eq_ignore_ascii_case(AUTH_AS)));
    // The rest stays as it was, in order.
    assert!(names.contains(&"Received-SPF") && names.contains(&"DKIM-Signature") && names.contains(&"Subject"));
    assert!(!vouched(Some(&microsoft()), &m));
}
