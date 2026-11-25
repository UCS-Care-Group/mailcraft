// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules of an IMAP account kept on its server (#9999): each one a block of MailCraft's Sieve
//! script ([`crate::sieve::script`]), after the Automatic Replies, in the rules' order.
//!
//! **Only what Sieve says as the rule means it.** A rule goes to the server when every one of its
//! conditions, exceptions and actions has a Sieve that does what it does here
//! ([`block`]); any other stays on this computer, and why is said as for a Microsoft account
//! ([`NotOnServer`]). The Sieve used: the base language (RFC 5228: `address`, `header`, `size`,
//! `allof`, `anyof`, `not`, `stop`), `fileinto` (RFC 5228 §4.1), `copy` (RFC 3894), `imap4flags`
//! (RFC 5232) and `body` (RFC 5173), each only when the server's scripts may use it. No rule
//! made in the form forwards, redirects, rejects, discards or replies.
//!
//! **A Sieve rule** ([`mailcraft_model::Rule::sieve`]) is Sieve the owner wrote: its block is that
//! text, under its id and a comment with its name ([`crate::sieve::comment_name`]), and its
//! extensions join the script's one `require`. It may do whatever the server's Sieve does,
//! redirect and reply too, when the server's scripts may use what it needs.
//!
//! **Folders** are named as `fileinto` takes them: the mailbox's whole name on the IMAP server, in
//! UTF-8 with the server's own separator ([`mailcraft_model::Folder::remote`]); RFC 5228 §2.7.2
//! leaves turning it into IMAP's modified UTF-7 to the server. A folder the server doesn't have
//! keeps the rule here.
//!
//! **Nothing in a rule becomes Sieve**: every word, address and folder name is one quoted string
//! ([`crate::sieve::quoted`]).

use mailcraft_model::{Action, Condition, FolderKind, Id, Mailbox, Rule};

use super::rules::NotOnServer;
use crate::sieve::{RuleScript, list, quoted};

/// The owner's own addresses on `account`: its address and the other addresses mail is sent from.
fn me(mb: &Mailbox, account: Id) -> Vec<String> {
    let Some(a) = mb.account(account) else { return Vec::new() };
    let mut me: Vec<String> = std::iter::once(a.email.as_str())
        .chain(a.sender_addresses.iter().map(|s| s.email.as_str()))
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .collect();
    me.dedup();
    me
}

/// What a condition is in Sieve, and the extensions it needs. `header`: a [`Condition::Header`]
/// may be written (a rule of the owner's own script, #9999); MailCraft's own rules have none.
fn test(c: &Condition, me: &[String], needs: &mut Vec<&'static str>, header: bool) -> Result<String, NotOnServer> {
    let to_cc = list(&["to", "cc"]);
    let mine = || if me.is_empty() { Err(NotOnServer::NoAddress) } else { Ok(list(me)) };
    Ok(match c {
        // In the addresses alone, as Microsoft's server reads them: a rule kept on the server reads
        // them so here too (`Mailbox::rule_reads_addresses`). Words that can't be in an address
        // look for a name, which the server doesn't.
        Condition::FromContains(w) | Condition::ToContains(w) if w.iter().any(|t| !super::rules::in_an_address(t)) => {
            return Err(NotOnServer::Names);
        }
        Condition::FromContains(w) => format!("address :all :contains \"from\" {}", list(w)),
        Condition::ToContains(w) => format!("address :all :contains {to_cc} {}", list(w)),
        Condition::SubjectContains(w) => format!("header :contains \"subject\" {}", list(w)),
        Condition::BodyContains(w) => {
            needs.push("body");
            format!("body :text :contains {}", list(w))
        }
        Condition::SubjectOrBodyContains(w) => {
            needs.push("body");
            format!("anyof (header :contains \"subject\" {0}, body :text :contains {0})", list(w))
        }
        Condition::From(people) => format!("address :all :is \"from\" {}", list(people)),
        Condition::SentTo(people) => format!("address :all :is {to_cc} {}", list(people)),
        Condition::ToMe => format!("address :all :is \"to\" {}", mine()?),
        Condition::CcMe => format!("address :all :is \"cc\" {}", mine()?),
        Condition::ToOrCcMe => format!("address :all :is {to_cc} {}", mine()?),
        Condition::NotToMe => format!("not address :all :is \"to\" {}", mine()?),
        Condition::SizeBetween { min, max } => {
            // Here: its size in whole KB is from `min` to `max`, both included.
            let low = min.checked_mul(1024).ok_or(NotOnServer::Size)?;
            let high = max.checked_add(1).and_then(|m| m.checked_mul(1024)).ok_or(NotOnServer::Size)?;
            format!("allof (not size :under {low}, size :under {high})")
        }
        Condition::HasAttachment => return Err(NotOnServer::Attachments),
        Condition::Importance(_) => return Err(NotOnServer::Importance),
        Condition::Sensitivity(_) => return Err(NotOnServer::Sensitivity),
        Condition::Category(_) => return Err(NotOnServer::Categories),
        Condition::MeetingRequest => return Err(NotOnServer::MeetingRequest),
        Condition::AutomaticReply => return Err(NotOnServer::AutomaticReply),
        Condition::SentOnlyToMe => return Err(NotOnServer::OnlyToMe),
        Condition::Flagged => return Err(NotOnServer::Flagged),
        Condition::ReceivedBetween { .. } => return Err(NotOnServer::Days),
        Condition::Header { names, words, exact } if header && !names.is_empty() && !words.is_empty() => {
            format!("header {} {} {}", if *exact { ":is" } else { ":contains" }, list(names), list(words))
        }
        // (A part a later version knows: never guessed at.)
        #[allow(unreachable_patterns)]
        _ => return Err(NotOnServer::Unknown),
    })
}

