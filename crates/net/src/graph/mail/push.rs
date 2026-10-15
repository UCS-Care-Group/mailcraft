//! What a run over Graph changes on the server: the Outbox sent, folders made, renamed, moved and
//! deleted, drafts put there, and messages marked, categorised, moved and deleted. What went
//! through is said in the run's answer as over IMAP; what didn't is a line in its errors, and is
//! asked for again by the next run.

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use mailcraft_mime::WriteOptions;
use mailcraft_model::sync::{
    CategoriesPushed, Created, Deleted, FlagsPushed, FolderOp, FolderPushed, Moved, RemoteFlags, RemoteOp, SyncRequest, Uploaded, renamed_path,
};
use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use super::super::{is_send_as_denied, send_as};
use super::folders::{INBOX, Node, Tree, join, level};
use super::{Client, categories_in, ends_run, id_of, object};
use crate::NetError;
use crate::http;
use crate::sync::{Run, messages, shown, subject_of};

/// Most folder changes, uploads and changes to messages made in one run; the rest waits.
const MAX_FOLDER_OPS: usize = 1_000;
const MAX_UPLOADS: usize = 200;
const MAX_OPS: usize = 5_000;
/// The largest message put on the server from here.
const MAX_UPLOAD: usize = 25 * 1024 * 1024;
/// Most categories a message is given there.
const MAX_CATEGORIES: usize = 25;

/// What a request that changes something came to.
enum Done {
    Yes,
    /// The message or folder is no longer there.
    Gone,
}

/// Ask Graph to change something: `Ok` with what it answered when it did (any 2xx), or when what
/// was to be changed is gone (404).
fn change(client: &Client<'_>, method: &str, path: &str, body: &Value) -> Result<Done, NetError> {
    let reply = client.send_json(method, path, body)?;
    match reply.status {
        200..=299 => Ok(Done::Yes),
        404 => Ok(Done::Gone),
        _ => Err(client.refusal(&reply, "")),
    }
}

/// Send the Outbox through Graph, as the mailbox the run is for, which files a copy of each in
/// its Sent Items. From a shared mailbox whose owner's sign-in doesn't cover sending as it
/// (`Mail.Send.Shared`), a message goes through the owner's own sending instead
/// ([`super::super::send_as`]): it is sent, and the copy is the owner's, so it isn't said filed
/// here. Returns whether the run goes on.
pub(crate) fn outbox(client: &Client<'_>, run: &mut Run<'_>) -> bool {
    let request = run.request;
    if request.outbox.is_empty() || run.cancelled() {
        return true;
    }
    let waiting = request.outbox.len();
    run.hand_over(format!("Sending {}", messages(waiting)));
    // (Said once: every message of the run goes the same way.)
    let mut other_way = false;
    for (at, message) in request.outbox.iter().enumerate() {
        if run.cancelled() {
            break;
        }
        let subject = subject_of(message);
        let outcome = match (client.send(message), client.mailbox()) {
            (Ok(()), _) => {
                run.out.filed.push(message.id);
                Ok(())
            }
            (Err(e), Some(mailbox)) if is_send_as_denied(&e) => Err((send_as_refused(&subject, mailbox, &e), e)),
            // The sign-in doesn't cover sending from the shared mailbox as itself: the owner's own
            // sending, with the mailbox as the sender (the same Send As right is checked).
            (Err(first @ NetError::Forbidden(_)), Some(mailbox)) => {
                if !std::mem::replace(&mut other_way, true) {
                    log::debug!("the sign-in doesn't cover sending from the shared mailbox as itself; its mail goes through the owner's own sending");
                }
                match send_as(&request.access_token, message, mailbox) {
                    Ok(()) => Ok(()),
                    Err(e) if is_send_as_denied(&e) => Err((send_as_refused(&subject, mailbox, &e), e)),
                    Err(e) => Err((format!("Couldn't send \"{subject}\" from {mailbox}: {first}; and through your own account: {e}"), e)),
                }
            }
            (Err(e), _) => Err((format!("Couldn't send \"{subject}\": {e}"), e)),
        };
        match outcome {
            Ok(()) => {
                run.out.sent.push(message.id);
                run.hand_over(format!("Sent {} of {waiting}", at + 1));
            }
            Err((line, e)) => {
                run.out.errors.push(line);
                if ends_run(&e) {
                    return false;
                }
            }
        }
    }
    true
}

