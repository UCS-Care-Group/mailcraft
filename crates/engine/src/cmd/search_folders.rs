//! Search folders: saved searches the folder pane lists under Search Folders. They hold no mail
//! of their own: choosing one shows every message that matches its query, wherever it is filed
//! (Deleted Items and Junk Email left out), and nothing of them is ever a folder on a server.

use mailcraft_model::search::Query;
use mailcraft_model::{Address, FolderKind, Id, MAX_SAVED, Message, SearchFolder, clean_name};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Module, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("folder.newSearchFolder", "New Search Folder", "Folder › New", new).params(
            r#"{"template": "unread"|"flagged"|"important"|"attachments"|"categorized"|"large"|"from"|"custom", "people"?: "ada@example.org; Bob" (for "from": anyone of them), "size"?: KB (for "large"; default 100), "category"?: string (for "categorized": that one only; default: any), "query"?: string (for "custom": the search box's language, `is:unread from:ada larger:2mb`), "name"?: string (default: after the template), "account"?: number (only that account's mail; default: every account's)} — makes the search folder and shows it. Answers {"id", "name", "query"}"#,
        ),
        CommandSpec::new("searchFolder.edit", "Customize This Search Folder", "Folder › New", edit)
            .params(r#"{"id"?: number (default: the search folder in view), "name"?: string, "query"?: string, "account"?: number | null}"#)
            .when(in_search_folder_or_any),
        CommandSpec::new("searchFolder.delete", "Delete Search Folder", "Folder › Actions", delete)
            .params(r#"{"id"?: number (default: the search folder in view)} — the messages it shows stay where they are"#)
            .when(in_search_folder_or_any),
        CommandSpec::new("searchFolder.list", "Search Folders", "Folder", list)
            .pure()
            .params(r#"{} — answers {"searchFolders": [{"id", "name", "query", "account", "messages", "unread"}]}"#),
    ]
}

/// Longest list of people a "from" search folder names.
const MAX_PEOPLE: usize = 50;

fn in_search_folder_or_any(s: &Session) -> Option<&'static str> {
    if s.mailbox.search_folders.is_empty() { Some("there are no search folders") } else { None }
}

/// The search folder with this id.
pub fn find(s: &Session, id: Id) -> Option<&SearchFolder> {
    s.mailbox.search_folders.iter().find(|f| f.id == id)
}

/// The search folder in view, when the folder in view is one.
pub fn in_view(s: &Session) -> Option<&SearchFolder> {
    find(s, s.view.folder)
}

/// Whether a message is one that `folder` shows: it matches the query, it is in an account the
/// search folder looks at, and it is not in Deleted Items or Junk Email (or a folder under them).
pub fn shows(s: &Session, folder: &SearchFolder, query: &Query, hidden: &[Id], m: &Message) -> bool {
    folder.account.is_none_or(|a| m.account == a) && !hidden.contains(&m.folder) && query.matches(m, s.offset_at(m.date))
}

/// The folders whose mail no search folder shows: Deleted Items and Junk Email, with what is in
/// them.
pub fn hidden_folders(s: &Session) -> Vec<Id> {
    let mb = &s.mailbox;
    mb.folders.iter().filter(|f| matches!(f.kind, FolderKind::Deleted | FolderKind::Junk)).flat_map(|f| mb.subtree(f.id)).collect()
}

/// How many messages `folder` shows, and how many of them are unread.
pub fn counts(s: &Session, folder: &SearchFolder) -> (usize, usize) {
    let query = Query::parse(&folder.query);
    let hidden = hidden_folders(s);
    let mut total = 0;
    let mut unread = 0;
    for m in s.mailbox.messages.values() {
        if shows(s, folder, &query, &hidden, m) {
            total += 1;
            unread += usize::from(!m.read);
        }
    }
    (total, unread)
}

/// A name or address for a query term: quoted when it has a space, without characters that
/// would end the term early.
fn person(text: &str) -> String {
    let clean: String = text.chars().filter(|c| !c.is_control() && !matches!(c, '"' | ',')).collect();
    let clean = clean.trim().to_string();
    if clean.contains(' ') { format!("\"{clean}\"") } else { clean }
}

