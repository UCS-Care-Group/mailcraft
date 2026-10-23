// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Favorites of Outlook against the strict stand-in, and the readers of the document and of
//! its ids against whatever can be thrown at them. Nothing here leaves this machine; every name
//! and every id is invented.

use std::cell::RefCell;

use proptest::prelude::*;

use super::xml::{Entry, Listed, MAX_ENTRIES, Malformed, favorites_of};
use super::*;
use crate::graph::standin::{CONFIGURATION_CANARY, Configuration, Favorite, StandIn, State, TOKEN};

/// An id that could be a folder's and is nobody's here: of another mailbox, or of a folder that
/// is long gone.
fn stranger(n: usize) -> String {
    format!("AAMkAGFub3RoZXItbWFpbGJveC1leGFtcGxlLm9yZwAuAAAAAAB7Q+x/Lm9SnT4qB3L0d9EBAFh2c/Kp+0aTmWvN4uYx1RoAAAIB{n:04}AAAA=")
}

/// Read the Favorites from a stand-in, noting how long was waited instead of waiting.
fn read(server: &StandIn) -> (Result<FavoritesDownload, NetError>, Vec<Duration>) {
    let pauses = RefCell::new(Vec::new());
    let read = favorites_at(&server.url(), TOKEN, &|pause| pauses.borrow_mut().push(pause));
    (read, pauses.into_inner())
}

fn names(download: &FavoritesDownload) -> Vec<String> {
    download.folders.iter().map(|folder| folder.path.join("/")).collect()
}

/// The mailbox of the tests: folders in the Inbox and at the top, three levels down at the
/// deepest, and Favorites that name seven of them among entries for folders that are gone.
fn mailbox() -> (StandIn, Vec<String>) {
    let server = StandIn::start().unwrap();
    let mut state = server.state();
    let inbox = state.well_known("inbox");
    let sent = state.well_known("sentitems");
    let projects = state.add_folder("Projects", Some(&inbox));
    let apollo = state.add_folder("Apollo", Some(&projects));
    let launch = state.add_folder("Launch", Some(&apollo));
    let receipts = state.add_folder("Receipts", None);
    let team = state.add_folder("Team Lumen", None);
    let travel = state.add_folder("Travel", Some(&receipts));
    // Outlook's order is the order of the indexes, whatever order the document has them in.
    state.add_favorite(&receipts, 12);
    state.add_favorite(&inbox, 0);
    state.add_favorite(&launch, 9);
    state.add_favorite(&sent, 3);
    state.add_favorite(&team, 15);
    state.add_favorite(&travel, 20);
    state.add_favorite(&projects, 5);
    let listed = state.favorites.get_or_insert_with(Vec::new);
    // Entries of long ago: the folders they name are not folders of this mailbox today.
    for n in 0..14 {
        listed.push(Favorite { name: "Inbox".into(), index: Some(1 + n), kind: "Folder".into(), folder_id: stranger(n as usize) });
    }
    drop(state);
    (server, vec![inbox, sent, projects, apollo, launch, receipts, team, travel])
}

