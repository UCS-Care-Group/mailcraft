// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Well-known brands** that phishing pretends to be, each with the domains its mail comes from,
//! for the sender warnings ([`crate::sender_warnings`]): a brand's name on somebody else's address,
//! and a domain made to look like a brand's.
//!
//! The brands are those Check Point Research's brand phishing reports rank among the ten most
//! impersonated in 2026 (Q1 to Q3), and nothing else, so that the list says why each is here. Each
//! domain is checked twice: it is the brand's (its own web site, or one that sends a visitor on to
//! the brand's), and it publishes a DMARC policy that has receivers quarantine or reject mail that
//! fails it (`_dmarc.<domain>`, `p=quarantine` or `p=reject`), so mail that really carries it is
//! the brand's. A brand that owns a top-level domain of its own (`.google`) has every name under
//! it.
//!
//! ## Adding a brand
//!
//! A country's own brands are welcome too, the ones phishing in that country pretends to be (a
//! carrier, a tax or health service, a bank), with where their place among its most impersonated
//! is said. For each, in one pull request:
//!
//! 1. Its name as senders write it, one word. A name that is also an ordinary word or a person's
//!    name (`Apple`, `Tesla`) is `common_word`: compared only when the sender's name is the brand
//!    alone or the brand and one more word ("Apple Support").
//! 2. Every registrable domain its mail comes from, each checked twice, and the comment above the
//!    entry saying how: it is the brand's (its own web site, a site that sends a visitor on to the
//!    brand's, or the brand's own page naming it), and its DMARC policy is `p=quarantine` or
//!    `p=reject` (`nslookup -type=TXT _dmarc.<domain>`, or `dig +short TXT _dmarc.<domain>`). A
//!    domain with `p=none` or none at all stays out, and so does the brand when its mail also
//!    comes from such a domain: a red warning on its own mail would be worse than none.
//! 3. `own_top_level` when the brand has a top-level domain of its own (`dig +short NS <name>`
//!    answers, and the registry's list names the brand as its owner).
//! 4. A line in `a_brands_own_mail_gives_no_warning` (`sender_warnings_tests.rs`) with an address
//!    its mail really comes from, and in `a_domain_made_to_look_like_a_brands_is_a_caution` when
//!    its name is short or common enough to be worth a look.

/// A brand: its name as it is written in a sender's name, the registrable domains its mail comes
/// from, whether every name under a top-level domain of its own (`.google`) is its too, and
/// whether its name is also an ordinary word or a person's name (compared only when it is about
/// all the sender's name says).
pub struct Brand {
    pub name: &'static str,
    pub domains: &'static [&'static str],
    pub own_top_level: bool,
    pub common_word: bool,
}

