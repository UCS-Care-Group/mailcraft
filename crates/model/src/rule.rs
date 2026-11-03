// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail rules: conditions on an arriving message and what to do with it.
//!
//! **Whose rule (#566).** A rule is for the arriving mail of one account ([`Rule::account`]). A
//! rule from before rules had accounts has none: it is for every account's mail, and is kept on
//! this computer until the owner moves it to an account's server.
//!
//! **Where it is kept.** A rule of a Microsoft account can be kept on its server ([`Keep::Server`]),
//! which runs it whether MailCraft is open or not, and shows it in Outlook on the web and on the
//! phone. Once the server has it ([`Rule::server`]) it never runs here on arriving mail: the
//! server has run it already. Until then it runs here, as every rule kept on this computer does.
//! A rule made in Outlook or on the web is the server's too; one with parts this version doesn't
//! know ([`ServerRule::unknown`]) is switched on and off, moved and deleted, never rewritten and
//! never run here.

use serde::{Deserialize, Serialize};

use crate::time::{Date, DateTime, Timestamp, Zone};
use crate::{Address, FlagStatus, Id, Importance, Message, ModelError, Sensitivity, contains_ci};

/// Most words one condition looks for, any of which is enough.
pub const MAX_WORDS_EACH: usize = 20;

/// The words of a condition, any of which is enough (Outlook's Search Text): one is written as
/// text, as rules had it before they took several (#566), so that an earlier MailCraft still
/// reads it; either is read.
mod words {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(words: &Vec<String>, s: S) -> Result<S::Ok, S::Error> {
        match words.as_slice() {
            [one] => one.serialize(s),
            more => more.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            One(String),
            More(Vec<String>),
        }
        Ok(match Either::deserialize(d)? {
            Either::One(word) => vec![word],
            Either::More(words) => words,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Condition {
    /// The sender's name or address contains one of the words (a rule kept on a Microsoft
    /// account's server: the address alone, as the server reads it).
    FromContains(#[serde(with = "words")] Vec<String>),
    /// A To or Cc recipient's name or address contains one of the words (a rule kept on a
    /// Microsoft account's server: the addresses alone).
    ToContains(#[serde(with = "words")] Vec<String>),
    SubjectContains(#[serde(with = "words")] Vec<String>),
    BodyContains(#[serde(with = "words")] Vec<String>),
    HasAttachment,
    Importance(Importance),
    /// The sender is one of these people, by their whole address (Outlook's "from people or
    /// public group").
    From(#[serde(with = "words")] Vec<String>),
    /// One of these people is among the To and Cc recipients, by their whole address.
    SentTo(#[serde(with = "words")] Vec<String>),
    /// The account's own address is the one recipient: alone in To, and nobody in Cc.
    SentOnlyToMe,
    /// The account's own address is in To.
    ToMe,
    /// It is in Cc.
    CcMe,
    /// It is in To or Cc.
    ToOrCcMe,
    /// It is not in To.
    NotToMe,
    SubjectOrBodyContains(#[serde(with = "words")] Vec<String>),
    /// A line of the header holds one of the words ([`Message::header_contains`]).
    HeaderContains(#[serde(with = "words")] Vec<String>),
    Sensitivity(Sensitivity),
    /// Its size is between these, in KB, both included.
    SizeBetween {
        min: u64,
        max: u64,
    },
    /// A meeting invitation or an update of one ([`Message::is_meeting_request`]).
    MeetingRequest,
    /// An automatic reply, an out-of-office one say ([`Message::is_automatic_reply`]).
    AutomaticReply,
    /// It has one of these categories.
    Category(#[serde(with = "words")] Vec<String>),
    /// It is flagged for follow up (and not complete).
    Flagged,
    /// It was received on one of these days, both included, by the clock of the zone the
    /// calendar is shown in ([`Matching::zone`]); a missing end is open. Only on this computer:
    /// a server's rules have no such condition.
    ReceivedBetween {
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<Date>,
        #[serde(skip_serializing_if = "Option::is_none")]
        to: Option<Date>,
    },
}

/// What a rule is matched by besides the message.
#[derive(Clone, Copy, Debug)]
pub struct Matching<'a> {
    /// Look for the words of a sender or a recipient in their addresses alone: a rule of an
    /// account whose server keeps rules does, as its server looks there alone (Exchange's
    /// "words in the sender's address"), and so does this computer for it.
    pub addresses_only: bool,
    /// The address of the account whose mail it is: the "me" of "sent only to me". Empty for
    /// none, and then no such condition holds.
    pub me: &'a str,
    /// The zone whose clock the days of [`Condition::ReceivedBetween`] are by.
    pub zone: Zone,
}

impl Default for Matching<'_> {
    fn default() -> Self {
        Matching { addresses_only: false, me: "", zone: Zone::fixed(0).unwrap_or_else(Zone::system) }
    }
}

/// The clock a rule runs by: when "today" is for a flag it sets, and the hour its due day ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuleClock {
    pub now: Timestamp,
    /// The zone the calendar is shown in.
    pub zone: Zone,
    /// The hour a flag falls due on its day: the end of the working day, as the ribbon's.
    pub due_hour: u8,
}

/// Whether one of `words` (an empty one looks for nothing) is in one of `texts`, whatever the
/// letters' case: each text is put in lower case once, for all the words, and only when the texts
/// before it don't have one.
fn any_in(words: &[String], texts: &[&str]) -> bool {
    let words: Vec<String> = words.iter().filter(|w| !w.is_empty()).map(|w| w.to_lowercase()).collect();
    !words.is_empty()
        && texts.iter().any(|text| {
            let text = text.to_lowercase();
            words.iter().any(|w| text.contains(w.as_str()))
        })
}

impl Condition {
    pub fn matches(&self, m: &Message) -> bool {
        self.matches_in(m, &Matching::default())
    }

    /// [`Condition::matches`], by `how` ([`Matching`]).
    pub fn matches_in(&self, m: &Message, how: &Matching<'_>) -> bool {
        let person = |a: &Address, t: &str| contains_ci(&a.email, t) || (!how.addresses_only && contains_ci(&a.name, t));
        let any = |words: &[String], hit: &dyn Fn(&str) -> bool| words.iter().filter(|w| !w.is_empty()).any(|w| hit(w));
        let is = |a: &Address, who: &str| !who.trim().is_empty() && a.email.trim().eq_ignore_ascii_case(who.trim());
        let me = |a: &Address| is(a, how.me);
        match self {
            Condition::FromContains(w) => any(w, &|t| person(&m.from, t)),
            Condition::ToContains(w) => any(w, &|t| m.to.iter().chain(&m.cc).any(|a| person(a, t))),
            Condition::SubjectContains(w) => any(w, &|t| contains_ci(&m.subject, t)),
            Condition::BodyContains(w) => any_in(w, &[&m.body.text, &m.hidden_text]),
            Condition::HasAttachment => m.has_attachments(),
            Condition::Importance(i) => m.importance == *i,
            Condition::From(people) => people.iter().any(|p| is(&m.from, p)),
            Condition::SentTo(people) => people.iter().any(|p| m.to.iter().chain(&m.cc).any(|a| is(a, p))),
            Condition::SentOnlyToMe => matches!(m.to.as_slice(), [only] if me(only)) && m.cc.is_empty(),
            Condition::ToMe => m.to.iter().any(me),
            Condition::CcMe => m.cc.iter().any(me),
            Condition::ToOrCcMe => m.to.iter().chain(&m.cc).any(me),
            Condition::NotToMe => !how.me.trim().is_empty() && !m.to.iter().any(me),
            Condition::SubjectOrBodyContains(w) => any_in(w, &[&m.subject, &m.body.text, &m.hidden_text]),
            Condition::HeaderContains(w) => {
                let lines = m.header_lines_lowered();
                any_in(w, &lines.iter().map(String::as_str).collect::<Vec<_>>())
            }
            Condition::Sensitivity(s) => m.sensitivity() == *s,
            Condition::SizeBetween { min, max } => m.size_known() && m.list_size().is_some_and(|bytes| (*min..=*max).contains(&(bytes / 1024))),
            Condition::MeetingRequest => m.is_meeting_request(),
            Condition::AutomaticReply => m.is_automatic_reply(),
            Condition::Category(names) => names.iter().any(|c| m.has_category(c)),
            Condition::Flagged => m.flag == FlagStatus::Flagged,
            Condition::ReceivedBetween { from, to } => {
                let at = if m.received != 0 { m.received } else { m.date };
                let day = how.zone.wall(at).date;
                (from.is_some() || to.is_some()) && from.is_none_or(|f| day >= f) && to.is_none_or(|t| day <= t)
            }
        }
    }

    /// The condition in words: `from contains "newsletter"`.
    pub fn describe(&self) -> String {
        self.describe_as(false)
    }

    /// [`Condition::describe`] for a rule that reads addresses alone ([`Matching::addresses_only`]).
    pub fn describe_as(&self, addresses_only: bool) -> String {
        let words = |w: &[String]| w.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" or ");
        match self {
            Condition::FromContains(t) if addresses_only => format!("sender's address contains {}", words(t)),
            Condition::ToContains(t) if addresses_only => format!("recipient's address contains {}", words(t)),
            Condition::FromContains(t) => format!("from contains {}", words(t)),
            Condition::ToContains(t) => format!("recipient contains {}", words(t)),
            Condition::SubjectContains(t) => format!("subject contains {}", words(t)),
            Condition::BodyContains(t) => format!("body contains {}", words(t)),
            Condition::HasAttachment => "has an attachment".into(),
            Condition::Importance(i) => format!("importance is {}", i.code()),
            Condition::From(p) => format!("from {}", words(p)),
            Condition::SentTo(p) => format!("sent to {}", words(p)),
            Condition::SentOnlyToMe => "sent only to me".into(),
            Condition::ToMe => "my address is in To".into(),
            Condition::CcMe => "my address is in Cc".into(),
            Condition::ToOrCcMe => "my address is in To or Cc".into(),
            Condition::NotToMe => "my address isn't in To".into(),
            Condition::SubjectOrBodyContains(t) => format!("subject or body contains {}", words(t)),
            Condition::HeaderContains(t) => format!("header contains {}", words(t)),
            Condition::Sensitivity(s) => format!("sensitivity is {}", s.code()),
            Condition::SizeBetween { min, max } => format!("size is {min} KB to {max} KB"),
            Condition::MeetingRequest => "is a meeting invitation or update".into(),
            Condition::AutomaticReply => "is an automatic reply".into(),
            Condition::Category(c) => format!("category is {}", words(c)),
            Condition::Flagged => "is flagged".into(),
            Condition::ReceivedBetween { from, to } => match (from, to) {
                (Some(f), Some(t)) => format!("received from {} to {}", f.iso(), t.iso()),
                (Some(f), None) => format!("received on or after {}", f.iso()),
                (None, Some(t)) => format!("received on or before {}", t.iso()),
                (None, None) => "received on any day".into(),
            },
        }
    }

    /// The words it looks for, when it looks for words.
    pub fn words(&self) -> Option<&[String]> {
        match self {
            Condition::FromContains(w)
            | Condition::ToContains(w)
            | Condition::SubjectContains(w)
            | Condition::BodyContains(w)
            | Condition::From(w)
            | Condition::SentTo(w)
            | Condition::SubjectOrBodyContains(w)
            | Condition::HeaderContains(w)
            | Condition::Category(w) => Some(w),
            _ => None,
        }
    }

    /// The same condition with other words (one that looks for none is itself).
    pub fn with_words(&self, words: Vec<String>) -> Condition {
        match self {
            Condition::FromContains(_) => Condition::FromContains(words),
            Condition::ToContains(_) => Condition::ToContains(words),
            Condition::SubjectContains(_) => Condition::SubjectContains(words),
            Condition::BodyContains(_) => Condition::BodyContains(words),
            Condition::From(_) => Condition::From(words),
            Condition::SentTo(_) => Condition::SentTo(words),
            Condition::SubjectOrBodyContains(_) => Condition::SubjectOrBodyContains(words),
            Condition::HeaderContains(_) => Condition::HeaderContains(words),
            Condition::Category(_) => Condition::Category(words),
            other => other.clone(),
        }
    }

    /// Whether it reads the body, which has to be in memory for it.
    pub fn reads_body(&self) -> bool {
        matches!(self, Condition::BodyContains(_) | Condition::SubjectOrBodyContains(_) | Condition::MeetingRequest)
    }

    /// [`Condition::matches_in`], and not on a guess: a sensitivity that can't be read yet doesn't
    /// hold, though no header reads as Normal. A word found in what is here of the text holds, as
    /// ever, and a size that can't be told never matches.
    pub fn holds_in(&self, m: &Message, how: &Matching<'_>) -> bool {
        self.matches_in(m, how) && !(matches!(self, Condition::Sensitivity(_)) && self.unread_in(m))
    }

    /// Whether it can't be told of `m` yet: it reads what isn't here, of a message only part of
    /// which is ([`Message::partly_here`]).
    pub fn unread_in(&self, m: &Message) -> bool {
        match self {
            // A server's list says whether there are attachments; the headers of a message too
            // large to download don't.
            Condition::HasAttachment => m.partly_here() && !m.unfetched,
            Condition::SizeBetween { .. } => !m.size_known(),
            Condition::HeaderContains(_) | Condition::Sensitivity(_) | Condition::AutomaticReply => m.unfetched || m.incomplete,
            // A calendar part kept on disk and not brought in (one too large to be read) doesn't
            // say whether it asks.
            Condition::MeetingRequest => m.partly_here() || m.attachments.iter().any(|a| a.is_calendar() && !a.is_loaded()),
            other => other.reads_body() && m.partly_here(),
        }
    }
}

/// Exceptions as a server keeps them: any one of them holding leaves a message alone, so two that
/// look for words (or people, or categories) of one kind are one that looks for either's, and one
/// the same as another is that other. More than [`MAX_WORDS_EACH`] words of a kind are an error:
/// cut short, they would let through mail they were to leave alone.
pub fn fold_exceptions(list: Vec<Condition>) -> Result<Vec<Condition>, ModelError> {
    let mut out: Vec<Condition> = Vec::new();
    for c in list {
        if out.contains(&c) {
            continue;
        }
        let kind = std::mem::discriminant(&c);
        match (c.words(), out.iter_mut().find(|d| std::mem::discriminant(*d) == kind && d.words().is_some())) {
            (Some(words), Some(had)) => {
                let mut all = had.words().unwrap_or_default().to_vec();
                for w in words.iter().filter(|w| !all.iter().any(|a| a.eq_ignore_ascii_case(w))).collect::<Vec<_>>() {
                    if all.len() >= MAX_WORDS_EACH {
                        return Err(ModelError::Invalid(format!("exceptions of one kind look for at most {MAX_WORDS_EACH} words or addresses")));
                    }
                    all.push(w.clone());
                }
                *had = had.with_words(all);
            }
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// When a flag a rule sets falls due (Outlook's "flag message for follow up at this time").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FollowUp {
    Today,
    Tomorrow,
    /// This week's Friday, as on the ribbon.
    ThisWeek,
    /// Next week's.
    NextWeek,
}

impl FollowUp {
    pub const ALL: [FollowUp; 4] = [FollowUp::Today, FollowUp::Tomorrow, FollowUp::ThisWeek, FollowUp::NextWeek];

    pub fn parse(s: &str) -> Option<FollowUp> {
        FollowUp::ALL.into_iter().find(|f| f.code().eq_ignore_ascii_case(s.trim()))
    }

    pub fn code(self) -> &'static str {
        match self {
            FollowUp::Today => "today",
            FollowUp::Tomorrow => "tomorrow",
            FollowUp::ThisWeek => "thisWeek",
            FollowUp::NextWeek => "nextWeek",
        }
    }

    /// The day it falls due, from `today`, as the Follow Up menu has it: this week's Friday (today
    /// at the weekend), next week's Friday.
    pub fn day(self, today: Date) -> Date {
        let weekday = i64::from(today.weekday().index());
        match self {
            FollowUp::Today => today,
            FollowUp::Tomorrow => today.add_days(1),
            FollowUp::ThisWeek => today.add_days((4 - weekday).max(0)),
            FollowUp::NextWeek => today.add_days(11 - weekday),
        }
    }

    /// The moment it falls due, by `clock`.
    pub fn due(self, clock: &RuleClock) -> Option<Timestamp> {
        let day = self.day(clock.zone.wall(clock.now).date);
        DateTime::new(day, clock.due_hour.min(23), 0, 0).map(|wall| clock.zone.instant(wall))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Action {
    /// Move to the folder with this id.
    MoveTo(Id),
    Categorize(String),
    Flag,
    MarkRead,
    SetImportance(Importance),
    /// Move to Deleted Items.
    Delete,
    /// Put a copy in the folder with this id.
    CopyTo(Id),
    /// Delete for good, past Deleted Items.
    DeleteForGood,
    /// Flag it, due then.
    FlagFor(FollowUp),
    ClearFlag,
    ClearCategories,
}

impl Action {
    pub fn describe(&self) -> String {
        match self {
            Action::MoveTo(_) => "move it to a folder".into(),
            Action::Categorize(c) => format!("categorize it as \"{c}\""),
            Action::Flag => "flag it".into(),
            Action::MarkRead => "mark it as read".into(),
            Action::SetImportance(i) => format!("set importance to {}", i.code()),
            Action::Delete => "delete it".into(),
            Action::CopyTo(_) => "copy it to a folder".into(),
            Action::DeleteForGood => "delete it permanently".into(),
            Action::FlagFor(f) => match f {
                FollowUp::Today => "flag it for today".into(),
                FollowUp::Tomorrow => "flag it for tomorrow".into(),
                FollowUp::ThisWeek => "flag it for this week".into(),
                FollowUp::NextWeek => "flag it for next week".into(),
            },
            Action::ClearFlag => "clear its flag".into(),
            Action::ClearCategories => "clear its categories".into(),
        }
    }

    /// When it runs among a rule's actions: what changes the message first, then a copy, then
    /// where it goes, and a deletion for good last (nothing is left to act on after it).
    pub fn stage(&self) -> u8 {
        match self {
            Action::Categorize(_)
            | Action::Flag
            | Action::MarkRead
            | Action::SetImportance(_)
            | Action::FlagFor(_)
            | Action::ClearFlag
            | Action::ClearCategories => 0,
            Action::CopyTo(_) => 1,
            Action::MoveTo(_) | Action::Delete => 2,
            Action::DeleteForGood => 3,
        }
    }
}

/// Where the owner wants a rule kept (#566).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Keep {
    /// On this computer: it runs while MailCraft is open.
    #[default]
    Here,
    /// On its account's server, which runs it whether MailCraft is open or not. Until the server
    /// has it ([`Rule::server`]), it runs here.
    Server,
}

impl Keep {
    pub fn is_here(&self) -> bool {
        *self == Keep::Here
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A rule as its account's server has it (#566).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerRule {
    /// What the server calls it.
    pub id: String,
    /// Where it stands among the server's rules (Graph's `sequence`), as it is to be: a move here
    /// changes it before the server is told.
    pub sequence: i32,
    /// Whether it is switched on, and where it stood, as the server last had it. What was
    /// changed here since is the difference ([`Rule::changed_here`]), and only that is sent.
    pub synced_enabled: bool,
    pub synced_sequence: i32,
    /// The parts of it this version doesn't know, by the server's names (`forwardTo`,
    /// `isVoicemail`…). Such a rule is switched on and off, moved and deleted, and never
    /// rewritten or run here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
    /// The server doesn't let it be changed or deleted (Graph's `isReadOnly`).
    #[serde(skip_serializing_if = "is_false")]
    pub read_only: bool,
    /// The server says it is in error (Graph's `hasError`).
    #[serde(skip_serializing_if = "is_false")]
    pub error: bool,
}

/// A rule deleted here that its server is yet to be told about (#566).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RuleGone {
    pub account: Id,
    /// What the server calls it.
    pub id: String,
    /// A rule deleted here while the server was making it, of which no answer came: the server
    /// may have it under an id nobody here knows, and `id` is empty. The next full read of the
    /// server's rules finds the one there that looks the same, which is then deleted there, or
    /// doesn't, and this is let go of. So is it when an answer comes after all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub made: Option<Box<Rule>>,
}

/// Most rules deleted here that wait for their server to be told.
pub const MAX_RULE_GONE: usize = 500;

/// The rules that couldn't tell of a message as it arrived ([`Rule::undecided_in`]), kept with it
/// until it is here whole (#566): a Microsoft account, a JMAP account and an Exchange server
/// account list new mail before it is fetched, and a rule that reads its text, its headers or its
/// size has its say only then ([`Mailbox::run_waiting_rules`](crate::Mailbox::run_waiting_rules)).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RulesWaiting {
    /// The folder the rules left it in: they act on it there and nowhere else, not once it has
    /// been moved (by the owner, say) meanwhile.
    pub folder: Id,
    /// In the order they ran as it arrived.
    #[serde(deserialize_with = "crate::sync::lenient_list")]
    pub rules: Vec<RuleWaiting>,
}

/// One of [`RulesWaiting::rules`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RuleWaiting {
    pub rule: Id,
    /// It acted as the message arrived, but for its delete for good: that waited on a rule
    /// before it that couldn't tell and stops the rules after it.
    #[serde(skip_serializing_if = "is_false")]
    pub delete_only: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Rule {
    pub id: Id,
    pub name: String,
    pub enabled: bool,
    /// All must hold. A rule without conditions matches nothing (it would act on every message).
    pub conditions: Vec<Condition>,
    /// Any one of them holding leaves the message alone (Outlook's "except if").
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// Later rules don't run on a message this one matched.
    pub stop: bool,
    /// The window shows a desktop alert for a message it acted on as it arrived, as for new mail
    /// (Outlook's "display a Desktop Alert"). Only on this computer: a server's rules have none.
    #[serde(skip_serializing_if = "is_false")]
    pub alert: bool,
    /// The account whose arriving mail it is for; 0 for a rule from before rules had accounts,
    /// which is for every account's mail and kept on this computer.
    #[serde(skip_serializing_if = "crate::is_zero_id")]
    pub account: Id,
    /// Where the owner wants it kept.
    #[serde(skip_serializing_if = "Keep::is_here")]
    pub keep: Keep,
    /// The server's copy: for a rule its account's server has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerRule>,
    /// Why the server didn't take it, when it didn't: it is kept here, and said so.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub refused: String,
    /// It went to the server to be made there and no answer came: the server may have it. It is
    /// not sent again until the server's rules have been read whole, which either have it (and
    /// it is taken as this one) or don't (and it is sent again).
    #[serde(skip_serializing_if = "is_false")]
    pub unanswered: bool,
}

impl Rule {
    pub fn matches(&self, m: &Message) -> bool {
        self.matches_in(m, &Matching::default())
    }

    /// [`Rule::matches`], by `how` ([`Matching`]). A rule with parts this version doesn't know
    /// matches nothing here: what it would do is not known.
    pub fn matches_in(&self, m: &Message, how: &Matching<'_>) -> bool {
        // An exception that can't be told yet may hold: a rule doesn't act on a guess.
        self.reaches(m, how) && !self.exceptions.iter().any(|c| c.unread_in(m) || c.matches_in(m, how))
    }

    /// Whether it may apply to `m`, for what can't be told yet: a condition or an exception that
    /// reads what isn't here. It doesn't act; should it stop the rules after it, those delete
    /// nothing for good ([`Mailbox::run_rules`](crate::Mailbox::run_rules)).
    pub fn undecided_in(&self, m: &Message, how: &Matching<'_>) -> bool {
        let open = |c: &Condition| c.unread_in(m) && !c.holds_in(m, how);
        self.enabled
            && self.known()
            && !self.conditions.is_empty()
            && self.conditions.iter().all(|c| open(c) || c.holds_in(m, how))
            && !self.exceptions.iter().any(|c| !c.unread_in(m) && c.matches_in(m, how))
            && (self.conditions.iter().any(open) || self.exceptions.iter().any(|c| c.unread_in(m)))
    }

    /// Whether it is on and its conditions hold, exceptions aside.
    fn reaches(&self, m: &Message, how: &Matching<'_>) -> bool {
        self.enabled && self.known() && !self.conditions.is_empty() && self.conditions.iter().all(|c| c.holds_in(m, how))
    }

    /// `When from contains "x" and has an attachment, except if it is flagged: flag it, mark it
    /// as read`.
    pub fn describe(&self) -> String {
        self.describe_as(false)
    }

    /// [`Rule::describe`] for a rule that reads addresses alone.
    pub fn describe_as(&self, addresses_only: bool) -> String {
        let when = self.conditions.iter().map(|c| c.describe_as(addresses_only)).collect::<Vec<_>>().join(" and ");
        let except = self.exceptions.iter().map(|c| c.describe_as(addresses_only)).collect::<Vec<_>>().join(" or ");
        let mut then: Vec<String> = self.actions.iter().map(Action::describe).collect();
        if self.alert {
            then.push("show a desktop alert".into());
        }
        let then = then.join(", ");
        if except.is_empty() { format!("When {when}: {then}") } else { format!("When {when}, except if {except}: {then}") }
    }

    /// Whether it reads the body of a message, which has to be in memory for it.
    pub fn reads_body(&self) -> bool {
        self.conditions.iter().chain(&self.exceptions).any(Condition::reads_body)
    }

    /// Whether it is for the arriving mail of `account`.
    pub fn is_for(&self, account: Id) -> bool {
        self.account == 0 || self.account == account
    }

    /// Whether the server has it.
    pub fn on_server(&self) -> bool {
        self.server.as_ref().is_some_and(|s| !s.id.is_empty())
    }

    /// Whether it is to go to the server and hasn't yet.
    pub fn waiting(&self) -> bool {
        self.keep == Keep::Server && !self.on_server()
    }

    /// Whether every part of it is known here: it can be shown whole and run here.
    pub fn known(&self) -> bool {
        self.server.as_ref().is_none_or(|s| s.unknown.is_empty())
    }

    /// Whether the server doesn't let it be changed or deleted.
    pub fn read_only(&self) -> bool {
        self.server.as_ref().is_some_and(|s| s.read_only)
    }

    /// Whether it was switched on or off, or moved, here since the server last had it.
    pub fn changed_here(&self) -> bool {
        self.server.as_ref().is_some_and(|s| !s.id.is_empty() && (self.enabled != s.synced_enabled || s.sequence != s.synced_sequence))
    }
}
