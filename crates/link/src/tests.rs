// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reader of web addresses: what is accepted and how it is written, what is refused (the
//! tricks a link can play on the person reading it), and that nothing panics.

use super::*;
use proptest::prelude::*;

fn written(input: &str) -> String {
    WebLink::parse(input).unwrap_or_else(|| panic!("{input:?} should read")).into_string()
}

fn host(input: &str) -> String {
    WebLink::parse(input).unwrap_or_else(|| panic!("{input:?} should read")).host().to_string()
}

#[test]
fn addresses_are_written_the_way_a_browser_writes_them() {
    for (input, expected) in [
        ("https://lumen.example/agenda?week=3", "https://lumen.example/agenda?week=3"),
        ("HTTPS://Example.org", "https://example.org/"),
        ("HTTP://EXAMPLE.org/a b", "http://example.org/a%20b"),
        ("HTTP://Lumen.Example:8080/x", "http://lumen.example:8080/x"),
        ("http://example.org:80/x", "http://example.org/x"),
        ("https://example.org:443", "https://example.org/"),
        ("https://example.org:/x", "https://example.org/x"),
        ("https://example.org:00443/x", "https://example.org/x"),
        ("http://example.org:443/", "http://example.org:443/"),
        ("https://a.example:8443/x?y#z", "https://a.example:8443/x?y#z"),
        ("https://a.example/?", "https://a.example/?"),
        ("https://a.example/#", "https://a.example/#"),
        ("https://a.example?x=1", "https://a.example/?x=1"),
        ("https://a.example#top", "https://a.example/#top"),
        ("  https://a.example/  ", "https://a.example/"),
        ("https://a.example./", "https://a.example./"),
        ("https://a.example/a/./b/../c", "https://a.example/a/c"),
        ("https://a.example/a/%2e%2E/c", "https://a.example/c"),
        ("https://a.example/a/b/..", "https://a.example/a/"),
        ("https://a.example/../..", "https://a.example/"),
        ("https://a.example/a b?q=\"x\" y#f g", "https://a.example/a%20b?q=%22x%22%20y#f%20g"),
        ("https://a.example/caf\u{e9}", "https://a.example/caf%C3%A9"),
        ("https://a.example/100%", "https://a.example/100%"),
        ("https://a.example/100%25", "https://a.example/100%25"),
        ("https://a.example/%zz", "https://a.example/%zz"),
        ("https://a.example/{x}|", "https://a.example/%7Bx%7D|"),
        ("https://a.example/?a=1&b='2'", "https://a.example/?a=1&b=%272%27"),
        ("https://a.example_b/", "https://a.example_b/"),
    ] {
        assert_eq!(written(input), expected, "{input:?}");
    }
}

#[test]
fn the_host_is_where_the_link_goes() {
    assert_eq!(host("https://lumen.example/"), "lumen.example");
    assert_eq!(host("https://bank.example@evil.example/login"), "evil.example");
    assert_eq!(host("https://bank.example:secret@evil.example:8080/login"), "evil.example");
    assert_eq!(host("https://bank.example@evil.example@worse.example/"), "worse.example");
    assert_eq!(host("https://bank.example%40evil.example@real.example/"), "real.example");
    assert_eq!(host("https://evil.example#@bank.example/"), "evil.example");
    assert_eq!(host("https://evil.example?@bank.example/"), "evil.example");
    assert_eq!(host("https://evil.example/@bank.example/"), "evil.example");
    // What is before the `@` stays in the address, so the person sees it, but is not the host.
    assert_eq!(written("https://bank.example@evil.example/login"), "https://bank.example@evil.example/login");
    assert_eq!(written("https://us er:p@ss@evil.example/"), "https://us%20er:p%40ss@evil.example/");
    assert!(WebLink::parse("https://@evil.example/").is_some_and(|l| l.host() == "evil.example"));
    assert!(WebLink::parse("https://bank.example@/").is_none());
    assert!(WebLink::parse("https://bank.example@:443/").is_none());
}