#[test]
fn the_favorites_come_in_outlooks_order_and_the_stale_ones_are_left_out() {
    let (server, _) = mailbox();
    let (download, pauses) = read(&server);
    let download = download.unwrap();
    assert_eq!(download.outcome, FavoritesOutcome::Read);
    assert_eq!(
        names(&download),
        ["Inbox", "Sent Items", "Inbox/Projects", "Inbox/Projects/Apollo/Launch", "Receipts", "Team Lumen", "Receipts/Travel"]
    );
    let kinds: Vec<(FolderKind, FolderKind)> = download.folders.iter().map(|folder| (folder.kind, folder.root)).collect();
    assert_eq!(
        kinds,
        [
            (FolderKind::Inbox, FolderKind::Inbox),
            (FolderKind::Sent, FolderKind::Sent),
            (FolderKind::Custom, FolderKind::Inbox),
            (FolderKind::Custom, FolderKind::Inbox),
            (FolderKind::Custom, FolderKind::Custom),
            (FolderKind::Custom, FolderKind::Custom),
            (FolderKind::Custom, FolderKind::Custom),
        ]
    );
    assert_eq!((download.skipped, download.unresolved), (0, 14));
    assert!(pauses.is_empty());

    // What was asked, and how: the one item by its name, with the token and nothing else; then
    // folders, twenty to a batch, each by its id as one piece of an address and for three of
    // its properties. Nothing was written, and nothing was asked that the service doesn't take.
    let state = server.state();
    let first = &state.requests[0];
    assert_eq!((first.method.as_str(), first.status), ("GET", 200));
    assert_eq!(first.target, "/beta/me/mailFolders/root/userConfigurations/Aggregated.OwaUserConfiguration");
    assert_eq!(first.header("authorization"), format!("Bearer {TOKEN}"));
    let batches: Vec<_> = state.requests.iter().filter(|asked| asked.path.last().is_some_and(|last| last == "$batch")).collect();
    assert!(batches.iter().all(|batch| batch.method == "POST" && batch.target == "/v1.0/$batch" && batch.status == 200));
    let sizes: Vec<usize> = batches.iter().map(|batch| batch.json()["requests"].as_array().unwrap().len()).collect();
    // Twenty-one entries and the eight folders that are known by name; then the folders above.
    assert_eq!(sizes, [20, 9, 1]);
    let folders: Vec<_> = state.requests.iter().filter(|asked| asked.path.len() == 4 && asked.path[2] == "mailFolders").collect();
    assert_eq!(folders.len(), 30);
    for asked in &folders {
        assert_eq!(asked.method, "GET");
        assert_eq!(asked.query, [("$select".to_string(), "id,displayName,parentFolderId".to_string())], "{}", asked.target);
        assert!(!asked.target.contains('+') && asked.target.matches('/').count() == 4, "{}", asked.target);
    }
    assert!(state.writes().is_empty());
    let refused: Vec<u16> = state.requests.iter().map(|asked| asked.status).filter(|status| *status >= 400).collect();
    assert_eq!(refused, [404; 14]);
    // Of the folders above the favourites only those that weren't known already were asked
    // about: "Apollo" (the Inbox, "Projects" and "Receipts" had been told).
    assert_eq!(batches[2].json()["requests"].as_array().unwrap().len(), 1);

    // Nothing of the rest of the document, and no id, is in what comes back.
    let shown = format!("{download:?} {}", serde_json::to_string(&download).unwrap());
    assert!(!shown.contains(CONFIGURATION_CANARY) && !shown.contains("AQMk") && !shown.contains("AAMk"), "{shown}");
}

#[test]
fn a_renamed_inbox_is_still_the_inbox() {
    let (server, folders) = mailbox();
    {
        let mut state = server.state();
        let inbox = folders[0].clone();
        state.folders.iter_mut().find(|folder| folder.id == inbox).unwrap().name = "Posteingang".into();
        let sent = folders[1].clone();
        state.folders.iter_mut().find(|folder| folder.id == sent).unwrap().name = "Gesendete Elemente".into();
        // The document still calls it what it was called once: nobody reads that.
        state.prefixes = ("arrays".into(), "cfg".into());
    }
    let download = read(&server).0.unwrap();
    assert_eq!(names(&download)[..4], ["Posteingang", "Gesendete Elemente", "Posteingang/Projects", "Posteingang/Projects/Apollo/Launch"]);
    assert_eq!((download.folders[0].kind, download.folders[1].kind), (FolderKind::Inbox, FolderKind::Sent));
    assert_eq!((download.folders[2].kind, download.folders[2].root), (FolderKind::Custom, FolderKind::Inbox));
    assert_eq!(download.folders.len(), 7);
}

