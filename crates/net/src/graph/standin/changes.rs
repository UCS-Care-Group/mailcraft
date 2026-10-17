//! The stand-in's changes of messages, as Graph v1.0 documents them ("Update message",
//! "message: move", "message: copy", "Delete message", "Get singleValueLegacyExtendedProperty",
//! "Create single-value extended property", "Combine multiple requests in one HTTP call using
//! JSON batching"):
//!
//! - `GET`, `PATCH` and `DELETE /v1.0/me/messages/{id}` (and the same under
//!   `/me/mailFolders/{folder}/messages/{id}`). A change may say `isRead`, `flag` (a
//!   `followupFlag`: `flagStatus` `notFlagged`, `flagged` or `complete`, and `startDateTime`,
//!   `dueDateTime` and `completedDateTime`, each a `dateTimeTimeZone`; a due date only with a
//!   start, as the reference has it, and a start only with a due date, as Microsoft 365 has it: 400 otherwise), `categories`, `importance` (`low`, `normal`, `high`) and
//!   `singleValueExtendedProperties` (each `{id, value}`, added or replaced by its id), and
//!   nothing else (400); it is made whole or not at all, answered 200 with the message, and
//!   shows in the folder's next round of delta. Marking a flag complete without a completed date
//!   gives it the stand-in's time; marking it not flagged takes its dates away. A delete is
//!   answered 204: the message is gone from every folder (Graph keeps it among the recoverable
//!   items, which no folder lists). Removing a message past them is not something the stand-in
//!   does: that address is answered as one that names nothing.
//! - `POST /v1.0/me/messages/{id}/move` and `/copy` with `{"destinationId"}` (a folder's id or a
//!   well-known name) answer 201 with the message where it is now. A move keeps the message's
//!   immutable id: it is `@removed` from the source's delta and comes in the destination's. A
//!   copy has an id of its own and everything the message has, its extended properties too.
//! - `GET /v1.0/me/mailFolders/{id}/messages` lists a folder's messages, newest first, with
//!   `$select`, `$top`, `$skip`, `$expand=singleValueExtendedProperties($filter=id eq '…')` and
//!   `$filter=singleValueExtendedProperties/any(ep: ep/id eq '…' and ep/value eq '…')`: how a
//!   client finds the copy its marker was carried to.
//! - **Faults on demand**, per message ([`Mail::faults`](super::Mail::faults)): a change of it
//!   is answered 404, 403 or 400, told to come back later (429 with `Retry-After`, inside a
//!   batch among that request's own headers), or carried out with the answer lost: the
//!   connection is closed without a word, and a batch that held it is lost whole (for its next
//!   copy alone, [`Fault::CopyAnswerLost`]: the marker goes on first, as usual). A whole batch
//!   is told to come back later with [`State::fail_next`] for `POST /v1.0/$batch`.
//!
//! `$batch` itself (up to 20 requests, each answered by itself and matched by its `id`, more
//! refused with 400) is [`State::batch`].

use serde_json::{Map, Value, json};

use super::{Answer, Asked, Refusal, State, bad, instant_in, not_found, refuse, wall_of};

/// What a change of one message is answered with instead of being made ([`super::Mail::faults`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// 404 `ErrorItemNotFound`, every time: the server says it no longer exists.
    Gone,
    /// 403 `ErrorAccessDenied`, every time.
    Denied,
    /// 400 `ErrorInvalidRequest`, every time.
    Invalid,
    /// 429 for this many changes to come, then made as usual.
    Throttled(usize),
    /// The next change is made, and its answer lost: the connection closes before a word of it.
    AnswerLost,
    /// [`Fault::AnswerLost`] for the next copy of it alone; any other change is made and
    /// answered as usual.
    CopyAnswerLost,
}

/// The kinds of single-value extended property the reference lists, which an id begins with.
const PROPERTY_TYPES: &[&str] = &[
    "Binary",
    "Boolean",
    "Currency",
    "CLSID",
    "Double",
    "Float",
    "Integer",
    "Long",
    "Object",
    "Short",
    "String",
    "SystemTime",
    "ApplicationTime",
    "Null",
];
/// Longest id or value of an extended property, and most categories, the stand-in takes.
const MAX_PROPERTY: usize = 1_024;
const MAX_CATEGORIES: usize = 100;

