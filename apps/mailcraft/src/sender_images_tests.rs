// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a sender's picture is looked up. Nothing here goes to the network: the one look-up
//! finds its picture already kept.

use super::*;

const ADA: &str = "https://gravatar.com/avatar/cfe00dde46ef942601ffafb9e2825e802858b460e586b8305b344c1eb826357d?s=96&d=404";

#[test]
fn gravatar_gets_a_hash_and_favicon_im_the_domain() {
    // The hash is of the address in lower case, without the space round it.
    assert_eq!(
        addresses(" Ada@Example.ORG "),
        vec![
            ADA.to_string(),
            "https://favicon.im/example.org?larger=true&throw-error-on-404=true".into(),
            "https://www.google.com/s2/favicons?domain=example.org&sz=96".into()
        ]
    );
    // Nothing of the address but the domain reaches favicon.im.
    assert!(addresses("grace.hopper@shop.example").iter().all(|url| !url.contains("grace")));
}

#[test]
fn favicon_im_is_asked_for_the_site_not_the_mail_subdomain() {
    for (address, site) in [
        ("news@mail.nutrient.io", "nutrient.io"),
        ("noreply@email.steampowered.com", "steampowered.com"),
        ("a@x.y.shop.example", "shop.example"),
        ("a@u8f3a.spam.example", "spam.example"),
        ("a@news.bbc.co.uk", "bbc.co.uk"),
        ("a@bbc.co.uk", "bbc.co.uk"),
        ("a@shop.com.au", "shop.com.au"),
        ("a@mail.bmw.de", "bmw.de"),
    ] {
        assert_eq!(addresses(address).get(1), Some(&format!("https://favicon.im/{site}?larger=true&throw-error-on-404=true")), "{address}");
    }
}

#[test]
fn a_providers_domain_has_no_icon_of_the_sender() {
    for address in [
        "someone@gmail.com",
        "someone@outlook.com",
        "someone@hotmail.co.uk",
        "someone@yahoo.fr",
        "alerts@mail.gmail.com",
        "it@contoso.onmicrosoft.com",
    ] {
        let urls = addresses(address);
        assert_eq!(urls.len(), 1, "{address}");
        assert!(urls[0].starts_with("https://gravatar.com/avatar/"), "{address}");
    }
}

#[test]
fn what_is_no_address_is_not_looked_up() {
    let long = format!("{}@example.org", "a".repeat(400));
    for junk in [
        "",
        "  ",
        "ada",
        "@example.org",
        "ada@",
        "ada@localhost",
        "ada@exa mple.org",
        "a da@example.org",
        "ada@example.org\r\nX: y",
        "ada@exam/ple.org",
        "ada@-",
        &long,
    ] {
        assert!(addresses(junk).is_empty(), "{junk:?}");
    }
}

#[test]
fn a_kept_picture_is_not_asked_for_again() {
    let dir = std::env::temp_dir().join(format!("mailcraft-sender-images-{}", std::process::id()));
    let cache = Cache::at(dir.clone());
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[7; 64]].concat();
    assert!(cache.put(ADA, &png));
    let alive = Arc::new(());
    assert_eq!(look_up("ada@example.org", Some(&cache), &Arc::downgrade(&alive)), Ok(png));
    assert!(look_up("not an address", Some(&cache), &Arc::downgrade(&alive)).is_err());
    // Once the window lets go (sender images turned off), nothing more is asked, kept or not.
    let gone = Arc::downgrade(&alive);
    drop(alive);
    assert!(look_up("ada@example.org", Some(&cache), &gone).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_kept_site_icon_is_found_before_gravatar_is_asked_again() {
    let dir = std::env::temp_dir().join(format!("mailcraft-sender-icons-{}", std::process::id()));
    let cache = Cache::at(dir.clone());
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[9; 64]].concat();
    let icon = addresses("ada@example.org").remove(1);
    assert!(cache.put(&icon, &png));
    // Gravatar had nothing last time: its address is not kept, and it is not asked again.
    let alive = Arc::new(());
    assert_eq!(look_up("ada@example.org", Some(&cache), &Arc::downgrade(&alive)), Ok(png));
    let _ = std::fs::remove_dir_all(dir);
}
