// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's search (#1725): `FindItem` of one folder, held to the schema of [MS-OXWSSRCH],
//! over the made-up mail of [`State::items`].
//!
//! A `QueryString` is read as the clauses a client writes (joined by `AND`; `from:`, `to:`, `cc:`,
//! `subject:`, `body:`, `attachment:`, `hasattachment:true`, `importance:high` or `low`, a word
//! or a text in quotes), a `Restriction` as `And`, `Or`, `Contains` and `IsEqualTo` over the
//! properties a client asks about. Either matches without regard to letter case. The items come
//! newest first, a page at a time (`IndexedPageItemView`), each with the properties the shape
//! names and no others.

use crate::soap::{MESSAGES, TYPES};
use crate::standin::{Check, Refusal, State, error_message, invalid, response};
use crate::standin_mail::{Item, ok_message};
use crate::xml::{Node, escape};

fn people(list: &[(String, String)]) -> String {
    list.iter().map(|(name, address)| format!("{name} {address} ")).collect::<String>().to_lowercase()
}

/// What an item says in the part of it a search names.
fn part(item: &Item, name: &str) -> String {
    match name {
        "subject" | "item:Subject" => item.subject.to_lowercase(),
        "body" | "item:Body" => format!("{} {} {}", item.preview, item.body.1, String::from_utf8_lossy(&item.mime)).to_lowercase(),
        "from" | "message:From" => people(std::slice::from_ref(&item.from)),
        "to" | "item:DisplayTo" => people(&item.to),
        "cc" | "item:DisplayCc" => people(&item.cc),
        "attachment" => item.files.iter().map(|(_, name, _)| format!("{name} ")).collect::<String>().to_lowercase(),
        _ => String::new(),
    }
}

/// Whether `item` matches one clause of a query string.
fn clause_matches(item: &Item, clause: &str) -> Result<bool, Refusal> {
    let clause = clause.trim();
    let (property, value) = match clause.split_once(':') {
        Some((property, value)) if !property.contains(['"', ' ']) => (property, value),
        _ => ("", clause),
    };
    let value = value.trim().trim_matches('"').to_lowercase();
    if value.is_empty() {
        return Err(Refusal { code: "ErrorInvalidRequest", why: "a clause of the query has no value".into() });
    }
    Ok(match property {
        "" => ["subject", "body", "from", "to", "cc"].iter().any(|name| part(item, name).contains(&value)),
        "from" | "to" | "cc" | "subject" | "body" | "attachment" => part(item, property).contains(&value),
        "hasattachment" => item.attachments == (value == "true"),
        "importance" => item.importance.to_lowercase() == value,
        _ => return Err(Refusal { code: "ErrorInvalidRequest", why: "the query names a property this server doesn't search".into() }),
    })
}

/// Whether `item` matches a restriction's element.
fn restriction_matches(item: &Item, node: &Node) -> Result<bool, Refusal> {
    let field = |node: &Node| -> Result<String, Refusal> {
        let uri = node.child_in(TYPES, "FieldURI").ok_or_else(|| invalid(format!("{} names a property", node.name)))?;
        Check::is(uri, TYPES, "FieldURI", &["FieldURI"])?;
        Ok(Check::attribute(uri, "FieldURI")?.to_owned())
    };
    match node.name.as_str() {
        "And" | "Or" if *node.namespace == *TYPES => {
            Check::no_text(node)?;
            if node.children.len() < 2 {
                return Err(invalid(format!("{} joins at least two", node.name)));
            }
            let mut all = node.name == "And";
            for child in &node.children {
                let one = restriction_matches(item, child)?;
                all = if node.name == "And" { all && one } else { all || one };
            }
            Ok(all)
        }
        "Contains" => {
            Check::is(node, TYPES, "Contains", &["ContainmentMode", "ContainmentComparison"])?;
            let name = field(node)?;
            let constant = node.child_in(TYPES, "Constant").ok_or_else(|| invalid("Contains needs a Constant"))?;
            Check::is(constant, TYPES, "Constant", &["Value"])?;
            let text = Check::attribute(constant, "Value")?.to_lowercase();
            if !["item:Subject", "item:Body", "message:From", "item:DisplayTo", "item:DisplayCc"].contains(&name.as_str()) {
                return Err(invalid(format!("{name} is no property this server searches in")));
            }
            Ok(part(item, &name).contains(&text))
        }
        "IsEqualTo" => {
            Check::is(node, TYPES, "IsEqualTo", &[])?;
            let name = field(node)?;
            let value = node
                .child_in(TYPES, "FieldURIOrConstant")
                .and_then(|holder| holder.child_in(TYPES, "Constant"))
                .and_then(|constant| constant.attribute("Value"))
                .ok_or_else(|| invalid("IsEqualTo needs a FieldURIOrConstant with a Constant"))?;
            match name.as_str() {
                "item:HasAttachments" => Ok(item.attachments == (value == "true")),
                "item:Importance" => Ok(item.importance == value),
                _ => Err(invalid(format!("{name} is no property this server compares"))),
            }
        }
        other => Err(invalid(format!("a restriction has no place for {other}"))),
    }
}