#[test]
fn entries_that_are_not_folders_are_skipped_and_one_folder_is_one_favourite() {
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        let inbox = state.well_known("inbox");
        let notes = state.add_folder("Notes & <Drafts>", None);
        let entry = |name: &str, index: Option<i64>, kind: &str, id: &str| Favorite {
            name: name.into(),
            index,
            kind: kind.into(),
            folder_id: if id.is_empty() { String::new() } else { State::configuration_id(id) },
        };
        state.favorites = Some(vec![
            // A person, a category and who knows what: not folders.
            entry("Bo Marsh", Some(0), "Persona", &inbox),
            entry("Red category", Some(1), "Category", ""),
            entry("", Some(2), "", &notes),
            // The same folder twice, the second time in another case of the word.
            entry("Notes", Some(7), "Folder", &notes),
            entry("Notes", Some(4), "folder", &notes),
            // Two with the same place: in the order the document has them. One with none: last.
            entry("Inbox", None, "Folder", &inbox),
            // A folder entry that names no folder, and one whose id can't be one.
            entry("Nowhere", Some(5), "Folder", ""),
            Favorite { name: "Odd".into(), index: Some(6), kind: "Folder".into(), folder_id: "../../me/messages?$top=999".into() },
        ]);
    }
    let download = read(&server).0.unwrap();
    assert_eq!(names(&download), ["Notes & <Drafts>", "Inbox"]);
    assert_eq!((download.skipped, download.unresolved), (3, 2));
    // What can't be an id was never put into an address.
    let state = server.state();
    assert!(state.requests.iter().all(|asked| !asked.target.contains("messages") && !asked.body.contains("messages")));
}

#[test]
fn a_mailbox_without_the_list_has_none_and_that_is_no_error() {
    // The mailbox has no such item (nobody has opened Outlook on the web for it).
    let server = StandIn::start().unwrap();
    server.state().configuration = Configuration::Missing;
    let download = read(&server).0.unwrap();
    assert_eq!((download.outcome, download.folders.len()), (FavoritesOutcome::Missing, 0));
    assert_eq!(server.state().requests.len(), 1);

    // The item is there, and has no list of Favorites in it: the same, and nothing more is asked.
    let server = StandIn::start().unwrap();
    assert_eq!(read(&server).0.unwrap().outcome, FavoritesOutcome::Missing);
    assert_eq!(server.state().requests.len(), 1);

    // A list with nothing in it is a list: Outlook shows no Favorites.
    let server = StandIn::start().unwrap();
    server.state().favorites = Some(Vec::new());
    let download = read(&server).0.unwrap();
    assert_eq!((download.outcome, download.folders.len(), download.skipped, download.unresolved), (FavoritesOutcome::Read, 0, 0, 0));
}

#[test]
fn what_graph_says_when_the_favorites_may_not_be_read() {
    // The sign-in doesn't cover the configuration item: for the host to note, and to offer.
    let (server, _) = mailbox();
    server.state().configuration = Configuration::Denied;
    let refused = read(&server).0.unwrap_err();
    assert!(matches!(refused, NetError::Forbidden(_)), "{refused:?}");
    assert_eq!(refused.to_string(), "this account may not read its Favorites through Microsoft Graph (ErrorAccessDenied)");
    assert_eq!(server.state().requests.len(), 1);
    // Or the folders: the same kind of no.
    let (server, _) = mailbox();
    server.state().folders_denied = true;
    let refused = read(&server).0.unwrap_err();
    assert!(matches!(refused, NetError::Forbidden(_)), "{refused:?}");
    assert!(refused.to_string().contains("may not read its folders"), "{refused}");
    // A token that is no longer taken, and none at all (nothing is asked then).
    let (server, _) = mailbox();
    server.state().token = "another".into();
    assert!(read(&server).0.unwrap_err().is_refused_sign_in());
    let (server, _) = mailbox();
    assert!(favorites_at(&server.url(), "", &|_| {}).is_err());
    assert!(server.state().requests.is_empty());
    // Nothing of the token in any of it.
    server.state().configuration = Configuration::Denied;
    let refused = read(&server).0.unwrap_err();
    assert!(!format!("{refused} {refused:?}").contains(TOKEN));
}