/// The query and the default name of a template.
fn template(v: &Value) -> Result<(String, String), CmdError> {
    let kind = p::str(v, "template").unwrap_or("custom");
    Ok(match kind.trim().to_ascii_lowercase().as_str() {
        "unread" => ("is:unread".into(), "Unread Mail".into()),
        "flagged" | "followup" => ("is:flagged".into(), "For Follow Up".into()),
        "important" => ("is:important".into(), "Important Mail".into()),
        "attachments" => ("has:attachment".into(), "Mail with Attachments".into()),
        "categorized" => match p::str(v, "category").and_then(clean_name) {
            Some(c) => (format!("category:{}", person(&c)), format!("Categorized as {c}")),
            None => ("is:categorized".into(), "Categorized Mail".into()),
        },
        "large" => {
            let kb = p::u64(v, "size").unwrap_or(100).clamp(1, 10_000_000);
            (format!("larger:{kb}kb"), format!("Larger Than {kb} KB"))
        }
        "from" => {
            let typed = p::str(v, "people").ok_or_else(|| bad("\"from\" needs `people`: the names or addresses to look for"))?;
            let people: Vec<String> = Address::parse_list(typed)
                .into_iter()
                .map(|a| if a.email.trim().is_empty() { a.name } else { a.email })
                .map(|t| person(&t))
                .filter(|t| !t.is_empty())
                .take(MAX_PEOPLE)
                .collect();
            if people.is_empty() {
                return Err(bad("\"from\" needs `people`: the names or addresses to look for"));
            }
            let named = people.iter().map(|t| t.trim_matches('"').to_string()).collect::<Vec<_>>().join(", ");
            (format!("from:{}", people.join(",")), format!("Mail from {named}"))
        }
        "custom" => {
            let query = SearchFolder::clean_query(p::str(v, "query").unwrap_or(""));
            if query.is_empty() {
                return Err(bad("a custom search folder needs a `query`"));
            }
            (query, "Custom Search Folder".into())
        }
        other => return Err(bad(format!("unknown template \"{other}\""))),
    })
}

fn account_param(s: &Session, v: &Value) -> Result<Option<Id>, CmdError> {
    match v.get("account") {
        None | Some(Value::Null) => Ok(None),
        Some(_) => {
            let a = p::req_id(v, "account")?;
            s.mailbox.account(a).map(|a| Some(a.id)).ok_or_else(|| fail(format!("no account with id {a}")))
        }
    }
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    let (query, default_name) = template(v)?;
    let name = p::str(v, "name").and_then(clean_name).or_else(|| clean_name(&default_name)).unwrap_or_else(|| "Search Folder".into());
    let account = account_param(s, v)?;
    if s.mailbox.search_folders.len() >= MAX_SAVED {
        return Err(fail("there are too many search folders"));
    }
    let id = s.mailbox.alloc_id();
    s.mailbox.search_folders.push(SearchFolder { id, name: name.clone(), query: query.clone(), account });
    // It opens, as a folder does when it is chosen.
    s.view.folder = id;
    s.view.module = Module::Mail;
    s.view.search.clear();
    s.sel.messages.clear();
    Ok(json!({"id": id, "name": name, "query": query}))
}

