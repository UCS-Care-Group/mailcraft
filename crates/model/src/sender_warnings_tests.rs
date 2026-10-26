// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sender warnings: each rule, when it says something and, as much, when it says nothing.
//!
//! The people, hosts and addresses are invented.

use proptest::prelude::*;

use crate::auth_results::{Failed, failed_check};
use crate::confusables::skeleton;
use crate::sender_warnings::{
    Facts, FactsBuilder, Level, NameReason, Warning, banner_level, check, link_mismatches, name_key, registrable, sender_domain, within_one_edit,
};
use crate::{Account, AccountKind, Address, FolderKind, Message, Security, Server};

fn imap(email: &str, name: &str) -> Account {
    Account {
        kind: AccountKind::Imap,
        email: email.into(),
        display_name: name.into(),
        incoming: Some(Server { host: "imap.lumen.example".into(), port: 993, security: Security::Tls }),
        ..Account::default()
    }
}

/// Avery at Lumen, who knows Maya (a contact), Dev at example.org and Lin at Harbor Works (both
/// written to).
fn facts() -> Facts {
    let me = imap("avery@lumen.example", "Avery Quinn");
    let mut b = FactsBuilder::default().accounts([&me]);
    b.known("maya@lumen.example", "Maya Okafor");
    b.known("dev.patel@example.org", "Dev Patel");
    b.known("lin.zhao@harborworks.example", "Lin Zhao");
    b.known("friend@gmail.com", "Sam Friend");
    b.build()
}

fn from(name: &str, email: &str) -> Message {
    Message { from: Address::new(name, email), uid: 3, ..Message::default() }
}

fn with_results(mut m: Message, value: &str) -> Message {
    m.headers = vec![("Authentication-Results".into(), value.into())];
    m
}

fn warnings(m: &Message) -> Vec<Warning> {
    check(m, Some(&imap("avery@lumen.example", "Avery Quinn")), Some(FolderKind::Inbox), &facts(), None::<Vec<(&str, &str)>>)
}

fn kinds(m: &Message) -> Vec<&'static str> {
    warnings(m).iter().map(Warning::kind).collect()
}

// ---- failed check ----

#[test]
fn a_dmarc_fail_from_the_own_server_is_a_warning() {
    let account = imap("avery@lumen.example", "Avery Quinn");
    let m =
        with_results(from("Shop", "news@shop.example"), "mx.lumen.example; spf=pass smtp.mailfrom=shop.example; dmarc=fail header.from=shop.example");
    assert_eq!(failed_check(Some(&account), &m), Some(Failed::Dmarc));
    // A clause that names no header.from judged the From: it had.
    let m = with_results(from("Shop", "news@shop.example"), "mx.lumen.example; dmarc=fail action=quarantine");
    assert_eq!(failed_check(Some(&account), &m), Some(Failed::Dmarc));
    assert_eq!(kinds(&m), ["failedCheck", "external"]);
    // Exchange Online writes no id.
    let microsoft = Account { kind: AccountKind::Microsoft, ..account.clone() };
    let m =
        with_results(from("Shop", "news@shop.example"), "spf=fail smtp.mailfrom=shop.example; dmarc=fail action=oreject header.from=shop.example");
    assert_eq!(failed_check(Some(&microsoft), &m), Some(Failed::Dmarc));
}

#[test]
fn spf_counts_only_without_a_dmarc_verdict_and_for_the_senders_domain() {
    let account = imap("avery@lumen.example", "Avery Quinn");
    let says = |value: &str| failed_check(Some(&account), &with_results(from("Shop", "news@shop.example"), &format!("mx.lumen.example; {value}")));
    assert_eq!(says("spf=fail smtp.mailfrom=news@shop.example"), Some(Failed::Spf));
    assert_eq!(says("spf=fail smtp.mailfrom=bounce.shop.example"), Some(Failed::Spf));
    // DMARC said its piece: it passed, so SPF's fail is not the sender's.
    assert_eq!(says("spf=fail smtp.mailfrom=shop.example; dmarc=pass header.from=shop.example"), None);
    // Another domain's SPF.
    assert_eq!(says("spf=fail smtp.mailfrom=other.example"), None);
    assert_eq!(says("spf=softfail smtp.mailfrom=shop.example"), None);
    assert_eq!(says("spf=fail"), None);
}

