// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What waits in the Outbox, sent through the server (RFC 8621 §7, with the upload of RFC 8620 §6.1).
//!
//! **What is asked**, once the changes have gone up:
//!
//! 1. Nothing, when the session does not offer `urn:ietf:params:jmap:submission`: every message is held,
//!    with a line that says so.
//! 2. `Identity/get`, once: the identity whose address is the account's (letters compared without regard
//!    to case), else one for the whole of its domain (`*@domain`). With neither, every message is held.
//! 3. `Mailbox/get` for the roles alone, once: the copy of what is sent is filed in the mailbox whose role
//!    is `sent`, or in `drafts` when the server has none. With neither, every message is held.
//! 4. For each message, in the order of the request, [`MAX_SENDS`] at the most: its octets are uploaded
//!    (`POST` to the session's upload address, `Content-Type: message/rfc822`), and then **one** request
//!    imports the blob as an email (`Email/import`, with the keywords `$seen` and `$draft`) and submits it
//!    (`EmailSubmission/set`: `emailId` names the import by its creation id, `identityId`, and an `envelope`
//!    whose `mailFrom` is the account's address and whose `rcptTo` are the message's To, Cc and Bcc, each
//!    once), with `onSuccessUpdateEmail` taking `$draft` off again. (A server that takes one call to a
//!    request is asked in two: the import, then the submission.)
//!
//! **The message** is written by the program's own writer, without a `Bcc` field: who is named there is in
//! the envelope only. Every recipient is checked before anything is asked (an address with a space, a line
//! break, a control character or angle brackets in it is none, and the message is held), and so are the
//! number of them and the size against the server's `maxSizeUpload`. A message that was never dated is
//! dated now, as it is for the other ways of sending.
//!
//! **What is reported**, a piece for each message as soon as it is known ([`SendDone`]):
//!
//! - [`SendOutcome::Sent`]: the server made the submission.
//! - [`SendOutcome::Held`] with why: the server refused it (the upload, the import or the submission), or it
//!   could not be asked at all (no submission, no identity, no mailbox, an address that is none, too large).
//!   Not sent again by itself.
//! - [`SendOutcome::Held`] with `lost`: the submission was asked and its answer never came, or says nothing
//!   of the submission. The server may have sent it. **Never twice:** a submission is asked once in a run
//!   and never again, and after a request got no answer the rest of the Outbox waits for the next run.
//! - Nothing: the message was not taken (the server busy, the sign-in refused, the run stopped, the server
//!   not reached before the submission was asked). It waits, and goes with the next run.
//!
//! When the server refuses a submission after it imported the email, the copy is destroyed again (once,
//! and whatever comes of that is passed over).

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::atomic::Ordering;

use mailcraft_mime::WriteOptions;
use mailcraft_model::Message;
use mailcraft_model::sync::{JmapMailResult, MAX_LISTED_FOLDERS, MAX_SENDS, SendDone, SendOutcome, SendUp};
use serde_json::{Map, Value, json};

use super::{LIST_BYTES, Outcome, Run, Stop, arguments, mail_request, refusal, said};
use crate::connection::unusable;
use crate::{ApiRequest, BuildError, CAPABILITY_SUBMISSION, CallFailure, Error, Failure, HttpExchange, Id, Redactor, TransportError};

/// Most recipients one message is sent to.
pub(super) const MAX_RECIPIENTS: usize = 1_000;
/// Most identities of an answer that are looked at.
const MAX_IDENTITIES: usize = 1_000;
/// The longest address (RFC 5321 §4.5.3.1.3 allows a path of 256 octets, its brackets counted), and the
/// longest part before the `@` (§4.5.3.1.1).
const MAX_ADDRESS: usize = 254;
const MAX_LOCAL: usize = 64;
/// The most an answer may be, in bytes: the identities, and the import with the submission.
const IDENTITY_BYTES: usize = 1 << 20;
const SUBMIT_BYTES: usize = 1 << 20;
/// The creation ids of the email imported and of its submission, within their request.
const EMAIL: &str = "m";
const SUBMISSION: &str = "s";

