// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What sends mail on the stand-in: the upload of a blob, `Email/import`, `Identity/get` and
//! `EmailSubmission/set` (RFC 8620 §6.1, RFC 8621 §4.8, §6 and §7). (`Email/set` is in `changing.rs`.)
//!
//! - **The upload** (`POST /jmap/upload/{accountId}/`) keeps the octets as a blob and answers `201` with its
//!   id; more than `maxSizeUpload` is a `413`.
//! - **`Email/import`** makes an email of a blob that was uploaded (or that an email has), in the mailboxes
//!   and with the keywords named. A blob or a mailbox it does not have is `invalidProperties`. With
//!   [`Data::no_duplicates`](super::Data::no_duplicates), an email it has already is `alreadyExists` with
//!   the `existingId`, and octets uploaded again keep their blob's id (unless
//!   [`Data::blob_per_upload`](super::Data::blob_per_upload) gives every upload an id of its own).
//! - **`Identity/get`** lists the identities the test gave ([`Data::identity`](super::Data::identity); one
//!   for [`USER`](super::USER) is there from the start).
//! - **`EmailSubmission/set`** makes submissions and nothing else. `emailId` may name an email made earlier
//!   in the request by `#` and its creation id. The envelope is the one given, or made of the message's
//!   own fields. Who it is from must be the identity's address, or of the domain a `*@domain` identity is
//!   for (`forbiddenMailFrom`, and `forbiddenFrom` for the message's own From); it needs a recipient
//!   (`noRecipients`), each an address (`invalidRecipients`), and no more than
//!   [`Data::max_recipients`](super::Data::max_recipients) (`tooManyRecipients`). Nothing leaves the process:
//!   what was submitted is kept for the test to look at ([`Data::submitted`](super::Data::submitted)).
//!   `onSuccessUpdateEmail` and `onSuccessDestroyEmail` are done for each submission made, and answered with
//!   an `Email/set` response of the same call, as the standard has it.
//! - **Trouble on demand.** [`Data::refuse_submission`](super::Data::refuse_submission) makes every
//!   submission fail with an error, and [`Data::refuse_next_upload`](super::Data::refuse_next_upload) answers
//!   the next uploads with a status.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::changing::{id_list, is_keyword, object, or_null, set_error, truths};
use super::{ACCOUNT, Data, Email, Outcome, account, allow_only, header, ids, invalid, problem, properties, refuse};

/// Most blobs kept from uploads (the oldest goes first), and most recipients read of an envelope.
const MAX_UPLOADS: usize = 1_000;
const MAX_ENVELOPE: usize = 10_000;
/// Most submissions kept for a test to look at.
const MAX_SUBMITTED: usize = 10_000;

const IDENTITY_PROPERTIES: &[&str] = &["id", "name", "email", "replyTo", "bcc", "textSignature", "htmlSignature", "mayDelete"];

/// An identity the account may send as (RFC 8621 §6): an address, or `*@domain` for every address of a domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub id: String,
    pub name: String,
    pub email: String,
}

/// A message the stand-in was asked to send. It went nowhere.
#[derive(Clone, PartialEq, Eq)]
pub struct Submission {
    pub id: String,
    /// The id of the email submitted, and of the identity it was sent as.
    pub email: String,
    pub identity: String,
    /// The envelope: who it is from, and who it goes to, in the order given.
    pub mail_from: String,
    pub rcpt_to: Vec<String>,
    /// The message, as it was when it was submitted.
    pub raw: Vec<u8>,
}

/// What may be shown of a submission: neither the message nor an address.
impl std::fmt::Debug for Submission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Submission")
            .field("id", &self.id)
            .field("email", &self.email)
            .field("identity", &self.identity)
            .field("recipients", &self.rcpt_to.len())
            .field("raw_bytes", &self.raw.len())
            .finish()
    }
}

/// Whether an identity for `identity` (an address, or `*@domain`) may send as `email`.
fn sends_as(identity: &str, email: &str) -> bool {
    if identity.eq_ignore_ascii_case(email) {
        return true;
    }
    let domain = email.rsplit_once('@').map(|(_, domain)| domain);
    identity.strip_prefix("*@").is_some_and(|of| domain.is_some_and(|domain| of.eq_ignore_ascii_case(domain)))
}

impl Data {
    /// One more identity the account may send as (`ada@example.org`, or `*@example.org`): its id.
    pub fn identity(&mut self, email: &str) -> String {
        let id = self.id("I");
        self.identities.push(Identity { id: id.clone(), name: String::new(), email: email.to_string() });
        self.changed();
        id
    }