/// The name on the server of the folder `folder`, which must be `account`'s and on the server.
fn folder_name(mb: &Mailbox, account: Id, folder: Id, copy: bool) -> Result<String, NotOnServer> {
    let not_there = if copy { NotOnServer::CopyNotThere } else { NotOnServer::FolderNotThere };
    let f = mb.folder(folder).ok_or(not_there)?;
    if f.account != account {
        let local = mb.account(f.account).is_some_and(|a| !a.kind.has_server());
        return Err(match (local, copy) {
            (true, false) => NotOnServer::FolderHere,
            (false, false) => NotOnServer::FolderElsewhere,
            (true, true) => NotOnServer::CopyHere,
            (false, true) => NotOnServer::CopyElsewhere,
        });
    }
    let name = f.remote.trim();
    if name.is_empty() {
        return Err(not_there);
    }
    Ok(name.to_string())
}

/// `rule`, for `account`'s server, as a block of the script, or why it can't be one.
pub fn block(mb: &Mailbox, rule: &Rule, account: Id) -> Result<RuleScript, NotOnServer> {
    let Some(a) = mb.account(account) else { return Err(NotOnServer::NoServer) };
    if rule.is_sieve() {
        return sieve_block(a, rule);
    }
    let (text, needs) = rule_sieve(mb, rule, account, false, "    ", "\r\n")?;
    Ok(RuleScript { id: rule.id, text: format!("{text}\r\n"), requires: needs, name: String::new() })
}

