// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rules of a Sieve script MailCraft didn't write (#9999): the owner's own active script, made
//! in webmail or another program, or MailCraft's own once it was changed elsewhere. As Outlook
//! lists the server's rules made elsewhere, they are shown in Rules and Alerts; they are the
//! owner's, so they are **read-only**: never run here (the server runs them), switched, moved,
//! deleted, edited or written back.
//!
//! The script is read with the bounded tokenizer ([`crate::sieve::tokens`]) into a small syntax
//! tree (RFC 5228 §8.2: commands, arguments, tests, blocks), within caps on its commands, their
//! nesting and their lists, and then each top-level `if` becomes a rule where its tests and actions
//! are ones MailCraft has ([`crate::cmd::sieve_rules`] read backwards). Whatever else is in a
//! rule is one of its parts not known here, named by its command or test alone: a script's
//! strings are never shown as what it does, nor taken for instructions. A `vacation` is no rule:
//! it says that the script sends Automatic Replies.
//!
//! **Names.** A rule is named by the comment a webmail writes before it: `# rule:[name]` right
//! before its `if`, or, in a script with the line `# RAINLOOP:SIEVE`, the `Name` of the filter
//! block it is in ([`crate::sieve_rainloop`]). A name is display text from the server: one line,
//! without control or format characters, at most [`MAX_NAME`] characters. Without one it is
//! "<script> N". A rule whose test is `false`, or a filter block the webmail says is off and that
//! has no `if`, is shown switched off.

use std::ops::Range;

use mailcraft_model::{Action, Condition, Id, Keep, Mailbox, Rule, ServerRule, Words, same_word};

use super::sieve_replies::SIEVE_ID;
use crate::sieve::{Comment, Tok, lex};
use crate::sieve_rainloop;

/// Where the ids of these rules begin: far above any a mailbox gives out, so that a command naming
/// one is known for what it is, and refused.
pub const ELSEWHERE_BASE: Id = 1 << 52;
/// Rules taken from one script, and ids kept for each account.
pub const MAX_RULES: usize = 256;
/// How deep tests and blocks may nest.
const MAX_DEPTH: usize = 16;
/// Commands read from one script, all levels together.
const MAX_COMMANDS: usize = 2_000;
/// Strings kept from one list, and the longest string kept.
const MAX_LIST: usize = 64;
const MAX_STRING: usize = 1_024;
/// Parts not known here kept for one rule.
const MAX_UNKNOWN: usize = 20;
/// The longest rule name kept from a comment, in characters.
pub const MAX_NAME: usize = sieve_rainloop::MAX_NAME;

/// A script's rules as Rules and Alerts shows them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Elsewhere {
    /// The script's name on the server.
    pub script: String,
    /// It is MailCraft's own, changed elsewhere (not the owner's other script).
    pub ours: bool,
    /// Its rules, read-only.
    pub rules: Vec<Rule>,
    /// It sends Automatic Replies (it has a `vacation`).
    pub replies: bool,
    /// It couldn't be read as Sieve at all.
    pub unreadable: bool,
    /// Its text exactly as the server gave it (never saved): what the owner's own script is
    /// edited from, whole ([`crate::cmd::sieve_own`]).
    pub text: String,
}

