// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The changes made here to a Microsoft account's list of categories, told to its server (#661):
//! Graph's `masterCategories`.
//!
//! The server knows a category by its name, which it never changes, and changes nothing of it but
//! its colour: a change says what is to be made, recoloured or deleted, by name. What the server
//! calls each one is read from its list just before, and never leaves this module or reaches a
//! log; nor does a category's name.
//!
//! Reading the list needs `MailboxSettings.Read`, which every sign-in has; changing it needs
//! `MailboxSettings.ReadWrite`: a sign-in that doesn't cover it is answered 403, which is
//! [`CategoryResult::Denied`]. What comes back is hostile JSON like everything else from a server:
//! the list is capped and an entry that is no category is skipped.

use mailcraft_model::Rgb;
use mailcraft_model::outlook_colors::{NO_COLOR, nearest_outlook_color, outlook_preset};
use mailcraft_model::remote::{
    CategoriesUpload, CategoriesUploaded, CategoryChangeKind, CategoryOutcome, CategoryResult, MAX_CATEGORY_CHANGES, MAX_REMOTE_CATEGORIES,
};
use mailcraft_model::sync::same_category;
use serde_json::{Value, json};

use super::tasks::valid_id;
use super::{Doing, Graph, MASTER_CATEGORIES, MAX_WRITTEN, WRITE_TIMEOUT, category_color, text};
use crate::NetError;
use crate::http;

/// Tell the account's server what was done here to its list of categories, one change after the
/// other; what it made of each comes back ([`CategoryOutcome`]).
///
/// - A category the server has already is not made again, and keeps the colour it has there
///   (another device made it): [`CategoryResult::Done`] with that colour, and the answer says it
///   was found there ([`CategoryOutcome::found`]). One it hasn't is not deleted again, and a
///   colour it has already is not sent: [`CategoryResult::Done`]. So is a change of colour of one
///   it deleted meanwhile: the next read takes it away here too.
/// - made (201), changed (200), deleted (204), any other 2xx, a deletion answered 404 or 410, a
///   creation answered 409 (made there meanwhile: Graph documents only that a name is unique in
///   the list, not the code a second one gets, so any other refusal of it is said as refused and
///   the category stays on this computer): [`CategoryResult::Done`].
/// - 403: [`CategoryResult::Denied`]; the rest of the upload is left untried.
/// - 412, 429, 503, 504, any other 5xx, no connection, the list not to be read: nothing more is
///   tried ([`CategoryResult::Failed`] for the change that was on its way), and all of it goes
///   again with the next job.
/// - any other refusal (400, 413…): [`CategoryResult::Refused`], for good.
/// - 401: nothing more is sent, and the upload says the sign-in was refused.
pub fn push_categories(token: &str, upload: &CategoriesUpload) -> CategoriesUploaded {
    push_categories_at(&super::graph_base(), token, upload)
}

/// The name Graph has for one of its colours (`preset0` to `preset24`, `none`): the one a colour
/// is, or else the nearest.
pub(super) fn color_name(color: Rgb) -> String {
    if color == NO_COLOR {
        return "none".into();
    }
    let preset = outlook_preset(color).or_else(|| outlook_preset(nearest_outlook_color(color))).unwrap_or_default();
    format!("preset{preset}")
}

/// A category on the server's list: what it calls it, its name and its colour.
struct There {
    id: String,
    name: String,
    color: Rgb,
}