#[test]
fn a_question_that_is_told_to_come_back_later_is_asked_once_more() {
    // Three of the first batch are told "later", with how long: they are asked again after
    // that long, and the list is whole and in order.
    let (server, _) = mailbox();
    server.state().throttle_folders = 3;
    let (download, pauses) = read(&server);
    let download = download.unwrap();
    assert_eq!(pauses, [Duration::from_secs(1)]);
    assert_eq!(names(&download).len(), 7);
    assert_eq!(names(&download)[..3], ["Inbox", "Sent Items", "Inbox/Projects"]);
    assert_eq!(download.unresolved, 14);
    let state = server.state();
    let sizes: Vec<usize> = state
        .requests
        .iter()
        .filter(|asked| asked.path.last().is_some_and(|last| last == "$batch"))
        .map(|batch| batch.json()["requests"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, [20, 3, 9, 1], "only what was told to come back is asked again");
    drop(state);

    // The whole batch is told "later": the same.
    let (server, _) = mailbox();
    server.state().fail_next.push(("POST".into(), 429, "TooManyRequests".into()));
    let (download, pauses) = read(&server);
    assert_eq!((names(&download.unwrap()).len(), pauses), (7, vec![Duration::from_secs(1)]));

    // Told "later" a second time: the read fails as a whole, rather than pass a list off as
    // whole that isn't. Nothing waits for longer than it may.
    let (server, _) = mailbox();
    server.state().throttle_folders = 50;
    let (download, pauses) = read(&server);
    assert!(matches!(download, Err(NetError::Busy { .. })), "{download:?}");
    assert_eq!(pauses.len(), 1);
    assert!(pauses.iter().all(|pause| *pause <= MAX_PAUSE));
    // The item itself is busy: nothing is waited for at all (the host asks again another time).
    let (server, _) = mailbox();
    server.state().fail_next.push(("GET".into(), 503, "ServiceUnavailable".into()));
    let (download, pauses) = read(&server);
    assert!(matches!(download, Err(NetError::Busy { .. })) && pauses.is_empty(), "{download:?}");
}

#[test]
fn a_document_that_is_huge_or_broken_is_an_error_and_never_a_list() {
    let whole = StandIn::start().unwrap().state().configuration_xml();
    let cut: String = {
        let (server, _) = mailbox();
        let document = server.state().configuration_xml();
        // Cut in the middle of the list.
        let at = document.find("OutlookFavorite>").unwrap() + 40;
        document[..at].to_string()
    };
    let nested = format!("{}{}", "<a>".repeat(200), "</a>".repeat(200));
    let with_dtd = format!("<!DOCTYPE x [<!ENTITY e \"{}\">]>{whole}", "boom ".repeat(10));
    for (document, what) in [
        (cut, "cut short"),
        ("not XML at all".to_string(), "no XML"),
        ("<a><b></a></b>".to_string(), "crossed"),
        (nested, "too deep"),
        (with_dtd, "a DTD"),
        ("\u{0}\u{1}<".to_string(), "control characters"),
    ] {
        let server = StandIn::start().unwrap();
        server.state().configuration_document = Some(document);
        match what {
            // (Text with no element in it has no list, and is no list.)
            "no XML" => assert_eq!(read(&server).0.unwrap().outcome, FavoritesOutcome::Missing),
            _ => {
                let refused = read(&server).0.unwrap_err();
                assert!(matches!(refused, NetError::Proto(ProtoError::Protocol(_))), "{what}: {refused:?}");
                assert_eq!(server.state().requests.len(), 1, "{what}: nothing more is asked");
            }
        }
    }
    // Larger than is read: the answer is not taken at all.
    let server = StandIn::start().unwrap();
    server.state().configuration_document = Some(format!("<a>{}</a>", "x".repeat(7 * 1024 * 1024)));
    assert!(read(&server).0.is_err());
    assert_eq!(server.state().requests.len(), 1);

    // A list of a thousand entries: as many as are read are asked about, the rest are counted.
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        let listed: Vec<Favorite> =
            (0..1_000usize).map(|n| Favorite { name: "x".into(), index: Some(n as i64), kind: "Folder".into(), folder_id: stranger(n) }).collect();
        state.favorites = Some(listed);
    }
    let download = read(&server).0.unwrap();
    assert_eq!((download.folders.len(), download.skipped as usize, download.unresolved as usize), (0, 1_000 - MAX_ENTRIES, MAX_ENTRIES));
}