/// What to say when Exchange wouldn't let the owner send as the shared mailbox `mailbox`.
fn send_as_refused(subject: &str, mailbox: &str, e: &NetError) -> String {
    format!(
        "Couldn't send \"{subject}\" from {mailbox}: the server didn't let your account send as that mailbox. Sending from it takes the Send As right on it, which an administrator of your organisation gives ({e})"
    )
}

/// Make a folder called `name` (one level, as [`level`] makes it) inside `parent` (`None`: at the
/// top). Its node, once Graph made it.
fn make_folder(client: &Client<'_>, tree: &mut Tree, parent: Option<&Node>, name: &str) -> Result<Node, NetError> {
    let path = match parent {
        Some(node) => format!("/v1.0/me/mailFolders/{}/childFolders", http::encode(&node.id)),
        None => "/v1.0/me/mailFolders".to_string(),
    };
    let reply = client.send_json("POST", &path, &json!({ "displayName": name }))?;
    if reply.status != 201 && reply.status != 200 {
        return Err(client.refusal(&reply, ""));
    }
    let id = id_of(object(&reply.body)?.get("id")).ok_or_else(|| super::nonsense("Microsoft Graph made a folder and didn't say which"))?;
    let parent_path = parent.map(|node| node.path.clone()).unwrap_or_default();
    let node =
        Node { id, path: join(&parent_path, name), parent: parent_path, name: name.to_string(), kind: FolderKind::Custom, total: 0, unread: 0 };
    tree.nodes.push(node.clone());
    Ok(node)
}

/// Make, rename, move and delete the folders the owner changed here, in the order it was done
/// ([`SyncRequest::folder_ops`]). `None` when there was nothing to do; else the request as the
/// rest of the run sees it, or `Err` when the run can't go on.
pub(crate) fn folder_ops(client: &Client<'_>, run: &mut Run<'_>, tree: &mut Tree) -> Option<Result<SyncRequest, ()>> {
    let request = run.request;
    if request.folder_ops.is_empty() || run.cancelled() {
        return None;
    }
    run.hand_over("Changing folders on the server".into());
    let mut renames: Vec<(String, String)> = Vec::new();
    let mut created: Vec<(Id, String)> = Vec::new();
    let current = |renames: &[(String, String)], path: &str| {
        let mut path = path.to_string();
        for (from, to) in renames {
            if let Some(moved) = renamed_path(&path, from, to, "/") {
                path = moved;
            }
        }
        path
    };
    for op in request.folder_ops.iter().take(MAX_FOLDER_OPS) {
        let (folder, from, outcome) = match op {
            FolderOp::Create { folder, parent, name } => {
                let parent = current(&renames, parent);
                (*folder, String::new(), create(client, tree, request, *folder, &parent, name))
            }
            FolderOp::Rename { folder, from, parent, name } => {
                let from = current(&renames, from);
                let parent = parent.as_deref().map(|parent| current(&renames, parent));
                let outcome = rename(client, tree, &from, parent.as_deref(), name);
                (*folder, from, outcome)
            }
            FolderOp::Delete { remote } => {
                let remote = current(&renames, remote);
                let outcome = delete(client, tree, &remote);
                (0, remote, outcome)
            }
        };
        match outcome {
            Ok(Ok(to)) => {
                match op {
                    FolderOp::Create { .. } => created.push((folder, to.clone())),
                    FolderOp::Rename { .. } if to != from => renames.push((from.clone(), to.clone())),
                    _ => {}
                }
                run.out.folders_pushed.push(FolderPushed { folder, from, to, delimiter: "/".into(), refused: false });
            }
            Ok(Err(line)) => {
                run.out.errors.push(line);
                run.out.folders_pushed.push(FolderPushed { folder, from, to: String::new(), delimiter: "/".into(), refused: true });
            }
            Err(line) => {
                run.out.errors.push(line);
                return Some(Err(()));
            }
        }
    }
    run.hand_over("Changed folders on the server".into());
    Some(Ok(crate::push::reshape(request, &renames, &created, "/")))
}

