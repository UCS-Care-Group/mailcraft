// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! E-mail addresses as people type and read them.

use serde::{Deserialize, Serialize};

/// A mailbox: an optional display name and an address.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Address {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub email: String,
}

impl Address {
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Address {
        Address { name: name.into().trim().to_string(), email: email.into().trim().to_string() }
    }

    /// From `Name <a@b>`, `"Last, First" <a@b>`, `<a@b>` or `a@b`. `None` for an empty string.
    pub fn parse(s: &str) -> Option<Address> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if let (Some(open), Some(close)) = (s.rfind('<'), s.rfind('>'))
            && open < close
        {
            let email = s.get(open + 1..close)?.trim();
            let name = s.get(..open)?.trim().trim_matches('"').replace("\\\"", "\"");
            return Some(Address::new(name.trim(), email));
        }
        Some(Address::new("", s.trim_matches('"')))
    }

    /// Split a recipient line on `,` and `;` (outside quotes and angle brackets) and parse each part.
    pub fn parse_list(s: &str) -> Vec<Address> {
        let mut out = Vec::new();
        let mut part = String::new();
        let (mut quoted, mut angle) = (false, false);
        let mut prev = '\0';
        for c in s.chars() {
            match c {
                '"' if prev != '\\' => quoted = !quoted,
                '<' if !quoted => angle = true,
                '>' if !quoted => angle = false,
                ',' | ';' if !quoted && !angle => {
                    out.extend(Address::parse(&part));
                    part.clear();
                    prev = c;
                    continue;
                }
                _ => {}
            }
            part.push(c);
            prev = c;
        }
        out.extend(Address::parse(&part));
        out
    }

    /// What a list shows: the name, or the address when there is none.
    pub fn display(&self) -> &str {
        if self.name.is_empty() { &self.email } else { &self.name }
    }

    /// `Name <a@b>` (the name quoted when it needs it), or just the address.
    pub fn full(&self) -> String {
        if self.name.is_empty() {
            return self.email.clone();
        }
        let plain = self.name.chars().all(|c| c.is_alphanumeric() || " .'-_".contains(c));
        if plain { format!("{} <{}>", self.name, self.email) } else { format!("\"{}\" <{}>", self.name.replace('"', "\\\""), self.email) }
    }

    /// Up to two initials for an avatar.
    pub fn initials(&self) -> String {
        let source = if self.name.is_empty() { self.email.split('@').next().unwrap_or("") } else { self.name.as_str() };
        let letters: String = source
            .split(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-')
            .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
            .take(2)
            .collect();
        letters.to_uppercase()
    }

    /// Whether the address looks deliverable: `local@domain`, no spaces.
    pub fn is_valid(&self) -> bool {
        match self.email.split_once('@') {
            Some((local, domain)) => {
                !local.is_empty()
                    && domain.contains('.')
                    && !domain.starts_with('.')
                    && !domain.ends_with('.')
                    && !self.email.contains(char::is_whitespace)
            }
            None => false,
        }
    }

    /// The same mailbox, whatever the display name or letter case.
    pub fn same_mailbox(&self, other: &Address) -> bool {
        self.email.eq_ignore_ascii_case(&other.email)
    }
}

/// Longest address a mailbox may have (RFC 5321's longest path, less its angle brackets).
pub const MAX_ADDRESS: usize = 254;

/// A mailbox's own address, strictly: `local@domain` in plain ASCII, with nothing that could
/// carry it out of a piece of a URL's path or a protocol line. For an address that is used to
/// name a mailbox to a server (a shared mailbox, `/users/{address}/…` in Microsoft Graph): the
/// local part is RFC 5322's dot-atom, the domain dot-separated labels of letters, digits and
/// hyphens, at most [`MAX_ADDRESS`] long. Trimmed, letter case kept; `None` when it isn't one.
pub fn mailbox_address(text: &str) -> Option<String> {
    let address = text.trim();
    if address.is_empty() || address.len() > MAX_ADDRESS || !address.is_ascii() {
        return None;
    }
    let (local, domain) = address.split_once('@')?;
    // RFC 5322's `atext` without `#`, `%`, `/` and `?`, which a URL reads.
    let atext = |b: u8| b.is_ascii_alphanumeric() || b"!$&'*+-=^_`{|}~".contains(&b);
    let dot_atom =
        |part: &str, allowed: &dyn Fn(u8) -> bool| !part.is_empty() && part.split('.').all(|piece| !piece.is_empty() && piece.bytes().all(allowed));
    let label = |b: u8| b.is_ascii_alphanumeric() || b == b'-';
    let labels_ok = domain.split('.').all(|piece| !piece.starts_with('-') && !piece.ends_with('-'));
    let fits = local.len() <= 64
        && dot_atom(local, &atext)
        && dot_atom(domain, &label)
        && labels_ok
        && domain.contains('.')
        && domain.split('.').all(|piece| piece.len() <= 63);
    fits.then(|| address.to_string())
}