/// An item with the properties `extra` names, and its id.
fn found_xml(item: &Item, extra: &[String]) -> String {
    let mut xml = format!("<t:Message><t:ItemId Id=\"{}\" ChangeKey=\"{}\"/>", escape(&item.id), escape(&item.change_key));
    for uri in extra {
        let one = match uri.as_str() {
            "item:Subject" => format!("<t:Subject>{}</t:Subject>", escape(&item.subject)),
            "item:DateTimeReceived" => format!("<t:DateTimeReceived>{}</t:DateTimeReceived>", item.received),
            "item:DateTimeSent" => format!("<t:DateTimeSent>{}</t:DateTimeSent>", item.sent),
            "item:Size" => format!("<t:Size>{}</t:Size>", item.mime.len()),
            "item:Importance" => format!("<t:Importance>{}</t:Importance>", item.importance),
            "item:HasAttachments" => format!("<t:HasAttachments>{}</t:HasAttachments>", item.attachments),
            "item:IsDraft" => format!("<t:IsDraft>{}</t:IsDraft>", item.draft),
            "item:Categories" if !item.categories.is_empty() => {
                let all: String = item.categories.iter().map(|c| format!("<t:String>{}</t:String>", escape(c))).collect();
                format!("<t:Categories>{all}</t:Categories>")
            }
            "item:Preview" => format!("<t:Preview>{}</t:Preview>", escape(&item.preview)),
            "item:Flag" => format!("<t:Flag><t:FlagStatus>{}</t:FlagStatus></t:Flag>", if item.flagged { item.flag.as_str() } else { "NotFlagged" }),
            // (As the service does in a list: the name, and no address.)
            "message:From" => format!("<t:From><t:Mailbox><t:Name>{}</t:Name></t:Mailbox></t:From>", escape(&item.from.0)),
            "message:IsRead" => format!("<t:IsRead>{}</t:IsRead>", item.read),
            "message:InternetMessageId" => format!("<t:InternetMessageId>{}</t:InternetMessageId>", escape(&item.message_id)),
            _ => String::new(),
        };
        xml.push_str(&one);
    }
    xml.push_str("</t:Message>");
    xml
}

/// Whether a `FindItem` looks for the copy of a message by the marker it carries (an extended
/// property compared with `IsEqualTo`), which `standin_write` answers, and is no search.
pub(crate) fn by_marker(operation: &Node) -> bool {
    operation
        .child_in(MESSAGES, "Restriction")
        .and_then(|restriction| restriction.child_in(TYPES, "IsEqualTo"))
        .is_some_and(|equal| equal.child_in(TYPES, "ExtendedFieldURI").is_some())
}