/// The Sieve of a rule made in the form (not a Sieve rule), as one `if` with its block, indented
/// by `indent` and with the line breaks `nl` (none after its `}`), and the extensions it needs;
/// or why it can't be said in Sieve. `header`: a [`Condition::Header`] may be written (#9999).
pub(crate) fn rule_sieve(mb: &Mailbox, rule: &Rule, account: Id, header: bool, indent: &str, nl: &str) -> Result<(String, Vec<String>), NotOnServer> {
    let Some(a) = mb.account(account) else { return Err(NotOnServer::NoServer) };
    if rule.is_sieve() {
        return Err(NotOnServer::SieveText);
    }
    if rule.alert {
        return Err(NotOnServer::Alert);
    }
    // A rule without conditions acts on nothing here; on the server it would act on every message.
    if rule.conditions.is_empty() {
        return Err(NotOnServer::NoConditions);
    }
    let me = me(mb, account);
    let mut needs: Vec<&'static str> = Vec::new();
    let mut tests = Vec::new();
    for c in &rule.conditions {
        tests.push(test(c, &me, &mut needs, header)?);
    }
    if !rule.exceptions.is_empty() {
        let mut spared = Vec::new();
        for c in &rule.exceptions {
            spared.push(test(c, &me, &mut needs, header)?);
        }
        tests.push(format!("not anyof ({})", spared.join(", ")));
    }
    // Flags first: `imap4flags` gives them to the copy each `fileinto` makes, and to what is kept.
    let (mut flags, mut copies, mut moves) = (Vec::new(), Vec::new(), Vec::new());
    for action in &rule.actions {
        match action {
            Action::MarkRead => flags.push(format!("addflag {};", quoted("\\Seen"))),
            Action::Flag => flags.push(format!("addflag {};", quoted("\\Flagged"))),
            Action::ClearFlag => flags.push(format!("removeflag {};", quoted("\\Flagged"))),
            Action::MoveTo(folder) => moves.push(folder_name(mb, account, *folder, false)?),
            Action::CopyTo(folder) => copies.push(folder_name(mb, account, *folder, true)?),
            Action::Delete => {
                let deleted = mb.folders.iter().find(|f| f.account == account && f.kind == FolderKind::Deleted && !f.remote.trim().is_empty());
                moves.push(deleted.map(|f| f.remote.trim().to_string()).ok_or(NotOnServer::NoDeletedItems)?);
            }
            Action::FlagFor(_) => return Err(NotOnServer::FlagDue),
            Action::Categorize(_) => return Err(NotOnServer::Categorize),
            Action::ClearCategories => return Err(NotOnServer::ClearCategories),
            Action::SetImportance(_) => return Err(NotOnServer::SetImportance),
            #[allow(unreachable_patterns)]
            _ => return Err(NotOnServer::Unknown),
        }
    }
    if moves.len() > 1 {
        return Err(NotOnServer::TwoMoves);
    }
    if copies.len() > 1 {
        return Err(NotOnServer::TwoCopies);
    }
    if !flags.is_empty() {
        needs.push("imap4flags");
    }
    if !copies.is_empty() {
        needs.extend(["fileinto", "copy"]);
    }
    if !moves.is_empty() {
        needs.push("fileinto");
    }
    needs.sort_unstable();
    needs.dedup();
    if needs.iter().any(|e| !a.sieve_offers(e)) {
        return Err(NotOnServer::Extension);
    }
    let condition = match tests.as_slice() {
        [one] => one.clone(),
        many => format!("allof ({})", many.join(", ")),
    };
    let mut text = format!("if {condition} {{{nl}");
    let lines = flags
        .into_iter()
        .chain(copies.iter().map(|f| format!("fileinto :copy {};", quoted(f))))
        .chain(moves.iter().map(|f| format!("fileinto {};", quoted(f))))
        .chain(rule.stop.then(|| "stop;".to_string()));
    for line in lines {
        text.push_str(&format!("{indent}{line}{nl}"));
    }
    text.push('}');
    Ok((text, needs.into_iter().map(str::to_string).collect()))
}

/// A Sieve rule ([`Rule::sieve`]) as a block of the script: its text as the owner wrote it,
/// checked again (a mailbox file is hostile too) as a block that can't reach out of its place
/// ([`crate::sieve_check::check_block`]), and what it needs, which the script's `require` names.
/// One whose text has a mistake, or needs what the server's scripts can't use, isn't written.
fn sieve_block(a: &mailcraft_model::Account, rule: &Rule) -> Result<RuleScript, NotOnServer> {
    if rule.sieve.len() > mailcraft_model::MAX_SIEVE_RULE {
        return Err(NotOnServer::SieveText);
    }
    let text = crate::sieve::crlf_lines(&rule.sieve);
    let checked = crate::sieve_check::check_block(&text, None).map_err(|_| NotOnServer::SieveText)?;
    if checked.needs.iter().any(|e| !a.sieve_offers(e)) {
        return Err(NotOnServer::Extension);
    }
    Ok(RuleScript { id: rule.id, text, requires: checked.needs, name: rule.name.clone() })
}

/// Why `rule` can't be one of `account`'s server's, when it can't ([`block`]).
pub fn cannot(mb: &Mailbox, rule: &Rule, account: Id) -> Option<NotOnServer> {
    block(mb, rule, account).err()
}

/// The rules of `account` that its server is to keep, in their order, each as its block: those
/// to be kept there that Sieve can say, and switched on (one switched off is kept here as it is,
/// and left out of the script, which then doesn't run it). Also every rule to be kept there whose
/// block could be made, switched on or not, and where it stands among them.
pub fn blocks(mb: &Mailbox, account: Id) -> (Vec<RuleScript>, Vec<(Id, bool, i32)>) {
    let mut written = Vec::new();
    let mut kept = Vec::new();
    for rule in mb.rules.iter().filter(|r| r.account == account && r.keep == mailcraft_model::Keep::Server) {
        let Ok(block) = block(mb, rule, account) else { continue };
        let at = i32::try_from(kept.len()).unwrap_or(i32::MAX);
        kept.push((rule.id, rule.enabled, at));
        if rule.enabled {
            written.push(block);
        }
    }
    (written, kept)
}