fn target(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = p::id(v, "id").unwrap_or(s.view.folder);
    find(s, id).map(|f| f.id).ok_or_else(|| fail(format!("no search folder with id {id}")))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let name = match p::str(v, "name") {
        Some(n) => Some(clean_name(n).ok_or_else(|| bad("`name` is empty"))?),
        None => None,
    };
    let query = match p::str(v, "query") {
        Some(q) => {
            let q = SearchFolder::clean_query(q);
            if q.is_empty() {
                return Err(bad("`query` is empty"));
            }
            Some(q)
        }
        None => None,
    };
    let account = if v.get("account").is_some() { Some(account_param(s, v)?) } else { None };
    let f = s.mailbox.search_folders.iter_mut().find(|f| f.id == id).ok_or_else(|| fail("the search folder is gone"))?;
    if let Some(n) = name {
        f.name = n;
    }
    if let Some(q) = query {
        f.query = q;
    }
    if let Some(a) = account {
        f.account = a;
    }
    Ok(json!({"id": id, "name": f.name, "query": f.query, "account": f.account}))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    s.mailbox.search_folders.retain(|f| f.id != id);
    if s.view.folder == id {
        s.view.folder = s.mailbox.inbox().unwrap_or(0);
        s.sel.messages.clear();
    }
    Ok(json!({"deleted": id}))
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    let folders: Vec<Value> = s
        .mailbox
        .search_folders
        .iter()
        .map(|f| {
            let (messages, unread) = counts(s, f);
            json!({"id": f.id, "name": f.name, "query": f.query, "account": f.account, "messages": messages, "unread": unread})
        })
        .collect();
    Ok(json!({"searchFolders": folders, "current": in_view(s).map(|f| f.id)}))
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{FolderKind, Importance};
    use serde_json::json;

    use crate::{Session, sample};

    fn session() -> Session {
        let now = 1_791_993_600; // 2026-10-14
        Session::at(sample::mailbox(now, 0), now, 0)
    }

    fn shown(s: &mut Session) -> Vec<u64> {
        s.rows().iter().map(|r| r.id).collect()
    }

    #[test]
    fn a_search_folder_shows_matching_mail_from_every_folder_and_holds_none() {
        let mut s = session();
        let unread: Vec<u64> = s.mailbox.messages.values().filter(|m| !m.read).map(|m| m.id).collect();
        assert!(!unread.is_empty());
        // One unread message is filed elsewhere, one is in Deleted Items.
        let account = s.mailbox.accounts[0].id;
        let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
        let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
        s.mailbox.move_message(unread[0], archive).unwrap();
        if let Some(gone) = unread.get(1) {
            s.mailbox.move_message(*gone, deleted).unwrap();
        }
        let folders_before = s.mailbox.folders.len();
        let made = s.run("folder.newSearchFolder", &json!({"template": "unread"})).unwrap();
        let id = made["id"].as_u64().unwrap();
        assert_eq!((made["name"].as_str(), made["query"].as_str()), (Some("Unread Mail"), Some("is:unread")));
        assert_eq!(s.view.folder, id, "it opens");
        assert_eq!(s.mailbox.folders.len(), folders_before, "no folder is made");
        let rows = shown(&mut s);
        assert!(rows.contains(&unread[0]), "mail filed elsewhere shows");
        if let Some(gone) = unread.get(1) {
            assert!(!rows.contains(gone), "deleted mail doesn't");
        }
        assert!(rows.iter().all(|id| !s.mailbox.message(*id).unwrap().read));
        // Selecting a message keeps the search folder in view.
        s.run("mail.select", &json!({"id": unread[0]})).unwrap();
        assert_eq!(s.view.folder, id);
        // The view survives normalising, and the counts agree with the list.
        s.normalize();
        assert_eq!(s.view.folder, id);
        let listed = s.run("searchFolder.list", &json!({})).unwrap();
        // (Opening the message marked it read: it leaves the list and the count alike.)
        assert_eq!(listed["searchFolders"][0]["messages"].as_u64().unwrap() as usize, s.rows().len());
        // Choosing a folder leaves it; folder.select takes it back.
        s.run("folder.select", &json!({"kind": "inbox"})).unwrap();
        assert_ne!(s.view.folder, id);
        s.run("folder.select", &json!({"id": id})).unwrap();
        assert_eq!(s.view.folder, id);
    }

    #[test]
    fn templates_make_the_queries_the_search_box_reads() {
        let mut s = session();
        let make = |s: &mut Session, v: serde_json::Value| s.run("folder.newSearchFolder", &v).unwrap();
        assert_eq!(make(&mut s, json!({"template": "large", "size": 2048}))["query"], "larger:2048kb");
        assert_eq!(make(&mut s, json!({"template": "categorized"}))["query"], "is:categorized");
        assert_eq!(make(&mut s, json!({"template": "categorized", "category": "Red category"}))["query"], "category:\"Red category\"");
        let from = make(&mut s, json!({"template": "from", "people": "Ada Lovelace <ada@example.org>; Bob, \"Carol Day\""}));
        assert_eq!(from["query"], "from:ada@example.org,Bob,\"Carol Day\"");
        assert_eq!(from["name"], "Mail from ada@example.org, Bob, Carol Day");
        let custom = make(&mut s, json!({"template": "custom", "query": "budget\r\n-is:read", "name": "Budget"}));
        assert_eq!((custom["query"].as_str(), custom["name"].as_str()), (Some("budget  -is:read"), Some("Budget")));
        assert!(s.run("folder.newSearchFolder", &json!({"template": "from", "people": " ; "})).is_err());
        assert!(s.run("folder.newSearchFolder", &json!({"template": "custom"})).is_err());
        assert!(s.run("folder.newSearchFolder", &json!({"template": "nonsense"})).is_err());
        assert!(s.run("folder.newSearchFolder", &json!({"template": "unread", "account": 999_999})).is_err());
    }

    #[test]
    fn a_from_folder_finds_any_of_its_people_and_edits_and_deletes() {
        let mut s = session();
        let senders: Vec<String> = s.mailbox.messages.values().map(|m| m.from.email.clone()).collect();
        let (a, b) = (senders[0].clone(), senders.iter().find(|e| **e != senders[0]).cloned().unwrap());
        let id = s.run("folder.newSearchFolder", &json!({"template": "from", "people": format!("{a}; {b}")})).unwrap()["id"].as_u64().unwrap();
        let rows = shown(&mut s);
        assert!(!rows.is_empty());
        for row in &rows {
            let from = &s.mailbox.message(*row).unwrap().from.email;
            assert!(from.eq_ignore_ascii_case(&a) || from.eq_ignore_ascii_case(&b), "{from}");
        }
        s.run("searchFolder.edit", &json!({"name": "People", "query": "importance:high"})).unwrap();
        let rows = shown(&mut s);
        assert!(rows.iter().all(|r| s.mailbox.message(*r).unwrap().importance == Importance::High));
        assert!(s.run("searchFolder.edit", &json!({"query": "  "})).is_err());
        // The search box narrows a search folder further.
        s.view.search = "zzzz-no-such-text".into();
        assert!(shown(&mut s).is_empty());
        s.view.search.clear();
        let messages = s.mailbox.messages.len();
        s.run("searchFolder.delete", &json!({"id": id})).unwrap();
        assert!(s.mailbox.search_folders.is_empty() && s.mailbox.messages.len() == messages);
        assert_ne!(s.view.folder, id);
        assert!(s.run("searchFolder.delete", &json!({"id": id})).is_err());
    }

    #[test]
    fn search_folders_survive_a_file_and_repair_keeps_their_ids_apart() {
        let mut s = session();
        s.run("folder.newSearchFolder", &json!({"template": "flagged"})).unwrap();
        let mut mb = s.mailbox.clone();
        // A file whose search folder took a folder's id gets a new one.
        mb.search_folders[0].id = mb.folders[0].id;
        mb.search_folders[0].query = "x\u{0}y".into();
        let json = serde_json::to_string(&mb).unwrap();
        let mut back: mailcraft_model::Mailbox = serde_json::from_str(&json).unwrap();
        assert!(!back.repair().is_empty());
        assert!(back.folders.iter().all(|f| f.id != back.search_folders[0].id));
        assert_eq!(back.search_folders[0].query, "x y");
        // And a file with junk where the list goes still opens.
        let junk = json.replacen("\"searchFolders\":[", "\"searchFolders\":[5,null,", 1);
        let back: mailcraft_model::Mailbox = serde_json::from_str(&junk).unwrap();
        assert_eq!(back.search_folders.len(), 1);
    }
}