/// What a change of a message says, checked before any of it is made.
#[derive(Default)]
struct Change {
    read: Option<bool>,
    flag: Option<FlagChange>,
    categories: Option<Vec<String>>,
    importance: Option<String>,
    extended: Vec<(String, String)>,
    /// A draft's own parts: its subject, body and recipients.
    written: super::drafts::Written,
}

/// What a change says of a flag: its status, and each date it names (`Some(None)` takes it away).
#[derive(Default)]
struct FlagChange {
    status: Option<String>,
    start: Option<Option<i64>>,
    due: Option<Option<i64>>,
    completed: Option<Option<i64>>,
}

/// How a message is to be written in an answer.
pub(super) struct Look {
    immutable: bool,
    select: Option<Vec<String>>,
    /// The extended property asked for with `$expand`, by its id.
    expand: Option<String>,
}

/// `text` without `prefix`, matched without regard to ASCII case.
fn strip_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| text.get(prefix.len()..)).flatten()
}

/// An OData string literal at the start of `text` (`'it''s'`), and what follows it.
fn quoted(text: &str) -> Option<(String, &str)> {
    let rest = text.strip_prefix('\'')?;
    let (mut out, mut chars) = (String::new(), rest.char_indices().peekable());
    while let Some((at, c)) = chars.next() {
        if c != '\'' {
            out.push(c);
        } else if chars.peek().is_some_and(|(_, next)| *next == '\'') {
            chars.next();
            out.push('\'');
        } else {
            return Some((out, rest.get(at + 1..)?));
        }
    }
    None
}

/// `singleValueExtendedProperties/any(ep: ep/id eq '…' and ep/value eq '…')`: the id and the value.
fn property_filter(text: &str) -> Option<(String, String)> {
    let rest = strip_ci(text.trim(), "singleValueExtendedProperties/any(")?.strip_suffix(')')?;
    let (name, condition) = rest.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let rest = strip_ci(condition.trim_start(), &format!("{name}/id eq "))?;
    let (id, rest) = quoted(rest.trim_start())?;
    let rest = strip_ci(rest.trim_start(), "and ")?;
    let rest = strip_ci(rest.trim_start(), &format!("{name}/value eq "))?;
    let (value, rest) = quoted(rest.trim_start())?;
    rest.trim().is_empty().then_some((id, value))
}

/// `singleValueExtendedProperties($filter=id eq '…')`: the id.
fn property_expansion(text: &str) -> Option<String> {
    let rest = strip_ci(text.trim(), "singleValueExtendedProperties($filter=")?.strip_suffix(')')?;
    let rest = strip_ci(rest.trim_start(), "id eq ")?;
    let (id, rest) = quoted(rest.trim_start())?;
    rest.trim().is_empty().then_some(id)
}

/// Whether `id` is written as an extended property's id is: its kind, then a property tag
/// (`0x0E08`) or a property set's GUID and a name or a number.
fn property_id_ok(id: &str) -> bool {
    let Some((kind, rest)) = id.split_once(' ') else { return false };
    let hex = |text: &str| strip_ci(text, "0x").is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_hexdigit()));
    let guid = |text: &str| {
        text.strip_prefix('{').and_then(|t| t.strip_suffix('}')).is_some_and(|inner| {
            inner.len() == 36 && inner.char_indices().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() })
        })
    };
    let named = || {
        let mut words = rest.splitn(3, ' ');
        match (words.next(), words.next(), words.next()) {
            (Some(set), Some(how), Some(what)) => {
                guid(set) && ((how.eq_ignore_ascii_case("Name") && !what.trim().is_empty()) || (how.eq_ignore_ascii_case("Id") && hex(what)))
            }
            _ => false,
        }
    };
    id.len() <= MAX_PROPERTY && PROPERTY_TYPES.iter().any(|known| known.eq_ignore_ascii_case(kind)) && (hex(rest) || named())
}