/// What a folder change came to: done (the folder's name here now), refused with what to tell
/// the owner, or `Err` with that when the run can't go on.
type Outcome = Result<Result<String, String>, String>;

/// A refusal of Graph's for a change: refused, or the end of the run.
fn refused(what: &str, e: &NetError) -> Outcome {
    let line = format!("{what}: {e}");
    if ends_run(e) { Err(line) } else { Ok(Err(line)) }
}

fn create(client: &Client<'_>, tree: &mut Tree, request: &SyncRequest, folder: Id, parent: &str, name: &str) -> Outcome {
    let what = format!("Couldn't make the folder \"{}\" on the server", shown(name));
    let Some(name) = level(name) else { return Ok(Err(format!("{what}: it has no usable name"))) };
    let above = if parent.is_empty() {
        None
    } else {
        match tree.find(parent) {
            Some(node) => Some(node.clone()),
            None => return Ok(Err(format!("{what}: the folder \"{}\" it goes into is not on the server", shown(parent)))),
        }
    };
    let path = join(above.as_ref().map_or("", |node| node.path.as_str()), &name);
    if tree.find(&path).is_some() {
        // Made meanwhile by another program: it is the folder's, unless a folder here follows it.
        if request.folders.iter().any(|f| f.id != folder && f.remote == path) {
            return Ok(Err(format!("{what}: it has another folder of that name")));
        }
        return Ok(Ok(path));
    }
    match make_folder(client, tree, above.as_ref(), &name) {
        Ok(node) => Ok(Ok(node.path)),
        Err(e) => refused(&what, &e),
    }
}

fn rename(client: &Client<'_>, tree: &mut Tree, from: &str, parent: Option<&str>, name: &str) -> Outcome {
    let what = format!("Couldn't move or rename the folder \"{}\" on the server", shown(from));
    let Some(source) = tree.find(from).filter(|node| node.path != INBOX).cloned() else {
        return Ok(Err(format!("{what}: it is no longer there")));
    };
    let Some(name) = level(name) else { return Ok(Err(format!("{what}: the new name can't be used"))) };
    let above = match parent {
        None => tree.find(&source.parent).cloned(),
        Some("") => None,
        Some(parent) => match tree.find(parent) {
            Some(node) => Some(node.clone()),
            None => return Ok(Err(format!("{what}: the folder \"{}\" it goes into is not on the server", shown(parent)))),
        },
    };
    let above_path = above.as_ref().map(|node| node.path.clone()).unwrap_or_default();
    let to = join(&above_path, &name);
    if to == source.path {
        return Ok(Ok(to));
    }
    if tree.find(&to).is_some() {
        return Ok(Err(format!("{what}: there is another folder of that name")));
    }
    let base = format!("/v1.0/me/mailFolders/{}", http::encode(&source.id));
    if above_path != source.parent {
        let destination = above.as_ref().map_or_else(|| "msgfolderroot".to_string(), |node| node.id.clone());
        match change(client, "POST", &format!("{base}/move"), &json!({ "destinationId": destination })) {
            Ok(Done::Yes) => {}
            Ok(Done::Gone) => return Ok(Err(format!("{what}: it is no longer there"))),
            Err(e) => return refused(&what, &e),
        }
    }
    let own = source.path.rsplit('/').next().unwrap_or(&source.path);
    if own != name {
        match change(client, "PATCH", &base, &json!({ "displayName": name })) {
            Ok(Done::Yes) => {}
            Ok(Done::Gone) => return Ok(Err(format!("{what}: it is no longer there"))),
            Err(e) => return refused(&what, &e),
        }
    }
    for node in &mut tree.nodes {
        if let Some(moved) = renamed_path(&node.path, &source.path, &to, "/") {
            node.path = moved;
        }
        if let Some(moved) = renamed_path(&node.parent, &source.path, &to, "/") {
            node.parent = moved;
        }
        if node.id == source.id {
            node.parent = above_path.clone();
            node.name = name.clone();
        }
    }
    Ok(Ok(to))
}