#[test]
fn international_names_are_written_in_punycode() {
    assert_eq!(host("https://b\u{fc}cher.example/kasse"), "xn--bcher-kva.example");
    assert_eq!(written("https://b\u{fc}cher.example/kasse"), "https://xn--bcher-kva.example/kasse");
    // A Cyrillic "a" in place of ours.
    assert_eq!(host("https://p\u{430}ypal.example/"), "xn--pypal-4ve.example");
    assert_eq!(host("https://B\u{dc}CHER.example/"), "xn--bcher-kva.example");
    assert_eq!(host("https://m\u{fc}nchen.de/"), "xn--mnchen-3ya.de");
    assert_eq!(host("https://\u{65e5}\u{672c}\u{8a9e}.example/"), "xn--wgv71a119e.example");
    assert_eq!(host("https://\u{30c9}\u{30e1}\u{30a4}\u{30f3}\u{540d}\u{4f8b}.jp/"), "xn--eckwd4c7cu47r2wf.jp");
    assert_eq!(host("https://\u{43f}\u{440}\u{438}\u{43c}\u{435}\u{440}.example/"), "xn--e1afmkfd.example");
    // Already in ASCII form: as it is.
    assert_eq!(host("https://XN--bcher-kva.example/"), "xn--bcher-kva.example");
    // An emoji is a label too.
    assert!(host("https://\u{1f4a9}.example/").starts_with("xn--"));
}