/// Why a message is held, where the reason is this program's own.
pub(super) const NO_SUBMISSION: &str = "this server does not send mail for the account (it offers no JMAP submission)";
pub(super) const NO_IDENTITY: &str = "the server has no sending identity for this account's address";
pub(super) const NO_MAILBOX: &str = "the server has no Sent or Drafts mailbox to file the message in";
pub(super) const BAD_SENDER: &str = "this account's address isn't one mail can be sent from";
pub(super) const NOTHING: &str = "the message has nothing to send";
pub(super) const NO_RECIPIENTS: &str = "the message has no recipients";
pub(super) const BAD_RECIPIENT: &str = "a recipient's address isn't one mail can be sent to";
pub(super) const TOO_MANY: &str = "the message has more recipients than can be sent to at once";
pub(super) const TOO_LARGE: &str = "the message is larger than the server takes";

/// Whether `email` is an address a message can be sent to or from, as it goes into an envelope: one `@`
/// with something on both sides, no longer than a server takes, and no space, control character, angle
/// bracket or other character that ends an address or begins something else.
pub(super) fn is_address(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else { return false };
    let plain = |c: char| !c.is_control() && !c.is_whitespace() && !"<>()[]\\,;:\"@".contains(c);
    email.len() <= MAX_ADDRESS
        && !local.is_empty()
        && local.len() <= MAX_LOCAL
        && !domain.is_empty()
        && local.chars().all(plain)
        && domain.chars().all(plain)
}

/// Who `message` goes to, for the envelope: every To, Cc and Bcc address, once (the first of each, the
/// domain compared without regard to case), in that order. Or why it cannot be sent.
pub(super) fn recipients(message: &Message) -> Result<Vec<String>, &'static str> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for address in message.recipients() {
        let email = address.email.trim_matches([' ', '\t']);
        if email.is_empty() {
            continue;
        }
        if !is_address(email) {
            return Err(BAD_RECIPIENT);
        }
        let key = match email.split_once('@') {
            Some((local, domain)) => format!("{local}@{}", domain.to_lowercase()),
            None => email.to_string(),
        };
        if seen.insert(key) {
            if out.len() >= MAX_RECIPIENTS {
                return Err(TOO_MANY);
            }
            out.push(email.to_string());
        }
    }
    if out.is_empty() { Err(NO_RECIPIENTS) } else { Ok(out) }
}

/// The octets `message` goes as: without its Bcc field, and dated now when it never was.
pub(super) fn written(message: &Message) -> Vec<u8> {
    let message = if message.date == 0 { Cow::Owned(Message { date: mailcraft_time::now(), ..message.clone() }) } else { Cow::Borrowed(message) };
    let options = WriteOptions { include_bcc: false, offset_minutes: mailcraft_time::local_offset_minutes(message.date) };
    mailcraft_mime::write(&message, &options)
}

/// The identity to send as `email` with, of those an answer to `Identity/get` lists: its id.
pub(super) fn identity_of(answer: &Value, email: &str) -> Option<String> {
    let list = answer.get("list").and_then(Value::as_array)?;
    let domain = email.rsplit_once('@').map(|(_, domain)| domain)?;
    let mut whole_domain = None;
    for identity in list.iter().take(MAX_IDENTITIES) {
        let (Some(id), Some(address)) = (identity.get("id").and_then(Value::as_str), identity.get("email").and_then(Value::as_str)) else { continue };
        if Id::new(id).is_err() {
            continue;
        }
        if address.eq_ignore_ascii_case(email) {
            return Some(id.to_string());
        }
        if whole_domain.is_none() && address.strip_prefix("*@").is_some_and(|of| of.eq_ignore_ascii_case(domain)) {
            whole_domain = Some(id.to_string());
        }
    }
    whole_domain
}

/// The mailbox a sent message is filed in, of those an answer to `Mailbox/get` lists: the first whose role
/// is `sent`, else the first whose role is `drafts`.
pub(super) fn filing_of(answer: &Value) -> Option<String> {
    let list = answer.get("list").and_then(Value::as_array)?;
    let with_role = |role: &str| {
        list.iter().take(MAX_LISTED_FOLDERS).find_map(|mailbox| {
            let id = mailbox.get("id").and_then(Value::as_str).filter(|id| Id::new(id).is_ok())?;
            mailbox.get("role").and_then(Value::as_str).filter(|has| has.eq_ignore_ascii_case(role)).map(|_| id.to_string())
        })
    };
    with_role("sent").or_else(|| with_role("drafts"))
}

