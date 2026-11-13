// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a link of a message really goes. Only `http`, `https`, `mailto`, `tel` and `cid` are
//! followed; a web address is read by `mailcraft-link`, which writes it the way it is shown and
//! launched, so that the host shown is the host reached. Everything else is no link.

use mailcraft_link::WebLink;

use crate::records::{LinkKind, LinkTarget};

/// `url` as a link that may be followed, or `None` when it is none: another scheme, a control or
/// invisible format character in it, a web address that is read in more than one way or has no
/// host, or one too long.
#[uniffi::export]
pub fn link_target(url: String) -> Option<LinkTarget> {
    let url = url.trim_matches(' ');
    if !mailcraft_html::is_safe_url(url) {
        return None;
    }
    let scheme = url.split_once(':').map(|(s, _)| s.to_ascii_lowercase())?;
    match scheme.as_str() {
        "http" | "https" => WebLink::parse(url).map(|l| LinkTarget { kind: LinkKind::Web, shown: l.into_string() }),
        "mailto" => Some(LinkTarget { kind: LinkKind::Mail, shown: url.to_string() }),
        "tel" => Some(LinkTarget { kind: LinkKind::Phone, shown: url.to_string() }),
        "cid" => Some(LinkTarget { kind: LinkKind::Part, shown: url.to_string() }),
        _ => None,
    }
}