#[test]
fn names_that_a_browser_would_read_differently_are_refused() {
    for bad in [
        // full stops of other scripts
        "https://evil\u{3002}example/",
        "https://evil\u{ff0e}example/",
        "https://evil\u{ff61}example/",
        // invisible and direction-changing characters
        "https://exam\u{200b}ple.example/",
        "https://exam\u{202e}ple.example/",
        "https://exam\u{feff}ple.example/",
        "https://exam\u{ad}ple.example/",
        "https://exam\u{a0}ple.example/",
        // a percent sign, a space, markup and other punctuation
        "https://exa%6dple.example/",
        "https://exa mple.org/",
        "http://exa<mple.org/",
        "https://exa\"mple.org/",
        "https://exa'mple.org/",
        "https://exa(mple.org/",
        "https://exa~mple.org/",
        "https://exa;mple.org/",
        "https://exa,mple.org/",
        "https://exa!mple.org/",
        "https://exa$mple.org/",
        "https://exa=mple.org/",
        // empty labels
        "https://.example/",
        "https://a..example/",
        "https://./",
        "https://../",
        // too long
        &format!("https://{}.example/", "a".repeat(64)),
        &format!("https://{}/", ["abcdefghi"; 30].join(".")),
    ] {
        assert!(WebLink::parse(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn numbers_for_a_host_are_written_as_the_address_they_are() {
    assert_eq!(host("http://2130706433/"), "127.0.0.1");
    assert_eq!(host("http://0x7f.1/"), "127.0.0.1");
    assert_eq!(host("http://0x7f000001/"), "127.0.0.1");
    assert_eq!(host("http://0177.0.0.01/"), "127.0.0.1");
    assert_eq!(host("http://127.1/"), "127.0.0.1");
    assert_eq!(host("http://192.168.0.1./"), "192.168.0.1");
    assert_eq!(host("http://4294967295/"), "255.255.255.255");
    assert_eq!(written("http://10.0.0.1:8080/x"), "http://10.0.0.1:8080/x");
    for bad in [
        "http://4294967296/",
        "http://256.1.1.1/",
        "http://1.2.3.4.5/",
        "http://1.2.3.999999/",
        "http://foo.123/",
        "http://08/",
        "http://1.2.3.0x/x1",
    ] {
        // `0x` alone is zero in a browser, so the last of these reads.
        if bad == "http://1.2.3.0x/x1" {
            assert_eq!(host(bad), "1.2.3.0");
        } else {
            assert!(WebLink::parse(bad).is_none(), "{bad:?}");
        }
    }
}

#[test]
fn ipv6_addresses_are_written_in_brackets() {
    assert_eq!(host("https://[::1]:8080/a"), "[::1]");
    assert_eq!(host("https://[2001:DB8:0:0:1:0:0:1]/"), "[2001:db8::1:0:0:1]");
    assert_eq!(host("https://[0:0:0:0:0:0:0:0]/"), "[::]");
    assert_eq!(host("https://[1:0:0:0:0:0:0:0]/"), "[1::]");
    assert_eq!(host("https://[1:0:2:0:3:4:5:6]/"), "[1:0:2:0:3:4:5:6]");
    assert_eq!(host("https://[::ffff:192.0.2.1]/"), "[::ffff:c000:201]");
    assert_eq!(written("https://[::1]:8080/a"), "https://[::1]:8080/a");
    for bad in [
        "https://[::1/",
        "https://[::1]x/",
        "https://[zz]/",
        "https://[::1%25eth0]/",
        "https://[]/",
        "https://[1:2:3]/",
        "https://::1/",
        "http://[::1]:99999/",
        "https://[::1]:-1/",
    ] {
        assert!(WebLink::parse(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn only_http_and_https_addresses_with_a_host_are_read() {
    for bad in [
        "",
        " ",
        "https",
        "https:",
        "https:/",
        "https://",
        "http://",
        "https:///path",
        "https:example.org",
        "https:/example.org",
        "https:\\\\example.org",
        "https://example.org\\@evil.example/",
        "https://example.org/a\\b",
        "ftp://example.org/",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "mailto:a@example.org",
        "data:text/html,x",
        "httpx://example.org/",
        "ht tp://example.org/",
        "://example.org",
        "/relative/path",
        "example.org",
        "https://:443/x",
        "https://example.org:99999/",
        "https://example.org:80x/",
        "https://example.org:-1/",
        "http://[::1",
        "http://[zz]/",
    ] {
        assert!(WebLink::parse(bad).is_none(), "{bad:?}");
    }
    assert!(WebLink::parse("HtTpS://example.org").is_some_and(|l| l.is_https()));
    assert!(WebLink::parse("http://example.org").is_some_and(|l| !l.is_https()));
}

#[test]
fn whitespace_and_control_characters_are_refused() {
    for bad in [
        "https://example.org/\n",
        "https://example.org/\t",
        "https://exa\tmple.org/",
        "ht\ntps://example.org/",
        "https://example.org/\u{0}",
        "https://example.org/\u{7f}",
        "https://example.org/\u{85}",
        "\u{b}https://example.org/",
        "https://example.org/a\rb",
    ] {
        assert!(WebLink::parse(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn an_address_that_looks_like_an_option_is_no_address() {
    for bad in ["-foo", "--help", "-x https://a.example/", "/? https://a.example/", "-https://a.example/"] {
        assert!(WebLink::parse(bad).is_none(), "{bad:?}");
    }
    // What is read always starts with its scheme and holds nothing a launcher could split or quote.
    let link = written("https://a.example/-x --flag \"q\" 'r' <s> `t` \u{2028}");
    assert!(link.starts_with("https://"));
    assert!(!link.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '<' | '>' | '`' | '\\')), "{link}");
}

#[test]
fn very_long_addresses_are_cut_off_cleanly() {
    let long = format!("https://a.example/{}", "x".repeat(MAX_LINK));
    assert!(WebLink::parse(&long).is_none());
    let fits = format!("https://a.example/{}", "x".repeat(MAX_LINK - 18));
    assert!(WebLink::parse(&fits).is_some_and(|l| l.as_str().len() == MAX_LINK));
    let many_dots = format!("https://{}/", ".".repeat(MAX_LINK - 10));
    assert!(WebLink::parse(&many_dots).is_none());
    let deep = format!("https://a.example/{}", "../".repeat(20_000));
    assert_eq!(written(&deep), "https://a.example/");
    let tabs = format!("https://a.example/{}", "\u{1f4a9}".repeat(10_000));
    assert!(WebLink::parse(&tabs).is_some_and(|l| l.as_str().len() == 18 + 10_000 * 12));
    let host = format!("https://{}/", "\u{e9}".repeat(5_000));
    assert!(WebLink::parse(&host).is_none());
}

#[test]
fn punycode_matches_the_standard_examples() {
    let encode = |s: &str| punycode::encode(&s.chars().collect::<Vec<_>>());
    // RFC 3492 section 7.1.
    assert_eq!(
        encode("\u{644}\u{64a}\u{647}\u{645}\u{627}\u{628}\u{62a}\u{643}\u{644}\u{645}\u{648}\u{634}\u{639}\u{631}\u{628}\u{64a}\u{61f}").as_deref(),
        Some("egbpdaj6bu4bxfgehfvwxn")
    );
    assert_eq!(encode("\u{4ed6}\u{4eec}\u{4e3a}\u{4ec0}\u{4e48}\u{4e0d}\u{8bf4}\u{4e2d}\u{6587}").as_deref(), Some("ihqwcrb4cv8a8dqg056pqjye"));
    assert_eq!(encode("Pro\u{10d}prost\u{11b}nemluv\u{ed}\u{10d}esky").as_deref(), Some("Proprostnemluvesky-uyb24dma41a"));
    assert_eq!(encode("b\u{fc}cher").as_deref(), Some("bcher-kva"));
    assert_eq!(encode("\u{fc}").as_deref(), Some("tda"));
    assert_eq!(encode("abc"), Some("abc-".to_string()));
    assert_eq!(encode(""), None);
    assert_eq!(encode(&"\u{10ffff}".repeat(5_000)), None);
}

#[test]
fn punycode_reads_back_the_standard_examples() {
    let decode = |s: &str| punycode::decode(s).map(|chars| chars.into_iter().collect::<String>());
    // RFC 3492 section 7.1, the other way.
    assert_eq!(decode("ihqwcrb4cv8a8dqg056pqjye").as_deref(), Some("\u{4ed6}\u{4eec}\u{4e3a}\u{4ec0}\u{4e48}\u{4e0d}\u{8bf4}\u{4e2d}\u{6587}"));
    assert_eq!(decode("Proprostnemluvesky-uyb24dma41a").as_deref(), Some("Pro\u{10d}prost\u{11b}nemluv\u{ed}\u{10d}esky"));
    assert_eq!(decode("bcher-kva").as_deref(), Some("b\u{fc}cher"));
    assert_eq!(decode("tda").as_deref(), Some("\u{fc}"));
    // Labels that are not what their letters would be written as (IDNA 2008 §5.4).
    for loose in ["abc-", "lumen-", "paypal-", "a-b-"] {
        assert_eq!(decode(loose), None, "{loose}");
    }
    // A Cyrillic "а" among Latin letters, as a lookalike domain has it, there and back.
    let lookalike: Vec<char> = "p\u{430}ypal".chars().collect();
    assert_eq!(punycode::encode(&lookalike).and_then(|text| punycode::decode(&text)), Some(lookalike));
    for bad in ["", "-", "a-!", "99999999999", "\u{e9}"] {
        assert_eq!(decode(bad), None, "{bad}");
    }
}

#[test]
fn a_component_for_a_query_leaves_letters_digits_and_a_few_marks() {
    assert_eq!(encode_component("abc XYZ-._*019"), "abc%20XYZ-._*019");
    assert_eq!(encode_component("a+b&c=d/e?f#g%h"), "a%2Bb%26c%3Dd%2Fe%3Ff%23g%25h");
    assert_eq!(encode_component("caf\u{e9} \u{1f4a9}\n"), "caf%C3%A9%20%F0%9F%92%A9%0A");
    assert_eq!(encode_component(""), "");
}

#[test]
fn the_rest_of_a_mail_or_phone_address_holds_no_space_or_quote() {
    assert_eq!(encode_opaque("a@example.org?subject=Hi there"), "a@example.org?subject=Hi%20there");
    assert_eq!(encode_opaque("\"A B\" <a@example.org>"), "%22A%20B%22%20%3Ca@example.org%3E");
    assert_eq!(encode_opaque("+441234"), "+441234");
    assert_eq!(encode_opaque("a%20b%zz"), "a%20b%zz");
    assert_eq!(encode_opaque("caf\u{e9}"), "caf%C3%A9");
}

proptest! {
    #[test]
    fn nothing_panics_and_what_is_read_is_safe_to_launch(input in ".{0,400}") {
        if let Some(link) = WebLink::parse(&input) {
            check(&link);
        }
    }

    #[test]
    fn nothing_panics_for_addresses_made_of_the_pieces_that_matter(
        scheme in prop::sample::select(vec!["http", "https", "HTTP", "ftp", ""]),
        user in prop::sample::select(vec!["", "bank.example@", "a:b@", "@", "%40@"]),
        host in "[a-zA-Z0-9.:\\[\\]%@_\u{e9}\u{430}\u{3002}-]{0,40}",
        tail in "[a-zA-Z0-9/?#%.&=~ \"<>\\\\\u{e9}-]{0,60}",
    ) {
        let input = format!("{scheme}://{user}{host}{tail}");
        if let Some(link) = WebLink::parse(&input) {
            check(&link);
        }
    }

    #[test]
    fn arbitrary_bytes_of_text_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let _ = WebLink::parse(&text);
        let _ = WebLink::parse(&format!("https://{text}"));
        let _ = WebLink::parse(&format!("https://x.example/{text}"));
        let _ = encode_opaque(&text);
        let _ = encode_component(&text);
    }

    #[test]
    fn punycode_never_panics(chars in prop::collection::vec(any::<char>(), 0..80)) {
        let _ = punycode::encode(&chars);
    }

    #[test]
    fn punycode_reads_back_what_it_writes(chars in prop::collection::vec(any::<char>(), 1..80), junk in "[ -~]{0,200}") {
        if let Some(text) = punycode::encode(&chars).filter(|_| chars.iter().any(|c| !c.is_ascii())) {
            prop_assert_eq!(punycode::decode(&text), Some(chars));
        }
        let _ = punycode::decode(&junk);
    }
}

/// What every address that was read promises.
fn check(link: &WebLink) {
    let text = link.as_str();
    assert!(text.starts_with("http://") || text.starts_with("https://"), "{text:?}");
    assert!(text.len() <= MAX_LINK * 3 + 64, "{}", text.len());
    assert!(!text.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '<' | '>' | '`' | '\\')), "{text:?}");
    assert!(text.is_ascii(), "{text:?}");
    let host = link.host();
    assert!(!host.is_empty() && host.len() <= 254, "{host:?}");
    assert!(host.is_ascii() && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'[' | b']')), "{host:?}");
    // The host is the part after the scheme and any user name, up to a port or a path.
    let after = &text[text.find("://").unwrap_or(0) + 3..];
    let authority = &after[..after.find(['/', '?', '#']).unwrap_or(after.len())];
    let hostport = &authority[authority.rfind('@').map_or(0, |at| at + 1)..];
    assert!(hostport.starts_with(host), "{text:?} {host:?}");
    // Reading it again changes nothing.
    assert_eq!(WebLink::parse(text).map(WebLink::into_string).as_deref(), Some(text));
}

#[test]
fn a_stray_percent_sign_stays_as_written() {
    assert_eq!(written("https://example.com/50%off?x=100%&y=%zz#f%"), "https://example.com/50%off?x=100%&y=%zz#f%");
    assert_eq!(written("https://u%s:p%@example.com/"), "https://u%s:p%@example.com/");
    assert_eq!(encode_opaque("a@b.example?subject=100%"), "a@b.example?subject=100%");
    // Writing it again changes nothing.
    assert_eq!(written(&written("https://example.com/50%off?x=100%")), "https://example.com/50%off?x=100%");
}

#[test]
fn a_joiner_is_accepted_only_where_idna2008_allows_it() {
    // Persian: heh, ZWNJ, alef (joining context).
    assert!(host("http://\u{646}\u{627}\u{645}\u{647}\u{200c}\u{627}\u{6cc}.ir/").starts_with("xn--"));
    // Hindi: after a virama, ZWNJ and ZWJ.
    assert!(host("http://\u{915}\u{94d}\u{200c}\u{937}.example/").starts_with("xn--"));
    assert!(host("http://\u{915}\u{94d}\u{200d}\u{937}.example/").starts_with("xn--"));
    // Anywhere else they hide in a name that looks like another.
    for bad in [
        "http://pay\u{200c}pal.example/",
        "http://pay\u{200d}pal.example/",
        "http://\u{200c}\u{627}.ir/",
        "http://\u{627}\u{200c}\u{628}.ir/",
        "http://\u{628}\u{200c}.ir/",
        "http://\u{915}\u{200d}\u{937}.example/",
    ] {
        assert!(WebLink::parse(bad).is_none(), "{bad:?}");
    }
    // Other invisible characters stay refused.
    assert!(WebLink::parse("http://a\u{200b}b.example/").is_none());
}

#[test]
fn forms_that_are_not_normalised_are_shown_as_themselves() {
    // No UTS #46 mapping: each is its own host, never the composed or plain one.
    assert!(host("http://e\u{301}.com/").starts_with("xn--"));
    assert_ne!(host("http://e\u{301}.com/"), host("http://\u{e9}.com/"));
    assert!(host("http://\u{fb01}sh.com/").starts_with("xn--"));
    assert_ne!(host("http://\u{fb01}sh.com/"), "fish.com");
    assert!(host("http://\u{ff21}.com/").starts_with("xn--"));
    assert_ne!(host("http://\u{ff21}.com/"), "a.com");
}
