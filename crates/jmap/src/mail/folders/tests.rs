// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mailboxes of a server as the folders of the engine: roles, names, ids, the tree and its limits.

use std::collections::HashMap;

use serde_json::json;

use super::*;

fn answer(list: Vec<Value>) -> Value {
    json!({"accountId": "A1", "state": "1", "list": list, "notFound": []})
}

fn mailbox(id: &str, name: &str, parent: Option<&str>, role: Option<&str>) -> Value {
    json!({"id": id, "name": name, "parentId": parent, "role": role, "totalEmails": 4, "unreadEmails": 1})
}

fn ids(listing: &FolderListing) -> Vec<&str> {
    listing.folders.iter().map(|f| f.id.as_str()).collect()
}

fn parent_of<'a>(listing: &'a FolderListing, id: &str) -> &'a str {
    listing.folders.iter().find(|f| f.id == id).map_or("?", |f| f.parent.as_str())
}

/// What the engine needs of any listing: parents first, no mailbox its own ancestor, none too deep.
fn is_a_tree(listing: &FolderListing) {
    let mut at: HashMap<&str, usize> = HashMap::new();
    for (n, folder) in listing.folders.iter().enumerate() {
        assert!(at.insert(folder.id.as_str(), n).is_none(), "{} twice", folder.id);
        assert!(folder.parent.is_empty() || at.get(folder.parent.as_str()).is_some_and(|parent| *parent < n), "{} before its parent", folder.id);
    }
    for folder in &listing.folders {
        let mut depth = 1;
        let mut up = folder;
        while let Some(parent) = listing.folders.iter().find(|f| f.id == up.parent) {
            (depth, up) = (depth + 1, parent);
        }
        assert!(depth <= MAX_DEPTH, "{} is {depth} levels down", folder.id);
    }
}

#[test]
fn a_role_gives_the_folder_its_kind_and_a_view_is_left_out() {
    let list = vec![
        mailbox("a", "Inbox", None, Some("inbox")),
        mailbox("b", "Sent Mail", None, Some("sent")),
        mailbox("c", "Notes", None, Some("drafts")),
        mailbox("d", "Bin", None, Some("trash")),
        mailbox("e", "Spam", None, Some("junk")),
        mailbox("f", "Old", None, Some("archive")),
        mailbox("g", "Mine", None, None),
        mailbox("h", "Odd", None, Some("snoozed")),
        mailbox("i", "Shouting", None, Some("INBOX")),
        mailbox("j", "Every message", None, Some("all")),
        mailbox("k", "Starred", None, Some("flagged")),
        mailbox("l", "Worth reading", None, Some("important")),
        mailbox("m", "Followed", None, Some("subscribed")),
        mailbox("n", "Empty role", None, Some("")),
        json!({"id": "o", "name": "No role at all"}),
    ];
    let listing = listing(&answer(list)).unwrap();
    let kinds: Vec<(&str, FolderKind)> = listing.folders.iter().map(|f| (f.id.as_str(), f.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("a", FolderKind::Inbox),
            ("b", FolderKind::Sent),
            ("c", FolderKind::Drafts),
            ("d", FolderKind::Deleted),
            ("e", FolderKind::Junk),
            ("f", FolderKind::Archive),
            ("g", FolderKind::Custom),
            ("h", FolderKind::Custom),
            // (Roles are lower case in the standard; a server that shouts is understood, and one Inbox is enough.)
            ("i", FolderKind::Custom),
            ("n", FolderKind::Custom),
            ("o", FolderKind::Custom),
        ]
    );
    assert!(listing.complete, "views are not mailboxes the server is missing from the list");
    assert_eq!(listing.folders.iter().map(|f| (f.total, f.unread)).next(), Some((4, 1)));
}

#[test]
fn a_role_that_a_second_mailbox_claims_is_that_ones_no_more() {
    let list =
        vec![mailbox("a", "First", None, Some("inbox")), mailbox("b", "Second", None, Some("inbox")), mailbox("c", "Sent", None, Some("sent"))];
    let listing = listing(&answer(list)).unwrap();
    let kinds: Vec<FolderKind> = listing.folders.iter().map(|f| f.kind).collect();
    assert_eq!(kinds, [FolderKind::Inbox, FolderKind::Custom, FolderKind::Sent]);
}

#[test]
fn names_are_cleaned_and_a_mailbox_with_none_left_is_passed_over() {
    let long = "x".repeat(300);
    let list = vec![
        mailbox("a", "  Quiet\u{7}\n folder \t", None, None),
        mailbox("b", &long, None, None),
        mailbox("c", "", None, None),
        mailbox("d", " \u{7}\n ", None, None),
        json!({"id": "e"}),
        json!({"id": "f", "name": 7}),
        mailbox("g", "Kept", None, None),
    ];
    let listing = listing(&answer(list)).unwrap();
    assert_eq!(ids(&listing), ["a", "b", "g"]);
    assert_eq!(listing.folders[0].name, "Quiet folder");
    assert_eq!(listing.folders[1].name.chars().count(), mailcraft_model::MAX_NAME);
    assert!(!listing.complete, "a mailbox the server has was not listed: nothing is removed for being missing");
    // A name keeps its slashes: the tree comes from the parents, not from the name.
    let slashes = self::listing(&answer(vec![mailbox("a", "Work/Home\\Away", None, None)])).unwrap();
    assert_eq!(slashes.folders[0].name, "Work/Home\\Away");
}