#[test]
fn a_failed_check_never_comes_from_what_the_sender_could_write() {
    let account = imap("avery@lumen.example", "Avery Quinn");
    let says = |values: &[&str], uid: u32| {
        let mut m = from("Shop", "news@shop.example");
        m.uid = uid;
        m.headers = values.iter().map(|v| ("Authentication-Results".to_string(), v.to_string())).collect();
        failed_check(Some(&account), &m)
    };
    let fail = "dmarc=fail header.from=shop.example";
    // Nothing said: nothing.
    assert_eq!(says(&[], 3), None);
    // Another server's header on top, or the own server's below another.
    assert_eq!(says(&[&format!("mx.other.example; {fail}")], 3), None);
    assert_eq!(says(&["forged.example; dmarc=pass", &format!("mx.lumen.example; {fail}")], 3), None);
    // A message from a file is nobody's word.
    assert_eq!(says(&[&format!("mx.lumen.example; {fail}")], 0), None);
    // The verdict in a comment or quoted string.
    assert_eq!(says(&["mx.lumen.example; dkim=pass (dmarc=fail header.from=shop.example)"], 3), None);
    assert_eq!(says(&["mx.lumen.example; dkim=pass reason=\"dmarc=fail\""], 3), None);
    // Of another domain.
    assert_eq!(says(&["mx.lumen.example; dmarc=fail header.from=elsewhere.example"], 3), None);
    // A pass, or no account.
    assert_eq!(says(&["mx.lumen.example; dmarc=pass header.from=shop.example"], 3), None);
    let m = with_results(from("Shop", "news@shop.example"), &format!("mx.lumen.example; {fail}"));
    assert_eq!(failed_check(None, &m), None);
}

// ---- a name that hides another address ----

#[test]
fn a_name_that_carries_another_address() {
    let m = from("billing@cloudline.example", "x@billing-desk.example");
    assert!(
        matches!(&warnings(&m)[..], [Warning::NameHidesAddress { reason: NameReason::AddressInName(a), .. }, ..] if a == "billing@cloudline.example")
    );
    let m = from("Cloudline (billing@cloudline.example) team", "x@billing-desk.example");
    assert!(kinds(&m).contains(&"nameHidesAddress"));
    // The sender's own address in the name says nothing new.
    for name in ["dev.patel@example.org", "Dev Patel <Dev.Patel@example.org>", "Dev Patel", "", "@", "a@b", "x@.example"] {
        assert!(!kinds(&from(name, "dev.patel@example.org")).contains(&"nameHidesAddress"), "{name}");
    }
}

#[test]
fn a_known_name_or_the_owners_on_another_address() {
    let m = from("Maya Okafor", "maya.okafor@freemail.example");
    assert!(matches!(&warnings(&m)[..], [Warning::NameHidesAddress { reason: NameReason::KnownName, .. }, ..]));
    // Case, quotes and spaces don't hide it.
    let m = from("  \"maya   OKAFOR\" ", "maya.okafor@freemail.example");
    assert!(kinds(&m).contains(&"nameHidesAddress"));
    let m = from("Avery Quinn", "avery.quinn@freemail.example");
    assert!(matches!(&warnings(&m)[..], [Warning::NameHidesAddress { reason: NameReason::OwnName, .. }, ..]));
    // On the person's own address: nothing.
    assert!(kinds(&from("Maya Okafor", "MAYA@lumen.example")).is_empty());
    // One word is nobody's name in particular; a stranger's name is nobody the owner knows.
    assert!(!kinds(&from("Maya", "maya@freemail.example")).contains(&"nameHidesAddress"));
    assert!(!kinds(&from("Robin Hale", "robin@freemail.example")).contains(&"nameHidesAddress"));
}

#[test]
fn names_compare_by_their_words() {
    assert_eq!(name_key(" Maya  Okafor "), Some("maya okafor".into()));
    assert_eq!(name_key("'Maya Okafor'"), Some("maya okafor".into()));
    assert_eq!(name_key("Maya"), None);
    assert_eq!(name_key("Maya 2"), None);
    assert_eq!(name_key("maya@lumen.example x"), None);
}

// ---- external ----

#[test]
fn external_is_a_tag_and_only_with_an_organisation() {
    let m = from("Lin Zhao", "lin.zhao@harborworks.example");
    assert_eq!(warnings(&m), [Warning::External { domain: "harborworks.example".into() }]);
    assert_eq!(banner_level(&warnings(&m)), None);
    // Inside: the organisation's domain and the names under it.
    assert!(kinds(&from("Maya Okafor", "maya@lumen.example")).is_empty());
    assert!(kinds(&from("Lumen News", "news@mail.lumen.example")).is_empty());
    // An owner with only personal accounts has no organisation.
    let personal = FactsBuilder::default().accounts([&imap("avery.quinn@gmail.com", "Avery Quinn")]).build();
    let m = from("Lin Zhao", "lin.zhao@harborworks.example");
    assert!(check(&m, None, Some(FolderKind::Inbox), &personal, None::<Vec<(&str, &str)>>).is_empty());
    assert_eq!(personal.organisation().count(), 0);
}