/// The script's text stays out of debug output: only its size.
impl std::fmt::Debug for Elsewhere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Elsewhere")
            .field("script", &self.script)
            .field("ours", &self.ours)
            .field("rules", &self.rules)
            .field("replies", &self.replies)
            .field("unreadable", &self.unreadable)
            .field("text", &self.text.len())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Arg {
    Str(String),
    List(Vec<String>),
    Num(u64),
    Tag(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Test {
    name: String,
    args: Vec<Arg>,
    tests: Vec<Test>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Command {
    name: String,
    args: Vec<Arg>,
    tests: Vec<Test>,
    block: Option<Vec<Command>>,
    /// Its bytes in the script, from its name to its `;` or `}`.
    at: Range<usize>,
}

struct Parser {
    toks: Vec<Tok>,
    /// Where each token is.
    spans: Vec<Range<usize>>,
    at: usize,
    commands: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        self.at = self.at.saturating_add(1);
        t
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.at = self.at.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn cut(s: String) -> String {
        if s.len() <= MAX_STRING {
            return s;
        }
        let mut end = MAX_STRING;
        while !s.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        s.get(..end).unwrap_or_default().to_string()
    }

    fn args(&mut self) -> Option<Vec<Arg>> {
        let mut args = Vec::new();
        loop {
            match self.peek() {
                Some(Tok::Str(_)) => {
                    let Some(Tok::Str(s)) = self.next() else { return None };
                    args.push(Arg::Str(Self::cut(s)));
                }
                Some(Tok::Num(_)) => {
                    let Some(Tok::Num(n)) = self.next() else { return None };
                    args.push(Arg::Num(n));
                }
                Some(Tok::Tag(_)) => {
                    let Some(Tok::Tag(t)) = self.next() else { return None };
                    args.push(Arg::Tag(t));
                }
                Some(Tok::Punct('[')) => {
                    self.at = self.at.saturating_add(1);
                    let mut list = Vec::new();
                    loop {
                        match self.next()? {
                            Tok::Str(s) => {
                                if list.len() < MAX_LIST {
                                    list.push(Self::cut(s));
                                }
                            }
                            _ => return None,
                        }
                        if self.eat(']') {
                            break;
                        }
                        if !self.eat(',') {
                            return None;
                        }
                    }
                    args.push(Arg::List(list));
                }
                _ => return Some(args),
            }
            if args.len() > MAX_LIST {
                return None;
            }
        }
    }

    fn test(&mut self, depth: usize) -> Option<Test> {
        if depth > MAX_DEPTH {
            return None;
        }
        let Some(Tok::Ident(name)) = self.next() else { return None };
        let args = self.args()?;
        let mut tests = Vec::new();
        if self.eat('(') {
            loop {
                tests.push(self.test(depth.saturating_add(1))?);
                if tests.len() > MAX_LIST {
                    return None;
                }
                if self.eat(')') {
                    break;
                }
                if !self.eat(',') {
                    return None;
                }
            }
        } else if matches!(self.peek(), Some(Tok::Ident(_))) {
            tests.push(self.test(depth.saturating_add(1))?);
        }
        Some(Test { name, args, tests })
    }

    fn commands(&mut self, depth: usize) -> Option<Vec<Command>> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut out = Vec::new();
        while let Some(t) = self.peek() {
            if *t == Tok::Punct('}') {
                break;
            }
            self.commands = self.commands.saturating_add(1);
            if self.commands > MAX_COMMANDS {
                return None;
            }
            let start = self.spans.get(self.at).map_or(0, |r| r.start);
            let Some(Tok::Ident(name)) = self.next() else { return None };
            let args = self.args()?;
            let mut tests = Vec::new();
            if matches!(self.peek(), Some(Tok::Ident(_))) {
                tests.push(self.test(depth.saturating_add(1))?);
            }
            let block = if self.eat('{') {
                let inner = self.commands(depth.saturating_add(1))?;
                if !self.eat('}') {
                    return None;
                }
                Some(inner)
            } else if self.eat(';') {
                None
            } else {
                return None;
            };
            let end = self.at.checked_sub(1).and_then(|i| self.spans.get(i)).map_or(start, |r| r.end);
            out.push(Command { name, args, tests, block, at: start..end });
        }
        Some(out)
    }
}

/// The script's commands and its comments, or `None` when it isn't Sieve or is beyond the caps.
fn parse(text: &str) -> Option<(Vec<Command>, Vec<Comment>)> {
    let lexed = lex(text)?;
    let (toks, spans) = lexed.toks.into_iter().unzip();
    let mut p = Parser { toks, spans, at: 0, commands: 0 };
    let commands = p.commands(0)?;
    (p.at >= p.toks.len()).then_some((commands, lexed.comments))
}

/// The name a `# rule:[name]` comment gives, cleaned: `None` when it isn't one, or nothing is left.
fn rule_comment(script: &str, c: &Comment) -> Option<String> {
    if c.bracketed {
        return None;
    }
    let inner = c.text(script).trim().strip_prefix("rule:[")?.strip_suffix(']')?;
    mailcraft_names::incoming(inner, MAX_NAME)
}

/// The header names of a test as they are shown: trimmed, each once whatever its letters' case.
fn shown_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names.iter().map(|n| n.trim()) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
            out.push(n.to_string());
        }
    }
    out
}

