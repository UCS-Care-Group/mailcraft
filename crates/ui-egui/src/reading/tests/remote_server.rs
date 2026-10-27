// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whose `Authentication-Results` header is believed when a remembered sender's pictures are
//! downloaded: only the topmost one, and only when the account's own server wrote it.
//!
//! The hosts and addresses are invented.

use mailcraft_model::{Account, AccountKind, Address, Message, Security, Server};

use crate::reading::remote::vouched;

fn account(kind: AccountKind, host: &str) -> Account {
    let incoming = (!host.is_empty()).then(|| Server { host: host.into(), port: 993, security: Security::Tls });
    Account { kind, incoming, ..Account::default() }
}

fn imap(host: &str) -> Account {
    account(AccountKind::Imap, host)
}

fn message(headers: &[&str]) -> Message {
    Message {
        from: Address::new("Shop", "news@shop.example"),
        // (As the server has it: mail from a file is nobody's word.)
        uid: 7,
        headers: headers.iter().map(|value| ("Authentication-Results".to_string(), value.to_string())).collect(),
        ..Message::default()
    }
}

const PASS: &str = "dmarc=pass header.from=shop.example";

#[test]
fn gmail_vouches_with_its_own_id() {
    let gmail = imap("imap.gmail.com");
    assert!(vouched(Some(&gmail), &message(&[&format!("mx.google.com; dkim=pass header.i=@shop.example; {PASS}")])));
    // Case and a trailing dot are the same name.
    assert!(vouched(Some(&gmail), &message(&[&format!("MX.Google.com.; {PASS}")])));
}

#[test]
fn a_forged_google_id_is_nothing_on_another_account() {
    let forged = message(&[&format!("mx.google.com; {PASS}")]);
    assert!(!vouched(Some(&imap("imap.lumen.example")), &forged));
    assert!(!vouched(Some(&imap("imap.fastmail.example")), &forged));
    assert!(!vouched(Some(&account(AccountKind::Local, "")), &forged));
    assert!(!vouched(None, &forged));
}

#[test]
fn no_id_counts_for_a_microsoft_account_alone() {
    let bare = message(&["spf=pass smtp.mailfrom=shop.example; dmarc=pass action=none header.from=shop.example; compauth=pass reason=100"]);
    assert!(vouched(Some(&account(AccountKind::Microsoft, "")), &bare));
    assert!(vouched(Some(&imap("outlook.office365.com")), &bare));
    assert!(vouched(Some(&imap("outlook.office.com")), &bare));
    assert!(!vouched(Some(&imap("imap.lumen.example")), &bare));
    assert!(!vouched(Some(&imap("imap.gmail.com")), &bare));
}

#[test]
fn a_company_host_vouches_for_names_under_its_domain() {
    let lumen = imap("imap.lumen.example");
    assert!(vouched(Some(&lumen), &message(&[&format!("mx.lumen.example; {PASS}")])));
    assert!(vouched(Some(&lumen), &message(&[&format!("lumen.example; {PASS}")])));
    assert!(vouched(Some(&lumen), &message(&[&format!("a.b.lumen.example; {PASS}")])));
    assert!(!vouched(Some(&lumen), &message(&[&format!("mx.other.example; {PASS}")])));
    assert!(!vouched(Some(&lumen), &message(&[&format!("mx.notlumen.example; {PASS}")])));
    assert!(!vouched(Some(&lumen), &message(&[&format!("lumen.example.evil.example; {PASS}")])));
}

#[test]
fn a_bare_suffix_is_not_a_company() {
    for host in ["imap.co.uk", "mail.com.au", "imap.github.io", "localhost", "lumen.example", "imap"] {
        let account = imap(host);
        for id in ["mx.co.uk", "co.uk", "mx.com.au", "mx.example", "example", "evil.co.uk", "evil.github.io"] {
            assert!(!vouched(Some(&account), &message(&[&format!("{id}; {PASS}")])), "{host} {id}");
        }
    }
    // Under a suffix, the domain itself is a company.
    assert!(vouched(Some(&imap("imap.lumen.co.uk")), &message(&[&format!("mx.lumen.co.uk; {PASS}")])));
    assert!(!vouched(Some(&imap("imap.lumen.co.uk")), &message(&[&format!("mx.other.co.uk; {PASS}")])));
}