/// The arguments of the `Email/import` that makes an email of the blob `blob` in `mailbox`.
pub(super) fn import_arguments(account: &str, blob: &str, mailbox: &str) -> Value {
    let mut mailboxes = Map::new();
    mailboxes.insert(mailbox.to_string(), Value::Bool(true));
    json!({
        "accountId": account,
        "emails": {EMAIL: {"blobId": blob, "mailboxIds": mailboxes, "keywords": {"$seen": true, "$draft": true}}},
    })
}

/// The arguments of the `EmailSubmission/set` that sends the email `email` (an id, or `#` and the creation
/// id of its import in the same request) as `identity`, from `from` to `to`.
pub(super) fn submission_arguments(account: &str, email: &str, identity: &str, from: &str, to: &[String]) -> Value {
    let to: Vec<Value> = to.iter().map(|email| json!({"email": email, "parameters": null})).collect();
    let mut on_success = Map::new();
    on_success.insert(format!("#{SUBMISSION}"), json!({"keywords/$draft": null}));
    json!({
        "accountId": account,
        "create": {SUBMISSION: {
            "emailId": email,
            "identityId": identity,
            "envelope": {"mailFrom": {"email": from, "parameters": null}, "rcptTo": to},
        }},
        "onSuccessUpdateEmail": on_success,
    })
}

/// What the answers to the import and the submission of one message come to.
#[derive(Debug, PartialEq)]
pub(super) struct Settled {
    /// What is reported of the message: nothing, when it waits for the next run.
    pub(super) outcome: Option<SendOutcome>,
    /// The email the import made, when the submission was refused: it is destroyed again.
    pub(super) orphan: Option<String>,
    /// The server said to try later (`serverUnavailable`): the rest of the Outbox waits too.
    pub(super) later: Option<CallFailure>,
}

fn held(why: impl Into<String>) -> SendOutcome {
    SendOutcome::Held { lost: false, why: why.into() }
}

fn lost() -> SendOutcome {
    SendOutcome::Held { lost: true, why: String::new() }
}

/// Why the server would not make the submission, in plain words for the errors the standard names
/// (RFC 8621 §7.5), else in the server's own, cleaned.
fn submission_refusal(secrets: &Redactor, error: &Value) -> String {
    let plain = match error.get("type").and_then(Value::as_str).unwrap_or_default() {
        "forbiddenFrom" => "the server does not let this account send with that From address",
        "forbiddenMailFrom" => "the server does not let this account send from its address",
        "forbiddenToSend" => "the server does not let this account send mail",
        "tooManyRecipients" => "the message has more recipients than the server takes",
        "noRecipients" => NO_RECIPIENTS,
        "invalidRecipients" => "the server says a recipient's address is not valid",
        "tooLarge" => TOO_LARGE,
        "overQuota" => "the account is over its quota on the server",
        "invalidEmail" => "the server says the message is not one it can send",
        _ => return refusal(secrets, error),
    };
    plain.to_string()
}