fn delete(client: &Client<'_>, tree: &mut Tree, remote: &str) -> Outcome {
    let what = format!("Couldn't delete the folder \"{}\" from the server", shown(remote));
    let Some(node) = tree.find(remote).cloned() else { return Ok(Ok(String::new())) };
    // Only a folder in Deleted Items is deleted for good, as Outlook has it.
    let inside = tree.of_kind(FolderKind::Deleted).is_some_and(|deleted| remote.starts_with(&format!("{}/", deleted.path)));
    if !inside {
        return Ok(Err(format!("{what}: only a folder in Deleted Items is deleted from the server")));
    }
    match change(client, "POST", &format!("/v1.0/me/mailFolders/{}/permanentDelete", http::encode(&node.id)), &json!({})) {
        Ok(_) => {
            let below = format!("{remote}/");
            tree.nodes.retain(|other| other.path != remote && !other.path.starts_with(&below));
            Ok(Ok(String::new()))
        }
        Err(e) => refused(&what, &e),
    }
}

/// `text` inside `'…'` of an OData query.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Put the drafts written here on the server ([`SyncRequest::uploads`]; the engine sends no
/// other kind for an account read through Graph). A draft Graph has already, by its
/// Message-ID, is taken for it. Returns whether the run goes on.
pub(crate) fn uploads(client: &Client<'_>, run: &mut Run<'_>, request: &SyncRequest, tree: &mut Tree) -> bool {
    let todo: Vec<_> = request.uploads.iter().filter(|up| !up.copy_only).take(MAX_UPLOADS).collect();
    if todo.is_empty() || run.cancelled() {
        return true;
    }
    run.hand_over(format!("Putting {} on the server", messages(todo.len())));
    for up in todo {
        if run.cancelled() {
            break;
        }
        let what = format!("Couldn't put \"{}\" on the server", subject_of(&up.message));
        let Some(destination) = tree.find(&up.to).cloned() else {
            run.out.errors.push(format!("{what}: the folder \"{}\" is no longer there", shown(&up.to)));
            continue;
        };
        let mut flags = RemoteFlags::of(&up.message, 0);
        let mut done = || -> Result<bool, NetError> {
            let message_id = up.message.message_id.trim();
            if !message_id.is_empty() && message_id.len() <= 900 && !message_id.chars().any(char::is_control) {
                let filter = format!("internetMessageId eq {}", quoted(&format!("<{message_id}>")));
                let path =
                    format!("/v1.0/me/mailFolders/{}/messages?$filter={}&$select=id&$top=1", http::encode(&destination.id), http::encode(&filter));
                let there = client.get_json(&path)?;
                if there.get("value").and_then(Value::as_array).is_some_and(|list| !list.is_empty()) {
                    return Ok(true);
                }
            }
            let offset_minutes = mailcraft_time::local_offset_minutes(up.message.date);
            let mime = mailcraft_mime::write(&up.message, &WriteOptions { include_bcc: true, offset_minutes });
            if mime.len() > MAX_UPLOAD {
                return Err(NetError::Config("it is too large".into()));
            }
            let body = STANDARD.encode(&mime);
            drop(mime);
            let reply =
                client.call("POST", &client.url("/v1.0/me/messages"), Some(("text/plain", &body)), super::PREFER, super::FETCH, super::MAX_PAGE)?;
            if reply.status != 201 && reply.status != 200 {
                return Err(client.refusal(&reply, &body));
            }
            let made = object(&reply.body)?;
            let id = id_of(made.get("id")).ok_or_else(|| super::nonsense("Microsoft Graph took the message and didn't say which it is"))?;
            flags.draft = made.get("isDraft").and_then(Value::as_bool).unwrap_or(true);
            // Graph puts it among the drafts.
            if destination.kind != FolderKind::Drafts {
                change(client, "POST", &format!("/v1.0/me/messages/{}/move", http::encode(&id)), &json!({ "destinationId": destination.id }))?;
            }
            Ok(true)
        };
        match done() {
            Ok(_) => {
                // (Its folder's listed count is from before this: the folder is read whatever
                // the count says.)
                run.touched.insert(destination.path.clone());
                run.out.uploaded.push(Uploaded {
                    message: up.message.id,
                    folder: up.message.folder,
                    to: up.to.clone(),
                    uid: 0,
                    uid_validity: 0,
                    message_id: up.message.message_id.clone(),
                    flags,
                    copied: false,
                })
            }
            Err(e) => {
                run.out.errors.push(format!("{what}: {e}"));
                if ends_run(&e) {
                    return false;
                }
            }
        }
    }
    true
}