#[test]
fn comments_and_a_version_do_not_hide_the_id() {
    let lumen = imap("imap.lumen.example");
    assert!(vouched(Some(&lumen), &message(&[&format!("(a (nested) note) mx.lumen.example 1 (v1); {PASS}")])));
    assert!(vouched(Some(&lumen), &message(&[&format!("mx.lumen.example (note; with a semicolon); {PASS}")])));
    // A comment is not the id, and the id is not hidden inside one.
    assert!(!vouched(Some(&lumen), &message(&[&format!("(mx.lumen.example) mx.other.example; {PASS}")])));
    assert!(!vouched(Some(&lumen), &message(&[&format!("(mx.lumen.example; {PASS})")])));
}

#[test]
fn only_the_topmost_header_counts() {
    let lumen = imap("imap.lumen.example");
    // The server's line is on top and says no; a forged line below says yes.
    assert!(!vouched(Some(&lumen), &message(&["mx.lumen.example; dmarc=fail header.from=shop.example", &format!("mx.lumen.example; {PASS}")])));
    // The sender's line is on top, the server added none.
    assert!(!vouched(Some(&lumen), &message(&[&format!("forged.example; {PASS}"), &format!("mx.lumen.example; {PASS}")])));
}

#[test]
fn an_id_that_is_no_host_name_is_nobody() {
    let lumen = imap("imap.lumen.example");
    for value in
        ["", ";", "mx.lumen.example@x; dmarc=pass", "mx lumen.example; dmarc=pass header.from=shop.example", "dmarc=pass header.from=shop.example"]
    {
        assert!(!vouched(Some(&lumen), &message(&[value])), "{value}");
    }
}

/// Where the server's verdict stands in the header, not just that the words occur: only the
/// result of the `dmarc` (or `compauth`) clause counts, with the `header.from` of that clause.
#[test]
fn only_the_dmarc_clause_and_its_own_header_from_count() {
    let lumen = imap("imap.lumen.example");
    let says = |rest: &str| vouched(Some(&lumen), &message(&[&format!("mx.lumen.example; {rest}")]));
    assert!(says("dmarc=pass header.from=shop.example"));
    assert!(says("spf=pass smtp.mailfrom=shop.example; DMARC=PASS (p=none) HEADER.FROM=Shop.Example"));
    assert!(says("dmarc=pass header.from=\"shop.example\""));
    assert!(says("dmarc=pass reason=\"all ok\" header.from=shop.example; dkim=none"));
    // The verdict inside a comment or a quoted string is the sender's text, not the server's.
    assert!(!says("spf=pass (dmarc=pass header.from=shop.example); dmarc=fail header.from=shop.example"));
    assert!(!says("dkim=pass header.i=\"dmarc=pass header.from=shop.example\""));
    assert!(!says("dkim=pass reason=\"x; dmarc=pass header.from=shop.example\""));
    assert!(!says("dkim=pass header.i=@\"a b\"; spf=fail smtp.helo=dmarc=pass header.from=shop.example"));
    // Another method's pass is not DMARC's.
    assert!(!says("dkim=pass header.from=shop.example"));
    assert!(!says("spf=pass dmarc=pass header.from=shop.example"));
    // The domain it judged must be named, and be the sender's.
    assert!(!says("dmarc=pass"));
    assert!(!says("dmarc=pass action=none"));
    assert!(!says("dmarc=pass header.from=elsewhere.example"));
    assert!(!says("dmarc=pass header.from=shop.example.evil.example"));
    // A header.from of another clause is not this clause's.
    assert!(!says("dmarc=pass; dkim=pass header.from=shop.example"));
    assert!(!says("dkim=pass header.from=shop.example; dmarc=pass"));
    // A fail, a quoted pass, a pass that only starts the word.
    assert!(!says("dmarc=fail header.from=shop.example"));
    assert!(!says("dmarc=passed header.from=shop.example"));
    // Exchange Online's verdict, with the domain it judged.
    assert!(says("compauth=pass header.from=shop.example reason=100"));
    assert!(!says("compauth=pass reason=100"));
    assert!(!says("compauth=fail header.from=shop.example reason=000"));
}