/// What the answers to the import (`imported`) and the submission (`submitted`) of one message say became
/// of it. Whatever they hold, this reads them without trusting them: the message is sent only when the
/// answer has the submission among those created; it is held with a reason only when the answer says the
/// submission, or the import it needs, was not made; and anything else is an answer that does not say, which
/// is a message that may have gone.
pub(super) fn settled(
    secrets: &Redactor,
    imported: Result<Map<String, Value>, CallFailure>,
    submitted: Result<Map<String, Value>, CallFailure>,
) -> Settled {
    let of = |answer: &Map<String, Value>, list: &str, creation: &str| {
        answer.get(list).and_then(Value::as_object).and_then(|all| all.get(creation)).cloned()
    };
    let made = imported.as_ref().ok().and_then(|answer| of(answer, "created", EMAIL));
    let orphan = made.and_then(|email| email.get("id").and_then(Value::as_str).filter(|id| Id::new(id).is_ok()).map(str::to_string));
    let is = |outcome: SendOutcome, orphan: Option<String>| Settled { outcome: Some(outcome), orphan, later: None };
    if let Ok(answer) = &submitted
        && of(answer, "created", SUBMISSION).is_some_and(|made| made.is_object())
    {
        return is(SendOutcome::Sent, None);
    }
    // Not made, as far as the answer says. The import it needed says why, when that is what failed.
    match &imported {
        Ok(answer) => {
            if let Some(error) = of(answer, "notCreated", EMAIL) {
                return is(held(format!("the server would not take the message: {}", refusal(secrets, &error))), None);
            }
        }
        Err(CallFailure::Method(error)) if error.kind().may_succeed_later() => {
            return Settled { outcome: None, orphan: None, later: Some(CallFailure::Method(error.clone())) };
        }
        Err(CallFailure::Method(error)) => return is(held(format!("the server would not take the message: {}", said(secrets, error))), None),
        Err(_) => {}
    }
    if let Ok(answer) = &submitted
        && let Some(error) = of(answer, "notCreated", SUBMISSION)
    {
        return is(held(submission_refusal(secrets, &error)), orphan);
    }
    match submitted {
        // The email is there and the server refused to send it, as a whole call.
        Err(CallFailure::Method(error)) if orphan.is_some() && error.kind().may_succeed_later() => {
            Settled { outcome: None, orphan, later: Some(CallFailure::Method(error)) }
        }
        Err(CallFailure::Method(error)) if orphan.is_some() => is(held(said(secrets, &error)), orphan),
        _ => is(lost(), None),
    }
}

/// What a message comes to, and whether the sending goes on after it.
type Fate = (Option<SendOutcome>, Option<Stop>);