#[test]
fn the_stand_in_takes_only_what_the_reference_documents() {
    let (server, folders) = mailbox();
    let graph = Graph::at(&server.url(), TOKEN).unwrap();
    let ask = |method: &str, path: &str| graph.ask(&graph.url(path), method, &[], None, ASK_TIMEOUT, MAX_BATCH).unwrap().status;
    let item = "/beta/me/mailFolders/root/userConfigurations/Aggregated.OwaUserConfiguration";
    assert_eq!(ask("GET", item), 200);
    // The item is asked for by its name, as it is, in the beta version, where it is kept; and it
    // is only ever read.
    assert_eq!(ask("GET", "/beta/me/mailFolders/root/userConfigurations"), 400);
    assert_eq!(ask("GET", &format!("{item}?%24select=xmlData")), 400);
    assert_eq!(ask("GET", &item.replace("/beta/", "/v1.0/")), 400);
    assert_eq!(ask("GET", &item.replace("/root/", "/inbox/")), 404);
    assert_eq!(ask("GET", "/beta/me/mailFolders/root/userConfigurations/OWA.SomethingElse"), 404);
    for method in ["PATCH", "DELETE", "POST"] {
        assert_eq!(ask(method, item), 403, "{method}");
    }
    // A folder is asked for by its id as one piece of an address, in Graph's own alphabet, or
    // by a well-known name; for properties a folder has, and with no other parameter.
    let inbox = &folders[0];
    let written = State::configuration_id(inbox);
    assert!(inbox.contains('-') && inbox.contains('_') && written.contains('/') && written.contains('+'));
    assert_eq!(ask("GET", &format!("/v1.0/me/mailFolders/{}", http::encode(inbox))), 200);
    assert_eq!(ask("GET", "/v1.0/me/mailFolders/inbox?%24select=id%2CdisplayName%2CparentFolderId"), 200);
    assert_eq!(ask("GET", &format!("/v1.0/me/mailFolders/{}", http::encode(&written))), 400);
    assert_eq!(ask("GET", &format!("/v1.0/me/mailFolders/{written}")), 400);
    assert_eq!(ask("GET", &format!("/v1.0/me/mailFolders/{}", http::encode(&stranger(1).replace('/', "-").replace('+', "_")))), 404);
    assert_eq!(ask("GET", "/v1.0/me/mailFolders/inbox?%24select=wellKnownName"), 400);
    assert_eq!(ask("GET", "/v1.0/me/mailFolders/inbox?%24filter=isHidden"), 400);
    assert_eq!(ask("GET", "/v1.0/me/mailFolders/Inbox"), 400);
    // (The special folders are not to be deleted.)
    assert_eq!(ask("DELETE", "/v1.0/me/mailFolders/inbox"), 403);
    // Without the token it was set up with, nothing is answered.
    let stranger = Graph::at(&server.url(), "somebody-elses-token").unwrap();
    let reply = stranger.ask(&stranger.url(item), "GET", &[], None, ASK_TIMEOUT, MAX_BATCH).unwrap();
    assert_eq!(reply.status, 401);
    // And the document is the one that was seen: a dictionary, the list one entry of it.
    let document = server.state().configuration_xml();
    assert!(document.starts_with("<AggregatedUserConfiguration.MementoClass ") && document.contains("<a:Key>OWA.OutlookFavorites</a:Key>"));
    assert_eq!(favorites_of(&document).unwrap().unwrap().entries.len(), 21);
}

fn entry(index: Option<i64>, kind: &str, folder_id: &str) -> Entry {
    Entry { index, kind: kind.into(), folder_id: folder_id.into() }
}