/// Whether two lists of words are the same words, A to Z in either case, in any order (as Sieve's
/// default comparator, `i;ascii-casemap`, compares them).
fn same_ascii(a: &[String], b: &[String]) -> bool {
    let has = |x: &[String], y: &[String]| x.iter().all(|w| y.iter().any(|v| v.eq_ignore_ascii_case(w)));
    has(a, b) && has(b, a)
}

/// The words of `a` with those of `b` that aren't among them.
fn union(kind: Words, a: &[String], b: &[String]) -> Vec<String> {
    let mut out = a.to_vec();
    for w in b {
        if !out.iter().any(|o| same_word(kind, o, w)) {
            out.push(w.clone());
        }
    }
    out
}

/// One condition that holds exactly when any of `conditions` does, when there is one: conditions
/// of one kind that look for words, any of which is enough, are one that looks for all their words;
/// header conditions that read the same lines, or look for the same words (in either case), are
/// one with all their words, or all their lines.
fn any_of(conditions: &[Condition]) -> Option<Condition> {
    let (first, rest) = conditions.split_first()?;
    let mut out = first.clone();
    for c in rest {
        out = match (&out, c) {
            (Condition::Header { names: n1, words: w1, exact: e1 }, Condition::Header { names: n2, words: w2, exact: e2 }) if e1 == e2 => {
                if same_ascii(n1, n2) {
                    Condition::Header { names: n1.clone(), words: union(Words::Text, w1, w2), exact: *e1 }
                } else if same_ascii(w1, w2) {
                    Condition::Header { names: shown_names(&[n1.clone(), n2.clone()].concat()), words: w1.clone(), exact: *e1 }
                } else {
                    return None;
                }
            }
            (a, b) if std::mem::discriminant(a) == std::mem::discriminant(b) => {
                let (x, y) = (a.words()?, b.words()?);
                a.with_words(union(a.word_kind(), x, y))
            }
            _ => return None,
        };
    }
    Some(out)
}

/// The strings of an argument (one string, or a list).
fn strings(a: &Arg) -> Option<Vec<String>> {
    match a {
        Arg::Str(s) => Some(vec![s.clone()]),
        Arg::List(l) => Some(l.clone()),
        _ => None,
    }
}

/// A test's tags, and its strings in order (header names, then keys).
fn split(args: &[Arg]) -> (Vec<String>, Vec<Vec<String>>) {
    let tags = args.iter().filter_map(|a| if let Arg::Tag(t) = a { Some(t.clone()) } else { None }).collect();
    let lists = args.iter().filter_map(strings).collect();
    (tags, lists)
}

/// The headers a test names, lower-cased and sorted, for comparing.
fn headers(names: &[String]) -> Vec<String> {
    let mut h: Vec<String> = names.iter().map(|n| n.trim().to_ascii_lowercase()).collect();
    h.sort();
    h.dedup();
    h
}