/// A recipient line: `Ada <ada@example.org>; bob@example.org`.
pub fn format_list(list: &[Address]) -> String {
    list.iter().map(Address::full).collect::<Vec<_>>().join("; ")
}

/// A recipient line of at most `max` names, and how many more there were: for showing a list a
/// sender wrote (which can hold thousands of names) without building all of it.
pub fn format_list_capped(list: &[Address], max: usize) -> (String, usize) {
    let shown = list.get(..max).unwrap_or(list);
    (format_list(shown), list.len().saturating_sub(shown.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_usual_forms() {
        assert_eq!(Address::parse("Ada Lovelace <ada@example.org>"), Some(Address::new("Ada Lovelace", "ada@example.org")));
        assert_eq!(Address::parse("\"Lovelace, Ada\" <ada@example.org>"), Some(Address::new("Lovelace, Ada", "ada@example.org")));
        assert_eq!(Address::parse("<ada@example.org>"), Some(Address::new("", "ada@example.org")));
        assert_eq!(Address::parse(" ada@example.org "), Some(Address::new("", "ada@example.org")));
        assert_eq!(Address::parse("  "), None);
    }

    #[test]
    fn lists_split_outside_quotes() {
        let l = Address::parse_list("\"Lovelace, Ada\" <ada@example.org>; bob@example.org, Carol <carol@example.org>;;");
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].name, "Lovelace, Ada");
        assert_eq!(l[1].email, "bob@example.org");
        assert_eq!(format_list(&l), "\"Lovelace, Ada\" <ada@example.org>; bob@example.org; Carol <carol@example.org>");
        assert_eq!(Address::parse_list(&format_list(&l)), l);
        assert!(Address::parse_list("").is_empty());
    }

    #[test]
    fn display_and_validity() {
        let a = Address::new("Ada Lovelace", "ada@example.org");
        assert_eq!((a.display(), a.initials().as_str()), ("Ada Lovelace", "AL"));
        assert_eq!(Address::new("", "grace.hopper@example.org").initials(), "GH");
        assert!(a.is_valid());
        for bad in ["", "ada", "ada@", "@example.org", "ada@example", "a da@example.org", "ada@.org"] {
            assert!(!Address::new("", bad).is_valid(), "{bad}");
        }
        assert!(a.same_mailbox(&Address::new("A.", "ADA@example.org")));
    }

    #[test]
    fn a_mailbox_address_is_plain_and_can_name_nothing_else() {
        for good in ["support@example.org", " Team.Sales+eu@mail.example.org ", "o'brien@example.org", "a@b-c.example"] {
            assert_eq!(mailbox_address(good).as_deref(), Some(good.trim()), "{good}");
        }
        let long = format!("{}@example.org", "a".repeat(65));
        for bad in [
            "",
            "support",
            "support@",
            "@example.org",
            "support@example",
            "sup port@example.org",
            "a@b@example.org",
            "a/../b@example.org",
            "a@example.org/messages",
            "a@example.org?x=1",
            "a%40b@example.org",
            "a#b@example.org",
            "a\\b@example.org",
            "a..b@example.org",
            ".a@example.org",
            "a@-example.org",
            "a@example-.org",
            "a@example..org",
            "\"a\"@example.org",
            "a@[127.0.0.1]",
            "a\r\n@example.org",
            "a@example.org\u{0}",
            "ü@example.org",
            long.as_str(),
        ] {
            assert_eq!(mailbox_address(bad), None, "{bad:?}");
        }
    }

    proptest::proptest! {
        /// Whatever it is given, what it lets through is plain and can't break out of a path.
        #[test]
        fn a_mailbox_address_never_lets_through_what_a_path_would_read(text in "\\PC{0,300}") {
            if let Some(address) = mailbox_address(&text) {
                proptest::prop_assert!(address.len() <= MAX_ADDRESS);
                proptest::prop_assert!(address.bytes().all(|b| b.is_ascii_graphic() && !b"/?#%\\\"<>[]".contains(&b)));
                proptest::prop_assert_eq!(address.matches('@').count(), 1);
            }
        }
    }
}