#[test]
fn the_document_is_read_by_local_names_whatever_its_prefixes() {
    let list = |xml: &str| favorites_of(xml).unwrap().map(|listed| listed.entries);
    let favorite = |p: &str, index: &str, kind: &str, id: &str| {
        format!(
            "<{p}OutlookFavorite><{p}DisplayName>Shown &amp; ignored</{p}DisplayName><{p}Index>{index}</{p}Index>\
             <{p}SingleValueSettings><{p}KeyValueType><{p}Key>ChangeKey</{p}Key><{p}Value>ck</{p}Value></{p}KeyValueType>\
             <{p}KeyValueType><{p}Key>FolderId</{p}Key><{p}Value>{id}</{p}Value></{p}KeyValueType></{p}SingleValueSettings>\
             <{p}Type>{kind}</{p}Type></{p}OutlookFavorite>"
        )
    };
    let document = |a: &str, b: &str, entries: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><!-- cached --><Root xmlns:q=\"urn:x\"><Parts>\
             <{a}Pair9><{a}Key>OWA.Other</{a}Key><{a}Value><{b}OutlookFavorite><{b}Index>99</{b}Index></{b}OutlookFavorite></{a}Value></{a}Pair9>\
             <{a}PairXyz_1><{a}Key> OWA.OutlookFavorites </{a}Key><{a}Value><{b}value>{entries}</{b}value></{a}Value></{a}PairXyz_1>\
             <{a}Pair9><{a}Key>After</{a}Key><{a}Value><{b}OutlookFavorite><{b}Index>98</{b}Index></{b}OutlookFavorite></{a}Value></{a}Pair9>\
             </Parts></Root>"
        )
    };
    let entries = format!("{}{}", favorite("b:", "1", "Folder", "AQMk/one+two=="), favorite("b:", " 0 ", "Persona", "x"));
    let want = vec![entry(Some(1), "Folder", "AQMk/one+two=="), entry(Some(0), "Persona", "x")];
    for (a, b) in [("a:", "b:"), ("", ""), ("arrays:", "q:"), ("b:", "a:")] {
        let entries = entries.replace("b:", b);
        assert_eq!(list(&document(a, b, &entries)), Some(want.clone()), "{a} {b}");
    }
    // Only the pair with the key: the lists of the others are not the list.
    assert_eq!(list(&document("a:", "b:", "")), Some(Vec::new()));
    // No such pair, a key without a value, a value that comes before its key: no list.
    assert_eq!(list("<Root><Parts><Pair><Key>OWA.Other</Key><Value/></Pair></Parts></Root>"), None);
    assert_eq!(list("<Root><Pair><Key>OWA.OutlookFavorites</Key></Pair><Pair><Key>x</Key><Value><OutlookFavorite/></Value></Pair></Root>"), None);
    assert_eq!(list("<Root><Pair><Value><OutlookFavorite/></Value><Key>OWA.OutlookFavorites</Key></Pair></Root>"), None);
    assert_eq!(list(""), None);
    // An empty value is an empty list.
    assert_eq!(list("<Root><Pair><Key>OWA.OutlookFavorites</Key><Value/></Pair></Root>"), Some(Vec::new()));

    // What a field may hold: characters by their numbers and XML's own five, in text or CDATA.
    let odd = "<P><Key>OWA.OutlookFavorites</Key><Value>\
               <OutlookFavorite><Index>&#49;&#x32;</Index><Type><![CDATA[Fol]]>der</Type>\
               <SingleValueSettings><KeyValueType><Key>Folder&#73;d</Key><Value>AQ&amp;Mk&lt;</Value></KeyValueType></SingleValueSettings></OutlookFavorite>\
               <OutlookFavorite><Index>3</Index><Type>&kind;</Type>\
               <SingleValueSettings><KeyValueType><Key>FolderId</Key><Value>&xxe;AQMk</Value></KeyValueType></SingleValueSettings></OutlookFavorite>\
               <OutlookFavorite><Index>12abc</Index><Type>Folder<x>nested</x></Type><SingleValueSettings>\
               <KeyValueType><Key>FolderId</Key><Value>first</Value></KeyValueType><KeyValueType><Key>FolderId</Key><Value>second</Value></KeyValueType>\
               </SingleValueSettings><OutlookFavorite><Index>77</Index></OutlookFavorite></OutlookFavorite>\
               </Value></P>";
    assert_eq!(
        list(odd),
        Some(vec![
            entry(Some(12), "Folder", "AQ&Mk<"),
            // A reference to who knows what is not looked up: the field it stands in is not taken.
            entry(Some(3), "", ""),
            // A number that isn't one is none; an element inside a field is no part of it; the
            // first of two settings of a name is the one; a favourite inside a favourite is nothing.
            entry(None, "Folder", "first"),
        ])
    );
    // A field longer than a field is: not taken.
    let long = format!(
        "<P><Key>OWA.OutlookFavorites</Key><Value><OutlookFavorite><Type>{}</Type><Index>4</Index></OutlookFavorite></Value></P>",
        "F".repeat(5_000)
    );
    assert_eq!(list(&long), Some(vec![entry(Some(4), "", "")]));
    // And what is no document.
    for broken in ["<P><Key>OWA.OutlookFavorites</Key><Value><OutlookFavorite>", "<a></b>", "<!DOCTYPE a><a/>", "<a>&</a>"] {
        assert_eq!(favorites_of(broken), Err(Malformed), "{broken}");
    }
}