/// Gmail names the organisation's domain it judged (`shop.example` for mail from
/// `news.shop.example`): a verdict on a domain stands for the names under it, never for those
/// under a bare suffix.
#[test]
fn a_verdict_on_the_organisations_domain_stands_for_the_names_under_it() {
    let gmail = imap("imap.gmail.com");
    let says = |address: &str, judged: &str| {
        let m = Message { from: Address::new("Shop", address), ..message(&[&format!("mx.google.com; dmarc=pass header.from={judged}")]) };
        vouched(Some(&gmail), &m)
    };
    assert!(says("hello@news.shop.example", "shop.example"));
    assert!(says("hello@a.b.shop.example", "Shop.Example"));
    // Not the other way round, nor a name that only ends the same.
    assert!(!says("hello@shop.example", "news.shop.example"));
    assert!(!says("hello@badshop.example", "shop.example"));
    // A bare suffix or a hosting platform is nobody's organisation.
    for (address, judged) in [("hello@bank.co.uk", "co.uk"), ("hello@evil.github.io", "github.io"), ("hello@shop.example", "example")] {
        assert!(!says(address, judged), "{address} {judged}");
    }
    // A fail on the organisation's domain is still a fail.
    let failed =
        Message { from: Address::new("Shop", "hello@news.shop.example"), ..message(&["mx.google.com; dmarc=fail header.from=shop.example"]) };
    assert!(!vouched(Some(&gmail), &failed));
}

/// A `dkim=pass` whose signer is the sender's domain (or the organisation's, above it) is what
/// makes DMARC pass, so where the server gave no DMARC verdict its word on the signature counts
/// as a `dmarc=pass` does; another domain's signature, a signer in a comment, and SPF do not.
#[test]
fn a_dkim_pass_for_the_senders_domain_says_what_dmarc_would() {
    let lumen = imap("imap.lumen.example");
    let says = |address: &str, rest: &str| {
        let m = Message { from: Address::new("Shop", address), ..message(&[&format!("mx.lumen.example; {rest}")]) };
        vouched(Some(&lumen), &m)
    };
    let shop = "news@shop.example";
    assert!(says(shop, "dkim=pass header.d=shop.example"));
    assert!(says(shop, "dkim=pass header.i=@shop.example"));
    assert!(says(shop, "dkim=pass header.i=news@shop.example"));
    assert!(says(shop, "spf=fail smtp.mailfrom=bounce.example; DKIM=Pass (2048-bit key) HEADER.D=\"Shop.Example\" header.s=sel header.b=AbC"));
    assert!(says("hello@news.shop.example", "dkim=pass header.d=shop.example"));
    // Somebody else's signature, as a mailing service signs with its own domain.
    assert!(!says(shop, "dkim=pass header.d=bounce.example"));
    assert!(!says(shop, "dkim=pass header.d=shop.example.evil.example"));
    assert!(!says(shop, "dkim=pass header.d=badshop.example"));
    // Only a name above the sender's, as for DMARC, never one under it or a bare suffix.
    assert!(!says(shop, "dkim=pass header.d=news.shop.example"));
    assert!(!says(shop, "dkim=pass header.i=@news.shop.example"));
    assert!(!says("hello@bank.co.uk", "dkim=pass header.d=co.uk"));
    // The signer's domain decides; its identity is read only where the clause names no domain.
    assert!(!says("hello@bank.co.uk", "dkim=pass header.d=co.uk header.i=@bank.co.uk"));
    assert!(!says(shop, "dkim=pass header.d=bounce.example header.i=@shop.example"));
    // A signer inside a comment is the comment's text, even written as a property.
    assert!(!says(shop, "dkim=pass (header.d=shop.example) header.d=bounce.example"));
    assert!(!says(shop, "dkim=pass (identity @shop.example); spf=pass"));
    // A failed signature for the domain, beside another domain's that passes.
    assert!(!says(shop, "dkim=pass header.d=bounce.example; dkim=fail header.d=shop.example"));
    for result in ["fail", "neutral", "policy", "temperror", "permerror", "none", "passed"] {
        assert!(!says(shop, &format!("dkim={result} header.d=shop.example")), "{result}");
    }
    // Another clause's signer, and SPF, which DMARC reads too but which this does not.
    assert!(!says(shop, "dkim=pass; spf=pass header.d=shop.example"));
    assert!(!says(shop, "spf=pass smtp.mailfrom=shop.example"));
    assert!(!says(shop, "dkim=pass smtp.mailfrom=shop.example"));
}

