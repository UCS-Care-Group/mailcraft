// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whose `Authentication-Results` is believed: on Exchange Online only the one it writes with its
//! own id above the hop where it took the mail in from the internet, for mail it says came in
//! (AU3-V5); and none that MailCraft itself carried onto a server (AU3-11).
//!
//! The headers are shaped as Exchange Online writes them, with invented hosts, addresses and ids.

use crate::auth_results::{AUTH_AS, Failed, dkim_passed, failed_check, forget_stamps, summary, vouched};
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
    // Another id above the hop.
    for other in ["mx.lumen.example; dmarc=pass header.from=example.org", "mx.microsoft.com.evil.example; dmarc=pass header.from=example.org"] {
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

/// Exchange Online writes no authserv-id for consumer mailboxes (and did not for others before):
/// such a verdict above the edge hop is its own, below the hop or without its word that the mail
/// came in it vouches for nothing.
const NO_ID_PASS: &str = "spf=pass (sender IP is 192.0.2.10) smtp.mailfrom=example.org; dkim=pass (signature was verified) \
                          header.d=example.org;dmarc=pass action=none header.from=example.org;compauth=pass reason=100";
const NO_ID_FAIL: &str = "spf=fail (sender IP is 198.51.100.7) smtp.mailfrom=example.org; dkim=none (message not signed) \
                          header.d=none;dmarc=fail action=oreject header.from=example.org;compauth=fail reason=000";

#[test]
fn a_verdict_without_an_id_above_the_edge_hop_is_exchanges() {
    for account in [microsoft(), microsoft_imap()] {
        let m = message("alerts@example.org", external(NO_ID_PASS, Vec::new()));
        assert!(vouched(Some(&account), &m));
        let m = message("alerts@example.org", external(NO_ID_FAIL, Vec::new()));
        assert!(!vouched(Some(&account), &m));
        assert_eq!(failed_check(Some(&account), &m), Some(Failed::Dmarc));
    }
    // Below the hop it came with the message.
    let mut headers = inside();
    headers.push(h("Received", EDGE));
    headers.extend(senders(vec![h("Authentication-Results", NO_ID_PASS)]));
    headers.extend(stamps("Incoming", "Anonymous"));
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", headers)));
    // Without Exchange's word that it came in, it vouches for nothing.
    let mut headers: Vec<_> =
        external(NO_ID_PASS, Vec::new()).into_iter().filter(|(name, _)| name != "X-MS-Exchange-Organization-MessageDirectionality").collect();
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", headers.clone())));
    headers.push(h("X-MS-Exchange-Organization-MessageDirectionality", "Originating"));
    assert!(!vouched(Some(&microsoft()), &message("alerts@example.org", headers)));
}

/// A failure above the edge hop is a warning even when Exchange's word on which way the mail went
/// is missing (a forged failure can only add one), so that losing that word never takes the
/// warning away.
#[test]
fn a_failure_above_the_edge_hop_counts_without_the_word_that_the_mail_came_in() {
    for verdict in [OWN_FAIL, NO_ID_FAIL] {
        let headers: Vec<_> =
            external(verdict, Vec::new()).into_iter().filter(|(name, _)| name != "X-MS-Exchange-Organization-MessageDirectionality").collect();
        let m = message("alerts@example.org", headers);
        assert!(!vouched(Some(&microsoft()), &m));
        assert_eq!(failed_check(Some(&microsoft()), &m), Some(Failed::Dmarc), "{verdict}");
    }
}

/// Mail from outside relayed to Exchange Online through another service, in the shape such mail
/// is delivered (invented hosts, addresses and ids): Exchange's own verdict above its topmost edge
/// hop, then what the relay carried, with a second edge hop lower down and the earlier verdicts
/// under other names. `results` is the topmost verdict.
fn relayed(results: &str, domain: &str) -> Vec<(String, String)> {
    let mut headers = inside();
    headers.extend([
        h("Authentication-Results", results),
        h("Received-SPF", &format!("Pass (protection.outlook.com: domain of {domain} designates 192.0.2.20 as permitted sender)")),
        h(
            "Received",
            "from AM0PR02CU001.outbound.protection.outlook.com (192.0.2.20) by LO1PEPF0000A0FD.mail.protection.outlook.com (10.167.16.2) \
             with Microsoft SMTP Server; Tue, 3 Nov 2026 10:00:01 +0000",
        ),
        h("ARC-Seal", "i=1; a=rsa-sha256; s=arcselector; d=microsoft.com; cv=none; b=AAAA"),
        h("ARC-Message-Signature", "i=1; a=rsa-sha256; c=relaxed/relaxed; d=microsoft.com; s=arcselector; b=BBBB"),
        h(
            "ARC-Authentication-Results",
            &format!("i=1; mx.microsoft.com 1; spf=fail smtp.mailfrom={domain}; dmarc=fail action=oreject header.from={domain}"),
        ),
        h("DKIM-Signature", &format!("v=1; a=rsa-sha256; d={domain}; s=s1; b=CCCC")),
        h("Received", "from relay.example.net (198.51.100.9) by AM0PR02CU001.outbound.protection.outlook.com with Microsoft SMTP Server"),
        h(
            "X-MS-Exchange-Authentication-Results",
            &format!(
                "mx.microsoft.com 1; spf=fail (sender IP is 198.51.100.9) smtp.mailfrom={domain}; dmarc=fail action=oreject header.from={domain}"
            ),
        ),
        h("Received-SPF", &format!("Fail (protection.outlook.com: domain of {domain} does not designate 198.51.100.9 as permitted sender)")),
        h(
            "Received",
            "from relay.example.net (198.51.100.9) by LO1PEPF0000A0CC.mail.protection.outlook.com (10.167.16.3) with Microsoft SMTP Server",
        ),
        h("From", &format!("Lin Zhao <lin@{domain}>")),
        h(
            "Authentication-Results-Original",
            &format!("mx.microsoft.com 1; dkim=none (message not signed) header.d=none; dmarc=none action=none header.from={domain}"),
        ),
        h("Subject", "Quarterly figures"),
    ]);
    headers.extend(stamps("Incoming", "Anonymous"));
    headers
}

/// Checked against the shapes of real headers (AU3-V5): the verdict above the topmost edge hop is
/// read, with or without an id; the lower hop, the failures under other names and the earlier
/// ARC verdict are not.
#[test]
fn relayed_mail_is_judged_by_the_verdict_above_the_topmost_edge_hop() {
    let pass = "mx.microsoft.com 1; spf=pass (sender IP is 192.0.2.20) smtp.mailfrom=example.org; dkim=pass (signature was verified) \
                header.d=example.org;dmarc=pass action=none header.from=example.org;compauth=pass reason=100";
    let m = message("lin@example.org", relayed(pass, "example.org"));
    assert!(vouched(Some(&microsoft()), &m));
    assert_eq!(failed_check(Some(&microsoft()), &m), None);
    // From a consumer mailbox: no id.
    let no_id = "spf=pass (sender IP is 192.0.2.20) smtp.mailfrom=example.com; dkim=pass (signature was verified) \
                 header.d=example.com;dmarc=pass action=none header.from=example.com;compauth=pass reason=100";
    let m = message("lin@example.com", relayed(no_id, "example.com"));
    assert!(vouched(Some(&microsoft()), &m));
    assert_eq!(failed_check(Some(&microsoft()), &m), None);
    // Without the topmost verdict, nothing below it is read in its place: not the failures under
    // other names, nor a verdict below the lower hop.
    let mut headers = relayed(pass, "example.org");
    headers.retain(|(name, _)| name != "Authentication-Results");
    let m = message("lin@example.org", headers.clone());
    assert!(!vouched(Some(&microsoft()), &m));
    assert_eq!(failed_check(Some(&microsoft()), &m), None);
    let at = headers.iter().position(|(_, value)| value.contains("LO1PEPF0000A0CC")).unwrap();
    headers.insert(at, h("Authentication-Results", pass));
    assert!(!vouched(Some(&microsoft()), &message("lin@example.org", headers)));
}

/// #1311: IONOS's servers sign their results `kundenserver.de`, under none of its IMAP hosts'
/// domains: a DMARC pass of theirs vouches for the sender, and the same words from any other
/// server don't.
#[test]
fn ionos_results_signed_kundenserver_vouch_for_the_sender() {
    let ionos = Account {
        kind: AccountKind::Imap,
        email: "luke@ionos-customer.example".into(),
        incoming: Some(Server { host: "imap.ionos.co.uk".into(), port: 993, security: Security::Tls }),
        ..Account::default()
    };
    let message = |results: &str| Message {
        uid: 7,
        from: Address { name: String::new(), email: "news@example.org".into() },
        headers: vec![h("Authentication-Results", results)],
        ..Message::default()
    };
    let pass = message("kundenserver.de; dkim=pass header.i=@example.org; spf=pass smtp.mailfrom=example.org; dmarc=pass header.from=example.org");
    assert!(vouched(Some(&ionos), &pass));
    assert!(!vouched(Some(&ionos), &message("kundenserver.de; dmarc=fail header.from=example.org")));
    assert!(!vouched(Some(&ionos), &message("mail.elsewhere.example; dmarc=pass header.from=example.org")));
    // Another provider's account doesn't take IONOS's word.
    let other = Account { incoming: Some(Server { host: "imap.mail.example".into(), port: 993, security: Security::Tls }), ..ionos.clone() };
    assert!(!vouched(Some(&other), &pass));
}

/// #1568: mail from outside that came through a filtering gateway before Exchange Online, in the
/// shape such mail is delivered (invented hosts, addresses and ids). Exchange checked the
/// gateway's address and a body the gateway had changed, so SPF, DKIM and DMARC failed, and it
/// says with `compauth=none reason=905` that it didn't enforce them for that reason. `results`
/// is Exchange's verdict above its edge hop; below it, the gateway's own, which came with the
/// message as far as anybody can tell.
fn through_a_gateway(results: &str) -> Vec<(String, String)> {
    let mut headers = inside();
    headers.extend([
        h("Authentication-Results", results),
        h("Received-SPF", "Fail (protection.outlook.com: domain of example.org does not designate 198.51.100.30 as permitted sender)"),
        h(
            "Received",
            "from mx1.gateway.example.net (198.51.100.30) by AM3PEPF0000A001.mail.protection.outlook.com (10.167.16.1) with Microsoft SMTP Server; \
             Tue, 3 Nov 2026 10:00:01 +0000",
        ),
        h("ARC-Seal", "i=1; a=rsa-sha256; d=gateway.example.net; s=arc; cv=none; b=DDDD"),
        h(
            "ARC-Authentication-Results",
            "i=1; mx1.gateway.example.net; spf=pass smtp.mailfrom=example.org; dkim=pass header.d=example.org; dmarc=pass header.from=example.org",
        ),
        h(
            "Authentication-Results",
            "mx1.gateway.example.net; spf=pass smtp.mailfrom=example.org; dkim=pass header.d=example.org; dmarc=pass header.from=example.org",
        ),
        h("Received", "from mail.example.org (192.0.2.10) by mx1.gateway.example.net; Tue, 3 Nov 2026 10:00:00 +0000"),
    ]);
    headers.extend(senders(Vec::new()));
    headers.extend(stamps("Incoming", "Anonymous"));
    headers
}

const ROUTED: &str = "mx.microsoft.com 1; spf=fail (sender IP is 198.51.100.30) smtp.mailfrom=example.org; dkim=fail (body hash did not verify) \
                      header.d=example.org;dmarc=fail action=none header.from=example.org;compauth=none reason=905";

#[test]
fn a_failure_exchange_says_came_of_the_route_is_no_warning() {
    for account in [microsoft(), microsoft_imap()] {
        let m = message("alerts@example.org", through_a_gateway(ROUTED));
        assert_eq!(failed_check(Some(&account), &m), None);
        // Nor is it vouched for: the gateway's pass below the edge hop is anybody's to write.
        assert!(!vouched(Some(&account), &m));
        // Without an authserv-id, as Exchange writes it for consumer mailboxes, the same.
        let no_id = ROUTED.trim_start_matches("mx.microsoft.com 1; ");
        assert_eq!(failed_check(Some(&account), &message("alerts@example.org", through_a_gateway(no_id))), None);
        // An SPF failure with no DMARC verdict, for the same reason.
        let spf = "mx.microsoft.com 1; spf=fail (sender IP is 198.51.100.30) smtp.mailfrom=example.org; compauth=none reason=905";
        assert_eq!(failed_check(Some(&account), &message("alerts@example.org", through_a_gateway(spf))), None);
    }
}

#[test]
fn only_the_complex_routing_reason_of_exchange_itself_takes_a_failure_away() {
    let account = microsoft();
    // Exchange let it by for another reason (an allow list, a rule) or didn't check: the failure stands.
    for reason in ["451", "301", "000", "9050", ""] {
        let verdict = ROUTED.replace("reason=905", &format!("reason={reason}"));
        assert_eq!(failed_check(Some(&account), &message("alerts@example.org", through_a_gateway(&verdict))), Some(Failed::Dmarc), "{reason}");
    }
    // A failed composite verdict, or the code in a comment only, is no reason either.
    for verdict in [ROUTED.replace("compauth=none", "compauth=fail"), ROUTED.replace("reason=905", "(reason=905)")] {
        assert_eq!(failed_check(Some(&account), &message("alerts@example.org", through_a_gateway(&verdict))), Some(Failed::Dmarc), "{verdict}");
    }
    // The code below the edge hop, where the sender or the gateway may have written it, is not read.
    let mut headers = external(OWN_FAIL, vec![h("Authentication-Results", ROUTED)]);
    assert_eq!(failed_check(Some(&account), &message("alerts@example.org", headers.clone())), Some(Failed::Dmarc));
    // Nor is a second clause of Exchange's own that gives it for another domain's sender.
    headers = through_a_gateway(&ROUTED.replace("compauth=none reason=905", "compauth=none reason=905 header.from=other.example"));
    assert_eq!(failed_check(Some(&account), &message("alerts@example.org", headers)), Some(Failed::Dmarc));
}

/// The same verdict for a sender at the account's own domain. Exchange gives `reason=905` from how
/// the tenant's mail is routed, so it says it too of a message sent straight to Microsoft 365 past
/// the gateway, where anybody can write a colleague's address on it.
fn spoof_of(from: &str, domain: &str, results: &str) -> Message {
    message(from, through_a_gateway(&results.replace("example.org", domain)))
}

#[test]
fn a_spoof_of_the_owners_own_domain_keeps_its_warning_past_the_route() {
    let spf = "mx.microsoft.com 1; spf=fail (sender IP is 203.0.113.9) smtp.mailfrom=example.org; compauth=none reason=905";
    for account in [microsoft(), microsoft_imap()] {
        assert_eq!(failed_check(Some(&account), &spoof_of("ceo@lumen.example", "lumen.example", ROUTED)), Some(Failed::Dmarc));
        assert_eq!(failed_check(Some(&account), &spoof_of("ceo@lumen.example", "lumen.example", spf)), Some(Failed::Spf));
        // Written in other capitals, with a dot at the end, or at a name under the account's own.
        assert_eq!(failed_check(Some(&account), &spoof_of("CEO@Lumen.EXAMPLE.", "LUMEN.example", ROUTED)), Some(Failed::Dmarc));
        assert_eq!(failed_check(Some(&account), &spoof_of("it@hr.lumen.example", "hr.lumen.example", ROUTED)), Some(Failed::Dmarc));
        // A domain that only starts or ends like the account's is somebody else's: no warning.
        for outside in ["lumen.example.org", "notlumen.example"] {
            assert_eq!(failed_check(Some(&account), &spoof_of(&format!("ceo@{outside}"), outside, ROUTED)), None, "{outside}");
        }
    }
    // The account's address at a name under the organisation's, the sender at the organisation's.
    let uk = Account { email: "ada@uk.lumen.example".into(), ..microsoft() };
    assert_eq!(failed_check(Some(&uk), &spoof_of("ceo@lumen.example", "lumen.example", ROUTED)), Some(Failed::Dmarc));
    // A domain written in other letters is the same domain however either address writes it.
    let idn = Account { email: "ada@xn--lmen-0ra.example".into(), ..microsoft() };
    assert_eq!(failed_check(Some(&idn), &spoof_of("ceo@l\u{fc}men.example", "xn--lmen-0ra.example", ROUTED)), Some(Failed::Dmarc));
    let idn = Account { email: "ada@l\u{fc}men.example".into(), ..microsoft() };
    assert_eq!(failed_check(Some(&idn), &spoof_of("ceo@xn--lmen-0ra.example", "xn--lmen-0ra.example", ROUTED)), Some(Failed::Dmarc));
}

#[test]
fn mail_exchange_calls_internal_is_not_vouched_for_past_a_failure_it_did_not_enforce() {
    // A connector may make Exchange call mail of the owner's own domain internal; with the
    // failure standing, it is not vouched for, as before #1568.
    let mut m = spoof_of("bo@lumen.example", "lumen.example", ROUTED);
    for (name, value) in &mut m.headers {
        if name == AUTH_AS {
            *value = "Internal".into();
        }
    }
    assert_eq!(failed_check(Some(&microsoft()), &m), Some(Failed::Dmarc));
    assert!(!vouched(Some(&microsoft()), &m));
}

// A server not on Exchange Online believes its own `Authentication-Results` only where nobody but
// it can have written it: a provider's that always writes one on top of whatever came with the
// message, or one that lies above the hop at which the account's server took the mail in. A
// sender can write any header below that hop, its server's id and `dmarc=pass` among it (RFC
// 8601, 7.1); a server that writes no verdict of its own leaves that one on top.

/// Ada's account at a server of her own; its authserv-id is a name under `example.org`.
fn own_server() -> Account {
    Account {
        kind: AccountKind::Imap,
        email: "ada@example.org".into(),
        incoming: Some(Server { host: "imap.example.org".into(), port: 993, security: Security::Tls }),
        ..Account::default()
    }
}

const SELF_PASS: &str = "mx.example.org; spf=pass smtp.mailfrom=bank.example; dkim=pass header.d=bank.example header.s=s1 header.b=AbCd; \
                         dmarc=pass header.from=bank.example";
const SELF_FAIL: &str = "mx.example.org; spf=fail smtp.mailfrom=bank.example; dkim=none; dmarc=fail header.from=bank.example";
/// The hop at which the account's server took the mail in from the internet.
const INTAKE: &str = "from mail.bank.example (mail.bank.example [198.51.100.7]) (using TLSv1.3 with cipher TLS_AES_256_GCM_SHA384 (256/256 bits)) \
                      by mx.example.org (Postfix) with ESMTPS id 4Ab1Cd for <ada@example.org>; Tue, 3 Nov 2026 10:00:01 +0000";
/// The hop at which it handed the mail on to the mailbox store.
const DELIVERY: &str = "from mx.example.org ([192.0.2.25]) by imap.example.org with LMTP id QmFzZTY0 (envelope-from <alerts@bank.example>) \
                        for <ada@example.org>; Tue, 3 Nov 2026 10:00:02 +0000";

/// What the sender wrote below the intake hop, `extra` on top of it.
fn sent(extra: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out = extra;
    out.push(h("Received", "from laptop.bank.example ([203.0.113.4]) by mail.bank.example with ESMTPSA id 77; Tue, 3 Nov 2026 10:00:00 +0000"));
    out.extend(senders(Vec::new()));
    out
}

fn bank(headers: Vec<(String, String)>) -> Message {
    message("alerts@bank.example", headers)
}

/// Whether anything the account's server is taken to have said of `m` vouches for or tells of it.
fn trusted(account: &Account, m: &Message) -> bool {
    vouched(Some(account), m) || !dkim_passed(Some(account), m).is_empty() || summary(Some(account), m).is_some()
}

#[test]
fn a_verdict_a_server_without_one_of_its_own_left_on_top_is_not_believed() {
    // The server wrote none; the sender wrote one with its id below the intake hop.
    let forged = [h("Authentication-Results", SELF_PASS)];
    let mut headers = vec![h("Received", INTAKE)];
    headers.extend(sent(forged.to_vec()));
    assert!(!trusted(&own_server(), &bank(headers.clone())));
    // With the delivery hop on top as well.
    headers.insert(0, h("Received", DELIVERY));
    assert!(!trusted(&own_server(), &bank(headers)));
    // With nothing at all below the forged one.
    assert!(!trusted(&own_server(), &bank(vec![h("Received", INTAKE), h("Authentication-Results", SELF_PASS)])));
    // With a hop of its own below it, made to look like the server's.
    let mut faked = vec![h("Received", INTAKE), h("Authentication-Results", SELF_PASS), h("Received", INTAKE)];
    faked.extend(sent(Vec::new()));
    assert!(!trusted(&own_server(), &bank(faked)));
    // A message no server took in (another program put it there) has no hop to go by.
    assert!(!trusted(&own_server(), &bank(vec![h("Authentication-Results", SELF_PASS)])));
}

#[test]
fn a_verdict_forged_below_the_servers_own_takes_nothing_away_from_its_word() {
    // The server says it failed; the sender's own below says it passed.
    let mut headers = vec![h("Authentication-Results", SELF_FAIL), h("Received", INTAKE)];
    headers.extend(sent(vec![h("Authentication-Results", SELF_PASS)]));
    let m = bank(headers);
    assert!(!vouched(Some(&own_server()), &m));
    assert_eq!(failed_check(Some(&own_server()), &m), Some(Failed::Dmarc));
    assert!(dkim_passed(Some(&own_server()), &m).is_empty());
    assert_eq!(summary(Some(&own_server()), &m).and_then(|s| s.dmarc), Some("fail".into()));
}

#[test]
fn the_servers_own_verdict_above_its_intake_hop_is_believed() {
    // As a filter at the intake writes it, above the server's hop.
    let mut headers = vec![h("Authentication-Results", SELF_PASS), h("Received", INTAKE)];
    headers.extend(sent(Vec::new()));
    let m = bank(headers.clone());
    assert!(vouched(Some(&own_server()), &m));
    assert_eq!(dkim_passed(Some(&own_server()), &m), vec![("bank.example".to_string(), Some("AbCd".to_string()))]);
    assert_eq!(summary(Some(&own_server()), &m).and_then(|s| s.dmarc), Some("pass".into()));
    // With the hops to the mailbox store above it, however many.
    headers.insert(0, h("Received", DELIVERY));
    headers.insert(0, h("Received", &DELIVERY.replace("LMTP", "LMTPS").replace(" ([192.0.2.25])", "")));
    // One over TLS, which says so before `by`.
    let tls = DELIVERY.replace("]) by", "]) (using TLSv1.3 with cipher TLS_AES_256_GCM_SHA384 (256/256 bits)) by").replace("LMTP", "LMTPS");
    headers.insert(0, h("Received", &tls));
    assert!(vouched(Some(&own_server()), &bank(headers.clone())));
    // A forged pass below it changes nothing; the server's failure stays a failure.
    headers.extend(sent(vec![h("Authentication-Results", SELF_FAIL)]));
    assert!(vouched(Some(&own_server()), &bank(headers)));
    let mut failed = vec![h("Received", DELIVERY), h("Authentication-Results", SELF_FAIL), h("Received", INTAKE)];
    failed.extend(sent(vec![h("Authentication-Results", SELF_PASS)]));
    assert!(!vouched(Some(&own_server()), &bank(failed.clone())));
    assert_eq!(failed_check(Some(&own_server()), &bank(failed)), Some(Failed::Dmarc));
}

#[test]
fn only_a_plain_hop_to_the_mailbox_store_is_looked_past() {
    // Hops that say LMTP where the sender writes (its HELO, its envelope address), or that are
    // not in the plain shape, are taken for the intake hop: what is below them isn't believed.
    for hop in [
        "from a)by)imap.example.org)with)LMTP( (mail.bank.example [198.51.100.7]) by mx.example.org (Postfix) with ESMTPS id 1 for <ada+)@example.org>; Tue, 3 Nov 2026 10:00:01 +0000",
        "from a by imap.example.org with LMTP by mx.example.org with ESMTP; Tue, 3 Nov 2026 10:00:01 +0000",
        "from [198.51.100.7] (helo=a) by mx.example.org with esmtps (TLS1.3) (envelope-from <\"x) with LMTP (\"@bank.example>) id 1; Tue, 3 Nov 2026",
        "from mail.bank.example (mail.bank.example [198.51.100.7]) by mx.example.org with LMTP id 1; Tue, 3 Nov 2026 10:00:01 +0000",
        "from x@mail.bank.example by mx.example.org with LMTP id 1; Tue, 3 Nov 2026 10:00:01 +0000",
        "from mx.example.org ([unix socket]) by imap.example.org with LMTPA; Tue, 3 Nov 2026 10:00:02 +0000",
        "by imap.example.org with LMTP id 1; Tue, 3 Nov 2026 10:00:02 +0000",
        // A hop over TLS from outside says so as a store does, but `with ESMTPS`.
        "from mail.bank.example ([198.51.100.7]) (using TLSv1.3 with cipher TLS_AES_256_GCM_SHA384 (256/256 bits)) by mx.example.org with ESMTPS id 1",
        // A comment left open, or a second `with` past it.
        "from mx.example.org ([192.0.2.25]) (using TLSv1.3 with cipher X (256/256 bits) by imap.example.org with LMTPS id 1",
        "from mx.example.org (using TLSv1.3) by imap.example.org with LMTPS id 1 with ESMTP",
    ] {
        let mut headers = vec![h("Received", hop), h("Authentication-Results", SELF_PASS)];
        headers.extend(sent(Vec::new()));
        assert!(!trusted(&own_server(), &bank(headers)), "{hop}");
    }
    // One too long to read whole isn't looked past either.
    let long = format!("{DELIVERY} {}", "x ".repeat(3_000));
    let mut headers = vec![h("Received", &long), h("Authentication-Results", SELF_PASS), h("Received", INTAKE)];
    headers.extend(sent(Vec::new()));
    assert!(!trusted(&own_server(), &bank(headers)));
}

#[test]
fn a_failure_is_read_wherever_the_servers_id_stands_on_top() {
    // A failure can only add a warning, so it is read below the intake hop too, as before: some
    // filters write theirs there.
    let mut headers = vec![h("Received", DELIVERY), h("Received", INTAKE), h("Authentication-Results", SELF_FAIL)];
    headers.extend(sent(Vec::new()));
    let m = bank(headers);
    assert_eq!(failed_check(Some(&own_server()), &m), Some(Failed::Dmarc));
    assert!(!trusted(&own_server(), &m));
}

/// Gmail writes its verdict below the hop at which it took the mail in, and on every message it
/// takes in, so its own is the topmost with its id wherever it stands.
#[test]
fn gmails_own_verdict_below_its_hop_is_believed() {
    let gmail = Account {
        kind: AccountKind::Imap,
        email: "ada@gmail.example".into(),
        incoming: Some(Server { host: "imap.gmail.com".into(), port: 993, security: Security::Tls }),
        ..Account::default()
    };
    let mut headers = vec![
        h("Received", "by 2002:a05:6a10:1234:b0:5c1:1234:5678 with SMTP id abc; Tue, 3 Nov 2026 02:00:02 -0800 (PST)"),
        h("ARC-Authentication-Results", "i=1; mx.google.com; dmarc=pass header.from=bank.example"),
        h(
            "Received",
            "from mail.bank.example (mail.bank.example. [198.51.100.7]) by mx.google.com with ESMTPS id x1; Tue, 3 Nov 2026 02:00:01 -0800 (PST)",
        ),
        h("Received-SPF", "pass (google.com: domain of alerts@bank.example designates 198.51.100.7 as permitted sender)"),
        h(
            "Authentication-Results",
            "mx.google.com; dkim=pass header.i=@bank.example; spf=pass smtp.mailfrom=bank.example; dmarc=pass header.from=bank.example",
        ),
    ];
    headers.extend(sent(Vec::new()));
    assert!(vouched(Some(&gmail), &bank(headers)));
}

#[test]
fn the_hops_a_verdict_is_read_by_stay_on_the_message() {
    // As a message is kept: what nothing reads by name goes to its block, but the topmost hops,
    // down to the one at which the server took the mail in, stay, and the server's verdict is
    // read by them as it was by every line.
    let mut all = vec![h("Received", DELIVERY), h("Authentication-Results", SELF_PASS), h("X-Spam-Score", "0.1"), h("Received", INTAKE)];
    all.extend(sent(vec![h("Received", DELIVERY)]));
    let (kept, block) = crate::headers::split(all.clone());
    let hops: Vec<&str> = kept.iter().filter(|(name, _)| name == "Received").map(|(_, value)| value.as_str()).collect();
    assert_eq!(hops, [DELIVERY, INTAKE]);
    assert_eq!(block.lines().iter().filter(|line| line.name == "Received").count(), 2);
    let m = Message { headers: kept, header_block: block, ..bank(Vec::new()) };
    assert!(vouched(Some(&own_server()), &m));
    assert_eq!(crate::headers::merge(&m.headers, &m.header_block.lines()), all);
    // The sender's verdict below the server's hop stays below it.
    let mut forged = vec![h("Received", DELIVERY), h("Received", INTAKE)];
    forged.extend(sent(vec![h("Authentication-Results", SELF_PASS)]));
    let (kept, block) = crate::headers::split(forged);
    assert!(!vouched(Some(&own_server()), &Message { headers: kept, header_block: block, ..bank(Vec::new()) }));
}

#[test]
fn older_mail_kept_with_only_a_hop_at_exchange_onlines_edge_does_not_vouch() {
    // An earlier version put every `Received` but one at Exchange Online's edge in the block. Mail
    // it kept for an account at a server of its own, whose server wrote no verdict, has the
    // sender's verdict with that server's id on top, and a hop the sender wrote "at the edge"
    // below it: the server's own intake hop is in the block, and nothing splits it again.
    let fake_edge = EDGE.replace("mail.example.org", "mail.bank.example");
    let all = [
        h("Received", DELIVERY),
        h("Received", INTAKE),
        h("Authentication-Results", SELF_PASS),
        h("Received", &fake_edge),
        h("DKIM-Signature", "v=1; a=rsa-sha256; d=bank.example; s=s1; b=AbCd"),
        h("From", "Bank <alerts@bank.example>"),
        h("Subject", "Your statement"),
    ];
    let (working, block): (Vec<_>, Vec<_>) = all.into_iter().enumerate().partition(|(at, _)| ![0, 1].contains(at));
    let block: Vec<crate::headers::HeaderLine> =
        block.into_iter().map(|(at, (name, value))| crate::headers::HeaderLine { at, name, value }).collect();
    let m = Message {
        headers: working.into_iter().map(|(_, line)| line).collect(),
        header_block: crate::headers::HeaderBlock::of(&block),
        ..bank(Vec::new())
    };
    assert!(!trusted(&own_server(), &m));
    // Nor does new mail with such a hop as the topmost below the verdict.
    let mut headers = vec![h("Received", DELIVERY), h("Authentication-Results", SELF_PASS), h("Received", &fake_edge)];
    headers.extend(sent(Vec::new()));
    assert!(!trusted(&own_server(), &bank(headers)));
    // A server of its own behind Exchange Online's edge keeps its hop above the edge's.
    let mut headers = vec![h("Authentication-Results", SELF_PASS), h("Received", INTAKE), h("Received", &fake_edge)];
    headers.extend(sent(Vec::new()));
    assert!(vouched(Some(&own_server()), &bank(headers)));
}

#[test]
fn a_comment_that_swallows_the_servers_own_by_is_not_looked_past() {
    // A HELO that opens a comment the server's `(using …` skip would read to the envelope
    // address, where the sender wrote a `by … with LMTP` of its own.
    for hop in [
        "from a.example (using ( (unknown [192.0.2.1]) by mx.example.org (Postfix) with ESMTP id 1 \
         for <\"ada+)) by h.example with LMTP x\"@example.org>; Tue, 3 Nov 2026 10:00:01 +0000",
        "from a.example ([192.0.2.1]) (using ( by mx.example.org (Postfix) with ESMTPS id 1 \
         for <\"ada+)) by h.example with LMTPS x\"@example.org>; Tue, 3 Nov 2026 10:00:01 +0000",
        "from a.example (using (( by mx.example.org with ESMTP id 1 for <\"x)))) by h.example with LMTPA\"@example.org>",
        // A comment holding the server's `by` and closing before a `by` of the sender's.
        "from a.example (using by mx.example.org with ESMTP id 1 for <x@example.org>) by h.example with LMTP id 2",
        // Quotes or an address inside it: no TLS comment holds one.
        "from a.example (using \"x\") by imap.example.org with LMTP id 1",
        "from a.example (using <x@example.org>) by imap.example.org with LMTP id 1",
    ] {
        let mut headers = vec![h("Received", hop), h("Authentication-Results", SELF_PASS)];
        headers.extend(sent(Vec::new()));
        assert!(!trusted(&own_server(), &bank(headers)), "{hop}");
    }
}

#[test]
fn a_verdict_below_too_many_hops_to_the_store_is_not_read() {
    // Each hop is read whole, and `vouched` may be asked often: past a few, nothing more is read.
    let with = |n: usize| {
        let mut headers = vec![h("Received", DELIVERY); n];
        headers.extend([h("Authentication-Results", SELF_PASS), h("Received", INTAKE)]);
        headers.extend(sent(Vec::new()));
        bank(headers)
    };
    assert!(vouched(Some(&own_server()), &with(8)));
    assert!(!trusted(&own_server(), &with(9)));
    assert!(!trusted(&own_server(), &with(1_000)));
}