/// A test MailCraft has a condition for, with the owner's addresses `me`.
fn condition(t: &Test, me: &[String]) -> Option<Condition> {
    let (tags, lists) = split(&t.args);
    let only = |allowed: &[&str]| tags.iter().all(|t| allowed.contains(&t.as_str()));
    let (names, keys) = match lists.as_slice() {
        [names, keys] => (headers(names), keys.clone()),
        [keys] => (Vec::new(), keys.clone()),
        _ => (Vec::new(), Vec::new()),
    };
    let contains = tags.iter().any(|t| t == "contains");
    let is = tags.iter().any(|t| t == "is");
    let to_cc = names == ["cc", "to"];
    match t.name.as_str() {
        "header" if only(&["contains", "is"]) && contains != is && !keys.is_empty() => match names.as_slice() {
            [n] if n == "subject" && contains => Some(Condition::SubjectContains(keys)),
            [n] if n == "from" && contains => Some(Condition::FromContains(keys)),
            _ if to_cc && contains => Some(Condition::ToContains(keys)),
            // (An empty name or word would hold of a line that is there at all; that isn't said here.)
            _ if names.is_empty() || names.iter().chain(&keys).any(|w| w.trim().is_empty()) => None,
            _ => {
                let raw = lists.first().map(|n| shown_names(n)).unwrap_or_default();
                Some(Condition::Header { names: raw, words: keys, exact: is })
            }
        },
        "address" if only(&["all", "contains", "is"]) && contains != is && !keys.is_empty() => {
            let mine = || {
                let mut k: Vec<String> = keys.iter().map(|k| k.trim().to_ascii_lowercase()).collect();
                k.sort();
                let mut m = me.to_vec();
                m.sort();
                !k.is_empty() && k == m
            };
            match (names.as_slice(), contains) {
                ([n], true) if n == "from" => Some(Condition::FromContains(keys)),
                ([n], false) if n == "from" => Some(Condition::From(keys)),
                (_, true) if to_cc => Some(Condition::ToContains(keys)),
                (_, false) if to_cc && mine() => Some(Condition::ToOrCcMe),
                (_, false) if to_cc => Some(Condition::SentTo(keys)),
                ([n], false) if n == "to" && mine() => Some(Condition::ToMe),
                ([n], false) if n == "cc" && mine() => Some(Condition::CcMe),
                _ => None,
            }
        }
        "body" if only(&["text", "contains"]) && contains && !keys.is_empty() && names.is_empty() => Some(Condition::BodyContains(keys)),
        _ => None,
    }
}

/// What a rule's tests come to: its conditions, its exceptions, and the tests not known here.
fn read_tests(t: &Test, me: &[String], rule: &mut Rule, unknown: &mut Vec<String>) {
    match t.name.as_str() {
        "allof" => {
            for sub in &t.tests {
                read_tests(sub, me, rule, unknown);
            }
        }
        "true" => {}
        // A rule switched off (as a webmail writes one): shown so, and what it tested is unknown.
        "false" => {
            rule.enabled = false;
            unknown.push("false".into());
        }
        "not" => {
            let spared: Vec<&Test> = match t.tests.as_slice() {
                [one] if one.name == "anyof" => one.tests.iter().collect(),
                [one] => vec![one],
                _ => Vec::new(),
            };
            let alone = t.tests.first().is_some_and(|f| f.name != "anyof");
            for sub in spared {
                match condition(sub, me) {
                    // `not address :is "to" [me]` is MailCraft's "my address isn't in To".
                    Some(Condition::ToMe) if alone => rule.conditions.push(Condition::NotToMe),
                    Some(c) => rule.exceptions.push(c),
                    None => unknown.push("not".into()),
                }
            }
        }
        "anyof" => match t.tests.as_slice() {
            [one] => read_tests(one, me, rule, unknown),
            many => {
                let each: Option<Vec<Condition>> = many.iter().map(|sub| condition(sub, me)).collect();
                match each.as_deref() {
                    Some([Condition::SubjectContains(x), Condition::BodyContains(y)]) if x == y => {
                        rule.conditions.push(Condition::SubjectOrBodyContains(x.clone()));
                    }
                    Some(all) => match any_of(all) {
                        Some(c) => rule.conditions.push(c),
                        None => unknown.push("anyof".into()),
                    },
                    None => unknown.push("anyof".into()),
                }
            }
        },
        "size" => {
            let over = t.args.windows(2).find_map(|w| match w {
                [Arg::Tag(tag), Arg::Num(n)] if tag == "over" => Some(*n),
                _ => None,
            });
            let under = t.args.windows(2).find_map(|w| match w {
                [Arg::Tag(tag), Arg::Num(n)] if tag == "under" => Some(*n),
                _ => None,
            });
            match (over, under) {
                (Some(n), None) => rule.conditions.push(Condition::SizeBetween { min: n.saturating_add(1).div_ceil(1024), max: u64::MAX / 1024 }),
                (None, Some(n)) => rule.conditions.push(Condition::SizeBetween { min: 0, max: n.saturating_sub(1) / 1024 }),
                _ => unknown.push("size".into()),
            }
        }
        _ => match condition(t, me) {
            Some(c) => rule.conditions.push(c),
            None => unknown.push(t.name.clone()),
        },
    }
}