/// The brands, by name.
pub const BRANDS: &[Brand] = &[
    // Microsoft's own domain (its account mail comes from `accountprotection.microsoft.com`) and
    // the ones Microsoft 365 sends from, each with `p=reject` or `p=quarantine`; the top-level
    // `.microsoft` is Microsoft's. Outlook.com, Hotmail and Live addresses are anybody's.
    Brand {
        name: "Microsoft",
        domains: &["microsoft.com", "microsoftonline.com", "microsoft365.com", "office.com", "office365.com"],
        own_top_level: true,
        common_word: false,
    },
    // `apple.com` (its mail comes from `email.apple.com` and `id.apple.com`), with `p=quarantine`;
    // the top-level `.apple` is Apple's. iCloud addresses are anybody's.
    Brand { name: "Apple", domains: &["apple.com"], own_top_level: true, common_word: true },
    // `google.com` (its account, Drive and Calendar mail come from names under it), with
    // `p=reject`; the top-level `.google` is Google's.
    Brand { name: "Google", domains: &["google.com"], own_top_level: true, common_word: false },
    // Its stores' domains and Prime Video's, each with `p=quarantine`; the top-level `.amazon` is
    // Amazon's.
    Brand {
        name: "Amazon",
        domains: &[
            "amazon.com",
            "amazon.ae",
            "amazon.ca",
            "amazon.cn",
            "amazon.co.jp",
            "amazon.co.uk",
            "amazon.com.au",
            "amazon.com.be",
            "amazon.com.br",
            "amazon.com.mx",
            "amazon.com.tr",
            "amazon.de",
            "amazon.eg",
            "amazon.es",
            "amazon.fr",
            "amazon.ie",
            "amazon.in",
            "amazon.it",
            "amazon.nl",
            "amazon.pl",
            "amazon.sa",
            "amazon.se",
            "amazon.sg",
            "primevideo.com",
        ],
        own_top_level: true,
        common_word: false,
    },
    // `linkedin.com`, with `p=reject`.
    Brand { name: "LinkedIn", domains: &["linkedin.com"], own_top_level: false, common_word: false },
    // Dropbox's site and `dropboxmail.com`, whose site sends a visitor on to Dropbox's; both with
    // `p=reject`.
    Brand { name: "Dropbox", domains: &["dropbox.com", "dropboxmail.com"], own_top_level: false, common_word: false },
    // Facebook's site, and `facebookmail.com`, which its notifications come from and whose site
    // sends a visitor to Facebook's help; both with `p=reject`.
    Brand { name: "Facebook", domains: &["facebook.com", "facebookmail.com"], own_top_level: false, common_word: false },
    // WhatsApp's site, and Meta's `facebookmail.com` (above); both with `p=reject`.
    Brand { name: "WhatsApp", domains: &["whatsapp.com", "facebookmail.com"], own_top_level: false, common_word: false },
    // `tesla.com`, with `p=reject`.
    Brand { name: "Tesla", domains: &["tesla.com"], own_top_level: false, common_word: true },
    // `youtube.com` and Google's `google.com` (above), both with `p=reject`; the top-level
    // `.youtube` is YouTube's.
    Brand { name: "YouTube", domains: &["youtube.com", "google.com"], own_top_level: true, common_word: false },
    // Adobe's site, and Acrobat Sign's `adobesign.com` and `echosign.com`, whose sites send a
    // visitor on to Adobe's; each with `p=reject`.
    Brand { name: "Adobe", domains: &["adobe.com", "adobesign.com", "echosign.com"], own_top_level: false, common_word: false },
    // The domains of its country sites that send mail, each sending a visitor on to `paypal.com`
    // and each with `p=reject`. PayPal's help pages give `service@paypal.com` as the address its
    // mail comes from.
    Brand {
        name: "PayPal",
        domains: &[
            "paypal.com",
            "paypal.at",
            "paypal.be",
            "paypal.ca",
            "paypal.ch",
            "paypal.co.uk",
            "paypal.com.au",
            "paypal.com.br",
            "paypal.com.mx",
            "paypal.de",
            "paypal.dk",
            "paypal.es",
            "paypal.fr",
            "paypal.ie",
            "paypal.it",
            "paypal.jp",
            "paypal.nl",
            "paypal.pl",
            "paypal.se",
        ],
        own_top_level: false,
        common_word: false,
    },
    // ChatGPT's site and OpenAI's, both with `p=reject`; its mail goes by either name.
    Brand { name: "ChatGPT", domains: &["chatgpt.com", "openai.com"], own_top_level: false, common_word: false },
    Brand { name: "OpenAI", domains: &["openai.com", "chatgpt.com"], own_top_level: false, common_word: false },
];

/// The brand whose name `word` is, read as its skeleton (so `P\u{430}yPal` is PayPal).
pub fn named(word: &str) -> Option<&'static Brand> {
    let word = crate::confusables::skeleton(word);
    BRANDS.iter().find(|b| crate::confusables::skeleton(b.name) == word)
}

impl Brand {
    /// The brand's name as its domains spell it, in lower case: `paypal`.
    pub fn label(&self) -> String {
        self.name.to_ascii_lowercase()
    }

    /// Whether the registrable domain `domain` (in lower case) is this brand's: one of its domains
    /// or a name under a top-level domain of its own.
    pub fn owns(&self, domain: &str) -> bool {
        self.domains.contains(&domain) || self.own_top_level && domain.rsplit('.').next() == Some(self.label().as_str())
    }
}
