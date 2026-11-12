// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the log says of a Send/Receive's problems, so that a diagnostics report shows what failed:
//! each problem's text, once, when it is first seen or has changed, cut short and capped. The log
//! file takes quoted text, addresses, links and ids out of every line as it is written
//! (`diagnostics::scrub`), and the copied report takes out hosts and paths too.

use mailcraft_model::Id;

/// Texts of one run put in the log at most.
pub(crate) const MAX_LOGGED: usize = 20;
/// Characters of one text put in the log.
pub(crate) const MAX_CHARS: usize = 300;
/// Texts remembered for an account (to tell a problem that comes again from a new one).
const MAX_SEEN: usize = 200;

/// One text as a single log line, cut at a character boundary.
fn one_line(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(MAX_CHARS).collect()
}

/// The texts of `errors` not among those the previous run for `account` had, at most
/// [`MAX_LOGGED`], and how many more there were; remembers this run's texts for the next.
pub(crate) fn fresh(seen: &mut Vec<(Id, Vec<String>)>, account: Id, errors: &[String]) -> (Vec<String>, usize) {
    let mut now: Vec<String> = Vec::new();
    for text in errors.iter().map(|e| one_line(e)) {
        if now.len() < MAX_SEEN && !now.contains(&text) {
            now.push(text);
        }
    }
    let before: &[String] = seen.iter().find(|(a, _)| *a == account).map(|(_, texts)| texts.as_slice()).unwrap_or_default();
    let new: Vec<String> = now.iter().filter(|t| !before.contains(t)).cloned().collect();
    let more = new.len().saturating_sub(MAX_LOGGED);
    match seen.iter_mut().find(|(a, _)| *a == account) {
        Some((_, texts)) => *texts = now,
        None => seen.push((account, now)),
    }
    (new.into_iter().take(MAX_LOGGED).collect(), more)
}

/// A run for `account` without problems: what it had is forgotten, so a problem that comes back
/// later is said again.
pub(crate) fn forget(seen: &mut Vec<(Id, Vec<String>)>, account: Id) {
    seen.retain(|(a, _)| *a != account);
}

/// Put what is new about `errors` in the log, after the count line.
pub(crate) fn log_new(seen: &mut Vec<(Id, Vec<String>)>, account: Id, errors: &[String]) {
    let (texts, more) = fresh(seen, account, errors);
    for text in texts {
        log::warn!("send/receive, account {account}: problem: {text}");
    }
    if more > 0 {
        log::warn!("send/receive, account {account}: and {more} more problem(s) not logged");
    }
}