/// What a block's commands come to as actions; whether it sends a reply.
fn read_actions(mb: &Mailbox, account: Id, block: &[Command], rule: &mut Rule, unknown: &mut Vec<String>) -> bool {
    let mut replies = false;
    let folder = |name: &str| mb.folders.iter().find(|f| f.account == account && f.remote.trim() == name.trim()).map(|f| f.id);
    for c in block {
        let (tags, lists) = split(&c.args);
        let one = match lists.as_slice() {
            [l] if l.len() == 1 => l.first().cloned(),
            _ => None,
        };
        match c.name.as_str() {
            "vacation" => replies = true,
            "stop" => rule.stop = true,
            "fileinto" if tags.iter().all(|t| t == "copy") => match one.as_deref().and_then(folder) {
                Some(f) if tags.is_empty() => rule.actions.push(Action::MoveTo(f)),
                Some(f) => rule.actions.push(Action::CopyTo(f)),
                None => unknown.push(c.name.clone()),
            },
            "addflag" | "setflag" if tags.is_empty() => match one.as_deref() {
                Some(f) if f.eq_ignore_ascii_case("\\Seen") => rule.actions.push(Action::MarkRead),
                Some(f) if f.eq_ignore_ascii_case("\\Flagged") => rule.actions.push(Action::Flag),
                _ => unknown.push(c.name.clone()),
            },
            "removeflag" if tags.is_empty() && one.as_deref().is_some_and(|f| f.eq_ignore_ascii_case("\\Flagged")) => {
                rule.actions.push(Action::ClearFlag);
            }
            _ => unknown.push(c.name.clone()),
        }
    }
    replies
}

/// The owner's addresses on `account`, lower-cased.
fn me(mb: &Mailbox, account: Id) -> Vec<String> {
    let Some(a) = mb.account(account) else { return Vec::new() };
    let mut me: Vec<String> = std::iter::once(a.email.as_str())
        .chain(a.sender_addresses.iter().map(|s| s.email.as_str()))
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    me.sort();
    me.dedup();
    me
}

