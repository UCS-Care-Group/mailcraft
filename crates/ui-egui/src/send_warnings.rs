// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Send warnings** in the window (#509): before the owner's message goes, a question when a
//! recipient looks like somebody they write to but isn't, or when people outside their
//! organisation would see each other's addresses (`mailcraft_engine::send_warnings`). Send Anyway
//! goes ahead (`"confirmed": true`); Move to Bcc, offered for the outsiders, moves them first; Cancel,
//! which Escape answers (Enter never sends), leaves the message as it is being written. An agent's send is
//! asked about in the agent's question ([`crate::agent_gate`]), and an agent can't answer either
//! for the owner.

use mailcraft_engine::send_warnings::{CONFIRMED, Lines, MOVE_TO_BCC, said};
use mailcraft_model::send_warnings::{SendWarning, has_outsiders};
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::Question;

/// Whether the question offers Move to Bcc: people outside the organisation would see each other.
pub(crate) fn offers_bcc(warnings: &[SendWarning]) -> bool {
    has_outsiders(warnings)
}

/// Each warning as a line the owner reads.
pub(crate) fn lines(warnings: &[SendWarning]) -> Vec<String> {
    warnings
        .iter()
        .map(|w| match w {
            SendWarning::LookalikeRecipient { address, like } => crate::i18n::fmt(
                tl!("{address} looks like {like}, which you write to, but isn't the same address."),
                &[("address", address.as_str()), ("like", like.as_str())],
            ),
            SendWarning::Outsiders { count, domains, .. } => crate::i18n::fmt(
                &tln!(
                    *count,
                    "{n} person outside your organisation, at {domains} domains, would see the others' addresses in To and Cc.",
                    "{n} people outside your organisation, at {domains} domains, would see each other's addresses in To and Cc."
                ),
                &[("domains", &domains.to_string())],
            ),
            SendWarning::TooManyRecipients { checked } => tln!(
                *checked,
                "This message has more recipients than MailCraft checks: only the first one was looked at.",
                "This message has more recipients than MailCraft checks: only the first {n} were looked at."
            ),
        })
        .collect()
}

/// The question before the owner's `compose.send` with `params` goes, when it has a warning and
/// the owner hasn't answered it yet.
pub(crate) fn question(app: &mut MailApp, params: &Value) -> Option<Question> {
    if said(params, CONFIRMED) {
        return None;
    }
    let warnings = app.session.send_warnings(params);
    if warnings.is_empty() {
        return None;
    }
    let text =
        std::iter::once(tl!("Check the recipients before this message goes:").to_string()).chain(lines(&warnings)).collect::<Vec<_>>().join("\n\n");
    let ask = Question::new(tl!("Check Recipients"), text, tl!("Send Anyway"), Value::Null);
    // The owner saw every reason in this one question: moved, it goes.
    Some(if offers_bcc(&warnings) && !said(params, MOVE_TO_BCC) {
        ask.or(tl!("Move to Bcc"), json!({"moveToBcc": true, "confirmed": true}))
    } else {
        ask
    })
}

/// The parameter that holds the recipient lines the owner's question was asked about, and the
/// warnings it showed ([`asked_about`]). The window adds it and takes it out again; nothing else
/// reads it.
pub(crate) const ASKED_ABOUT: &str = "recipientsAskedAbout";

/// What the owner's question about the message `params` name is asked about, as
/// [`ASKED_ABOUT`] holds it: its recipient lines ([`snapshot`]) and its warnings.
pub(crate) fn asked_about(app: &mut MailApp, params: &Value) -> Value {
    let warnings: Vec<Value> = app.session.send_warnings(params).iter().map(SendWarning::json).collect();
    json!({"lines": snapshot(app, params), "warnings": warnings})
}

/// The recipient lines of the message `params` name, as a question remembers them.
pub(crate) fn snapshot(app: &MailApp, params: &Value) -> Value {
    Lines::named(&app.session, params).map_or(Value::Null, |l| json!([l.to, l.cc, l.bcc]))
}

/// The owner's `compose.send` `params` as the window runs them: what its question was asked about
/// taken out, and its yes with it when the message has changed since (an agent may have changed it
/// while the question was up) or has a warning the question didn't show (the message it answers
/// was deleted meanwhile, say), so that it is asked again about what it is now.
pub(crate) fn settle(app: &mut MailApp, mut params: Value) -> Value {
    let Some(asked) = params.as_object_mut().and_then(|map| map.remove(ASKED_ABOUT)) else { return params };
    let shown = asked.get("warnings").and_then(Value::as_array).cloned().unwrap_or_default();
    let changed = asked.get("lines").is_none_or(|lines| snapshot(app, &params) != *lines)
        || app.session.send_warnings(&params).iter().any(|w| !shown.contains(&w.json()));
    if changed && let Some(map) = params.as_object_mut() {
        map.remove(CONFIRMED);
    }
    params
}

/// An agent's `params` for `id` without the owner's answers to a send warning, which an agent
/// can't give (`confirmed`, `moveToBcc`), and the warnings of what it would send.
pub(crate) fn for_agent(app: &mut MailApp, id: &str, mut params: Value) -> (Value, Vec<SendWarning>) {
    if id != "compose.send" {
        return (params, Vec::new());
    }
    if let Some(fields) = params.as_object_mut() {
        fields.remove(CONFIRMED);
        fields.remove(MOVE_TO_BCC);
    }
    let warnings = app.session.send_warnings(&params);
    (params, warnings)
}

/// An agent's `compose.send` the owner said yes to, asked about with `asked` (its warnings and
/// [`snapshot`]): with the owner's answer in it (Move to Bcc when `bcc`), or why it can't go on
/// that yes (it would now send to others, or with warnings, than the owner was asked about).
pub(crate) fn answered_for_agent(app: &mut MailApp, (asked, lines): (&[SendWarning], &Value), mut params: Value, bcc: bool) -> Result<Value, String> {
    if snapshot(app, &params) != *lines {
        return Err("what `compose.send` would send has changed since the owner was asked: nothing was sent".into());
    }
    let Some(fields) = params.as_object_mut() else { return Ok(params) };
    if bcc {
        fields.insert(MOVE_TO_BCC.into(), json!(true));
    }
    let now = app.session.send_warnings(&params);
    if now.iter().any(|w| !asked.contains(w)) {
        return Err("what `compose.send` would send has changed since the owner was asked: nothing was sent".into());
    }
    if !now.is_empty()
        && let Some(fields) = params.as_object_mut()
    {
        fields.insert(CONFIRMED.into(), json!(true));
    }
    Ok(params)
}
