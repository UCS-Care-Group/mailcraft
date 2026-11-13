// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::links::link_target;
use crate::records::LinkKind;

#[test]
fn only_five_kinds_of_link_are_followed() {
    let kind = |u: &str| link_target(u.to_string()).map(|t| t.kind);
    assert_eq!(kind("https://example.org/a?b=c"), Some(LinkKind::Web));
    assert_eq!(kind("HTTP://Example.ORG/"), Some(LinkKind::Web));
    assert_eq!(kind("mailto:ada@example.org"), Some(LinkKind::Mail));
    assert_eq!(kind("tel:+15550100"), Some(LinkKind::Phone));
    assert_eq!(kind("cid:logo"), Some(LinkKind::Part));
    for bad in [
        "",
        "javascript:alert(1)",
        "data:text/html,<b>",
        "file:///etc/passwd",
        "intent://x#Intent;scheme=http;end",
        "ftp://example.org/",
        "example.org/relative",
        "https://",
        "https://exa mple.org/",
        "https://example.org\\@evil.example/",
        "https://ex\u{200b}ample.org/",
        "https://example.org/\u{202e}gpj.exe",
        "http\t://example.org/",
        "https://user@example.org:99999999/",
    ] {
        assert_eq!(kind(bad), None, "{bad:?}");
    }
    assert_eq!(kind(&format!("https://example.org/{}", "a".repeat(70_000))), None);
}

#[test]
fn the_host_shown_is_the_host_reached() {
    let shown = |u: &str| link_target(u.to_string()).map(|t| t.shown).unwrap_or_default();
    assert_eq!(shown("https://EXAMPLE.org/Path"), "https://example.org/Path");
    // A letter from another script shows as Punycode, so that a look-alike is seen for what it is.
    assert!(shown("https://exаmple.org/").contains("xn--"));
}

#[test]
fn the_platform_is_handed_only_web_addresses_and_phone_numbers() {
    use crate::Core;
    use crate::HostError;
    use crate::fake_tests::FakePlatform;
    let platform = FakePlatform::new();
    let core = Core::new(platform.clone()).unwrap();
    assert!(core.open_link("https://example.org/a".to_string()).is_ok());
    assert!(core.open_link("tel:+15550100".to_string()).is_ok());
    for refused in
        ["mailto:ada@example.org", "cid:logo", "javascript:alert(1)", "file:///etc/passwd", "intent://x#Intent;scheme=http;end", "", "https://"]
    {
        assert_eq!(core.open_link(refused.to_string()), Err(HostError::Unsupported), "{refused:?}");
    }
    assert_eq!(*platform.opened.lock().unwrap(), vec!["https://example.org/a".to_string(), "tel:+15550100".to_string()]);
}