// ---- lookalike ----

#[test]
fn one_character_from_a_known_domain_is_a_lookalike() {
    for (sender, like) in [
        ("x@harborwork.example", "harborworks.example"),
        ("x@harborworkss.example", "harborworks.example"),
        // Two letters swapped side by side are one edit.
        ("x@harborwokrs.example", "harborworks.example"),
        ("x@harborw0rks.example", "harborworks.example"),
        ("x@lumen-example.example", ""),
        ("x@lunen.example", "lumen.example"),
        ("x@billing.lumem.example", "lumen.example"),
    ] {
        let found = warnings(&from("Someone", sender)).into_iter().find_map(|w| match w {
            Warning::LookalikeSender { like, .. } => Some(like),
            _ => None,
        });
        assert_eq!(found.as_deref(), Some(like).filter(|like| !like.is_empty()), "{sender}");
    }
}

#[test]
fn letters_drawn_alike_make_a_lookalike() {
    // A Cyrillic "о" for the Latin one, as it comes over the wire and as it is written.
    let m = from("Lin", "lin@harb\u{43e}rworks.example");
    let found = warnings(&m);
    assert!(
        matches!(&found[..], [Warning::LookalikeSender { domain, like }, ..] if domain.starts_with("xn--") && like == "harborworks.example"),
        "{found:?}"
    );
    let wire = mailcraft_link::WebLink::parse("http://harb\u{43e}rworks.example/").map(|l| l.host().to_string()).unwrap_or_default();
    assert!(kinds(&from("Lin", &format!("lin@{wire}"))).contains(&"lookalikeSender"));
    // "rn" for "m".
    assert!(kinds(&from("Lumen", "it@lurnen.example")).contains(&"lookalikeSender"));
    assert_eq!(skeleton("rnicrosoft"), skeleton("microsoft"));
    assert_eq!(skeleton("p\u{430}yp\u{430}l"), "paypal");
    assert_eq!(skeleton("\u{ff50}\u{ff41}ypal"), "paypal");
    assert_eq!(skeleton("pay\u{200b}pal"), "paypal");
}

#[test]
fn a_lookalike_never_fires_for_what_the_owner_knows_or_short_names() {
    // Known domains themselves, and names under them.
    for sender in ["x@harborworks.example", "x@mail.harborworks.example", "x@example.org", "x@lumen.example"] {
        assert!(!kinds(&from("Someone", sender)).contains(&"lookalikeSender"), "{sender}");
    }
    // Short names are a letter from many others.
    let short = FactsBuilder::default().accounts([&imap("avery@acme.example", "Avery Quinn")]).build();
    let m = from("Someone", "x@acne.example");
    assert!(check(&m, None, Some(FolderKind::Inbox), &short, None::<Vec<(&str, &str)>>).iter().all(|w| w.kind() != "lookalikeSender"));
    // A public mail provider is nobody's lookalike: gmail.com is known, gmx.com is a provider.
    assert!(!kinds(&from("Someone", "x@gmx.com")).contains(&"lookalikeSender"));
    // Far from everything.
    assert!(!kinds(&from("Someone", "x@cloudline.example")).contains(&"lookalikeSender"));
}

#[test]
fn edits_and_registrable_parts() {
    let c = |s: &str| s.chars().collect::<Vec<_>>();
    assert!(within_one_edit(&c("lumen"), &c("lumen")));
    assert!(within_one_edit(&c("lumen"), &c("lumon")));
    assert!(within_one_edit(&c("lumen"), &c("lumn")));
    assert!(within_one_edit(&c("lumen"), &c("lumens")));
    assert!(within_one_edit(&c("lumen"), &c("lmuen")));
    assert!(!within_one_edit(&c("lumen"), &c("lmune")));
    assert!(!within_one_edit(&c("lumen"), &c("lu")));
    assert!(within_one_edit(&c(""), &c("a")));
    assert_eq!(registrable("mail.lumen.example"), "lumen.example");
    assert_eq!(registrable("lumen.co.uk"), "lumen.co.uk");
    assert_eq!(registrable("mail.lumen.co.uk"), "lumen.co.uk");
    assert_eq!(registrable("lumen.github.io"), "lumen.github.io");
    assert_eq!(registrable("example"), "example");
    // A name over the wire is read back into its letters, whatever its case.
    let wire = mailcraft_link::WebLink::parse("http://harb\u{43e}rworks.example/").map(|l| l.host().to_uppercase()).unwrap_or_default();
    assert_eq!(sender_domain(&format!("x@{wire}.")), Some("harb\u{43e}rworks.example".into()));
    assert_eq!(sender_domain("no address"), None);
    assert_eq!(sender_domain("x@a..b"), None);
}

