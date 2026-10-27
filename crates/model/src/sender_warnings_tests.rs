// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sender warnings: each rule, when it says something and, as much, when it says nothing.
//!
//! The people, hosts and addresses are invented.

use proptest::prelude::*;

use crate::auth_results::{Failed, failed_check};
use crate::confusables::{as_seen, skeleton};
use crate::sender_warnings::{
    Facts, FactsBuilder, Level, NameReason, Warning, banner_level, check, link_mismatches, link_targets, name_key, registrable, sender_domain,
    within_one_edit,
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
    // A full-width at sign reads as one.
    let m = from("billing\u{ff20}cloudline.example", "x@billing-desk.example");
    assert!(
        matches!(&warnings(&m)[..], [Warning::NameHidesAddress { reason: NameReason::AddressInName(a), .. }, ..] if a == "billing@cloudline.example")
    );
    assert!(!kinds(&from("dev.patel\u{ff20}example\u{ff0e}org", "dev.patel@example.org")).contains(&"nameHidesAddress"));
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
fn a_link_rewritten_by_safe_links_is_compared_as_the_link_it_stands_for() {
    let safe = |target: &str| format!("https://eur01.safelinks.protection.outlook.com/?url={target}&data=05%7C02&reserved=0");
    let agrees = safe("https%3A%2F%2Fpay.lumen.example%2Finvoice%3Fn%3D4");
    assert!(link_mismatches([("lumen.example", agrees.as_str())]).is_empty());
    let disagrees = safe("https%3A%2F%2Flumen-pay.example%2Flogin");
    let found = link_mismatches([("https://lumen.example/pay", disagrees.as_str())]);
    assert_eq!(found.len(), 1);
    let Some(Warning::LinkMismatch { host, url, .. }) = found.first() else { panic!("{found:?}") };
    assert_eq!(host, "lumen-pay.example");
    assert_eq!(url, "https://lumen-pay.example/login");
    assert_eq!(link_targets(&disagrees).iter().map(|link| link.host()).collect::<Vec<_>>(), ["lumen-pay.example"]);
}

#[test]
fn a_link_through_a_service_that_rewrites_links_is_not_compared() {
    for href in [
        "https://urldefense.com/v3/__https://lumen.example/pay__;!!abc$",
        "https://urldefense.proofpoint.com/v2/url?u=https-3A__lumen.example&d=x",
        "https://protect-eu.mimecast.com/s/AbCdEf?domain=lumen.example",
    ] {
        assert!(link_mismatches([("https://lumen.example/pay", href)]).is_empty(), "{href}");
    }
    // Only Proofpoint's rewriting host: the rest of its site is compared as any is.
    let found = link_mismatches([("lumen.example", "https://www.proofpoint.com/us/login")]);
    assert!(matches!(found.first(), Some(Warning::LinkMismatch { host, .. }) if host == "proofpoint.com"), "{found:?}");
}

#[test]
fn a_host_that_only_looks_like_safe_links_is_not_unwrapped() {
    for href in [
        "https://safelinks.protection.outlook.com.evil.example/?url=https%3A%2F%2Flumen.example",
        "https://eur01-safelinks-protection-outlook.example/?url=https%3A%2F%2Flumen.example",
        "https://urldefense.com.evil.example/?url=https%3A%2F%2Flumen.example",
    ] {
        let found = link_mismatches([("lumen.example", href)]);
        assert_eq!(found.len(), 1, "{href}");
        assert!(matches!(found.first(), Some(Warning::LinkMismatch { text_host, .. }) if text_host == "lumen.example"));
    }
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
fn mail_in_the_owners_name_that_failed_its_check_is_still_warned_of() {
    let facts =
        FactsBuilder::default().accounts_by([("avery@lumen.example", "Avery Quinn", false), ("help@lumen.example", "Help Desk", true)]).build();
    let account = imap("avery@lumen.example", "Avery Quinn");
    for address in ["Avery@Lumen.example", "help@lumen.example"] {
        let forged = with_results(from("Someone Else", address), "mx.lumen.example; dmarc=fail header.from=lumen.example");
        // Only the failed check: the other rules still leave the owner's own mail alone.
        let found = check(&forged, Some(&account), Some(FolderKind::Inbox), &facts, Some([("lumen.example", "https://evil.example/")]));
        assert_eq!(found, [Warning::FailedCheck(Failed::Dmarc)], "{address}");
        let fine = with_results(from("Someone Else", address), "mx.lumen.example; dmarc=pass header.from=lumen.example");
        assert!(check(&fine, Some(&account), Some(FolderKind::Inbox), &facts, None::<Vec<(&str, &str)>>).is_empty());
    }
}

#[test]
fn a_sender_written_in_other_letters_still_fails_its_check() {
    // A Cyrillic "е" in the mailbox name; the server judged the domain all the same.
    let m = with_results(from("IT Desk", "itd\u{435}sk@lumen.example"), "mx.lumen.example; dmarc=fail header.from=lumen.example");
    assert!(warnings(&m).contains(&Warning::FailedCheck(Failed::Dmarc)));
    // The server may name the domain in letters or in `xn--`, the sender either way too.
    let m = with_results(from("Shop", "orders@b\u{fc}cher.example"), "mx.lumen.example; dmarc=fail header.from=xn--bcher-kva.example");
    assert!(warnings(&m).contains(&Warning::FailedCheck(Failed::Dmarc)));
    let m = with_results(from("Shop", "orders@xn--bcher-kva.example"), "mx.lumen.example; dmarc=fail header.from=b\u{fc}cher.example");
    assert!(warnings(&m).contains(&Warning::FailedCheck(Failed::Dmarc)));
    // Another domain's failure is still not the sender's.
    let m = with_results(from("IT Desk", "itd\u{435}sk@lumen.example"), "mx.lumen.example; dmarc=fail header.from=other.example");
    assert!(!warnings(&m).iter().any(|w| matches!(w, Warning::FailedCheck(_))));
}

#[test]
fn exchanges_composite_pass_overrides_a_dmarc_failure() {
    let m = with_results(
        from("Lin", "lin@harborworks.example"),
        "mx.lumen.example; dmarc=fail action=oreject header.from=harborworks.example; compauth=pass reason=105",
    );
    assert!(!warnings(&m).iter().any(|w| matches!(w, Warning::FailedCheck(_))));
    let m = with_results(
        from("Lin", "lin@harborworks.example"),
        "mx.lumen.example; dmarc=fail action=oreject header.from=harborworks.example; compauth=fail reason=000",
    );
    assert!(warnings(&m).contains(&Warning::FailedCheck(Failed::Dmarc)));
}

#[test]
fn a_label_that_is_not_what_it_would_be_written_as_is_read_as_sent() {
    // `xn--lumen-` is no Punycode of anything; it must not pass for `lumen`.
    assert_eq!(sender_domain("it@xn--lumen-.example").as_deref(), Some("xn--lumen-.example"));
    assert_eq!(sender_domain("pay@xn--paypal-.com").as_deref(), Some("xn--paypal-.com"));
    let m = from("IT Desk", "it@xn--lumen-.example");
    assert!(warnings(&m).iter().any(|w| matches!(w, Warning::External { domain } if domain == "xn--lumen-.example")));
    // A real one is still read.
    assert_eq!(sender_domain("x@xn--bcher-kva.example").as_deref(), Some("b\u{fc}cher.example"));
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

// ---- what draws nothing, and letters drawn like others ----

#[test]
fn a_known_name_with_an_invisible_or_lookalike_letter_is_still_known() {
    // Maya Okafor is a contact at maya@lumen.example.
    for name in ["Maya\u{200b} Okafor", "Maya\u{200d} Okafor", "M\u{430}ya Okafor", "Maya Okafor."] {
        let m = from(name, "maya.okafor@freemail.example");
        assert!(matches!(&warnings(&m)[..], [Warning::NameHidesAddress { reason: NameReason::KnownName, .. }, ..]), "{name:?}: {:?}", kinds(&m));
    }
}

#[test]
fn an_address_in_a_name_with_an_invisible_or_lookalike_mark_is_still_one() {
    for name in ["maya@lu\u{200b}men.example", "maya@lu\u{ad}men.example", "maya@lumen\u{2024}example", "maya\u{fe6b}lumen.example"] {
        let m = from(name, "maya.okafor@freemail.example");
        assert!(
            matches!(&warnings(&m)[..], [Warning::NameHidesAddress { reason: NameReason::AddressInName(a), .. }, ..] if a == "maya@lumen.example"),
            "{name:?}: {:?}",
            kinds(&m)
        );
    }
}

#[test]
fn a_link_text_with_an_invisible_letter_names_the_host_it_shows() {
    let found = link_mismatches([("www.lumen\u{200b}.example/pay", "https://evil.example/")]);
    assert!(
        matches!(&found[..], [Warning::LinkMismatch { text_host, host, .. }] if text_host == "lumen.example" && host == "evil.example"),
        "{found:?}"
    );
}

#[test]
fn a_name_reads_as_it_is_seen() {
    // The window colours a name that looks like an address with this same reading.
    for name in ["maya@lu\u{200b}men.example", "maya\u{fe6b}lumen.example", "maya\u{ff20}lumen.example"] {
        assert!(as_seen(name, 1_000).contains('@'), "{name:?}");
    }
    assert_eq!(as_seen("maya@lu\u{ad}men\u{2024}example", 1_000), "maya@lumen.example");
    // The limit counts what is kept: what isn't seen takes no room, and blanks run together.
    assert_eq!(as_seen("M\u{200d}aya\u{e0041}", 2), "Ma");
    assert_eq!(as_seen("M\u{200d}aya\u{e0041}", 9), "Maya");
    assert_eq!(as_seen("Maya \u{3164}\u{2800}  Okafor", 100), "Maya Okafor");
    assert_eq!(name_key("\u{201c}M\u{430}ya\u{201d}  Okafor."), name_key("Maya Okafor"));
    assert_eq!(name_key("Maya\u{200b}"), None);
}

// ---- a domain that asks for nothing to be done ----

#[test]
fn a_dmarc_failure_with_policy_none_is_a_caution() {
    let account = imap("avery@lumen.example", "Avery Quinn");
    let says = |value: &str| failed_check(Some(&account), &with_results(from("Shop", "news@shop.example"), &format!("mx.lumen.example; {value}")));
    assert_eq!(says("dmarc=fail (p=NONE sp=NONE dis=NONE) header.from=shop.example"), Some(Failed::DmarcPolicyNone));
    assert_eq!(says("dmarc=fail policy.dmarc=none header.from=shop.example"), Some(Failed::DmarcPolicyNone));
    // Red for any other policy, or none said.
    assert_eq!(says("dmarc=fail (p=QUARANTINE sp=NONE dis=QUARANTINE) header.from=shop.example"), Some(Failed::Dmarc));
    assert_eq!(says("dmarc=fail (p=REJECT) header.from=shop.example"), Some(Failed::Dmarc));
    assert_eq!(says("dmarc=fail header.from=shop.example"), Some(Failed::Dmarc));
    assert_eq!(says("spf=fail smtp.mailfrom=shop.example"), Some(Failed::Spf));
    // A comment never gives a property: its `header.from=` is not the judged domain.
    assert_eq!(says("dmarc=fail (header.from=shop.example) header.from=other.example"), None);
    let m = with_results(from("Shop", "news@shop.example"), "mx.lumen.example; dmarc=fail (p=NONE sp=NONE dis=NONE) header.from=shop.example");
    let found = warnings(&m);
    assert_eq!(found.first(), Some(&Warning::FailedCheck(Failed::DmarcPolicyNone)));
    assert_eq!(banner_level(&found), Some(Level::Caution));
    let json = found.first().map(Warning::json).unwrap_or_default();
    assert_eq!((json["kind"].as_str(), json["level"].as_str(), json["policy"].as_str()), (Some("failedCheck"), Some("caution"), Some("none")));
}

// ---- names that are not compared ----

#[test]
fn a_list_writing_a_members_name_is_not_a_hidden_name() {
    let m = from("Maya Okafor via Lumen Team", "team@groups.example");
    assert!(!kinds(&m).contains(&"nameHidesAddress"));
    // An address in the name of a list's mail, with the list's own header, is the member's.
    let mut m = from("maya@lumen.example via Lumen Team", "team@groups.example");
    assert!(kinds(&m).contains(&"nameHidesAddress"));
    m.headers = vec![("List-Id".into(), "Lumen Team <team.groups.example>".into())];
    assert!(!kinds(&m).contains(&"nameHidesAddress"));
    // Without the "via", a list's mail in a contact's name is still warned of.
    let mut m = from("Maya Okafor", "team@groups.example");
    m.headers = vec![("List-Post".into(), "<mailto:team@groups.example>".into())];
    assert!(kinds(&m).contains(&"nameHidesAddress"));
}

#[test]
fn a_mail_provider_is_nobodys_lookalike() {
    // email.com is one letter from gmail.com, a known correspondent's.
    assert!(!kinds(&from("Sam", "sam@email.com")).contains(&"lookalikeSender"));
}

// ---- rewritten links ----

#[test]
fn a_url_defense_link_is_compared_as_the_link_it_stands_for() {
    let away = [
        "https://urldefense.proofpoint.com/v2/url?u=https-3A__lumen-2Dpay.example_login&d=DwMF&c=abc",
        "https://urldefense.com/v3/__https://lumen-pay.example/login__;!!abc$",
    ];
    for href in away {
        let found = link_mismatches([("lumen.example", href)]);
        assert!(matches!(&found[..], [Warning::LinkMismatch { host, .. }] if host == "lumen-pay.example"), "{href}: {found:?}");
    }
    let home = [
        "https://urldefense.proofpoint.com/v2/url?u=https-3A__pay.lumen.example_invoice-3Fn-3D4&d=DwMF",
        "https://urldefense.com/v3/__https://pay.lumen.example/invoice__;!!abc$",
    ];
    for href in home {
        assert!(link_mismatches([("lumen.example", href)]).is_empty(), "{href}");
    }
    // What can't be read whole is not compared, and never a panic.
    for href in [
        "https://urldefense.com/v3/__https://lumen-pay.exa*ple/login__;bQ!!abc$",
        "https://urldefense.com/v3/__https://lumen-pay.example/login",
        "https://urldefense.com/v3/",
        "https://urldefense.com/",
        "https://urldefense.proofpoint.com/v2/url?d=DwMF",
        "https://urldefense.proofpoint.com/v2/url?u=-ZZ-",
        "https://urldefense.proofpoint.com/v1/url?u=https-3A__lumen-2Dpay.example",
        "https://urldefense.proofpoint.com/v2/url?u=",
        "https://urldefense.proofpoint.com/v2/url?u=-",
    ] {
        assert!(link_mismatches([("lumen.example", href)]).is_empty(), "{href}");
    }
}

#[test]
fn safe_links_are_read_in_any_case_and_every_url_they_name_is_compared() {
    let href = "https://eur01.safelinks.protection.outlook.com/?URL=https%3A%2F%2Flumen-pay.example%2F&data=05";
    assert!(matches!(&link_mismatches([("lumen.example", href)])[..], [Warning::LinkMismatch { host, .. }] if host == "lumen-pay.example"));
    let twice = "https://eur01.safelinks.protection.outlook.com/?url=https%3A%2F%2Flumen.example%2F&url=https%3A%2F%2Flumen-pay.example%2F";
    let found = link_mismatches([("lumen.example", twice)]);
    assert!(matches!(&found[..], [Warning::LinkMismatch { host, .. }] if host == "lumen-pay.example"), "{found:?}");
    assert_eq!(link_targets(twice).len(), 2);
}

#[test]
fn links_alike_in_their_first_characters_are_told_apart() {
    let start = format!("https://evil.example/{}", "a".repeat(300));
    let (one, two) = (format!("{start}/1"), format!("{start}/2"));
    let found = link_mismatches([("lumen.example", one.as_str()), ("lumen.example", two.as_str()), ("lumen.example", one.as_str())]);
    assert_eq!(found.len(), 2);
    assert!(matches!(found.first(), Some(Warning::LinkMismatch { url, .. }) if *url == one));
    let json = found.first().map(Warning::json).unwrap_or_default();
    assert!(json["url"].as_str().is_some_and(|url| url.chars().count() <= 200));
}

// ---- padding, capitals, blank fillers, links wrapped twice ----

fn hides_a_name(name: &str) -> bool {
    kinds(&from(name, "maya.okafor@freemail.example")).contains(&"nameHidesAddress")
}

#[test]
fn padding_of_what_draws_nothing_hides_nothing() {
    let pad = |n: usize| "\u{200b}".repeat(n);
    assert!(hides_a_name(&format!("{}Maya Okafor", pad(1_000))));
    assert!(hides_a_name(&format!("{}maya@lumen.example", pad(1_000))));
    assert!(hides_a_name(&format!("{}Someone", pad(70_000))));
    let evil = "https://evil.example/";
    let leading = format!("{}www.lumen.example/pay", pad(2_049));
    let trailing = format!("www.lumen.example/pay{}", pad(3_000));
    for text in [leading.as_str(), trailing.as_str()] {
        assert!(matches!(&link_mismatches([(text, evil)])[..], [Warning::LinkMismatch { host, .. }] if host == "evil.example"), "{}", text.len());
    }
    // A link's text made mostly of what isn't shown is no plain text either.
    let hidden = format!("{}Pay", pad(70_000));
    assert_eq!(link_mismatches([(hidden.as_str(), evil)]).len(), 1);
    // A word or two that a joiner holds together is nothing to say.
    assert!(!hides_a_name("Ma\u{200d}ya Someone"));
    assert!(link_mismatches([("Click\u{200b} here", evil)]).is_empty());
}

#[test]
fn capitals_drawn_like_latin_ones_are_read_as_them() {
    let mut b = FactsBuilder::default().accounts([&imap("avery@lumen.example", "Avery Quinn")]);
    b.known("maya@lumen.example", "Maya Okafor");
    b.known("nick@lumen.example", "Nick Hale");
    b.known("ben@lumen.example", "Ben Ito");
    let facts = b.build();
    let account = imap("avery@lumen.example", "Avery Quinn");
    for name in ["\u{41c}aya Okafor", "\u{39c}aya Okafor", "\u{39d}ick Hale", "\u{412}en Ito", "\u{1d40c}aya Okafor", "\u{1d5ac}aya \u{1d5ae}kafor"] {
        let m = from(name, "x@freemail.example");
        let found = check(&m, Some(&account), Some(FolderKind::Inbox), &facts, None::<Vec<(&str, &str)>>);
        assert!(found.iter().any(|w| matches!(w, Warning::NameHidesAddress { reason: NameReason::KnownName, .. })), "{name:?}: {found:?}");
    }
}

#[test]
fn a_blank_filler_is_a_gap_between_words() {
    for gap in ['\u{3164}', '\u{115f}', '\u{ffa0}', '\u{2800}'] {
        let m = from(&format!("Maya{gap}Okafor"), "maya.okafor@freemail.example");
        assert!(matches!(&warnings(&m)[..], [Warning::NameHidesAddress { reason: NameReason::KnownName, .. }, ..]), "{gap:?}: {:?}", kinds(&m));
    }
}

#[test]
fn more_full_stops_that_are_drawn_as_one() {
    for dot in ['\u{a4f8}', '\u{701}', '\u{6d4}', '\u{2e31}'] {
        let m = from(&format!("maya@lumen{dot}example"), "maya.okafor@freemail.example");
        assert!(
            matches!(&warnings(&m)[..], [Warning::NameHidesAddress { reason: NameReason::AddressInName(a), .. }, ..] if a == "maya@lumen.example"),
            "{dot:?}"
        );
    }
}

#[test]
fn safe_links_round_another_rewriter_are_unwrapped_once_more() {
    let safe = |target: &str| format!("https://eur01.safelinks.protection.outlook.com/?url={target}&data=05");
    let home = safe("https%3A%2F%2Furldefense.com%2Fv3%2F__https%3A%2F%2Flumen.example%2Fpay__%3B%21%21abc%24");
    assert!(link_mismatches([("lumen.example", home.as_str())]).is_empty());
    let away = safe("https%3A%2F%2Furldefense.com%2Fv3%2F__https%3A%2F%2Flumen-pay.example%2Flogin__%3B%21%21abc%24");
    assert!(matches!(&link_mismatches([("lumen.example", away.as_str())])[..], [Warning::LinkMismatch { host, .. }] if host == "lumen-pay.example"));
    let mimecast = safe("https%3A%2F%2Fprotect-eu.mimecast.com%2Fs%2FAbCdEf");
    assert!(link_mismatches([("lumen.example", mimecast.as_str())]).is_empty());
}

#[test]
fn only_the_registered_policy_properties_are_read() {
    let account = imap("avery@lumen.example", "Avery Quinn");
    let says = |value: &str| failed_check(Some(&account), &with_results(from("Shop", "news@shop.example"), &format!("mx.lumen.example; {value}")));
    assert_eq!(says("dmarc=fail policy.published-domain-policy=none header.from=shop.example"), Some(Failed::DmarcPolicyNone));
    assert_eq!(says("dmarc=fail policy.whatever=none header.from=shop.example"), Some(Failed::Dmarc));
    // Exchange Online's `action=none` says what it did, not the domain's policy: still red.
    assert_eq!(says("dmarc=fail action=none header.from=shop.example"), Some(Failed::Dmarc));
}

#[test]
fn a_last_clause_cut_off_or_left_open_is_not_read() {
    let account = imap("avery@lumen.example", "Avery Quinn");
    let says = |value: &str| failed_check(Some(&account), &with_results(from("Shop", "news@shop.example"), value));
    // Cut at 4,000 characters just after `shop.example` of `shop.example.evil.example`.
    let head = "mx.lumen.example; spf=none reason=";
    let tail = "; dmarc=fail header.from=shop.example";
    let pad = "x".repeat(4_000 - head.len() - tail.len());
    assert_eq!(says(&format!("{head}{pad}{tail}.evil.example")), None);
    assert_eq!(says(&format!("{head}{pad}{tail}")), Some(Failed::Dmarc));
    assert_eq!(says("mx.lumen.example; dmarc=fail header.from=shop.example (unclosed"), None);
    assert_eq!(says("mx.lumen.example; dmarc=fail header.from=\"shop.example"), None);
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