pub(super) fn push_categories_at(base: &str, token: &str, upload: &CategoriesUpload) -> CategoriesUploaded {
    let mut uploaded = CategoriesUploaded { account: upload.account, email: upload.email.clone(), ..Default::default() };
    let Some(first) = upload.changes.first() else { return uploaded };
    let Ok(graph) = Graph::at(base, token) else {
        uploaded.token_refused = true;
        return uploaded;
    };
    // The list as the server has it now, to know what it calls each category.
    let mut there: Vec<There> = Vec::new();
    // (As far as a read of the list goes, [`MAX_REMOTE_CATEGORIES`]: what lies past it isn't
    // known here, so nothing is sent for it.)
    let read = graph.list(Doing::Categories, graph.url(MASTER_CATEGORIES), &[], MAX_REMOTE_CATEGORIES, &mut |entry: &Value| {
        // (Its name as a read keeps it, which is the name a change here has for it.)
        let (id, name) = (text(entry, "id"), mailcraft_model::remote::category_name(text(entry, "displayName")));
        if valid_id(id) && !name.is_empty() {
            there.push(There { id: id.to_string(), name, color: category_color(text(entry, "color")) });
        }
    });
    let whole = match read {
        Ok(whole) => whole,
        Err(e) if e.is_refused_sign_in() => {
            uploaded.token_refused = true;
            return uploaded;
        }
        Err(NetError::Forbidden(_)) => {
            let denied = CategoryResult::Denied;
            let error = "this account may not change its list of categories".to_string();
            uploaded.outcomes.push(CategoryOutcome {
                kind: first.kind,
                name: first.name.clone(),
                color: first.color,
                result: denied,
                error,
                found: false,
            });
            return uploaded;
        }
        // Busy, or not to be had now: nothing is sent, and all of it goes again after the wait,
        // as when the server is too busy for a change.
        Err(e) => {
            log::debug!("categories: the server's list couldn't be read before a change: {e}");
            uploaded.outcomes.push(CategoryOutcome {
                kind: first.kind,
                name: first.name.clone(),
                color: first.color,
                result: CategoryResult::Failed,
                error: e.to_string(),
                found: false,
            });
            return uploaded;
        }
    };
    for change in upload.changes.iter().take(MAX_CATEGORY_CHANGES) {
        let name = change.name.trim();
        let outcome = |result: CategoryResult, color: Rgb, error: &str| CategoryOutcome {
            kind: change.kind,
            name: change.name.clone(),
            color,
            result,
            error: error.into(),
            found: false,
        };
        let found = there.iter().position(|c| same_category(&c.name, name));
        let one = |i: usize| there.get(i).map(|c| format!("{MASTER_CATEGORIES}/{}", http::encode(&c.id))).unwrap_or_default();
        let colored = json!({"color": color_name(change.color)});
        let (method, path, body) = match (change.kind, found) {
            _ if name.is_empty() => {
                uploaded.outcomes.push(outcome(CategoryResult::Refused, change.color, "a category has a name"));
                continue;
            }
            (CategoryChangeKind::Create, None) => {
                ("POST", MASTER_CATEGORIES.to_string(), json!({"displayName": name, "color": color_name(change.color)}))
            }
            // Made there already (another device, a moment ago): it is the server's own, in the
            // colour it has there, which is not changed (#661). A name is one category there.
            (CategoryChangeKind::Create, Some(i)) => {
                let color = there.get(i).map_or(change.color, |c| c.color);
                uploaded.outcomes.push(CategoryOutcome { found: true, ..outcome(CategoryResult::Done, color, "") });
                continue;
            }
            (CategoryChangeKind::Color, Some(i)) => match there.get(i) {
                Some(c) if c.color == change.color => {
                    uploaded.outcomes.push(outcome(CategoryResult::Done, c.color, ""));
                    continue;
                }
                _ => ("PATCH", one(i), colored),
            },
            (CategoryChangeKind::Delete, Some(i)) => ("DELETE", one(i), Value::Null),
            // Not on the server's list, read whole: deleted there meanwhile, or never made.
            (CategoryChangeKind::Color | CategoryChangeKind::Delete, None) if whole => {
                uploaded.outcomes.push(outcome(CategoryResult::Done, change.color, ""));
                continue;
            }
            (CategoryChangeKind::Color | CategoryChangeKind::Delete, None) => {
                let why = "the server's list of categories is too long to find it in";
                uploaded.outcomes.push(outcome(CategoryResult::Refused, change.color, why));
                continue;
            }
        };
        let body = (!body.is_null()).then(|| body.to_string());
        let headers: &[(&str, &str)] = if body.is_some() { &[("Content-Type", "application/json")] } else { &[] };
        let reply = match graph.ask(&graph.url(&path), method, headers, body.as_deref(), WRITE_TIMEOUT, MAX_WRITTEN) {
            Ok(reply) => reply,
            Err(e) if e.is_refused_sign_in() => {
                uploaded.token_refused = true;
                break;
            }
            Err(e) => {
                uploaded.outcomes.push(outcome(CategoryResult::Failed, change.color, &e.to_string()));
                break;
            }
        };
        let sent = body.as_deref().unwrap_or("");
        let mut done = outcome(CategoryResult::Done, change.color, "");
        let stop = match reply.status {
            401 => {
                uploaded.token_refused = true;
                break;
            }
            200..=299 => {
                let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
                if let Some(color) = json.get("color").and_then(Value::as_str) {
                    done.color = category_color(color);
                }
                match (change.kind, found) {
                    (CategoryChangeKind::Delete, Some(i)) => {
                        there.remove(i);
                    }
                    (_, Some(i)) => {
                        if let Some(c) = there.get_mut(i) {
                            c.color = done.color;
                        }
                    }
                    (_, None) => {
                        let id = text(&json, "id");
                        if valid_id(id) {
                            there.push(There { id: id.to_string(), name: name.to_string(), color: done.color });
                        }
                    }
                }
                false
            }
            // Deleted there already, or meanwhile (a change of colour too: the next read takes
            // it away here). It is on the server's list no more, so that a name made after it in
            // this run is made there, not taken for found (#661).
            404 | 410 if change.kind != CategoryChangeKind::Create => {
                if let Some(i) = found {
                    there.remove(i);
                }
                false
            }
            // Made there meanwhile: the next read brings its colour, and it is the server's own.
            409 if found.is_none() => {
                done.found = change.kind == CategoryChangeKind::Create;
                false
            }
            403 => {
                done.result = CategoryResult::Denied;
                done.error = "this account may not change its list of categories".into();
                true
            }
            404 | 409 | 410 | 412 | 429 | 503 | 504 => {
                done.result = CategoryResult::Failed;
                done.error = "Microsoft Graph is busy or the category changed there meanwhile".into();
                true
            }
            status if status >= 500 => {
                done.result = CategoryResult::Failed;
                done.error = format!("Microsoft Graph failed (status {status})");
                true
            }
            status => {
                done.result = CategoryResult::Refused;
                done.error = match super::refusal(Doing::Categories, &reply, &[token, sent], mailcraft_time::now()) {
                    NetError::Proto(mailcraft_proto::ProtoError::Rejected(why)) => why.to_string(),
                    e => e.to_string(),
                };
                if done.error.is_empty() {
                    done.error = format!("Microsoft Graph refused it (status {status})");
                }
                false
            }
        };
        uploaded.outcomes.push(done);
        if stop {
            break;
        }
    }
    uploaded.tidy();
    log::debug!("categories: {} changes told, sign-in refused: {}", uploaded.outcomes.len(), uploaded.token_refused);
    uploaded
}