#[test]
fn the_list_is_put_in_outlooks_order() {
    let id = |n: usize| State::configuration_id(&format!("AQMkAGV4YW1wbGUtb3JnLWZvbGRlcnMALgAAA5Nb_0Me0ZEvp07agdy9HfRAQBYdnP-Kp{n:04}=="));
    let listed = Listed {
        entries: vec![
            entry(Some(5), "Folder", &id(5)),
            entry(None, "Folder", &id(90)),
            entry(Some(2), "Folder", &id(21)),
            entry(Some(-1), "Folder", &id(0)),
            entry(Some(2), "Folder", &id(22)),
            entry(Some(1), "Group", &id(1)),
            entry(Some(i64::MAX), "Folder", &id(99)),
            entry(Some(2), "Folder", &id(21)),
            entry(None, "Folder", &id(91)),
            entry(Some(3), "Folder", "short"),
        ],
        more: 4,
    };
    let wanted = wanted(listed);
    let order: Vec<&str> = wanted.ids.iter().map(|id| &id[id.len() - 6..id.len() - 2]).collect();
    assert_eq!(order, ["0000", "0021", "0022", "0005", "0099", "0090", "0091"]);
    assert_eq!((wanted.skipped, wanted.unresolved), (5, 1));
    // As Graph writes them: nothing that would end a piece of an address, the padding kept.
    assert!(wanted.ids.iter().all(|id| !id.contains(['/', '+']) && id.ends_with("==") && id.contains('-') && id.contains('_')));
}

#[test]
fn ids_are_written_for_an_address_or_not_taken() {
    assert_eq!(graph_id(" AQMkADAwATM0/MDAA+MS1iNTcw== ").as_deref(), Some("AQMkADAwATM0-MDAA_MS1iNTcw=="));
    assert_eq!(graph_id("AQMkADAwATM0-MDAA_MS1iNTcw=").as_deref(), Some("AQMkADAwATM0-MDAA_MS1iNTcw="));
    let long = "A".repeat(600);
    for odd in [
        "",
        "short",
        "AQMkADAwATM0/MDAA+MS1iNTcw===",
        "AQMk=ADAwATM0/MDAA+MS1iNTcw",
        "AQMkADAwATM0/MDAA MS1iNTcwAAAA",
        "AQMkADAwATM0/MDAA?$top=1&x=AAAA",
        "AQMkADAwATM0/../../messagesAAAA.",
        "AQMkADAwATM0%2FMDAAMS1iNTcwAAAA",
        "AQMkADAwATM0\r\nMDAAMS1iNTcwAAAA",
        "ÄQMkADAwATM0MDAAMS1iNTcwAAAAAA",
        long.as_str(),
    ] {
        assert_eq!(graph_id(odd), None, "{odd}");
    }
}

#[test]
fn the_document_is_unpacked_from_either_alphabet() {
    use mailcraft_charset::base64::{STANDARD, URL_SAFE};
    // Bytes that make `+` and `/` in one alphabet, `-` and `_` in the other.
    let text = "<a>\u{fb}\u{ff}\u{fe} ~~~ ???>>></a>";
    for packed in [STANDARD.encode(text), URL_SAFE.encode(text), STANDARD_NO_PAD.encode(text), URL_SAFE_NO_PAD.encode(text)] {
        assert_eq!(document(&packed).as_deref(), Some(text), "{packed}");
    }
    assert_eq!(document(&format!(" {}\r\n", STANDARD.encode("<a/>"))).as_deref(), Some("<a/>"));
    // With a byte order mark, in UTF-8 and in UTF-16 either way round.
    assert_eq!(document(&STANDARD.encode("\u{feff}<a>é</a>")).as_deref(), Some("<a>é</a>"));
    let utf16 = |big: bool| -> Vec<u8> {
        let mark: [u8; 2] = if big { [0xFE, 0xFF] } else { [0xFF, 0xFE] };
        let units = "<a>é€</a>".encode_utf16().flat_map(|unit| if big { unit.to_be_bytes() } else { unit.to_le_bytes() });
        mark.into_iter().chain(units).collect()
    };
    assert_eq!(document(&STANDARD.encode(utf16(false))).as_deref(), Some("<a>é€</a>"));
    assert_eq!(document(&STANDARD.encode(utf16(true))).as_deref(), Some("<a>é€</a>"));
    // What isn't base64, isn't text, or is half a character: no document.
    let mut odd = utf16(false);
    odd.pop();
    for packed in ["not base64!".to_string(), STANDARD.encode([0xC3u8, 0x28]), STANDARD.encode(odd), "A".to_string()] {
        assert_eq!(document(&packed), None, "{packed}");
    }
    assert_eq!(document(""), Some(String::new()));
}

