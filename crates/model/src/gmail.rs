// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's inbox categories (#911): which of Gmail's five groups (Primary, Social, Promotions,
//! Updates, Forums) Gmail put a message of a Gmail account's Inbox in. Gmail's decision, read from
//! its server like a read mark, and never one of the owner's own categories ([`crate::Category`]).
//!
//! It is read over IMAP with Gmail's own search (`X-GM-RAW`, from Google's "Gmail IMAP
//! extensions"): one search for each of the four that aren't Primary, and Primary is what none of
//! them found.
//!
//! It is changed through Gmail's API (`messages.batchModify`, from Google's Gmail API reference): the
//! category's label is added and the other four taken off ([`GmailCategory::api_label`]). Only an
//! account that signs in with Google has a token the API takes ([`Account::moves_gmail_categories`]).

use serde::{Deserialize, Serialize};

/// Whether mail can be moved between Gmail's categories in this build: the move goes through
/// Gmail's API, which the justification Google is reviewing doesn't name yet. Off, the engine
/// disables `mail.setGmailCategory`, the window offers no menu or drop for it, and `mailcraft-net`
/// sends nothing to Gmail's API whatever a request asks.
// ponytail: on once Google has verified the app (#232).
pub const GMAIL_CATEGORY_MOVES: bool = false;

/// After how many refusals by Gmail of a message's move on its own the move is given
/// up, and the message stays where Gmail has it ([`crate::Message::gmail_refusals`]).
pub const MAX_GMAIL_REFUSALS: u8 = 3;

use crate::{Account, Message, OAuthProvider};

/// One of Gmail's five inbox categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GmailCategory {
    Primary,
    Social,
    Promotions,
    Updates,
    Forums,
}

impl GmailCategory {
    /// The five, in the order Gmail shows them.
    pub const ALL: [GmailCategory; 5] =
        [GmailCategory::Primary, GmailCategory::Social, GmailCategory::Promotions, GmailCategory::Updates, GmailCategory::Forums];

    /// The four Gmail's search is asked about; Primary is what is left.
    pub const SEARCHED: [GmailCategory; 4] = [GmailCategory::Social, GmailCategory::Promotions, GmailCategory::Updates, GmailCategory::Forums];

    /// Its code, as files, commands and the search box write it: `primary`, `social`…
    pub fn code(self) -> &'static str {
        match self {
            GmailCategory::Primary => "primary",
            GmailCategory::Social => "social",
            GmailCategory::Promotions => "promotions",
            GmailCategory::Updates => "updates",
            GmailCategory::Forums => "forums",
        }
    }

    /// From its code, whatever the case and the spaces round it.
    pub fn parse(text: &str) -> Option<GmailCategory> {
        let text = text.trim();
        GmailCategory::ALL.into_iter().find(|c| c.code().eq_ignore_ascii_case(text))
    }

    /// Its name in English, as the folder pane shows it (the window translates it).
    pub fn name(self) -> &'static str {
        match self {
            GmailCategory::Primary => "Primary",
            GmailCategory::Social => "Social",
            GmailCategory::Promotions => "Promotions",
            GmailCategory::Updates => "Updates",
            GmailCategory::Forums => "Forums",
        }
    }

    /// The label Gmail's API knows it by (`CATEGORY_SOCIAL`…; Primary is `CATEGORY_PERSONAL`).
    pub fn api_label(self) -> &'static str {
        match self {
            GmailCategory::Primary => "CATEGORY_PERSONAL",
            GmailCategory::Social => "CATEGORY_SOCIAL",
            GmailCategory::Promotions => "CATEGORY_PROMOTIONS",
            GmailCategory::Updates => "CATEGORY_UPDATES",
            GmailCategory::Forums => "CATEGORY_FORUMS",
        }
    }

    /// What Gmail's search is asked to find the messages of it (`category:social`); `None` for
    /// Primary, which is what the others leave.
    pub fn query(self) -> Option<&'static str> {
        match self {
            GmailCategory::Primary => None,
            GmailCategory::Social => Some("category:social"),
            GmailCategory::Promotions => Some("category:promotions"),
            GmailCategory::Updates => Some("category:updates"),
            GmailCategory::Forums => Some("category:forums"),
        }
    }
}

/// Most of Gmail's labels kept for a message (#1153).
pub const MAX_GMAIL_LABELS: usize = 32;
/// Longest of Gmail's labels kept, in characters.
pub const MAX_GMAIL_LABEL_LEN: usize = 128;
/// Most entries of a label list looked at before [`MAX_GMAIL_LABELS`] are kept: the rest are
/// not read.
const MAX_LABELS_READ: usize = 256;