impl State {
    pub(crate) fn search_items(&self, operation: &Node) -> Result<(u16, String, String), Refusal> {
        Check::is(operation, MESSAGES, "FindItem", &["Traversal"])?;
        if Check::attribute(operation, "Traversal")? != "Shallow" {
            return Err(invalid("Traversal is Shallow here"));
        }
        let order = ["ItemShape", "IndexedPageItemView", "Restriction", "SortOrder", "ParentFolderIds", "QueryString"];
        let parts = Check::children(operation, MESSAGES, &order)?;
        let at = |name: &str| order.iter().position(|have| *have == name).and_then(|i| parts.get(i).copied().flatten());
        let shape = Check::required(operation, at("ItemShape"), "ItemShape")?;
        let (base, _, extra) = Self::item_shape(shape, false)?;
        if base != "IdOnly" {
            return Err(invalid("this stand-in lists with the IdOnly shape and the properties named"));
        }
        if extra.iter().any(|e| e == "message:ToRecipients" || e == "message:CcRecipients") {
            let message = error_message("FindItem", "ErrorInvalidPropertyForOperation");
            return Ok((200, "ErrorInvalidPropertyForOperation".into(), response("FindItem", &message)));
        }
        let names_preview = extra.iter().any(|e| e == "item:Preview" || e == "item:Flag");
        if self.exchange2010 && names_preview {
            return Err(invalid("the schema has no such property"));
        }
        if self.lacks_preview && names_preview {
            let message = error_message("FindItem", "ErrorInvalidPropertyRequest");
            return Ok((200, "ErrorInvalidPropertyRequest".into(), response("FindItem", &message)));
        }
        let page = Check::required(operation, at("IndexedPageItemView"), "IndexedPageItemView")?;
        Check::is(page, MESSAGES, "IndexedPageItemView", &["MaxEntriesReturned", "Offset", "BasePoint"])?;
        Check::empty(page)?;
        if Check::attribute(page, "BasePoint")? != "Beginning" {
            return Err(invalid("BasePoint is Beginning here"));
        }
        let number = |name: &str| Check::attribute(page, name)?.parse::<usize>().map_err(|_| invalid(format!("{name} is a number")));
        let (offset, mut max) = (number("Offset")?, number("MaxEntriesReturned")?);
        if max == 0 {
            return Err(invalid("MaxEntriesReturned is at least 1"));
        }
        if self.find_page > 0 {
            max = max.min(self.find_page);
        }
        if let Some(sort) = at("SortOrder") {
            Check::is(sort, MESSAGES, "SortOrder", &[])?;
            let [by] = sort.children.as_slice() else { return Err(invalid("SortOrder names one order here")) };
            Check::is(by, TYPES, "FieldOrder", &["Order"])?;
            if Check::attribute(by, "Order")? != "Descending" {
                return Err(invalid("this stand-in lists newest first"));
            }
        }
        let folders = Check::required(operation, at("ParentFolderIds"), "ParentFolderIds")?;
        Check::is(folders, MESSAGES, "ParentFolderIds", &[])?;
        let [folder] = folders.children.as_slice() else { return Err(invalid("ParentFolderIds names one folder here")) };
        Check::is(folder, TYPES, "FolderId", &["Id", "ChangeKey"])?;
        Check::empty(folder)?;
        let Some(folder) = self.folder(Check::attribute(folder, "Id")?, false) else {
            return Ok((200, "ErrorFolderNotFound".into(), response("FindItem", &error_message("FindItem", "ErrorFolderNotFound"))));
        };
        let (query, restriction) = (at("QueryString"), at("Restriction"));
        if query.is_some() && self.lacks_query_string {
            return Err(invalid("the schema has no QueryString"));
        }
        let mut found: Vec<&Item> = Vec::new();
        for item in self.items.iter().filter(|item| item.folder == folder.id) {
            let matches = match (query, restriction) {
                (Some(query), None) => {
                    Check::is(query, MESSAGES, "QueryString", &["ResetCache", "ReturnHighlightTerms", "ReturnDeletedItems"])?;
                    Check::no_children(query)?;
                    let mut all = true;
                    for clause in query.text.split(" AND ") {
                        all &= clause_matches(item, clause)?;
                    }
                    all
                }
                (None, Some(restriction)) => {
                    Check::is(restriction, MESSAGES, "Restriction", &[])?;
                    let [inner] = restriction.children.as_slice() else { return Err(invalid("Restriction holds one expression")) };
                    restriction_matches(item, inner)?
                }
                (Some(_), Some(_)) => return Err(invalid("a search is a QueryString or a Restriction, not both")),
                (None, None) => true,
            };
            if matches {
                found.push(item);
            }
        }
        // Newest first; of two that came at the same time, the later one in the mailbox first.
        found.reverse();
        found.sort_by(|a, b| b.received.cmp(&a.received));
        let total = found.len();
        let listed: String = found.iter().skip(offset).take(max).map(|item| found_xml(item, &extra)).collect();
        let reached = offset.saturating_add(max).min(total.max(offset));
        let inner = format!(
            "<m:RootFolder IndexedPagingOffset=\"{reached}\" TotalItemsInView=\"{total}\" IncludesLastItemInRange=\"{}\"><t:Items>{listed}</t:Items></m:RootFolder>",
            reached >= total
        );
        Ok((200, "NoError".into(), response("FindItem", &ok_message("FindItem", &inner))))
    }
}