// ---- links ----

#[test]
fn a_link_that_names_one_place_and_goes_to_another() {
    let found = link_mismatches([
        ("https://lumen.example/pay", "https://lumen-pay.example/login"),
        ("www.cloudline.example", "http://cloudline.example.evil.example/"),
        ("cloudline.example/billing", "https://203.0.113.9/billing"),
    ]);
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(matches!(&found[0], Warning::LinkMismatch { text_host, host, .. } if text_host == "lumen.example" && host == "lumen-pay.example"));
    assert!(found.iter().all(|w| w.level() == Level::Caution));
}

#[test]
fn a_link_never_fires_for_words_the_same_place_or_what_is_no_web_address() {
    let found = link_mismatches([
        ("Click here", "https://evil.example/"),
        ("Open the billing page", "https://cloudline.example/billing"),
        ("https://cloudline.example/billing", "https://cloudline.example/billing?x=1"),
        ("www.cloudline.example", "https://cloudline.example/"),
        ("cloudline.example", "https://WWW.Cloudline.Example./x"),
        ("cloudline.example", "https://pay.cloudline.example/"),
        ("billing@cloudline.example", "mailto:billing@evil.example"),
        ("cloudline.example", "mailto:x@evil.example"),
        ("cloudline.example", "cid:logo"),
        ("version 1.2", "https://evil.example/"),
        ("3.14", "https://evil.example/"),
        ("", "https://evil.example/"),
        ("Q4 report.pdf now", "https://evil.example/"),
    ]);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn many_links_are_bounded() {
    let links: Vec<(String, String)> = (0..5_000).map(|i| (format!("site{i}.example"), format!("https://other{i}.example/"))).collect();
    let found = link_mismatches(links.iter().map(|(a, b)| (a.as_str(), b.as_str())));
    assert_eq!(found.len(), 20);
}

// ---- never checked ----

#[test]
fn own_mail_and_outgoing_folders_are_never_checked() {
    let facts = facts();
    let account = imap("avery@lumen.example", "Avery Quinn");
    let bad = with_results(from("maya@lumen.example", "x@lumem.example"), "mx.lumen.example; dmarc=fail");
    for kind in [FolderKind::Sent, FolderKind::Drafts, FolderKind::Outbox] {
        assert!(check(&bad, Some(&account), Some(kind), &facts, Some([("lumen.example", "https://evil.example/")])).is_empty());
    }
    // Junk E-mail is checked.
    assert!(!check(&bad, Some(&account), Some(FolderKind::Junk), &facts, None::<Vec<(&str, &str)>>).is_empty());
    let own = from("Someone Else", "Avery@Lumen.example");
    assert!(check(&own, Some(&account), Some(FolderKind::Inbox), &facts, Some([("lumen.example", "https://evil.example/")])).is_empty());
}

#[test]
fn the_banner_takes_the_heaviest() {
    let m = with_results(from("Lin", "lin@harborwork.example"), "mx.lumen.example; dmarc=fail");
    let found = warnings(&m);
    assert_eq!(found.iter().map(Warning::kind).collect::<Vec<_>>(), ["failedCheck", "lookalikeSender", "external"]);
    assert_eq!(banner_level(&found), Some(Level::Warning));
    assert_eq!(banner_level(&found[1..]), Some(Level::Caution));
    let json = found[1].json();
    assert_eq!(json["kind"], "lookalikeSender");
    assert_eq!(json["level"], "caution");
    assert_eq!(json["like"], "harborworks.example");
}

proptest! {
    #[test]
    fn hostile_names_addresses_and_headers_never_panic(
        name in ".{0,300}",
        email in ".{0,300}",
        header in ".{0,600}",
        text in ".{0,200}",
        href in ".{0,200}",
    ) {
        let facts = facts();
        let account = imap("avery@lumen.example", "Avery Quinn");
        let mut m = from(&name, &email);
        m.headers = vec![("Authentication-Results".into(), header.clone())];
        let _ = check(&m, Some(&account), Some(FolderKind::Inbox), &facts, Some([(text.as_str(), href.as_str())]));
        let _ = skeleton(&name);
        let _ = sender_domain(&email);
        let mut b = FactsBuilder::default();
        b.known(&email, &name);
        let _ = b.build();
    }
}