/// Two ids of one extended property: the same, whatever their case.
fn same_property(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The instant a `dateTimeTimeZone` names (`null` names none).
fn date_of(value: &Value, name: &str) -> Result<Option<i64>, Refusal> {
    if value.is_null() {
        return Ok(None);
    }
    let wrong = || bad(format!("'{name}' is a dateTimeTimeZone: a dateTime and a timeZone the service knows."));
    let map = value.as_object().ok_or_else(wrong)?;
    if map.keys().any(|key| !matches!(key.as_str(), "dateTime" | "timeZone" | "@odata.type")) {
        return Err(wrong());
    }
    let wall = map.get("dateTime").and_then(Value::as_str).and_then(wall_of).ok_or_else(wrong)?;
    let zone = map.get("timeZone").and_then(Value::as_str).ok_or_else(wrong)?;
    instant_in(wall, zone).map(Some).ok_or_else(wrong)
}

impl Change {
    /// What `body` says, or why it isn't a change the service takes.
    fn of(body: &Value) -> Result<Change, Refusal> {
        let map = body.as_object().ok_or_else(|| bad("A change of a message is a JSON object."))?;
        let mut change = Change::default();
        for (name, value) in map {
            match name.as_str() {
                "isRead" => change.read = Some(value.as_bool().ok_or_else(|| bad("'isRead' is true or false."))?),
                "importance" => match value.as_str() {
                    Some(level @ ("low" | "normal" | "high")) => change.importance = Some(level.to_string()),
                    _ => return Err(bad("'importance' is low, normal or high.")),
                },
                "categories" => {
                    let list = value.as_array().ok_or_else(|| bad("'categories' is a list of names."))?;
                    if list.len() > MAX_CATEGORIES {
                        return Err(bad("A message has at most 100 categories here."));
                    }
                    let names: Option<Vec<String>> = list.iter().map(|name| name.as_str().map(str::to_string)).collect();
                    change.categories = Some(names.ok_or_else(|| bad("'categories' is a list of names."))?);
                }
                "flag" => change.flag = Some(FlagChange::of(value)?),
                "singleValueExtendedProperties" => change.extended = extended_of(value)?,
                other if change.written.take(other, value)? => {}
                "internetMessageHeaders" => {
                    return Err(bad("A message's internetMessageHeaders are written when it is made, and never after."));
                }
                other if super::messages::PROPERTIES.contains(&other) => {
                    return Err(bad(format!("The property '{other}' is not one this stand-in takes in a change of a message.")));
                }
                other => {
                    return Err(refuse(400, "RequestBodyRead", format!("The property '{other}' does not exist on type 'microsoft.graph.message'.")));
                }
            }
        }
        Ok(change)
    }
}

/// `singleValueExtendedProperties` as a change or a new message writes them: each id and value.
pub(super) fn extended_of(value: &Value) -> Result<Vec<(String, String)>, Refusal> {
    let list = value.as_array().ok_or_else(|| bad("'singleValueExtendedProperties' is a list."))?;
    let mut out = Vec::new();
    for property in list {
        let id = property.get("id").and_then(Value::as_str).filter(|id| property_id_ok(id));
        let value = property.get("value").and_then(Value::as_str).filter(|value| value.len() <= MAX_PROPERTY);
        let (Some(id), Some(value)) = (id, value) else {
            return Err(bad("An extended property has an id as the reference writes them and a value that is text."));
        };
        out.push((id.to_string(), value.to_string()));
    }
    Ok(out)
}

impl FlagChange {
    fn of(value: &Value) -> Result<FlagChange, Refusal> {
        let map: &Map<String, Value> = value.as_object().ok_or_else(|| bad("'flag' is a followupFlag."))?;
        let mut flag = FlagChange::default();
        for (name, value) in map {
            match name.as_str() {
                "flagStatus" => match value.as_str() {
                    Some(status @ ("notFlagged" | "flagged" | "complete")) => flag.status = Some(status.to_string()),
                    _ => return Err(bad("'flagStatus' is notFlagged, flagged or complete.")),
                },
                "startDateTime" => flag.start = Some(date_of(value, name)?),
                "dueDateTime" => flag.due = Some(date_of(value, name)?),
                "completedDateTime" => flag.completed = Some(date_of(value, name)?),
                "@odata.type" => {}
                other => {
                    return Err(refuse(
                        400,
                        "RequestBodyRead",
                        format!("The property '{other}' does not exist on type 'microsoft.graph.followupFlag'."),
                    ));
                }
            }
        }
        // "To set the due date, you must also specify the startDateTime."
        if matches!(flag.due, Some(Some(_))) && !matches!(flag.start, Some(Some(_))) {
            return Err(bad("To set the due date, the start date must be set too."));
        }
        // And, as Microsoft 365 answers it, a start with no due date (`null`, or none) is 400.
        if matches!(flag.start, Some(Some(_))) && !matches!(flag.due, Some(Some(_))) {
            return Err(bad("A start date needs a due date."));
        }
        Ok(flag)
    }
}

impl State {
    /// A message id as an address names it, as the immutable id the stand-in keeps it by.
    pub(super) fn message_id(id: &str) -> Result<String, Refusal> {
        if id.len() < 20 || id.chars().any(|c| !(c.is_ascii_alphanumeric() || "-_=".contains(c))) {
            return Err(refuse(400, "ErrorInvalidIdMalformed", "Id is malformed."));
        }
        Ok(super::messages::immutable_of(id))
    }

    /// The fault a change of `id` is to meet, if any: the refusal, or `Ok` to go on (with the
    /// answer to be lost when that is the fault).
    pub(super) fn fault(&mut self, id: &str) -> Result<(), Refusal> {
        let Some(fault) = self.mailbox.faults.get(id).copied() else { return Ok(()) };
        match fault {
            Fault::Gone => Err(not_found()),
            Fault::Denied => Err(refuse(403, "ErrorAccessDenied", "Access is denied. Check credentials and try again.")),
            Fault::Invalid => Err(refuse(400, "ErrorInvalidRequest", "The request is invalid.")),
            Fault::Throttled(n) => {
                if n <= 1 {
                    self.mailbox.faults.remove(id);
                } else {
                    self.mailbox.faults.insert(id.to_string(), Fault::Throttled(n - 1));
                }
                Err(refuse(429, "ApplicationThrottled", "Application is over its MailboxConcurrency limit."))
            }
            Fault::AnswerLost => {
                self.mailbox.faults.remove(id);
                self.lose_answer = true;
                Ok(())
            }
            Fault::CopyAnswerLost => Ok(()),
        }
    }

    pub(super) fn look_of(asked: &Asked, list: bool) -> Result<Look, Refusal> {
        let allowed: &[&str] = if list { &["$select", "$top", "$skip", "$expand", "$filter"] } else { &["$select", "$expand"] };
        if let Some((other, _)) = asked.query.iter().find(|(name, _)| !allowed.contains(&name.as_str())) {
            return Err(bad(format!("The query parameter '{other}' is not one this stand-in answers here.")));
        }
        let select: Option<Vec<String>> = asked.param("$select").map(|names| names.split(',').map(|n| n.trim().to_string()).collect());
        if let Some(unknown) = select.iter().flatten().find(|name| !super::messages::PROPERTIES.contains(&name.as_str())) {
            return Err(refuse(400, "BadRequest", format!("Could not find a property named '{unknown}' on type 'microsoft.graph.message'.")));
        }
        let expand = match asked.param("$expand") {
            None => None,
            Some(text) => Some(property_expansion(text).ok_or_else(|| bad("The only expansion here is of one extended property, by its id."))?),
        };
        let immutable = asked.header("prefer").to_ascii_lowercase().contains("idtype=\"immutableid\"");
        Ok(Look { immutable, select, expand })
    }

    pub(super) fn message_json(&self, id: &str, look: &Look) -> Value {
        let Some(message) = self.mailbox.messages.get(id) else { return Value::Null };
        let mut json = message.json(look.immutable, look.select.as_deref(), false);
        if let Some(asked) = &look.expand {
            let found: Vec<Value> = message
                .extended
                .iter()
                .filter(|(have, _)| same_property(have, asked))
                .map(|(have, value)| json!({"id": have, "value": value}))
                .collect();
            if !found.is_empty() {
                json["singleValueExtendedProperties"] = json!(found);
            }
        }
        json
    }

    /// `/me/messages/{id}[/move|/copy]`, and the same under a folder.
    pub(super) fn message_at(&mut self, asked: &Asked, method: &str, id: &str, action: Option<&str>) -> Result<Answer, Refusal> {
        let id = State::message_id(id)?;
        match (method, action) {
            ("GET", None) => {
                let look = State::look_of(asked, false)?;
                if !self.mailbox.messages.contains_key(&id) {
                    return Err(not_found());
                }
                Ok((200, Some(self.message_json(&id, &look))))
            }
            ("PATCH", None) => {
                let change = Change::of(&asked.json())?;
                let look = State::look_of(asked, false)?;
                if !self.mailbox.messages.contains_key(&id) {
                    return Err(not_found());
                }
                if change.written.any() && !self.mailbox.messages.get(&id).is_some_and(|m| m.draft) {
                    return Err(bad("Only a draft's subject, body and recipients can be changed."));
                }
                self.fault(&id)?;
                let now = self.clock;
                self.mailbox.update(&id, |m| {
                    change.written.apply(m);
                    if let Some(read) = change.read {
                        m.read = read;
                    }
                    if let Some(importance) = change.importance {
                        m.importance = importance;
                    }
                    if let Some(categories) = change.categories {
                        m.categories = categories;
                    }
                    if let Some(flag) = change.flag {
                        if let Some(start) = flag.start {
                            m.flag_start = start;
                        }
                        if let Some(due) = flag.due {
                            m.flag_due = due;
                        }
                        if let Some(completed) = flag.completed {
                            m.flag_completed = completed;
                        }
                        if let Some(status) = flag.status {
                            match status.as_str() {
                                "notFlagged" => (m.flag_start, m.flag_due, m.flag_completed) = (None, None, None),
                                "flagged" => m.flag_completed = None,
                                _ => m.flag_completed = m.flag_completed.or(Some(now)),
                            }
                            m.flag = status;
                        }
                    }
                    for (id, value) in change.extended {
                        match m.extended.iter_mut().find(|(have, _)| same_property(have, &id)) {
                            Some(property) => property.1 = value,
                            None => m.extended.push((id, value)),
                        }
                    }
                });
                Ok((200, Some(self.message_json(&id, &look))))
            }
            ("DELETE", None) => {
                if !asked.query.is_empty() {
                    return Err(bad("A delete takes no query parameters."));
                }
                if !self.mailbox.messages.contains_key(&id) {
                    return Err(not_found());
                }
                self.fault(&id)?;
                self.mailbox.delete(&id);
                Ok((204, None))
            }
            ("POST", Some(action @ ("move" | "copy"))) => {
                let body = asked.json();
                let map = body.as_object().ok_or_else(|| bad("A move or a copy says where to: {\"destinationId\"}."))?;
                if map.keys().any(|key| key != "destinationId") {
                    return Err(refuse(400, "RequestBodyRead", "A move or a copy says its destinationId and nothing else."));
                }
                let to =
                    map.get("destinationId").and_then(Value::as_str).ok_or_else(|| bad("A move or a copy says where to: {\"destinationId\"}."))?;
                let to = self.mail_folder(to)?.id.clone();
                let look = State::look_of(asked, false)?;
                if !self.mailbox.messages.contains_key(&id) {
                    return Err(not_found());
                }
                self.fault(&id)?;
                if action == "copy" && self.mailbox.faults.get(&id) == Some(&Fault::CopyAnswerLost) {
                    self.mailbox.faults.remove(&id);
                    self.lose_answer = true;
                }
                let answered = if action == "move" {
                    if self.mailbox.messages.get(&id).is_some_and(|m| m.folder != to) {
                        self.mailbox.move_to(&id, &to);
                    }
                    id
                } else {
                    self.mailbox.copy_to(&id, &to).ok_or_else(not_found)?
                };
                Ok((201, Some(self.message_json(&answered, &look))))
            }
            (_, Some(other @ ("move" | "copy"))) => Err(refuse(405, "MethodNotAllowed", format!("'{other}' is asked for with POST."))),
            (_, Some(other)) => Err(refuse(400, "BadRequest", format!("Resource not found for the segment '{other}'."))),
            _ => Err(refuse(405, "MethodNotAllowed", "The method is not allowed on this resource.")),
        }
    }

    /// `GET /me/mailFolders/{id}/messages`: newest first, perhaps only those with an extended
    /// property of a value.
    pub(super) fn folder_messages(&mut self, asked: &Asked, folder: &str) -> Result<Answer, Refusal> {
        let folder = self.mail_folder(folder)?.id.clone();
        let look = State::look_of(asked, true)?;
        let filter = match asked.param("$filter") {
            None => None,
            Some(text) => Some(property_filter(text).ok_or_else(|| {
                bad("The only filter this stand-in answers is singleValueExtendedProperties/any(ep: ep/id eq '…' and ep/value eq '…').")
            })?),
        };
        let mut found: Vec<&super::Message> = self
            .mailbox
            .messages
            .values()
            .filter(|m| m.folder == folder)
            .filter(|m| filter.as_ref().is_none_or(|(id, value)| m.extended.iter().any(|(have, is)| same_property(have, id) && is == value)))
            .collect();
        found.sort_by(|a, b| b.received.cmp(&a.received).then_with(|| a.id.cmp(&b.id)));
        let ids: Vec<String> = found.into_iter().map(|m| m.id.clone()).collect();
        let list = ids.iter().map(|id| self.message_json(id, &look)).collect();
        self.page(asked, list)
    }
}

#[cfg(test)]
mod tests;
