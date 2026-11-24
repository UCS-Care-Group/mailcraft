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

use mailcraft_model::{Action, Condition, Id, Keep, Mailbox, Rule, ServerRule};

use super::sieve_replies::SIEVE_ID;
use crate::sieve::{Tok, tokens};

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

/// A script's rules as Rules and Alerts shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
}

struct Parser {
    toks: Vec<Tok>,
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
            out.push(Command { name, args, tests, block });
        }
        Some(out)
    }
}

/// The script's commands, or `None` when it isn't Sieve or is beyond the caps.
fn parse(text: &str) -> Option<Vec<Command>> {
    let mut p = Parser { toks: tokens(text)?, at: 0, commands: 0 };
    let commands = p.commands(0)?;
    (p.at >= p.toks.len()).then_some(commands)
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
        "header" if contains && only(&["contains"]) && !keys.is_empty() => match names.as_slice() {
            [n] if n == "subject" => Some(Condition::SubjectContains(keys)),
            _ => None,
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
            [a, b] => match (condition(a, me), condition(b, me)) {
                (Some(Condition::SubjectContains(x)), Some(Condition::BodyContains(y))) if x == y => {
                    rule.conditions.push(Condition::SubjectOrBodyContains(x));
                }
                _ => unknown.push("anyof".into()),
            },
            _ => unknown.push("anyof".into()),
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
    let mut out = Elsewhere { script: script.chars().take(MAX_STRING).collect(), ours, ..Default::default() };
    let Some(commands) = parse(text) else {
        out.unreadable = true;
        return out;
    };
    let me = me(mb, account);
    let base = ELSEWHERE_BASE.saturating_add(account.saturating_mul(MAX_RULES as u64));
    for c in &commands {
        if out.rules.len() >= MAX_RULES {
            break;
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
        rule.name = format!("{} {}", out.script, n.saturating_add(1));
        rule.server = Some(ServerRule { id: format!("{SIEVE_ID}{}", out.script), read_only: true, unknown, ..Default::default() });
        out.rules.push(rule);
    }
    out
}

/// Whether `id` is one of these rules' (they are refused by every command that would change one).
pub fn is_elsewhere(id: Id) -> bool {
    id >= ELSEWHERE_BASE
}