    /// `POST /jmap/upload/{accountId}/` (RFC 8620 §6.1): the octets become a blob. The status and the body.
    pub(super) fn upload(&mut self, fields: &[(&str, &str)], path: &str, body: &[u8]) -> (u16, Value) {
        if !self.authorized(fields) {
            return (401, problem(401, "about:blank", "the credentials were refused"));
        }
        if path.strip_prefix(super::UPLOAD_PATH) != Some(&format!("{ACCOUNT}/")) {
            return (404, problem(404, "about:blank", "no such account to upload to"));
        }
        if let Some((status, retry_after)) = self.refuse_next_upload.pop_front() {
            self.retry_after = retry_after;
            return (status, problem(status, "about:blank", "the server can't take this upload just now"));
        }
        if body.len() > self.max_upload {
            return (413, problem(413, "about:blank", "the upload is larger than maxSizeUpload"));
        }
        // The oldest goes first: by when it came, which the order of the ids (`B10` before `B9`) is not.
        while self.uploads.len() >= MAX_UPLOADS {
            let Some(oldest) = self.upload_order.pop_front() else { break };
            self.uploads.remove(&oldest);
        }
        // Octets it has already keep the id they have, on a server that names content by one id.
        let had = || {
            let uploaded = self.uploads.iter().find(|(_, octets)| octets.as_slice() == body).map(|(id, _)| id.clone());
            uploaded.or_else(|| self.emails.iter().find(|email| email.raw == body).map(|email| email.blob.clone()))
        };
        let blob = match (self.no_duplicates && !self.blob_per_upload).then(had).flatten() {
            Some(blob) => blob,
            None => {
                let blob = self.id("B");
                self.uploads.insert(blob.clone(), body.to_vec());
                self.upload_order.push_back(blob.clone());
                blob
            }
        };
        let media_type = header(fields, "content-type").unwrap_or("application/octet-stream");
        (201, json!({"accountId": ACCOUNT, "blobId": blob, "type": media_type, "size": body.len()}))
    }