impl<H: HttpExchange> Run<'_, H> {
    /// Send what the request says waits in the Outbox, a piece for each message as soon as it is known what
    /// became of it. An error is what ends the run (stopped, the sign-in refused, the server busy); a step
    /// that could not be asked otherwise is a line, and the run goes on to read.
    pub(super) async fn sends(&self, out: &mut Outcome, emit: &mut dyn FnMut(JmapMailResult)) -> Result<(), Stop> {
        let sends: Vec<&SendUp> = self.request.sends.iter().take(MAX_SENDS).collect();
        if sends.is_empty() {
            return Ok(());
        }
        let mut report = |message: mailcraft_model::Id, outcome: SendOutcome| {
            let done = SendDone { message, outcome, draft: None, draft_left: String::new() };
            emit(self.piece(JmapMailResult { sent: vec![done], partial: true, ..Default::default() }));
        };
        let mut hold_all = |why: &str| sends.iter().for_each(|send| report(send.message, held(why)));
        if !self.connection.client().has_capability(CAPABILITY_SUBMISSION) {
            log::info!("jmap mail: the server offers no submission; what waits in the Outbox is held");
            hold_all(NO_SUBMISSION);
            return Ok(());
        }
        if !is_address(&self.request.email) {
            hold_all(BAD_SENDER);
            return Ok(());
        }
        // This step's trouble is a line, and the Outbox waits; anything else ends the run.
        let mut told = |stop: Stop| match stop {
            Stop::Failed(error) | Stop::Down(error) => {
                log::info!("jmap mail: the Outbox could not be sent now; it waits for the next run");
                out.sends.get_or_insert_with(|| self.said(&error));
                Ok(())
            }
            stop => Err(stop),
        };
        self.check()?;
        let identity =
            match self.submission_call("Identity/get", json!({"accountId": self.account(), "properties": ["id", "email"]}), IDENTITY_BYTES).await {
                Ok(answer) => identity_of(&answer, &self.request.email),
                Err(stop) => return told(stop),
            };
        let Some(identity) = identity else {
            hold_all(NO_IDENTITY);
            return Ok(());
        };
        self.check()?;
        let mailbox = match self.one_call("Mailbox/get", json!({"accountId": self.account(), "properties": ["id", "role"]}), LIST_BYTES).await {
            Ok(answer) => filing_of(&answer),
            Err(stop) => return told(stop),
        };
        let Some(mailbox) = mailbox else {
            hold_all(NO_MAILBOX);
            return Ok(());
        };
        let mut sent = 0usize;
        let mut end = Ok(());
        for send in &sends {
            if let Err(stop) = self.check() {
                end = Err(stop);
                break;
            }
            let (outcome, stop) = self.send(send, &identity, &mailbox).await;
            if let Some(outcome) = outcome {
                sent = sent.saturating_add(usize::from(outcome == SendOutcome::Sent));
                report(send.message, outcome);
            }
            if let Some(stop) = stop {
                end = told(stop);
                break;
            }
        }
        log::info!("jmap mail: {sent} of {} messages sent", sends.len());
        end
    }

    /// A request of one call that needs the capability of sending, and its answer.
    async fn submission_call(&self, method: &str, args: Value, most: usize) -> Result<Value, Stop> {
        let mut request = submission_request(most)?;
        let call = request.add_call(method, arguments(args))?;
        let mut answers = self.post(&request).await?;
        Self::take(&mut answers, &call, method)
    }

    /// One message: checked, uploaded, imported and submitted.
    async fn send(&self, send: &SendUp, identity: &str, mailbox: &str) -> Fate {
        let Some(message) = send.mime.as_deref() else { return (Some(held(NOTHING)), None) };
        let to = match recipients(message) {
            Ok(to) => to,
            Err(why) => return (Some(held(why)), None),
        };
        let octets = written(message);
        let limit = self.connection.client().limits().max_size_upload();
        if u64::try_from(octets.len()).unwrap_or(u64::MAX) > limit {
            return (Some(held(TOO_LARGE)), None);
        }
        let blob = match self.connection.upload("message/rfc822", octets, &self.secrets).await {
            Ok(blob) => blob,
            Err(error) => return self.not_uploaded(error),
        };
        // Nothing is sent yet: a run stopped here leaves a blob the server forgets.
        if let Err(stop) = self.check() {
            return (None, Some(stop));
        }
        let (outcome, stop, orphan) = self.submit(blob.as_str(), identity, mailbox, &to).await;
        if let Some(orphan) = orphan
            && self.check().is_ok()
        {
            // The copy of a message the server would not send. Whatever comes of this is passed over: the
            // message is reported as it is either way.
            let args = json!({"accountId": self.account(), "destroy": [orphan]});
            if self.one_call("Email/set", args, SUBMIT_BYTES).await.is_err() {
                log::debug!("jmap mail: the copy of a message the server refused to send could not be removed");
            }
        }
        (outcome, stop)
    }

    /// What an upload that failed means: nothing was sent, whatever it was.
    fn not_uploaded(&self, error: Error) -> Fate {
        match error {
            Error::Build(BuildError::TooLarge { .. }) | Error::TooLarge { .. } => (Some(held(TOO_LARGE)), None),
            Error::Refused { .. } | Error::NotFound | Error::Http { status: 400..=499, .. } => {
                (Some(held(format!("the server would not take the message: {}", self.said(&error)))), None)
            }
            Error::Build(_) | Error::Template(_) | Error::Address(_) => (None, Some(Stop::Failed(error.into()))),
            other => (None, Some(self.call_stop(other))),
        }
    }

    /// Import the blob and submit the email, in one request where the server takes two calls: what became
    /// of the message, why the sending ends here (if it does), and an email to destroy again.
    async fn submit(&self, blob: &str, identity: &str, mailbox: &str, to: &[String]) -> (Option<SendOutcome>, Option<Stop>, Option<String>) {
        let account = self.account();
        let from = self.request.email.as_str();
        let import = import_arguments(account, blob, mailbox);
        let failed = |error: BuildError| (None, Some(Stop::from(error)), None);
        let (imported, submitted) = if self.two_calls {
            let built = submission_request(SUBMIT_BYTES).and_then(|mut request| {
                let import = request.add_call("Email/import", arguments(import))?;
                let submission = submission_arguments(account, &format!("#{EMAIL}"), identity, from, to);
                let submit = request.add_call("EmailSubmission/set", arguments(submission))?;
                Ok((request, import, submit))
            });
            let (request, import, submit) = match built {
                Ok(built) => built,
                Err(error) => return failed(error),
            };
            let mut answers = match self.connection.call(&request, &self.secrets).await {
                Ok(answers) => answers,
                Err(error) => {
                    let (outcome, stop) = self.after_asking(error);
                    return (outcome, stop, None);
                }
            };
            (answers.take_named(&import, "Email/import"), answers.take_named(&submit, "EmailSubmission/set"))
        } else {
            // A server that takes one call to a request: the import first, which sends nothing.
            let built = submission_request(SUBMIT_BYTES).and_then(|mut request| {
                let call = request.add_call("Email/import", arguments(import))?;
                Ok((request, call))
            });
            let (request, call) = match built {
                Ok(built) => built,
                Err(error) => return failed(error),
            };
            let imported = match self.post(&request).await {
                Ok(mut answers) => answers.take_named(&call, "Email/import"),
                Err(stop) => return (None, Some(stop), None),
            };
            let made =
                imported.as_ref().ok().and_then(|answer| answer.get("created")?.get(EMAIL)?.get("id")?.as_str().filter(|id| Id::new(id).is_ok()));
            let Some(email) = made.map(str::to_string) else {
                // Nothing was asked to go: the import says why, or the server answered nothing of use.
                return match settled(&self.secrets, imported, Err(CallFailure::Missing)) {
                    Settled { outcome: Some(SendOutcome::Held { lost: false, why }), .. } => (Some(held(why)), None, None),
                    Settled { later: Some(later), .. } => (None, Some(Stop::Down(later.into())), None),
                    _ => (None, Some(Stop::Failed(unusable("the answer does not say what became of the message").into())), None),
                };
            };
            if let Err(stop) = self.check() {
                return (None, Some(stop), None);
            }
            let built = submission_request(SUBMIT_BYTES).and_then(|mut request| {
                let submission = submission_arguments(account, &email, identity, from, to);
                let call = request.add_call("EmailSubmission/set", arguments(submission))?;
                Ok((request, call))
            });
            let (request, call) = match built {
                Ok(built) => built,
                Err(error) => return failed(error),
            };
            let mut answers = match self.connection.call(&request, &self.secrets).await {
                Ok(answers) => answers,
                Err(error) => {
                    let (outcome, stop) = self.after_asking(error);
                    return (outcome, stop, None);
                }
            };
            (imported, answers.take_named(&call, "EmailSubmission/set"))
        };
        let Settled { outcome, orphan, later } = settled(&self.secrets, imported, submitted);
        (outcome, later.map(|later| Stop::Down(later.into())), orphan)
    }

    /// What it means that the request with the submission in it failed as a whole. One that never left, or
    /// that the server turned away before reading it (the sign-in, a busy server, a request it would not
    /// read), sent nothing. Anything else may have reached the server: the message is held as one that may
    /// have gone, and nothing more is sent in this run.
    fn after_asking(&self, error: Error) -> Fate {
        let gone = |error: Error| (Some(lost()), Some(Stop::Down(Failure::from(error))));
        match error {
            Error::Build(_) => (None, Some(Stop::Failed(error.into()))),
            Error::CredentialsRefused | Error::Forbidden { .. } => (None, Some(Stop::Auth)),
            Error::Busy { .. } => (None, Some(self.call_stop(error))),
            Error::Transport(TransportError::Unreachable | TransportError::Tls | TransportError::RedirectRefused)
            | Error::Redirected { .. }
            | Error::RedirectRefused(_) => (None, Some(Stop::Down(error.into()))),
            Error::TooLarge { .. } | Error::Refused { .. } | Error::NotFound => {
                (Some(held(format!("the server would not take the request to send it: {}", self.said(&error)))), None)
            }
            // The owner stopped the run while the request was on its way.
            Error::Transport(TransportError::Aborted) if self.cancel.load(Ordering::Relaxed) => {
                log::info!("jmap mail: a send was called off on its way; the message is held, not sent again by itself");
                (Some(lost()), Some(Stop::Cancelled))
            }
            other => {
                log::info!("jmap mail: the answer to a send was lost; the message is held, not sent again by itself");
                gone(other)
            }
        }
    }
}

/// A request of calls that send mail, whose answer may be `most` bytes at the most: the capabilities of
/// mail and of submission in `using`, beside the core's (RFC 8621 §1.3).
fn submission_request(most: usize) -> Result<ApiRequest, BuildError> {
    let mut request = mail_request(most)?;
    request.using(CAPABILITY_SUBMISSION)?;
    Ok(request)
}