/// Where the server gave a DMARC verdict, that verdict decides: a signature of the sender's
/// domain doesn't overrule a `fail` (strict alignment, `adkim=s`, fails a parent's signature),
/// and doesn't stand in for a `dmarc` or `compauth` clause of any result.
#[test]
fn the_servers_dmarc_verdict_is_never_overruled_by_a_signature() {
    let lumen = imap("imap.lumen.example");
    let says = |rest: &str| {
        let m = Message { from: Address::new("Shop", "news@news.shop.example"), ..message(&[&format!("mx.lumen.example; {rest}")]) };
        vouched(Some(&lumen), &m)
    };
    assert!(says("dkim=pass header.d=shop.example"));
    assert!(!says("dkim=pass header.d=shop.example; dmarc=fail (p=reject) header.from=news.shop.example"));
    assert!(!says("dmarc=fail header.from=news.shop.example; dkim=pass header.d=news.shop.example"));
    assert!(!says("dkim=pass header.d=shop.example; dmarc=none header.from=news.shop.example"));
    assert!(!says("dkim=pass header.d=shop.example; dmarc/1=temperror"));
    assert!(!says("dkim=pass header.d=shop.example; compauth=fail reason=000"));
    assert!(says("dkim=pass header.d=bounce.example; dmarc=pass header.from=news.shop.example"));
    // A DMARC filter may write its own header below a DKIM filter's, with the same id; a header
    // below can only take the signature away (its own pass is not read: it may be forged), and
    // another server's header below is not this server's verdict.
    let stacked = |below: &str| {
        let mut m =
            Message { from: Address::new("Shop", "news@news.shop.example"), ..message(&["mx.lumen.example; dkim=pass header.d=shop.example"]) };
        m.headers.push(("Authentication-Results".into(), below.into()));
        vouched(Some(&lumen), &m)
    };
    assert!(!stacked("mx.lumen.example; dmarc=fail (p=reject) header.from=news.shop.example"));
    assert!(!stacked("MX.Lumen.Example; dmarc=none header.from=news.shop.example"));
    assert!(!stacked("mx.lumen.example; dmarc=pass header.from=news.shop.example"));
    assert!(!stacked(&format!("mx.lumen.example; spf=pass (open{}", " x".repeat(10))));
    assert!(stacked("mx.other.example; dmarc=fail header.from=news.shop.example"));
    assert!(stacked("mx.lumen.example; spf=pass smtp.mailfrom=news.shop.example"));
    // A header with no authserv-id on a Microsoft account follows the same rule (Exchange Online
    // itself always writes `dmarc` and `compauth`).
    let microsoft = account(AccountKind::Microsoft, "");
    let exchange = |rest: &str| {
        let m = Message { from: Address::new("Shop", "news@shop.example"), ..message(&[rest]) };
        vouched(Some(&microsoft), &m)
    };
    assert!(exchange("spf=pass smtp.mailfrom=shop.example; dkim=pass (signature was verified) header.d=shop.example"));
    assert!(!exchange(
        "spf=fail smtp.mailfrom=shop.example; dkim=pass (signature was verified) header.d=shop.example; dmarc=fail action=quarantine header.from=shop.example; compauth=fail reason=000"
    ));
}

/// A header longer than is read loses its last, cut-off clause, and one that ends inside a
/// quoted string or a comment loses the clause left open: the signer's `d=` is the signer's to
/// choose, and the start of `shop.example.evil.example` is not `shop.example`. Nor does a
/// signature count in a header not read to its end, whose DMARC verdict may be in the rest.
#[test]
fn a_clause_cut_off_or_left_open_is_not_read() {
    let lumen = imap("imap.lumen.example");
    let says = |value: &str| vouched(Some(&lumen), &message(&[value]));
    let head = "mx.lumen.example; ";
    let cut_at = "dkim=pass header.d=shop.example";
    // One signature with a long selector fills the header up to the cut.
    let fill = 4_000 - head.len() - cut_at.len() - "dkim=pass header.s=; ".len();
    let pad = format!("dkim=pass header.s={}; ", "x".repeat(fill));
    let cut = format!("{head}{pad}{cut_at}.evil.example");
    assert_eq!(cut.chars().take(4_000).collect::<String>(), format!("{head}{pad}{cut_at}"));
    assert!(!says(&cut));
    // The same for a `dmarc` clause: the start of the domain it judged is not that domain.
    let cut_dmarc = "dmarc=pass header.from=shop.example";
    let fill = 4_000 - head.len() - cut_dmarc.len() - "dkim=pass header.s=; ".len();
    let dmarc_cut = format!("{head}dkim=pass header.s={}; {cut_dmarc}.evil.example", "x".repeat(fill));
    assert!(dmarc_cut.chars().take(4_000).collect::<String>().ends_with(cut_dmarc));
    assert!(!says(&dmarc_cut));
    // A `dmarc=pass` before the cut still counts; a signature doesn't, as the verdict may follow.
    assert!(says(&format!("{head}{PASS}; {pad}{cut_at}.evil.example")));
    assert!(!says(&format!("{head}dkim=pass header.d=shop.example; {pad}{cut_at}.evil.example")));
    // Left open: a quoted string or a comment that never ends.
    assert!(!says("mx.lumen.example; dkim=pass header.i=\"@shop.example"));
    assert!(!says("mx.lumen.example; dkim=pass header.d=shop.example (cut"));
    assert!(!says("mx.lumen.example; dkim=pass header.d=shop.example; spf=pass (open"));
    // The server's `dmarc=fail` cut off in part or wholly, past the clauses read, or left open.
    let sub = |value: &str| {
        let m = Message { from: Address::new("Shop", "news@news.shop.example"), ..message(&[value]) };
        vouched(Some(&lumen), &m)
    };
    let signed = "dkim=pass header.d=shop.example; ";
    let fail = "dmarc=fail (p=reject) header.from=news.shop.example";
    assert!(sub(&format!("{head}{signed}")));
    assert!(!sub(&format!("{head}{signed}{fail}")));
    let fill = 4_000 - head.len() - signed.len() - "dkim=pass header.s=; ".len() - 20;
    let into_fail = format!("{head}{signed}dkim=pass header.s={}; {fail}", "x".repeat(fill));
    assert!(into_fail.chars().take(4_000).collect::<String>().ends_with("; dmarc=fail (p=reject"));
    assert!(!sub(&into_fail));
    assert!(!sub(&format!("{head}{signed}{pad}{fail}")));
    assert!(!sub(&format!("{head}{signed}{}{fail}", "x; ".repeat(250))));
    assert!(!sub(&format!("{head}{signed}dmarc=fail (p=reject")));
}