#[test]
fn an_id_that_is_none_passes_the_mailbox_over_and_an_id_twice_is_one_mailbox() {
    let list = vec![
        mailbox("a", "One", None, None),
        mailbox("a", "One again", None, None),
        mailbox("b c", "Space", None, None),
        mailbox("", "Empty", None, None),
        mailbox("d/e", "Slash", None, None),
        json!({"name": "No id"}),
        json!({"id": 5, "name": "A number"}),
        json!(null),
        json!("a string"),
        mailbox(&"i".repeat(256), "Too long", None, None),
        mailbox(&"i".repeat(255), "Long enough", None, None),
    ];
    let listing = listing(&answer(list)).unwrap();
    assert_eq!(listing.folders.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["One", "Long enough"]);
    assert!(!listing.complete);
    // An id twice alone is not a mailbox lost.
    assert!(self::listing(&answer(vec![mailbox("a", "One", None, None), mailbox("a", "Two", None, None)])).unwrap().complete);
}

#[test]
fn what_is_not_a_list_of_mailboxes_makes_no_listing() {
    for bad in [json!({}), json!({"list": null}), json!({"list": {}}), json!({"list": "x"}), json!([]), json!(null), json!("list")] {
        assert!(listing(&bad).is_none(), "{bad}");
    }
    // A list with no mailbox in it is a listing, but not a complete one: an account always has an Inbox, and a
    // server that has lost its folders for a moment answers just so. Neither is a list of views alone.
    let empty = listing(&answer(vec![])).unwrap();
    assert!(empty.folders.is_empty() && !empty.complete);
    let views = listing(&answer(vec![mailbox("a", "All mail", None, Some("all")), mailbox("b", "Starred", None, Some("flagged"))])).unwrap();
    assert!(views.folders.is_empty() && !views.complete);
}

#[test]
fn the_counts_a_server_states_are_the_ones_that_are_numbers_of_the_mailboxes_that_are_listed() {
    let entry = |id: &str, total: Value| json!({"id": id, "name": id, "totalEmails": total});
    let list = json!({"list": [
        entry("a", json!(0)), entry("b", json!(7)), entry("c", json!(null)), entry("d", json!("5")), entry("e", json!(-1)),
        entry("f", json!(2.5)), entry("g", json!(u64::MAX)), json!({"id": "h", "name": "No count"}), entry("bad id", json!(3)),
        json!({"name": "No id", "totalEmails": 4}), json!(null), json!("x"),
        // A view is not listed, and a mailbox that has no name is passed over: neither has a count here.
        json!({"id": "v", "name": "All mail", "role": "all", "totalEmails": 9}), json!({"id": "n", "name": " ", "totalEmails": 8}),
        // An id twice is the first of them, in the listing and in the counts.
        entry("a", json!(4)), entry("i", json!(1)), entry("i", json!(0)),
    ]});
    let (listing, counts) = listing_with_counts(&list).unwrap();
    let mut got: Vec<(&str, u32)> = counts.iter().map(|(id, n)| (id.as_str(), *n)).collect();
    got.sort_unstable();
    // Zero is a count the server states; no count, or none that is a number, is not one.
    assert_eq!(got, [("a", 0), ("b", 7), ("g", u32::MAX), ("i", 1)]);
    assert_eq!(listing.folders.iter().find(|f| f.id == "a").map(|f| f.total), Some(0));
    for bad in [json!({}), json!({"list": null}), json!({"list": {}}), json!(null), json!([])] {
        assert!(listing_with_counts(&bad).is_none(), "{bad}");
    }
}

#[test]
fn parents_come_before_the_mailboxes_in_them() {
    // Listed deepest first, and shuffled.
    let list = vec![
        mailbox("c", "Third", Some("b"), None),
        mailbox("x", "Other top", None, None),
        mailbox("b", "Second", Some("a"), None),
        mailbox("d", "Also third", Some("b"), None),
        mailbox("a", "First", None, None),
        mailbox("y", "Other child", Some("x"), None),
    ];
    let listing = listing(&answer(list)).unwrap();
    // By level, and in the server's order within a level.
    assert_eq!(ids(&listing), ["x", "a", "b", "y", "c", "d"]);
    assert_eq!((parent_of(&listing, "c"), parent_of(&listing, "b"), parent_of(&listing, "a"), parent_of(&listing, "y")), ("b", "a", "", "x"));
    is_a_tree(&listing);
}