/// The rules of the script `script` (its text `text`) on `account`'s server, read-only.
pub fn read(mb: &Mailbox, account: Id, script: &str, text: &str, ours: bool) -> Elsewhere {
    let mut out = Elsewhere { script: script.chars().take(MAX_STRING).collect(), ours, text: text.to_string(), ..Default::default() };
    let Some((commands, comments)) = parse(text) else {
        out.unreadable = true;
        return out;
    };
    let me = me(mb, account);
    let base = ELSEWHERE_BASE.saturating_add(account.saturating_mul(MAX_RULES as u64));
    let filters = if sieve_rainloop::signed(text, &comments) { sieve_rainloop::filters(text, &comments) } else { Vec::new() };
    // Where each filter block's Sieve is: from its comment to its end, or to the next one's.
    let regions: Vec<Range<usize>> = filters
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let next = filters.get(i.saturating_add(1)).map_or(text.len(), |n| n.header.start);
            f.header.end..f.end.as_ref().map_or(next, |e| e.start.min(next))
        })
        .collect();
    let is_if = |c: &Command| c.name == "if";
    // A block the webmail says is off, with no `if` in it, is a rule switched off.
    let mut off: Vec<(usize, String)> = filters
        .iter()
        .zip(&regions)
        .filter(|(f, r)| f.enabled == Some(false) && !commands.iter().any(|c| is_if(c) && r.contains(&c.at.start)))
        .map(|(f, _)| (f.header.start, f.name.clone().unwrap_or_default()))
        .collect();
    off.reverse();
    let mut named_filter = vec![false; filters.len()];
    let mut before = 0;
    let push_off = |out: &mut Elsewhere, name: String| {
        let n = out.rules.len();
        let name = if name.is_empty() { format!("{} {}", out.script, n.saturating_add(1)) } else { name };
        let server = ServerRule { id: format!("{SIEVE_ID}{}", out.script), read_only: true, unknown: vec!["filter".into()], ..Default::default() };
        out.rules.push(Rule {
            id: base.saturating_add(n as u64),
            name,
            enabled: false,
            account,
            keep: Keep::Server,
            server: Some(server),
            ..Default::default()
        });
    };
    for c in &commands {
        while out.rules.len() < MAX_RULES && off.last().is_some_and(|(at, _)| *at < c.at.start) {
            if let Some((_, name)) = off.pop() {
                push_off(&mut out, name);
            }
        }
        if out.rules.len() >= MAX_RULES {
            break;
        }
        let between = before..c.at.start;
        before = c.at.end;
        // Its name: the filter block it is the first `if` of, or the `# rule:[…]` right before it.
        let mut name = None;
        if is_if(c) {
            if let Some(i) = regions.iter().position(|r| r.contains(&c.at.start))
                && let Some(seen) = named_filter.get_mut(i)
                && !*seen
            {
                *seen = true;
                name = filters.get(i).and_then(|f| f.name.clone());
            }
            if name.is_none() {
                name = comments
                    .iter()
                    .filter(|k| between.start <= k.at.start && k.at.end <= between.end)
                    .filter_map(|k| rule_comment(text, k))
                    .next_back();
            }
        }
        let mut rule = Rule { enabled: true, account, keep: Keep::Server, ..Default::default() };
        let mut unknown = Vec::new();
        match c.name.as_str() {
            "require" => continue,
            "if" | "elsif" | "else" => {
                if c.name != "if" {
                    unknown.push(c.name.clone());
                }
                for t in &c.tests {
                    read_tests(t, &me, &mut rule, &mut unknown);
                }
                let block = c.block.as_deref().unwrap_or_default();
                // A block that only replies is the script's Automatic Replies, no rule.
                if !block.is_empty() && block.iter().all(|b| b.name == "vacation") {
                    out.replies = true;
                    continue;
                }
                out.replies |= read_actions(mb, account, block, &mut rule, &mut unknown);
            }
            "vacation" => {
                out.replies = true;
                continue;
            }
            // An action for every message, outside any test.
            _ => unknown.push(c.name.clone()),
        }
        unknown.truncate(MAX_UNKNOWN);
        unknown.dedup();
        let n = out.rules.len();
        rule.id = base.saturating_add(n as u64);
        rule.name = name.unwrap_or_else(|| format!("{} {}", out.script, n.saturating_add(1)));
        rule.server = Some(ServerRule { id: format!("{SIEVE_ID}{}", out.script), read_only: true, unknown, ..Default::default() });
        out.rules.push(rule);
    }
    while out.rules.len() < MAX_RULES {
        let Some((_, name)) = off.pop() else { break };
        push_off(&mut out, name);
    }
    out
}

/// Whether `id` is one of these rules' (they are refused by every command that would change one).
pub fn is_elsewhere(id: Id) -> bool {
    id >= ELSEWHERE_BASE
}