/// Gmail's labels of a message as they are kept (#1153): each cleaned as a name is
/// ([`mailcraft_names::incoming`]: one line, no control or format characters, trimmed, at most
/// [`MAX_GMAIL_LABEL_LEN`] characters), the empty ones dropped, each once (ignoring case), at most
/// [`MAX_GMAIL_LABELS`], in the server's order.
pub fn tidy_labels<S: AsRef<str>>(labels: impl IntoIterator<Item = S>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Each kept label in small letters, worked out once.
    let mut seen: Vec<String> = Vec::new();
    for label in labels.into_iter().take(MAX_LABELS_READ) {
        if out.len() >= MAX_GMAIL_LABELS {
            break;
        }
        let Some(label) = mailcraft_names::incoming(label.as_ref(), MAX_GMAIL_LABEL_LEN) else { continue };
        let lower = label.to_lowercase();
        if label_name(&label).trim().is_empty() || seen.contains(&lower) {
            continue;
        }
        seen.push(lower);
        out.push(label);
    }
    out
}

/// Whether a label is one of Gmail's own (`\Inbox`, `\Sent`, `\Drafts`, `\Starred`,
/// `\Important`…), which Gmail writes with a backslash in front.
pub fn is_system_label(label: &str) -> bool {
    label.starts_with('\\')
}

/// A label as the search box names it: without the backslash of Gmail's own (`important`).
pub fn label_name(label: &str) -> &str {
    label.strip_prefix('\\').unwrap_or(label)
}

/// The last part of a nested label (`Clients` of `Work/Clients`), as the message list and the
/// reading pane show it; the whole path is the tooltip. Gmail parts nested labels with `/`.
pub fn label_leaf(label: &str) -> &str {
    label.rsplit('/').find(|part| !part.trim().is_empty()).unwrap_or(label)
}

/// Gmail's labels of a mailbox file's message, cleaned as a synchronisation's are: a file can say
/// anything there. What isn't a list of text is no labels.
pub(crate) fn labels_from_file<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let serde_json::Value::Array(entries) = serde_json::Value::deserialize(deserializer)? else { return Ok(Vec::new()) };
    Ok(tidy_labels(entries.iter().filter_map(serde_json::Value::as_str)))
}

/// Whether a mail server's name is Gmail's (`imap.gmail.com`, and the same under
/// `googlemail.com`), whatever the case and a dot at its end.
pub fn is_gmail_host(host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    ["gmail.com", "googlemail.com"].iter().any(|domain| host == *domain || host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.')))
}

impl Account {
    /// Whether the account's mail is on Gmail's servers (its incoming server is Gmail's), personal
    /// or Google Workspace, signed in with Google or with an app password: the accounts whose
    /// Inbox has Gmail's categories.
    pub fn is_gmail(&self) -> bool {
        self.incoming.as_ref().is_some_and(|server| is_gmail_host(&server.host))
    }

    /// Whether the folder pane shows Gmail's categories under the account's Inbox when there are
    /// any (#911): a Gmail account whose owner didn't turn them off.
    pub fn shows_gmail_categories(&self) -> bool {
        self.is_gmail() && !self.hide_gmail_categories
    }

    /// Whether mail of the account's Inbox can be moved between Gmail's categories (#911): a Gmail
    /// account that signs in with Google, whose token Gmail's API takes. One signed in with an app
    /// password has categories to look at, and none to move between.
    pub fn moves_gmail_categories(&self) -> bool {
        self.is_gmail() && self.oauth.as_ref().is_some_and(|oauth| oauth.provider == OAuthProvider::Google)
    }
}

impl Message {
    /// Gmail's labels of the message that the message list and the reading pane show (#1153): the
    /// owner's own, not Gmail's (`\Inbox`, `\Sent`, `\Drafts` are the folder it is in,
    /// `\Starred` its flag, and `\Important` is Gmail's guess, not the sender's importance nor
    /// the owner's, and would mark most mail), and not the label of the folder it is shown in
    /// (`folder`, the folder's path on the server), which says nothing new.
    pub fn shown_gmail_labels<'a>(&'a self, folder: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.gmail_labels.iter().map(String::as_str).filter(move |label| !is_system_label(label) && !label.eq_ignore_ascii_case(folder))
    }

    /// The Gmail category the owner moved the message to here, while Gmail hasn't heard of the
    /// move (#911): it differs from what Gmail last said, and Gmail's id of it is known.
    pub fn gmail_move_waiting(&self) -> Option<crate::GmailCategory> {
        let base = self.gmail_synced?;
        self.gmail_category.filter(|here| *here != base && self.gmail_id != 0)
    }
}