#[test]
fn a_parent_that_is_not_listed_is_the_top() {
    let list = vec![
        mailbox("a", "Lost", Some("zz"), None),
        mailbox("b", "Bad parent", Some("not an id"), None),
        json!({"id": "c", "name": "Number parent", "parentId": 5}),
        mailbox("d", "Child of lost", Some("a"), None),
        mailbox("e", "Named after itself", Some("e"), None),
    ];
    let listing = listing(&answer(list)).unwrap();
    assert_eq!(listing.folders.iter().map(|f| f.parent.as_str()).collect::<Vec<_>>(), ["", "", "", "", "a"]);
    assert!(listing.complete);
    is_a_tree(&listing);
}

#[test]
fn a_mailbox_that_is_its_own_ancestor_is_cut_loose_where_the_walk_comes_round() {
    // a <- b <- c <- a: the walk from a comes round to a.
    let three = vec![mailbox("a", "A", Some("c"), None), mailbox("b", "B", Some("a"), None), mailbox("c", "C", Some("b"), None)];
    let listing = listing(&answer(three)).unwrap();
    assert_eq!((parent_of(&listing, "a"), parent_of(&listing, "b"), parent_of(&listing, "c")), ("", "a", "b"));
    assert_eq!(ids(&listing), ["a", "b", "c"]);
    is_a_tree(&listing);
    // Two of them, with a tail hanging on the cycle and another cycle behind it.
    let list = vec![
        mailbox("t", "Tail", Some("a"), None),
        mailbox("a", "A", Some("b"), None),
        mailbox("b", "B", Some("a"), None),
        mailbox("p", "P", Some("q"), None),
        mailbox("q", "Q", Some("p"), None),
        mailbox("s", "Self", Some("s"), None),
    ];
    let listing = self::listing(&answer(list)).unwrap();
    assert_eq!(listing.folders.len(), 6);
    assert_eq!(listing.folders.iter().filter(|f| f.parent.is_empty()).count(), 3, "one root in each cycle, the self-parent, and nothing else");
    is_a_tree(&listing);
}

#[test]
fn a_tree_deeper_than_32_levels_goes_to_the_top_at_the_32nd() {
    let list: Vec<Value> =
        (0..40).map(|n| mailbox(&format!("m{n}"), &format!("Level {n}"), (n > 0).then(|| format!("m{}", n - 1)).as_deref(), None)).collect();
    let listing = listing(&answer(list)).unwrap();
    assert_eq!(listing.folders.len(), 40);
    // m0 to m31 are 1 to 32 levels down; m32 would be the 33rd, and is a top mailbox with m33 to m39 in it.
    assert_eq!(parent_of(&listing, "m31"), "m30");
    assert_eq!(parent_of(&listing, "m32"), "");
    assert_eq!(parent_of(&listing, "m33"), "m32");
    assert_eq!(parent_of(&listing, "m39"), "m38");
    is_a_tree(&listing);
    // The same reversed, so that the deepest is met first.
    let mut reversed: Vec<Value> = (0..40).map(|n| mailbox(&format!("m{n}"), "x", (n > 0).then(|| format!("m{}", n - 1)).as_deref(), None)).collect();
    reversed.reverse();
    let again = self::listing(&answer(reversed)).unwrap();
    assert_eq!(parent_of(&again, "m32"), "");
    is_a_tree(&again);
}

#[test]
fn a_hundred_thousand_mailboxes_are_held_to_ten_thousand_and_the_list_is_not_complete() {
    let list: Vec<Value> = (0..mailcraft_model::sync::MAX_LISTED_FOLDERS + 5).map(|n| mailbox(&format!("m{n}"), "x", None, None)).collect();
    let listing = listing(&answer(list)).unwrap();
    assert_eq!(listing.folders.len(), mailcraft_model::sync::MAX_LISTED_FOLDERS);
    assert!(!listing.complete);
    // A chain of ten thousand is walked without recursion.
    let chain: Vec<Value> = (0..10_000).map(|n| mailbox(&format!("m{n}"), "x", (n > 0).then(|| format!("m{}", n - 1)).as_deref(), None)).collect();
    let listing = self::listing(&answer(chain)).unwrap();
    assert_eq!(listing.folders.len(), 10_000);
    is_a_tree(&listing);
}

#[test]
fn counts_are_held_to_what_a_folder_keeps() {
    let counts = |total: Value, unread: Value| {
        let list = answer(vec![json!({"id": "a", "name": "A", "totalEmails": total, "unreadEmails": unread})]);
        let folder = listing(&list).unwrap().folders.remove(0);
        (folder.total, folder.unread)
    };
    assert_eq!(counts(json!(12), json!(3)), (12, 3));
    assert_eq!(counts(json!(u64::MAX), json!(5_000_000_000u64)), (u32::MAX, u32::MAX));
    assert_eq!(counts(json!(-4), json!("7")), (0, 0));
    assert_eq!(counts(json!(2.5), json!(null)), (0, 0));
    assert_eq!(counts(json!({}), json!([1])), (0, 0));
}
