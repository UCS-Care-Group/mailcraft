// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Manually merged correspondent address groups (#1907): kept on this computer alone so multiple
//! email addresses belonging to one person can be viewed as a single conversation.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

/// Most manually merged correspondent groups kept in a mailbox.
pub const MAX_CORRESPONDENT_GROUPS: usize = 2_000;
/// Most email addresses kept in a single merged correspondent group.
pub const MAX_GROUP_ADDRESSES: usize = 50;
/// Longest custom display name (in characters) kept for a merged correspondent group.
pub const MAX_GROUP_NAME: usize = 200;
/// Longest email address normalized as a correspondent key (matches `people_merge::email_key`).
const MAX_KEY_BYTES: usize = 320;

/// A group of normalized email addresses that belong to the same person, merged manually by the
/// owner for contact-based conversation grouping (#1907).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CorrespondentGroup {
    /// Optional custom display name chosen by the owner for this merged person.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Normalized (trimmed, lowercased) email addresses belonging to this person.
    pub addresses: Vec<String>,
}

/// Normalize an email address for correspondent grouping: trimmed and lowercased, no whitespace or
/// control characters, valid `local@domain.tld` format, at most 320 UTF-8 bytes after lowercasing.
pub fn normalize_address(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let lower = t.to_lowercase();
    if lower.len() <= 2 || lower.len() > MAX_KEY_BYTES {
        return None;
    }
    let (local, domain) = lower.split_once('@')?;
    if domain.contains('@') || local.is_empty() || domain.is_empty() {
        return None;
    }
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return None;
    }
    if local.chars().any(|c| matches!(c, '<' | '>' | '(' | ')' | ',' | ';' | ':' | '"' | '\\' | '[' | ']')) {
        return None;
    }
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') || domain.contains("..") {
        return None;
    }
    let domain_ok = domain
        .split('.')
        .all(|label| !label.is_empty() && !label.starts_with('-') && !label.ends_with('-') && label.chars().all(|c| c.is_alphanumeric() || c == '-'));
    domain_ok.then_some(lower)
}

/// Sanitize a custom display name for a merged correspondent group: trimmed, no control characters,
/// capped at [`MAX_GROUP_NAME`] characters.
pub fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw.trim().chars().filter(|c| !c.is_control()).take(MAX_GROUP_NAME).collect();
    cleaned.trim().to_string()
}

/// Repair `groups` loaded from disk or untrusted input: cap group count, sanitize names, normalize
/// and deduplicate addresses, ensure each address appears in at most one group, and drop groups
/// with fewer than two addresses. Returns `true` when anything was changed.
pub fn repair_groups(groups: &mut Vec<CorrespondentGroup>) -> bool {
    let incoming = std::mem::take(groups);
    let mut changed = incoming.len() > MAX_CORRESPONDENT_GROUPS;
    let mut claimed: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(incoming.len().min(MAX_CORRESPONDENT_GROUPS));

    for group in incoming.into_iter().take(MAX_CORRESPONDENT_GROUPS) {
        let name = sanitize_name(&group.name);
        if name != group.name {
            changed = true;
        }
        let mut addresses: Vec<String> = Vec::new();
        for raw in &group.addresses {
            let Some(addr) = normalize_address(raw) else {
                changed = true;
                continue;
            };
            if addr != *raw || claimed.contains(&addr) || addresses.contains(&addr) {
                changed = true;
                continue;
            }
            if addresses.len() >= MAX_GROUP_ADDRESSES {
                changed = true;
                break;
            }
            addresses.push(addr);
        }
        if !addresses.windows(2).all(|w| w[0] <= w[1]) {
            addresses.sort();
            changed = true;
        }
        if addresses.len() >= 2 {
            for addr in &addresses {
                claimed.insert(addr.clone());
            }
            out.push(CorrespondentGroup { name, addresses });
        } else {
            changed = true;
        }
    }
    *groups = out;
    changed
}