/// Make on the server the changes made here to its messages ([`SyncRequest::ops`]). Returns
/// whether the run goes on.
pub(crate) fn ops(client: &Client<'_>, run: &mut Run<'_>, request: &SyncRequest, tree: &mut Tree) -> bool {
    if request.ops.is_empty() || run.cancelled() {
        return true;
    }
    // Which message is which, by its folder and its number.
    let ids: HashMap<(&str, u32), &str> =
        request.folders.iter().flat_map(|f| f.remote_ids.iter().map(move |r| ((f.remote.as_str(), r.uid), r.id.as_str()))).collect();
    let id = |folder: &str, uid: u32| ids.get(&(folder, uid)).copied();
    for op in request.ops.iter().take(MAX_OPS) {
        let outcome = match op {
            RemoteOp::Flags { folder, uid, read, flagged, answered } => {
                let mut body = serde_json::Map::new();
                if let Some(read) = read {
                    body.insert("isRead".into(), json!(read));
                }
                if let Some(flagged) = flagged {
                    body.insert("flag".into(), json!({ "flagStatus": if *flagged { "flagged" } else { "notFlagged" } }));
                }
                let done = match id(folder, *uid) {
                    Some(id) if !body.is_empty() => {
                        change(client, "PATCH", &format!("/v1.0/me/messages/{}", http::encode(id)), &Value::Object(body)).map(|_| ())
                    }
                    // Whether it was answered is kept here: Graph has no word for it.
                    _ => Ok(()),
                };
                done.map(|()| {
                    run.out.flagged.push(FlagsPushed { folder: folder.clone(), uid: *uid, read: *read, flagged: *flagged, answered: *answered })
                })
                .map_err(|e| ("Couldn't change a message's flags on the server", e))
            }
            RemoteOp::Categories { folder, uid, add, remove } => {
                let done = match id(folder, *uid) {
                    Some(id) => recategorize(client, id, add, remove),
                    None => Ok(()),
                };
                done.map(|()| {
                    run.out.categorized.push(CategoriesPushed { folder: folder.clone(), uid: *uid, add: add.clone(), remove: remove.clone() })
                })
                .map_err(|e| ("Couldn't change a message's categories on the server", e))
            }
            RemoteOp::Move { message, from, uid, to, to_folder, create } => match destination(client, run, tree, to, *to_folder, create) {
                Ok(Some(destination)) => {
                    let done = match id(from, *uid) {
                        Some(id) => {
                            change(client, "POST", &format!("/v1.0/me/messages/{}/move", http::encode(id)), &json!({ "destinationId": destination }))
                                .map(|_| ())
                        }
                        None => Ok(()),
                    };
                    done.map(|()| {
                        // (The folder it went to is read whatever its listed count says.)
                        if let Some(node) = tree.nodes.iter().find(|node| node.id == destination) {
                            run.touched.insert(node.path.clone());
                        }
                        run.out.moved.push(Moved { message: *message, from: from.clone(), uid: *uid, to_folder: *to_folder, ..Default::default() })
                    })
                    .map_err(|e| ("Couldn't move a message on the server", e))
                }
                Ok(None) => {
                    run.out.errors.push(format!("Couldn't move messages to \"{}\": the folder is no longer on the server", shown(to)));
                    Ok(())
                }
                Err(e) => Err(("Couldn't make a folder on the server", e)),
            },
            RemoteOp::Delete { from, uid } => {
                let done = match id(from, *uid) {
                    Some(id) => change(client, "POST", &format!("/v1.0/me/messages/{}/permanentDelete", http::encode(id)), &json!({})).map(|_| ()),
                    None => Ok(()),
                };
                done.map(|()| run.out.deleted.push(Deleted { from: from.clone(), uid: *uid }))
                    .map_err(|e| ("Couldn't delete a message on the server", e))
            }
        };
        if let Err((what, e)) = outcome {
            run.out.errors.push(format!("{what}: {e}"));
            if ends_run(&e) {
                return false;
            }
        }
    }
    run.hand_over("Saved the changes on the server".into());
    true
}