    /// `Email/import` (RFC 8621 §4.8): an email of each blob named.
    pub(super) fn email_import(&mut self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "ifInState", "emails"])?;
        account(args)?;
        let old = self.state();
        self.in_state(args)?;
        let Some(Value::Object(emails)) = args.get("emails") else {
            return Err(invalid("emails is an object of the emails to make"));
        };
        if emails.len() > self.max_set {
            return Err(refuse("requestTooLarge", format!("{} emails, and {} are taken at a time", emails.len(), self.max_set)));
        }
        let (mut created, mut not_created) = (Map::new(), Map::new());
        for (creation, email) in emails {
            match self.import(email) {
                Ok(made) => {
                    self.created.insert(creation.clone(), made.id.clone());
                    created.insert(
                        creation.clone(),
                        json!({"id": made.id, "blobId": made.blob, "threadId": super::thread_of(&made), "size": made.raw.len()}),
                    );
                    self.emails.push(made);
                    self.changed();
                }
                Err(error) => {
                    not_created.insert(creation.clone(), error);
                }
            }
        }
        Ok(json!({"accountId": ACCOUNT, "oldState": old, "newState": self.state(), "created": or_null(created), "notCreated": or_null(not_created)}))
    }

    /// The email one entry of an import makes, or the error it is refused with.
    fn import(&mut self, email: &Value) -> Result<Email, Value> {
        let wrong = |property: &str| json!({"type": "invalidProperties", "description": "what an import needs is missing or wrong", "properties": [property]});
        let entry = email.as_object().ok_or_else(|| wrong("blobId"))?;
        if let Some(other) = entry.keys().find(|key| !["blobId", "mailboxIds", "keywords", "receivedAt"].contains(&key.as_str())) {
            return Err(wrong(&other.chars().take(40).collect::<String>()));
        }
        let blob = entry.get("blobId").and_then(Value::as_str).ok_or_else(|| wrong("blobId"))?;
        let raw =
            self.uploads.get(blob).or_else(|| self.emails.iter().find(|e| e.blob == blob).map(|e| &e.raw)).cloned().ok_or_else(|| wrong("blobId"))?;
        let mailboxes = entry.get("mailboxIds").and_then(truths).ok_or_else(|| wrong("mailboxIds"))?;
        if mailboxes.is_empty() || mailboxes.iter().any(|id| !self.mailboxes.iter().any(|m| &m.id == id)) {
            return Err(wrong("mailboxIds"));
        }
        if self.profile.one_mailbox && mailboxes.len() > 1 {
            return Err(set_error("tooManyMailboxes", "an email lies in one mailbox on this server"));
        }
        let keywords = match entry.get("keywords") {
            None | Some(Value::Null) => BTreeSet::new(),
            Some(value) => truths(value).filter(|all| all.iter().all(|k| is_keyword(k))).ok_or_else(|| wrong("keywords"))?,
        };
        if self.no_duplicates
            && let Some(has) = self.emails.iter().find(|email| email.raw == raw)
        {
            return Err(json!({"type": "alreadyExists", "description": "the account has this email already", "existingId": has.id}));
        }
        let received = match entry.get("receivedAt") {
            None | Some(Value::Null) => self.now.clone(),
            Some(Value::String(at)) if mailcraft_time::iso_to_timestamp(at, 0).is_some() => at.clone(),
            Some(_) => return Err(wrong("receivedAt")),
        };
        Ok(Email { id: self.id("E"), blob: blob.to_string(), mailboxes, keywords, received, raw })
    }

    /// `Identity/get` (RFC 8621 §6.1).
    pub(super) fn identity_get(&self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "ids", "properties"])?;
        account(args)?;
        let wanted = ids(args, self.max_get)?;
        let properties = properties(args, IDENTITY_PROPERTIES, IDENTITY_PROPERTIES)?;
        let shown = |identity: &Identity| {
            let mut out = Map::new();
            out.insert("id".into(), json!(identity.id));
            for property in &properties {
                let value = match property.as_str() {
                    "name" => json!(identity.name),
                    "email" => json!(identity.email),
                    "replyTo" | "bcc" => Value::Null,
                    "textSignature" | "htmlSignature" => json!(""),
                    "mayDelete" => json!(false),
                    _ => continue,
                };
                out.insert(property.clone(), value);
            }
            Value::Object(out)
        };
        let (mut list, mut not_found) = (Vec::new(), Vec::new());
        match wanted {
            None => list.extend(self.identities.iter().map(shown)),
            Some(wanted) => {
                for id in wanted {
                    match self.identities.iter().find(|identity| identity.id == id) {
                        Some(identity) => list.push(shown(identity)),
                        None => not_found.push(id),
                    }
                }
            }
        }
        Ok(json!({"accountId": ACCOUNT, "state": self.state(), "list": list, "notFound": not_found}))
    }

    /// `EmailSubmission/set` (RFC 8621 §7.5): submissions are made, and what is to become of their emails
    /// is done.
    pub(super) fn submission_set(&mut self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "ifInState", "create", "update", "destroy", "onSuccessUpdateEmail", "onSuccessDestroyEmail"])?;
        account(args)?;
        let old = self.state();
        self.in_state(args)?;
        let (Some(create), Some(update), Some(on_update)) = (object(args, "create"), object(args, "update"), object(args, "onSuccessUpdateEmail"))
        else {
            return Err(invalid("create, update and onSuccessUpdateEmail are objects, or null"));
        };
        let destroy = id_list(args, "destroy")?;
        if update.is_some_and(|update| !update.is_empty()) || !destroy.is_empty() {
            return Err(invalid("a submission is made here, and neither changed nor undone"));
        }
        let on_destroy: Vec<&str> = match args.get("onSuccessDestroyEmail") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(keys)) => {
                keys.iter().map(Value::as_str).collect::<Option<Vec<&str>>>().ok_or_else(|| invalid("onSuccessDestroyEmail is a list of ids"))?
            }
            Some(_) => return Err(invalid("onSuccessDestroyEmail is a list of ids, or null")),
        };
        if create.map_or(0, Map::len) > self.max_set {
            return Err(refuse("requestTooLarge", "more submissions than are taken at a time"));
        }
        let (mut created, mut not_created) = (Map::new(), Map::new());
        // The submissions made: by `#` and their creation id, and by their own id, the email each sent.
        let mut made: BTreeMap<String, String> = BTreeMap::new();
        for (creation, submission) in create.into_iter().flatten() {
            match self.submit(submission) {
                Ok(done) => {
                    created.insert(creation.clone(), json!({"id": done.id, "undoStatus": "final", "sendAt": self.now}));
                    made.insert(format!("#{creation}"), done.email.clone());
                    made.insert(done.id.clone(), done.email.clone());
                    if self.submitted.len() < MAX_SUBMITTED {
                        self.submitted.push(done);
                    }
                    self.changed();
                }
                Err(error) => {
                    not_created.insert(creation.clone(), error);
                }
            }
        }
        // What becomes of the emails of the submissions made, answered as an `Email/set` of the same call.
        let before = self.state();
        let (mut updated, mut not_updated, mut destroyed) = (Map::new(), Map::new(), Vec::new());
        for (key, patch) in on_update.into_iter().flatten() {
            let Some(email) = made.get(key).cloned() else { continue };
            match self.update_email(&email, patch) {
                Ok(changed) => updated.insert(email, changed),
                Err(error) => not_updated.insert(email, error),
            };
        }
        for key in on_destroy {
            if let Some(email) = made.get(key).cloned()
                && self.remove(&email)
            {
                destroyed.push(email);
            }
        }
        if !made.is_empty() && (on_update.is_some() || args.get("onSuccessDestroyEmail").is_some_and(|keys| !keys.is_null())) {
            let answer = json!({
                "accountId": ACCOUNT,
                "oldState": before,
                "newState": self.state(),
                "created": null,
                "updated": or_null(updated),
                "destroyed": if destroyed.is_empty() { Value::Null } else { json!(destroyed) },
                "notCreated": null,
                "notUpdated": or_null(not_updated),
                "notDestroyed": null,
            });
            self.extra.push(("Email/set".to_string(), answer));
        }
        Ok(json!({
            "accountId": ACCOUNT,
            "oldState": old,
            "newState": self.state(),
            "created": or_null(created),
            "updated": null,
            "destroyed": null,
            "notCreated": or_null(not_created),
            "notUpdated": null,
            "notDestroyed": null,
        }))
    }

    /// The submission one entry of a `create` makes, or the error it is refused with.
    fn submit(&mut self, submission: &Value) -> Result<Submission, Value> {
        let wrong = |property: &str| json!({"type": "invalidProperties", "description": "what a submission needs is missing or wrong", "properties": [property]});
        if let Some((kind, description)) = &self.refuse_submission {
            return Err(set_error(kind, description));
        }
        let entry = submission.as_object().ok_or_else(|| wrong("emailId"))?;
        if let Some(other) = entry.keys().find(|key| !["identityId", "emailId", "envelope"].contains(&key.as_str())) {
            return Err(wrong(&other.chars().take(40).collect::<String>()));
        }
        let identity = entry.get("identityId").and_then(Value::as_str).and_then(|id| self.identities.iter().find(|identity| identity.id == id));
        let identity = identity.cloned().ok_or_else(|| wrong("identityId"))?;
        // An email made earlier in the request is named by `#` and its creation id (RFC 8620 §5.3).
        let named = entry.get("emailId").and_then(Value::as_str).ok_or_else(|| wrong("emailId"))?;
        let email = match named.strip_prefix('#') {
            Some(creation) => self.created.get(creation).map(String::as_str),
            None => Some(named),
        };
        let email = email.and_then(|id| self.email(id)).ok_or_else(|| wrong("emailId"))?;
        let message = mailcraft_mime_model::parse(&email.raw);
        let (mail_from, rcpt_to) = match entry.get("envelope") {
            // The server's own, from the message's fields (RFC 8621 §7).
            None | Some(Value::Null) => {
                let from = if identity.email.starts_with("*@") { message.from.email.clone() } else { identity.email.clone() };
                (from, message.recipients().map(|address| address.email.clone()).take(MAX_ENVELOPE).collect::<Vec<String>>())
            }
            Some(envelope) => {
                let address = |value: &Value| {
                    let address = value.as_object().filter(|a| a.keys().all(|key| key == "email" || key == "parameters"))?;
                    address.get("email").and_then(Value::as_str).map(str::to_string)
                };
                let from = envelope.get("mailFrom").and_then(address).ok_or_else(|| wrong("envelope"))?;
                let to = envelope.get("rcptTo").and_then(Value::as_array).ok_or_else(|| wrong("envelope"))?;
                (from, to.iter().take(MAX_ENVELOPE).map(address).collect::<Option<Vec<String>>>().ok_or_else(|| wrong("envelope"))?)
            }
        };
        if !sends_as(&identity.email, &mail_from) {
            return Err(set_error("forbiddenMailFrom", "the envelope is from an address this identity may not send as"));
        }
        if !message.from.email.is_empty() && !sends_as(&identity.email, &message.from.email) {
            return Err(set_error("forbiddenFrom", "the message is from an address this identity may not send as"));
        }
        if rcpt_to.is_empty() {
            return Err(set_error("noRecipients", "the envelope names nobody to send to"));
        }
        if rcpt_to.len() > self.max_recipients {
            return Err(json!({"type": "tooManyRecipients", "description": "more recipients than are taken", "maxRecipients": self.max_recipients}));
        }
        let bad: Vec<&String> = rcpt_to
            .iter()
            .filter(|to| !to.contains('@') || to.chars().any(|c| c.is_control() || c.is_whitespace() || c == '<' || c == '>'))
            .collect();
        if !bad.is_empty() {
            return Err(json!({"type": "invalidRecipients", "description": "a recipient is no address", "invalidRecipients": bad}));
        }
        let (email, raw) = (email.id.clone(), email.raw.clone());
        Ok(Submission { id: self.id("S"), email, identity: identity.id, mail_from, rcpt_to, raw })
    }
}
