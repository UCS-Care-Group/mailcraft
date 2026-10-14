//! Contact groups: a name that stands for several people. Typed on the To, Cc or Bcc line, the
//! group's name becomes its members when the message is sent.

use std::collections::HashSet;

use mailcraft_model::{Address, ContactGroup, GroupMember, Id, MAX_SAVED, clean_name, format_list};
use serde_json::{Value, json};

use super::{bad, deliver, fail, file_name};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("people.newGroup", "New Contact Group", "Home › New", new).params(
            r#"{"name": string, "contacts"?: [contact id], "addresses"?: "Ada <ada@example.org>; bob@example.org", "notes"?: string} — answers {"id", "members"}; its name on a recipient line stands for its members when the message is sent"#,
        ),
        CommandSpec::new("people.editGroup", "Edit Contact Group", "Contact Group", edit)
            .params(r#"{"id"?: number (default: the selected group), "name"?: string, "notes"?: string, "categories"?: [string]}"#)
            .when(needs_group),
        CommandSpec::new("people.addMembers", "Add Members", "Contact Group › Members", add_members)
            .params(r#"{"id"?: number, "contacts"?: [contact id], "addresses"?: "a@example.org; Bob <bob@example.org>"} — an address that is in the group already is skipped"#)
            .when(needs_group),
        CommandSpec::new("people.removeMembers", "Remove Member", "Contact Group › Members", remove_members)
            .params(r#"{"id"?: number, "emails": [string]}"#)
            .when(needs_group),
        CommandSpec::new("people.deleteGroup", "Delete Group", "Contact Group", delete).params(ONE).when(needs_group),
        CommandSpec::new("people.selectGroup", "Open Contact Group", "People", select).pure().params(r#"{"id": number | null}"#),
        CommandSpec::new("people.listGroups", "Contact Groups", "People", list).pure().params(r#"{} — answers {"groups": [{"id", "name", "members": [{"name", "email", "contact"}]}]}"#),
        CommandSpec::new("people.emailGroup", "Email Group", "Contact Group › Communicate", email).pure().params(ONE).when(needs_group),
        CommandSpec::new("people.exportGroups", "Export Contact Groups", "File › Open & Export", export)
            .pure()
            .params(r#"{"ids"?: [number] (default: every group), "path"?: string} — .vcf, vCard 4.0 cards of KIND:group with one MEMBER for each member"#)
            .when(needs_group),
    ]
}

const ONE: &str = r#"{"id"?: number} (default: the selected group)"#;

fn needs_group(s: &Session) -> Option<&'static str> {
    if s.mailbox.contact_groups.is_empty() { Some("there are no contact groups") } else { None }
}

fn find(s: &Session, id: Id) -> Option<&ContactGroup> {
    s.mailbox.contact_groups.iter().find(|g| g.id == id)
}

/// The group whose name this is, when one is.
pub fn named<'a>(s: &'a Session, text: &str) -> Option<&'a ContactGroup> {
    s.mailbox.contact_groups.iter().find(|g| g.named(text))
}

/// The addresses a group stands for: each member's, as the contact it was taken from has it now
/// when that contact is still here.
pub fn members(s: &Session, g: &ContactGroup) -> Vec<Address> {
    g.members
        .iter()
        .map(|m| {
            m.contact.and_then(|c| s.mailbox.contact(c)).and_then(|c| c.address()).filter(Address::is_valid).unwrap_or_else(|| m.address.clone())
        })
        .collect()
}

/// A recipient line with the names of contact groups replaced by their members (each address
/// once). What isn't a group's name stays as it was typed.
pub fn expand_line(s: &Session, line: &str) -> String {
    if s.mailbox.contact_groups.is_empty() {
        return line.to_string();
    }
    let typed = Address::parse_list(line);
    if !typed.iter().any(|a| !a.is_valid() && named(s, &a.email).or_else(|| named(s, &a.name)).is_some()) {
        return line.to_string();
    }
    let mut out: Vec<Address> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |a: Address, out: &mut Vec<Address>| {
        if !a.is_valid() || seen.insert(a.email.trim().to_lowercase()) {
            out.push(a);
        }
    };
    for a in typed {
        match (!a.is_valid()).then(|| named(s, &a.email).or_else(|| named(s, &a.name))).flatten() {
            Some(g) => {
                for m in members(s, g) {
                    push(m, &mut out);
                }
            }
            None => push(a, &mut out),
        }
    }
    format_list(&out)
}

fn target(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = p::id(v, "id").or(s.sel.group).ok_or_else(|| fail("open a contact group first"))?;
    find(s, id).map(|g| g.id).ok_or_else(|| fail(format!("no contact group with id {id}")))
}

/// The members `contacts` and `addresses` name.
fn wanted(s: &Session, v: &Value) -> Result<Vec<GroupMember>, CmdError> {
    let mut out = Vec::new();
    if let Some(list) = v.get("contacts").and_then(Value::as_array) {
        for id in list.iter().take(mailcraft_model::MAX_MEMBERS).filter_map(Value::as_u64) {
            let c = s.mailbox.contact(id).ok_or_else(|| fail(format!("no contact with id {id}")))?;
            let address = c.address().filter(Address::is_valid).ok_or_else(|| fail(format!("{} has no e-mail address", c.name())))?;
            out.push(GroupMember { address, contact: Some(c.id) });
        }
    }
    if let Some(text) = p::str(v, "addresses") {
        for a in Address::parse_list(text).into_iter().take(mailcraft_model::MAX_MEMBERS) {
            if !a.is_valid() {
                return Err(fail(format!("\"{}\" isn't an e-mail address", a.email)));
            }
            // An address that is a contact's is that contact.
            let contact = s.mailbox.contact_by_email(&a.email).map(|c| c.id);
            out.push(GroupMember { address: a, contact });
        }
    }
    Ok(out)
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    if named(s, &name).is_some() {
        return Err(fail(format!("there is a contact group called \"{name}\" already")));
    }
    if s.mailbox.contact_groups.len() >= MAX_SAVED {
        return Err(fail("there are too many contact groups"));
    }
    let mut g = ContactGroup { name, notes: p::str(v, "notes").unwrap_or("").chars().take(10_000).collect(), ..Default::default() };
    for m in wanted(s, v)? {
        g.add(m);
    }
    g.id = s.mailbox.alloc_id();
    let (id, count) = (g.id, g.members.len());
    s.mailbox.contact_groups.push(g);
    s.sel.group = Some(id);
    Ok(json!({"id": id, "members": count}))
}

fn group_mut(s: &mut Session, id: Id) -> Result<&mut ContactGroup, CmdError> {
    s.mailbox.contact_groups.iter_mut().find(|g| g.id == id).ok_or_else(|| fail("the contact group is gone"))
}

fn edit(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let name = match p::str(v, "name") {
        Some(n) => {
            let n = clean_name(n).ok_or_else(|| bad("`name` is empty"))?;
            if named(s, &n).is_some_and(|other| other.id != id) {
                return Err(fail(format!("there is a contact group called \"{n}\" already")));
            }
            Some(n)
        }
        None => None,
    };
    let g = group_mut(s, id)?;
    if let Some(n) = name {
        g.name = n;
    }
    if let Some(n) = p::str(v, "notes") {
        g.notes = n.chars().take(10_000).collect();
    }
    if let Some(c) = p::strings(v, "categories") {
        g.categories = c.iter().filter_map(|c| clean_name(c)).take(100).collect();
    }
    Ok(json!({"id": id, "name": g.name}))
}

fn add_members(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let more = wanted(s, v)?;
    if more.is_empty() {
        return Err(bad("name `contacts` or `addresses` to add"));
    }
    let g = group_mut(s, id)?;
    let added = more.into_iter().filter(|m| g.add(m.clone())).count();
    Ok(json!({"added": added, "members": g.members.len()}))
}

fn remove_members(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let emails = p::strings(v, "emails").ok_or_else(|| bad("`emails` (a list) is required"))?;
    let g = group_mut(s, id)?;
    let before = g.members.len();
    g.members.retain(|m| !emails.iter().any(|e| e.trim().eq_ignore_ascii_case(m.address.email.trim())));
    Ok(json!({"removed": before - g.members.len(), "members": g.members.len()}))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    s.mailbox.contact_groups.retain(|g| g.id != id);
    if s.sel.group == Some(id) {
        s.sel.group = None;
    }
    Ok(json!({"deleted": id}))
}

fn select(s: &mut Session, v: &Value) -> CmdResult {
    s.sel.group = match p::id(v, "id") {
        Some(id) => Some(find(s, id).map(|g| g.id).ok_or_else(|| fail(format!("no contact group with id {id}")))?),
        None => None,
    };
    if s.sel.group.is_some() {
        s.sel.contact = None;
    }
    Ok(json!({"group": s.sel.group}))
}

fn group_json(s: &Session, g: &ContactGroup) -> Value {
    let members: Vec<Value> =
        g.members.iter().zip(members(s, g)).map(|(m, a)| json!({"name": a.name, "email": a.email, "contact": m.contact})).collect();
    json!({"id": g.id, "name": g.name, "notes": g.notes, "categories": g.categories, "members": members})
}

fn list(s: &mut Session, _: &Value) -> CmdResult {
    let mut groups: Vec<&ContactGroup> = s.mailbox.contact_groups.iter().collect();
    groups.sort_by_cached_key(|g| g.name.to_lowercase());
    Ok(json!({"groups": groups.into_iter().map(|g| group_json(s, g)).collect::<Vec<_>>(), "selected": s.sel.group}))
}

fn email(s: &mut Session, v: &Value) -> CmdResult {
    let id = target(s, v)?;
    let name = find(s, id).map(|g| g.name.clone()).unwrap_or_default();
    let registry = s.registry.clone();
    let spec = registry.get("mail.new").ok_or_else(|| fail("mail.new is missing"))?;
    (spec.run)(s, &json!({"to": name}))
}

fn export(s: &mut Session, v: &Value) -> CmdResult {
    let groups: Vec<&ContactGroup> = match p::ids(v) {
        Some(ids) => ids.iter().map(|id| find(s, *id).ok_or_else(|| fail(format!("no contact group with id {id}")))).collect::<Result<_, _>>()?,
        None => s.mailbox.contact_groups.iter().collect(),
    };
    if groups.is_empty() {
        return Err(fail("there are no contact groups to export"));
    }
    let name = match groups.as_slice() {
        [one] => file_name(&one.name, "vcf"),
        _ => "contact groups.vcf".to_string(),
    };
    let count = groups.len();
    let mut r = deliver(v, &name, mailcraft_vcard::write_groups(&groups).into_bytes())?;
    if let Some(o) = r.as_object_mut() {
        o.insert("groups".into(), json!(count));
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{AccountKind, FolderKind};
    use serde_json::json;

    use crate::{Session, sample};

    /// The sample, without its own contact group.
    fn session() -> Session {
        let now = 1_791_993_600;
        let mut s = Session::at(sample::mailbox(now, 0), now, 0);
        s.mailbox.contact_groups.clear();
        s
    }

    #[test]
    fn the_samples_group_writes_to_its_people() {
        let now = 1_791_993_600;
        let mut s = Session::at(sample::mailbox(now, 0), now, 0);
        let group = s.mailbox.contact_groups[0].id;
        s.run("people.emailGroup", &json!({"id": group})).unwrap();
        let line = crate::cmd::groups::expand_line(&s, &s.compose.as_ref().unwrap().to);
        assert_eq!(line.matches("@lumen.example").count(), 3, "{line}");
    }

    #[test]
    fn a_group_on_the_to_line_becomes_its_members_when_sent() {
        let mut s = session();
        let contact = s.mailbox.contacts.values().find(|c| c.address().is_some()).map(|c| (c.id, c.address().unwrap())).unwrap();
        let made = s
            .run("people.newGroup", &json!({"name": "Book club", "contacts": [contact.0], "addresses": "Zed <zed@example.org>; ZED@example.org"}))
            .unwrap();
        assert_eq!(made["members"], 2, "each address once");
        let group = made["id"].as_u64().unwrap();
        assert_eq!(s.sel.group, Some(group));
        assert!(s.run("people.newGroup", &json!({"name": "book CLUB"})).is_err(), "names are unique");
        s.run("mail.new", &json!({"to": "book club; carol@example.org", "subject": "Next month"})).unwrap();
        // Check Names leaves a group's name alone.
        assert_eq!(s.run("compose.checkNames", &json!({})).unwrap()["unresolved"], json!([]));
        let account = s.mailbox.accounts[0].id;
        let sent = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
        let m = s.mailbox.message(sent).unwrap();
        let to: Vec<&str> = m.to.iter().map(|a| a.email.as_str()).collect();
        assert_eq!(to, [contact.1.email.as_str(), "zed@example.org", "carol@example.org"]);
        let _ = (account, AccountKind::Local, FolderKind::Sent);
    }

    #[test]
    fn members_follow_their_contact_and_are_added_and_removed() {
        let mut s = session();
        let cid = s.mailbox.contacts.values().find(|c| c.address().is_some()).map(|c| c.id).unwrap();
        let group = s.run("people.newGroup", &json!({"name": "Team"})).unwrap()["id"].as_u64().unwrap();
        s.run("people.addMembers", &json!({"contacts": [cid], "addresses": "x@example.org"})).unwrap();
        assert!(s.run("people.addMembers", &json!({"addresses": "not an address"})).is_err());
        assert!(s.run("people.addMembers", &json!({})).is_err());
        // The contact's address changes: the group follows.
        s.mailbox.contact_mut(cid).unwrap().emails[0].value = "moved@example.org".into();
        let listed = s.run("people.listGroups", &json!({})).unwrap();
        assert_eq!(listed["groups"][0]["members"][0]["email"], "moved@example.org");
        assert_eq!(s.run("people.removeMembers", &json!({"emails": ["X@example.org"]})).unwrap()["removed"], 1);
        s.run("people.editGroup", &json!({"name": "The team", "notes": "n"})).unwrap();
        // The vCard export is a group card.
        let out = s.run("people.exportGroups", &json!({})).unwrap();
        let bytes = crate::cmd::base64_decode(out["data"].as_str().unwrap()).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("KIND:group") && text.contains("FN:The team"), "{text}");
        // Emailing the group starts a message to its name.
        s.run("people.emailGroup", &json!({})).unwrap();
        assert_eq!(s.compose.as_ref().unwrap().to, "The team");
        s.compose = None;
        s.run("people.deleteGroup", &json!({"id": group})).unwrap();
        assert!(s.mailbox.contact_groups.is_empty() && s.sel.group.is_none());
    }
}
