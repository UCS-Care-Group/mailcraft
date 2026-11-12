// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the log says of a Send/Receive's problems, so that a diagnostics report shows what failed:
//! each problem's text, once, when it is first seen or has changed, cut short and capped.
//!
//! Privacy does not rest on quotes pairing up: a problem told as a [`Text`] is logged with every
//! value it was given (a subject, a folder, an address) put aside ([`masked`]), and a free text
//! has the quotes inside the names it embeds turned to `'` where it is made. The log file also
//! takes quoted text, addresses, links and ids out of every line as it is written
//! (`diagnostics::scrub`), and the copied report takes out hosts and paths too.

use mailcraft_i18n::Text;
use mailcraft_model::Id;

/// New texts of one run put in the log at most (a run is one job's answers together).
pub(crate) const MAX_LOGGED: usize = 20;
/// Characters of one text put in the log.
pub(crate) const MAX_CHARS: usize = 300;
/// Texts remembered for an account's job (to tell a problem that comes again from a new one).
pub(crate) const MAX_SEEN: usize = 200;
/// A word this long of base64's letters is not a word (a sign-in's echo, a token).
const LONG_WORD: usize = 24;

/// Who a problem came from: the account and the kind of job (mail, calendar, tasks, rules, shared
/// mailboxes), which each keep their own memory.
type Key = (Id, &'static str);

/// What the log has said already, and how much of this run's allowance is used.
#[derive(Default)]
pub(crate) struct ProblemLog {
    /// Per job, the keys of what was logged, oldest first.
    seen: Vec<(Key, Vec<String>)>,
    /// Per job, how many texts this run has logged.
    used: Vec<(Key, usize)>,
}

/// `text` as one log line: control characters as spaces, base64-looking long words dropped, cut at
/// a character boundary.
fn one_line(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let long_base64 = |w: &str| w.chars().count() >= LONG_WORD && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='));
    let kept: Vec<&str> = flat.split(' ').filter(|w| !long_base64(w)).collect();
    kept.join(" ").chars().take(MAX_CHARS).collect()
}

/// What tells two texts apart for "said before": the digits (a count, a time, of any length) don't.
fn key_of(line: &str) -> String {
    let mut key = String::with_capacity(line.len());
    for c in line.chars() {
        if !c.is_ascii_digit() {
            key.push(c);
        } else if !key.ends_with('#') {
            key.push('#');
        }
    }
    key
}

/// `text` in English with every value it was given put aside as `…`, whatever the value is:
/// a subject, a folder's name, an address. A number alone stays.
pub(crate) fn masked(text: &Text) -> String {
    text.clone()
        .map_plain(&|value: String| if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) { value } else { "…".to_string() })
        .to_string()
}

impl ProblemLog {
    /// The texts of `errors` that this job hasn't had logged, as lines, at most what is left of
    /// the run's allowance, and how many more there were; remembers what it returns.
    pub(crate) fn fresh(&mut self, account: Id, source: &'static str, errors: &[String]) -> (Vec<String>, usize) {
        let key = (account, source);
        let at = match self.seen.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                self.seen.push((key, Vec::new()));
                self.seen.len() - 1
            }
        };
        let used_at = match self.used.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                self.used.push((key, 0));
                self.used.len() - 1
            }
        };
        let (mut out, mut more) = (Vec::new(), 0);
        for line in errors.iter().map(|e| one_line(e)).filter(|l| !l.is_empty()) {
            let k = key_of(&line);
            let Some((_, memory)) = self.seen.get_mut(at) else { break };
            if memory.contains(&k) {
                continue;
            }
            let Some((_, used)) = self.used.get_mut(used_at) else { break };
            if *used >= MAX_LOGGED {
                more += 1;
                continue;
            }
            *used += 1;
            memory.push(k);
            if memory.len() > MAX_SEEN {
                memory.remove(0);
            }
            out.push(line);
        }
        (out, more)
    }

    /// A run of this job is over: its allowance is its own again.
    pub(crate) fn end_run(&mut self, account: Id, source: &'static str) {
        self.used.retain(|(k, _)| *k != (account, source));
    }

    /// A run of this job finished without problems: a problem that comes back is said again.
    pub(crate) fn forget(&mut self, account: Id, source: &'static str) {
        self.seen.retain(|(k, _)| *k != (account, source));
    }

    /// Put what is new in `lines` in the log, after the count line.
    pub(crate) fn log_new(&mut self, account: Id, source: &'static str, lines: &[String]) {
        let (texts, more) = self.fresh(account, source, lines);
        for text in texts {
            log::warn!("send/receive, account {account}: problem: {text}");
        }
        if more > 0 {
            log::warn!("send/receive, account {account}: and {more} more problem(s) not logged");
        }
    }
}