/// Mail brought in from a file has no place on a server: whatever Authentication-Results it
/// carries, it is the file's own.
#[test]
fn mail_imported_from_a_file_is_never_vouched_for() {
    let lumen = imap("imap.lumen.example");
    let forged = ["mx.lumen.example; dmarc=pass header.from=shop.example"];
    let mut file = message(&forged);
    (file.uid, file.server) = (0, None);
    assert!(!vouched(Some(&lumen), &file));
    // The same with a UID on the server (IMAP), or a server's own record of it (Graph).
    file.uid = 12;
    assert!(vouched(Some(&lumen), &file));
    file.uid = 0;
    file.server = Some(mailcraft_model::sync::ServerMessage::default());
    assert!(vouched(Some(&account(AccountKind::Microsoft, "")), &file));
    // (A Microsoft account's mail from a file is just as much the file's.)
    (file.uid, file.server) = (0, None);
    assert!(!vouched(Some(&account(AccountKind::Microsoft, "")), &file));
}

/// The message as the server hands it over, read the way every message is read.
fn delivered(results: &str) -> Message {
    let raw = format!("Authentication-Results: {results}\r\nFrom: News <news@victim.example>\r\nSubject: x\r\n\r\nHello\r\n");
    Message { uid: 7, ..mailcraft_engine::mime::parse(raw.as_bytes()) }
}

/// What the server copied into its header from the sender (the envelope's address) is read as
/// the server wrote it: an encoded-word in it is no clause of the server's.
#[test]
fn what_the_server_copied_from_the_sender_is_never_its_verdict() {
    let lumen = imap("imap.lumen.example");
    // The same words, written by the server itself, would vouch.
    assert!(vouched(Some(&lumen), &delivered("mx.lumen.example; spf=pass smtp.mailfrom=x; dkim=pass header.d=victim.example")));
    assert!(vouched(Some(&lumen), &delivered("mx.lumen.example; spf=pass smtp.mailfrom=x; dmarc=pass header.from=victim.example")));
    for copied in [
        "=?us-ascii?q?x=3B_dkim=3Dpass_header.d=3Dvictim.example_x?=",
        "=?us-ascii?q?x=3B_dmarc=3Dpass_header.from=3Dvictim.example_x?=",
        "=?utf-8?b?eDsgZGtpbT1wYXNzIGhlYWRlci5kPXZpY3RpbS5leGFtcGxlIHg=?=",
    ] {
        let results = format!("mx.lumen.example; spf=pass smtp.mailfrom={copied}@sender.example");
        let m = delivered(&results);
        assert!(!vouched(Some(&lumen), &m), "{copied}");
        let kept = m.headers.iter().find(|(name, _)| name == "Authentication-Results").map(|(_, value)| value.as_str());
        assert_eq!(kept, Some(results.as_str()));
    }
    // The same through a Microsoft account, which believes a header with no id.
    let microsoft = account(AccountKind::Microsoft, "");
    let m = delivered("spf=pass smtp.mailfrom==?us-ascii?q?x=3B_dmarc=3Dpass_header.from=3Dvictim.example_x?=@sender.example");
    assert!(!vouched(Some(&microsoft), &m));
}
