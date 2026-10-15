//! The folders of mail of a mailbox, as Graph lists them, and the names the engine knows them
//! by: the Inbox is `INBOX` and what is in it `INBOX/…`, as over IMAP; any other folder is the
//! names from the top of the mailbox down to it, parted by `/` (a `/` in a name is written `_`).

use std::collections::{HashMap, HashSet, VecDeque};

use mailcraft_model::FolderKind;
use mailcraft_model::sync::{ListedFolder, SyncScope};
use serde_json::Value;

use super::{Client, Stop, id_of};
use crate::NetError;
use crate::http;
use crate::sync::{NOT_MAIL, turn};

/// What is asked of each folder.
const FOLDER_FIELDS: &str = "id,displayName,parentFolderId,childFolderCount,totalItemCount,unreadItemCount";
/// The special folders by the names Graph knows them by wherever they are and whatever they are
/// called.
const WELL_KNOWN: [(&str, FolderKind); 7] = [
    ("inbox", FolderKind::Inbox),
    ("sentitems", FolderKind::Sent),
    ("drafts", FolderKind::Drafts),
    ("deleteditems", FolderKind::Deleted),
    ("junkemail", FolderKind::Junk),
    ("archive", FolderKind::Archive),
    ("outbox", FolderKind::Outbox),
];
/// Most folders read, and how deep.
pub(crate) const MAX_FOLDERS: usize = 2_000;
const MAX_DEPTH: usize = 16;
/// Longest name of a folder, in characters.
pub(crate) const MAX_NAME: usize = 255;
/// The Inbox's name here.
pub(crate) const INBOX: &str = "INBOX";

/// One folder of mail.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Node {
    /// What Graph calls it. Never leaves this crate.
    pub(crate) id: String,
    /// Its name here.
    pub(crate) path: String,
    /// The name here of the folder it lies in; empty at the top.
    pub(crate) parent: String,
    /// Its own name, as people read it.
    pub(crate) name: String,
    pub(crate) kind: FolderKind,
    /// How many messages Graph says it holds, and how many of them are unread.
    pub(crate) total: u64,
    pub(crate) unread: u64,
}

/// The folders of a mailbox.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tree {
    pub(crate) nodes: Vec<Node>,
    /// Every folder of mail is here: nothing was left out, and the Inbox is among them.
    pub(crate) whole: bool,
    /// What went wrong on the way, for the run's answer.
    pub(crate) errors: Vec<String>,
}

impl Tree {
    pub(crate) fn find(&self, path: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.path == path)
    }

    pub(crate) fn of_kind(&self, kind: FolderKind) -> Option<&Node> {
        self.nodes.iter().find(|node| node.kind == kind)
    }

    /// The folders as the run's answer lists them.
    pub(crate) fn listed(&self) -> Vec<ListedFolder> {
        self.nodes
            .iter()
            .map(|node| ListedFolder { remote: node.path.clone(), kind: node.kind, parent: node.parent.clone(), name: node.name.clone() })
            .collect()
    }

    /// The folders a run of `scope` reads, in the order they are read: the Inbox, then sent mail
    /// and drafts, and so on.
    pub(crate) fn plan(&self, scope: SyncScope) -> Vec<Node> {
        let mut plan: Vec<Node> = self.nodes.iter().filter(|node| scope.covers(&node.path, node.kind)).cloned().collect();
        plan.sort_by_key(|node| turn(node.kind));
        plan
    }
}

/// A folder's own name as one level of a name here: one line, not too long, `/` as `_`.
pub(crate) fn level(name: &str) -> Option<String> {
    let name: String = name.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.replace('/', "_"))
}

/// `name` inside the folder called `parent` here (empty: at the top).
pub(crate) fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") }
}