/// Give and take categories: the message's list as Graph has it now, with the changes made here.
fn recategorize(client: &Client<'_>, id: &str, add: &[String], remove: &[String]) -> Result<(), NetError> {
    let path = format!("/v1.0/me/messages/{}", http::encode(id));
    let now = match client.get_json(&format!("{path}?$select=categories")) {
        Ok(value) => categories_in(value.get("categories")),
        Err(e) if matches!(&e, NetError::Proto(mailcraft_proto::ProtoError::Rejected(why)) if why.contains("status 404")) => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut names: Vec<String> = now.into_iter().filter(|name| !remove.iter().any(|gone| gone.eq_ignore_ascii_case(name))).collect();
    for name in add {
        if names.len() < MAX_CATEGORIES && !names.iter().any(|have| have.eq_ignore_ascii_case(name)) {
            names.push(name.clone());
        }
    }
    change(client, "PATCH", &path, &json!({ "categories": names })).map(|_| ())
}

/// The id of the folder a move goes to: `to`, or the folders `create` names below it, made
/// first. `None` when `to` isn't there.
fn destination(
    client: &Client<'_>,
    run: &mut Run<'_>,
    tree: &mut Tree,
    to: &str,
    to_folder: Id,
    create: &[String],
) -> Result<Option<String>, NetError> {
    if create.is_empty() {
        return Ok(tree.find(to).map(|node| node.id.clone()));
    }
    let mut above = if to.is_empty() {
        None
    } else {
        match tree.find(to) {
            Some(node) => Some(node.clone()),
            None => return Ok(None),
        }
    };
    for name in create {
        let Some(name) = level(name) else { return Ok(None) };
        let path = join(above.as_ref().map_or("", |node| node.path.as_str()), &name);
        let node = match tree.find(&path) {
            Some(node) => node.clone(),
            None => make_folder(client, tree, above.as_ref(), &name)?,
        };
        above = Some(node);
    }
    let Some(node) = above else { return Ok(None) };
    run.out.created.push(Created { folder: to_folder, remote: node.path.clone() });
    Ok(Some(node.id))
}

/// Make the changes made here since the run began, when any wait. Returns whether the run goes on.
pub(crate) fn top_up(client: &Client<'_>, run: &mut Run<'_>, tree: &mut Tree) -> bool {
    match run.more_ops() {
        Some(request) => ops(client, run, &request, tree),
        None => true,
    }
}
