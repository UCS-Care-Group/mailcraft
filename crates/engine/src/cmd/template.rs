// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reusable local message templates with plain-text field filling (#952).

use mailcraft_i18n::date::{self, Day};
use mailcraft_model::{Address, Contact, MAX_TEMPLATES, Template, TemplateFields};
use serde_json::{Value, json};

use super::{bad, done, fail, needs_compose};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("template.create", "Save as Template", "Message › Include › Templates", create).params(
            r#"{"name"?: string, "subject"?: string, "body"?: string} — saves the message being written (or the given subject and body) as a local template"#,
        ),
        CommandSpec::new("template.list", "List Templates", "Message › Include › Templates", list)
            .reads()
            .params(r#"{} — answers {"templates": [{"id", "name", "subject", "body", "created"}]}"#),
        CommandSpec::new("template.remove", "Remove Template", "Message › Include › Templates", remove)
            .params(r#"{"id": number} — removes the template by its unique id"#),
        CommandSpec::new("template.insert", "Insert Template", "Message › Include › Templates", insert)
            .pure()
            .params(
                r#"{"id"?: number, "name"?: string, "replaceSubject"?: bool} — inserts the template at the caret with {recipient.name}, {recipient.company} and {date} filled in as plain text"#,
            )
            .when_writing(needs_compose),
    ]
}

fn opt_str<'a>(v: &'a Value, k: &str) -> Result<Option<&'a str>, CmdError> {
    match v.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(bad(format!("`{k}` is text"))),
    }
}

fn compose_body_plain(c: &crate::session::Compose) -> String {
    if let Some((before, _sig, _after)) = c.body.text().around_signature() {
        let plain = before.map(|r| r.to_plain()).unwrap_or_default();
        if plain.trim().is_empty() {
            return String::new();
        }
        return plain.strip_suffix('\n').unwrap_or(&plain).to_string();
    }
    c.body.plain()
}

fn create(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["name", "subject", "body"])?;
    let name = opt_str(v, "name")?.unwrap_or_default();
    let given_subject = opt_str(v, "subject")?;
    let given_body = opt_str(v, "body")?;

    let (subject, body) = match s.compose.as_ref() {
        Some(c) => (
            given_subject.map(str::to_string).unwrap_or_else(|| c.subject.clone()),
            given_body.map(str::to_string).unwrap_or_else(|| compose_body_plain(c)),
        ),
        None => {
            if given_subject.is_none() && given_body.is_none() {
                return Err(fail("no message is being written"));
            }
            (given_subject.unwrap_or_default().to_string(), given_body.unwrap_or_default().to_string())
        }
    };

    if s.mailbox.templates().len() >= MAX_TEMPLATES {
        return Err(fail("too many templates"));
    }

    let now = s.now();
    let id = s.mailbox.alloc_id();
    let template = Template::new(id, name, &subject, &body, now);
    if template.name.is_empty() && template.subject.is_empty() && template.body.is_empty() {
        return Err(fail("give the template a name, subject or body"));
    }
    if !s.mailbox.add_template(template.clone()) {
        return Err(fail("too many templates"));
    }
    Ok(json!({
        "id": template.id,
        "name": template.name,
        "subject": template.subject,
        "body": template.body,
        "created": template.created,
    }))
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &[])?;
    let templates: Vec<Value> = s
        .mailbox
        .templates_sorted()
        .into_iter()
        .map(|t| {
            json!({
                "id": t.id,
                "name": t.name,
                "subject": t.subject,
                "body": t.body,
                "created": t.created,
            })
        })
        .collect();
    Ok(json!({ "templates": templates }))
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id"])?;
    let id = p::req_id(v, "id")?;
    if !s.mailbox.remove_template(id) {
        return Err(failed!("no template with id {id}", id));
    }
    done()
}

fn insert(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "name", "replaceSubject"])?;
    let replace_subject = p::opt_bool(v, "replaceSubject")?.unwrap_or(false);

    let template = if v.get("id").is_some() {
        let id = p::req_id(v, "id")?;
        s.mailbox.template(id).cloned().ok_or_else(|| failed!("no template with id {id}", id))?
    } else if let Some(name) = opt_str(v, "name")? {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(bad("`id` (number) or `name` (string) is required"));
        }
        s.mailbox.template_by_name(trimmed).cloned().ok_or_else(|| failed!("no template named \"{name}\"", name = trimmed))?
    } else {
        return Err(bad("`id` (number) or `name` (string) is required"));
    };

    let to = s.compose.as_ref().ok_or_else(|| fail("no message is being written"))?.to.clone();
    let fields = resolve_fields(s, &to);
    let (expanded_subject, expanded_body) = template.expand(&fields);

    let c = s.compose.as_mut().ok_or_else(|| fail("no message is being written"))?;
    c.keep_out_of_signature();
    let before_body = c.body.text().clone();
    let before_subject = c.subject.clone();
    if !expanded_body.is_empty() {
        c.body.type_text(&expanded_body).map_err(|e| fail(e.to_string()))?;
    }
    if !expanded_subject.is_empty() && (c.subject.trim().is_empty() || replace_subject) {
        c.subject = expanded_subject.clone();
    }
    if *c.body.text() != before_body || c.subject != before_subject {
        c.dirty = true;
    }
    let sel = c.body.selection();
    Ok(json!({
        "id": template.id,
        "name": template.name,
        "subject": expanded_subject,
        "body": expanded_body,
        "from": sel.anchor,
        "to": sel.head,
        "length": c.body.text().len(),
    }))
}

fn resolve_fields(s: &Session, to: &str) -> TemplateFields {
    let (recipient_name, recipient_company) = recipient_fields(s, to);
    let d = s.today();
    let day = Day { year: d.year, month: d.month, day: d.day, weekday: d.weekday().index() };
    let date = date::long_without_weekday(&s.dated(), day);
    TemplateFields { recipient_name, recipient_company, date }
}

fn contact_person_name(k: &Contact) -> String {
    if !k.display_name.trim().is_empty() {
        return k.display_name.trim().to_string();
    }
    let full = [k.first.trim(), k.middle.trim(), k.last.trim()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ");
    if !full.is_empty() {
        return full;
    }
    if !k.company.trim().is_empty() {
        return k.company.trim().to_string();
    }
    String::new()
}

fn recipient_fields(s: &Session, to: &str) -> (String, String) {
    let Some(first) = Address::parse_list(to).into_iter().next() else {
        return (String::new(), String::new());
    };
    let contact =
        if !first.email.is_empty() && first.email.contains('@') { s.mailbox.contacts.values().find(|k| k.has_email(&first.email)) } else { None };
    let name = if !first.name.is_empty() {
        first.name
    } else if let Some(k) = contact {
        contact_person_name(k)
    } else {
        String::new()
    };
    let company = contact.map(|k| k.company.trim().to_string()).unwrap_or_default();
    (name, company)
}