/// Read the folders of mail. `tied` are the folders here that follow one already: a folder that
/// isn't mail (Exchange's `Sync Issues`) is left out unless one does.
pub(crate) fn list(client: &Client<'_>, tied: &[(String, FolderKind)]) -> Result<Tree, NetError> {
    // Which of them are the special ones.
    let requests: Vec<(&str, String, Option<Value>)> =
        WELL_KNOWN.iter().map(|(name, _)| ("GET", format!("/me/mailFolders/{name}?$select=id"), None)).collect();
    let answers = client.batch(&requests, super::MAX_PAGE)?;
    let mut kinds: HashMap<String, FolderKind> = HashMap::new();
    for ((_, kind), answer) in WELL_KNOWN.iter().zip(answers) {
        match answer {
            Some((200, body)) => {
                if let Some(id) = id_of(body.get("id")) {
                    kinds.entry(id).or_insert(*kind);
                }
            }
            Some((401, _)) => return Err(NetError::Proto(mailcraft_proto::ProtoError::Auth("Microsoft Graph didn't take the sign-in".into()))),
            // A mailbox without an archive, say.
            _ => {}
        }
    }
    let mut tree = Tree::default();
    let mut paths: HashSet<String> = HashSet::new();
    let mut whole = true;
    // Folders to look into: their id, their name here, their depth.
    let mut queue: VecDeque<(String, String, usize)> = VecDeque::new();
    let top = client.url(&format!("/v1.0/me/mailFolders?$select={FOLDER_FIELDS}&$top=100"));
    let mut found: Vec<(Value, String, usize)> = Vec::new();
    match client.pages(top, MAX_FOLDERS, &mut |value| found.push((value.clone(), String::new(), 0))) {
        Ok(listed) => whole &= listed.whole,
        Err(stop) => return Err(stop.into_error()),
    }
    loop {
        for (value, parent, depth) in found.drain(..) {
            if tree.nodes.len() >= MAX_FOLDERS {
                whole = false;
                break;
            }
            let Some(id) = id_of(value.get("id")) else { continue };
            let raw = value.get("displayName").and_then(Value::as_str).unwrap_or("");
            let Some(name) = level(raw) else { continue };
            let kind = kinds.get(&id).copied().unwrap_or(FolderKind::Custom);
            let path = if kind == FolderKind::Inbox && parent.is_empty() { INBOX.to_string() } else { join(&parent, &name) };
            let followed = tied.iter().any(|(remote, _)| *remote == path);
            // Not mail, unless a folder here follows it already.
            if parent.is_empty() && kind != FolderKind::Inbox && NOT_MAIL.iter().any(|other| name.eq_ignore_ascii_case(other)) && !followed {
                continue;
            }
            // Two folders of one name here: the second is left out, with what is in it.
            if !paths.insert(path.clone()) {
                log::debug!("two folders of mail have the same name here; the second is left out");
                continue;
            }
            let children = value.get("childFolderCount").and_then(Value::as_u64).unwrap_or(0);
            if children > 0 {
                if depth + 1 < MAX_DEPTH {
                    queue.push_back((id.clone(), path.clone(), depth + 1));
                } else {
                    whole = false;
                }
            }
            let shown_name: String = raw.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
            let shown_name = if path == INBOX { "Inbox".to_string() } else { shown_name.trim().to_string() };
            let total = value.get("totalItemCount").and_then(Value::as_u64).unwrap_or(0);
            let unread = value.get("unreadItemCount").and_then(Value::as_u64).unwrap_or(0);
            tree.nodes.push(Node { id, path, parent, name: shown_name, kind, total, unread });
        }
        let Some((id, path, depth)) = queue.pop_front() else { break };
        if tree.nodes.len() >= MAX_FOLDERS {
            whole = false;
            break;
        }
        let children = client.url(&format!("/v1.0/me/mailFolders/{}/childFolders?$select={FOLDER_FIELDS}&$top=100", http::encode(&id)));
        match client.pages(children, MAX_FOLDERS, &mut |value| found.push((value.clone(), path.clone(), depth))) {
            Ok(listed) => whole &= listed.whole,
            Err(Stop::Net(e)) if super::ends_run(&e) => return Err(e),
            Err(stop) => {
                whole = false;
                tree.errors.push(format!("Couldn't read the folders in \"{}\": {}", crate::sync::shown(&path), stop.into_error()));
                found.clear();
            }
        }
    }
    tree.whole = whole && tree.nodes.iter().any(|node| node.path == INBOX);
    Ok(tree)
}