/// Text that looks like the document in places and like nothing in others.
fn xmlish() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("<a:Key>OWA.OutlookFavorites</a:Key>".to_string()),
        Just("<a:Value>".to_string()),
        Just("</a:Value>".to_string()),
        Just("<b:OutlookFavorite>".to_string()),
        Just("</b:OutlookFavorite>".to_string()),
        Just("<b:SingleValueSettings><b:KeyValueType><b:Key>FolderId</b:Key>".to_string()),
        Just("</b:KeyValueType></b:SingleValueSettings>".to_string()),
        Just("<Pair>".to_string()),
        Just("</Pair>".to_string()),
        Just("<!DOCTYPE x [<!ENTITY e \"eee\">]>".to_string()),
        Just("<![CDATA[".to_string()),
        Just("]]>".to_string()),
        Just("&e;&amp;&#0;&#x110000;&#65;".to_string()),
        "<b:(Index|Type|Value|Key)>[ -~]{0,12}</b:(Index|Type|Value|Key)>",
        "<[a-zA-Z:]{0,6}/?>",
        "\\PC{0,16}",
    ];
    proptest::collection::vec(piece, 0..40).prop_map(|pieces| pieces.concat())
}

proptest! {
    #[test]
    fn any_text_is_a_list_no_list_or_no_document(text in "\\PC{0,400}", shaped in xmlish()) {
        for document in [text, shaped] {
            if let Ok(Some(listed)) = favorites_of(&document) {
                prop_assert!(listed.entries.len() <= MAX_ENTRIES);
                for entry in &listed.entries {
                    prop_assert!(entry.kind.len() <= 2_048 && entry.folder_id.len() <= 2_048);
                }
                // And what is made of it is ids that can stand in an address, each once.
                let wanted = wanted(listed);
                let mut seen = std::collections::HashSet::new();
                for id in &wanted.ids {
                    prop_assert!(id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'=')), "{}", id);
                    prop_assert!(seen.insert(id.clone()));
                }
            }
        }
    }

    #[test]
    fn any_bytes_are_a_document_or_none(bytes in proptest::collection::vec(any::<u8>(), 0..300), text in "\\PC{0,200}") {
        use mailcraft_charset::base64::STANDARD;
        let _ = document(&text);
        if let Some(unpacked) = document(&STANDARD.encode(&bytes)) {
            let _ = favorites_of(&unpacked);
        }
    }

    #[test]
    fn any_text_is_an_id_for_an_address_or_none(text in "\\PC{0,200}", shaped in "[A-Za-z0-9+/_=-]{0,140}") {
        for candidate in [text, shaped] {
            if let Some(id) = graph_id(&candidate) {
                prop_assert!((20..=512).contains(&id.len()));
                prop_assert!(id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'=')), "{}", id);
                // Written once, it stays as it is.
                prop_assert_eq!(graph_id(&id), Some(id.clone()));
                prop_assert_eq!(http::encode(&id).replace("%3D", "="), id);
            }
        }
    }

}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn whatever_the_list_holds_the_read_ends_without_a_panic(
        entries in proptest::collection::vec((proptest::option::of(any::<i64>()), "(Folder|folder|Persona|\\PC{0,6})", "(\\PC{0,30}|[A-Za-z0-9+/]{24,40}={0,2})"), 0..12)
    ) {
        let server = StandIn::start().unwrap();
        let listed = entries.iter().map(|(index, kind, id)| Favorite { name: "n".into(), index: *index, kind: kind.clone(), folder_id: id.clone() }).collect();
        server.state().favorites = Some(listed);
        let download = favorites_at(&server.url(), TOKEN, &|_| {});
        // The stand-in writes what it is given into the document as it is: a text that is no
        // XML makes no document, and that is an error and nothing worse.
        if let Ok(download) = download {
            prop_assert!(download.folders.is_empty());
            prop_assert!(download.skipped as usize + download.unresolved as usize <= entries.len());
        }
    }
}
