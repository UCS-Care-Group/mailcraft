// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use mailcraft_model::sync::{FolderState, SyncResult};
use serde_json::json;

/// Long after every message of the tests came: all of them have settled.
const LATER: i64 = 2_000_000_000;

fn listed(id: &str, created: &str) -> Listed {
    let (secs, millis) = instant(created).unwrap();
    Listed { id: id.into(), secs, millis, said: json!({"id": id, "created_at": created, "subject": id}), named: Vec::new(), wanted: true }
}

#[test]
fn a_page_of_resends_list_is_read_and_what_isnt_usable_left_out() {
    let body = br#"{"object":"list","has_more":true,"data":[
        {"id":"a39999a6-88e3-48b1-888b-beaabcde1b33","created_at":"2026-10-09T14:37:40.951Z","subject":"Hello"},
        {"id":"../../evil","created_at":"2026-10-09T14:37:40Z"},
        {"id":"b1","created_at":"not a date"},
        {"created_at":"2026-10-09T14:37:40Z"}]}"#;
    let (page, more) = read_page(body, "example.org").unwrap();
    assert!(more);
    assert_eq!(page.len(), 1);
    assert_eq!((page[0].id.as_str(), page[0].millis, page[0].said["subject"].as_str()), ("a39999a6-88e3-48b1-888b-beaabcde1b33", 951, Some("Hello")));
    // A list that doesn't say whether more follow isn't read.
    for junk in [&b""[..], b"[]", b"{\"data\":7}", b"\xff", b"{\"data\":[]}", b"{\"has_more\":\"no\",\"data\":[]}"] {
        assert!(read_page(junk, "example.org").is_none());
    }
    assert_eq!(instant("2026-10-09T14:37:40.9Z").unwrap().1, 900);
}

#[test]
fn uids_follow_when_mail_came_and_stay_the_same_from_run_to_run() {
    let first =
        numbered(vec![listed("c", "2026-10-09T14:37:41.000Z"), listed("b", "2026-10-09T14:37:40.951Z"), listed("a", "2026-10-09T14:37:40.1Z")]);
    let ids: Vec<&str> = first.iter().map(|(_, l)| l.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    let uids: Vec<u32> = first.iter().map(|(uid, _)| *uid).collect();
    assert!(uids[0] < uids[1] && uids[1] < uids[2], "{uids:?}");
    // A later run that lists one more in the same second numbers the others as before.
    let later = numbered(vec![
        listed("d", "2026-10-09T14:37:41.500Z"),
        listed("c", "2026-10-09T14:37:41.000Z"),
        listed("b", "2026-10-09T14:37:40.951Z"),
        listed("a", "2026-10-09T14:37:40.1Z"),
    ]);
    assert_eq!(&later[..3], &first[..]);
    assert!(later[3].0 > uids[2]);
    // Mail from before the UIDs' start, and a list that names one twice, never make 0 or two.
    let old = numbered(vec![listed("x", "2020-01-01T00:00:00Z"), listed("x", "2020-01-01T00:00:00Z")]);
    assert_eq!(old.len(), 1);
    assert_eq!(old[0].0, 1);
}

#[test]
fn the_key_goes_to_resends_api_alone_and_the_bytes_come_from_resend() {
    let url = |text: &str| Url::parse(text).unwrap();
    assert!(may_go_to(&url("https://api.resend.com/emails/receiving")).is_ok());
    for elsewhere in ["https://api.resend.com.example/", "https://api.resend.com:8443/", "https://example.resend.com/", "http://127.0.0.1/"] {
        assert!(may_go_to(&url(elsewhere)).is_err(), "{elsewhere}");
    }
    assert!(raw_may_come_from(&url("https://inbound-cdn.resend.com/raw/1?Signature=x")).is_ok());
    assert!(raw_may_come_from(&url("https://d1a2b3c4.cloudfront.net/raw/1?Signature=x")).is_ok());
    assert!(raw_may_come_from(&url("https://cdn.resend.app/raw/1?Signature=x")).is_ok());
    for elsewhere in [
        "https://resend.com.example/raw",
        "https://evilresend.com/raw",
        "http://127.0.0.1/raw",
        "https://evilcloudfront.net/raw",
        "https://resend.app.example/raw",
    ] {
        assert!(raw_may_come_from(&url(elsewhere)).is_err(), "{elsewhere}");
    }
    assert!(is_id("4ef9a417-02e9-4d39-ad75-9611e0fcc33c") && !is_id("a/b") && !is_id("") && !is_id(&"a".repeat(65)));
    // A key that can't be one is never sent.
    let calm = AtomicBool::new(false);
    assert!(matches!(get("", "/emails/receiving", &calm), Err(NetError::Proto(ProtoError::Auth(_)))));
    assert!(matches!(get("re_a b", "/emails/receiving", &calm), Err(NetError::Proto(ProtoError::Auth(_)))));
}

#[test]
fn a_call_off_stops_a_wait_for_resend_at_once() {
    let stopped = AtomicBool::new(true);
    let started = Instant::now();
    let e = pause(Duration::from_secs(10), &stopped).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(matches!(e, NetError::Proto(ProtoError::Io(why)) if why.to_string() == transport::STOPPED));
    // Nothing is asked of Resend once the run is called off, the key checked first.
    assert!(matches!(get("re_key", "/emails/receiving", &stopped), Err(NetError::Proto(ProtoError::Io(_)))));
}

/// `bring` as a run does it, at `now`, as far as `limits` go, with `fetch` standing in for Resend:
/// the pieces it hands over, taken in one after the other.
fn brought_within(
    request: &SyncRequest,
    listed: Vec<Listed>,
    now: i64,
    limits: super::super::Limits,
    fetch: &mut dyn FnMut(&str) -> Result<Got, NetError>,
) -> SyncResult {
    let cancel = AtomicBool::new(false);
    let mut whole = SyncResult::default();
    let mut emit = |piece| whole.absorb(piece);
    let mut run = Run {
        request,
        limits,
        cancel: &cancel,
        emit: &mut emit,
        out: super::super::blank(request),
        stopped: false,
        handed_over: Instant::now(),
        phase: mailcraft_model::sync::SyncPhase::Sending,
        asked: 0,
        counted: 0,
    };
    bring(&mut run, listed, now, fetch);
    run.finish();
    whole
}

fn brought(request: &SyncRequest, listed: Vec<Listed>, fetch: &mut dyn FnMut(&str) -> Result<Got, NetError>) -> SyncResult {
    brought_within(request, listed, LATER, super::super::Limits::in_pieces(), fetch)
}

fn mail(subject: &str) -> Got {
    Got::Whole(format!("From: noor@example.org\r\nTo: avery@lumen.example\r\nSubject: {subject}\r\n\r\nHello\r\n").into_bytes())
}

fn subjects(result: &SyncResult) -> Vec<&str> {
    result.folders.iter().flat_map(|f| &f.new).map(|m| m.subject.as_str()).collect()
}

fn read_to(uid: u32) -> SyncRequest {
    SyncRequest {
        folders: vec![FolderState {
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 1,
            uid_seen: uid,
            known_uids: vec![uid],
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn a_message_that_cant_be_had_for_now_holds_the_count_back_for_the_next_run() {
    let request = SyncRequest::default();
    let list = || vec![listed("a", "2026-10-09T14:37:40Z"), listed("b", "2026-10-09T14:37:41Z"), listed("c", "2026-10-09T14:37:42Z")];
    let uids: Vec<u32> = numbered(list()).iter().map(|(uid, _)| *uid).collect();
    let mut fetch = |id: &str| match id {
        "b" => Err(NetError::Busy { text: "Resend is busy; try again later".into(), retry_after: None }),
        _ => Ok(mail(id)),
    };
    let result = brought(&request, list(), &mut fetch);
    let inbox = &result.folders[0];
    // Only "a" came; "b" and "c" stay above the count, to be asked for again.
    assert_eq!(subjects(&result), ["a"]);
    assert_eq!((inbox.uid_seen, inbox.complete, inbox.remote.as_str()), (uids[0], false, "INBOX"));
    assert_eq!(result.errors.len(), 1);

    // The next run starts there and brings the rest.
    let result = brought(&read_to(uids[0]), list(), &mut |id: &str| Ok(mail(id)));
    assert_eq!(subjects(&result), ["b", "c"]);
    assert_eq!((result.folders[0].uid_seen, result.folders[0].complete), (uids[2], true));
}

#[test]
fn a_message_resend_cant_give_is_kept_as_its_header_and_holds_nothing_back() {
    let request = SyncRequest::default();
    let list = vec![listed("a", "2026-10-09T14:37:40Z"), listed("gone", "2026-10-09T14:37:41Z"), listed("c", "2026-10-09T14:37:42Z")];
    let mut fetch = |id: &str| match id {
        "gone" => Err(NetError::Proto(ProtoError::Rejected("Resend answered with status 404".into()))),
        _ => Ok(mail(id)),
    };
    let result = brought(&request, list, &mut fetch);
    assert_eq!(subjects(&result), ["a", "gone", "c"]);
    let gone = &result.folders[0].new[1];
    assert!(gone.body.text.contains("couldn't download this message from Resend") && gone.body.text.contains("404"), "{}", gone.body.text);
    assert!(result.folders[0].complete);
    assert!(result.errors.iter().any(|e| e.contains("kept as its header")), "{:?}", result.errors);
}

#[test]
fn a_message_too_large_is_kept_as_its_header_and_two_in_one_eighth_of_a_second_are_said() {
    let request = SyncRequest::default();
    let mut big = listed("big", "2026-10-09T14:37:40Z");
    big.said = json!({
        "from": "Noor <noor@example.org>\r\nBcc: evil@example.org", "to": ["avery@lumen.example"], "subject": "Big",
        "created_at": "2026-10-09T14:37:40Z", "attachments": [{"size": 40_000_000u64}]
    });
    // Ten in one second, at .000, .100… .900: .000 and .100 share an eighth, as do .500 and .600.
    let mut list: Vec<Listed> = (0..10).map(|n| listed(&format!("m{n}"), &format!("2026-10-09T14:37:41.{:03}Z", n * 100))).collect();
    list.push(big);
    let mut fetch = |id: &str| if id == "big" { Ok(Got::TooLarge) } else { Ok(mail(id)) };
    let result = brought(&request, list, &mut fetch);
    let inbox = &result.folders[0];
    // The big one, then eight of the ten that came in one second.
    assert_eq!(inbox.new.len(), 9);
    let big = &inbox.new[0];
    assert_eq!((big.subject.as_str(), big.not_downloaded), ("Big", 40_000_000));
    assert!(big.body.text.starts_with(mailcraft_model::NOT_DOWNLOADED_NOTE));
    // What Resend said went into one header line, and smuggled in no other.
    assert!(big.headers.iter().all(|(name, _)| !name.eq_ignore_ascii_case("Bcc")));
    let uids: std::collections::HashSet<u32> = inbox.new.iter().map(|m| m.uid).collect();
    assert_eq!(uids.len(), 9);
    assert!(result.errors.iter().any(|e| e.contains("2 of them weren't brought")), "{:?}", result.errors);
}

#[test]
fn new_mail_comes_at_once_and_one_listed_late_in_its_second_takes_no_ones_uid() {
    let now = instant("2026-10-09T14:37:42Z").unwrap().0;
    // Two seconds on, "a" comes, but the count stays short of its second.
    let result = brought_within(
        &SyncRequest::default(),
        vec![listed("a", "2026-10-09T14:37:40.900Z")],
        now,
        super::super::Limits::in_pieces(),
        &mut |id: &str| Ok(mail(id)),
    );
    assert_eq!(subjects(&result), ["a"]);
    let a = result.folders[0].new[0].uid;
    assert_eq!(result.folders[0].uid_seen, 0);
    // Later, "b" of the same second but earlier in it has been listed too: it comes with a UID of
    // its own, "a" isn't brought twice, and the count moves past both.
    let mut request = read_to(0);
    request.folders[0].known_uids = vec![a];
    let later = vec![listed("a", "2026-10-09T14:37:40.900Z"), listed("b", "2026-10-09T14:37:40.100Z")];
    let result = brought_within(&request, later, now + SETTLE, super::super::Limits::in_pieces(), &mut |id: &str| Ok(mail(id)));
    assert_eq!(subjects(&result), ["b"]);
    assert!(result.folders[0].new[0].uid < a);
    assert_eq!(result.folders[0].uid_seen, a);
}

#[test]
fn a_run_goes_as_far_as_its_limits_and_hands_mail_over_in_pieces() {
    let list: Vec<Listed> = (0..5).map(|n| listed(&format!("m{n}"), &format!("2026-10-09T14:37:4{n}Z"))).collect();
    let uids: Vec<u32> = numbered(list.clone()).iter().map(|(uid, _)| *uid).collect();
    let mut limits = super::super::Limits::in_pieces();
    (limits.messages, limits.batch_messages) = (3, 2);
    let mut pieces = 0;
    let result = brought_within(&SyncRequest::default(), list, LATER, limits, &mut |id: &str| {
        pieces += 1;
        Ok(mail(id))
    });
    // Three of five, the count after the third: the other two come with the next run.
    assert_eq!((pieces, subjects(&result).len()), (3, 3));
    assert_eq!((result.folders[0].uid_seen, result.folders[0].complete), (uids[2], false));
}

#[test]
fn what_is_asked_again_or_put_into_the_account_is_settled_and_said() {
    let mut request = read_to(5);
    request.folders[0].again = vec![5];
    let result = brought(&request, Vec::new(), &mut |id: &str| Ok(mail(id)));
    assert_eq!(result.folders[0].refetch_done, [5]);
    assert!(result.errors.iter().any(|e| e.contains("can't send a message it received again")));
}

fn named(field: &str, addresses: &[&str]) -> Vec<String> {
    named_domains(&json!({ field: addresses }), "example.org")
}

#[test]
fn the_domains_a_message_was_written_to_are_found_in_to_cc_and_bcc() {
    assert_eq!(named("to", &["owner@example.org"]), ["example.org"]);
    assert_eq!(named("cc", &["Owner <Someone@EXAMPLE.org>", "x@other.example"]), ["example.org", "other.example"]);
    assert_eq!(named("bcc", &["x@example.org."]), ["example.org"]);
    // The account's own comes first, wherever it is in a long list.
    let mut many: Vec<String> = (0..400).map(|n| format!("u{n}@d{n}.example")).collect();
    many.push("owner@example.org".into());
    let found = named_domains(&json!({ "to": many }), "example.org");
    assert_eq!((found[0].as_str(), found.len()), ("example.org", MAX_DOMAINS + 1));
    assert!(named_domains(&json!({ "to": "owner@example.org" }), "example.org").is_empty());
    assert!(named("to", &["owner@sub.example.org"]) == ["sub.example.org"]);
}

#[test]
fn mail_for_the_accounts_domain_or_for_none_of_the_teams_is_kept() {
    let team: Vec<String> = vec!["example.org".into(), "other.example".into()];
    let wanted = |addresses: &[&str]| is_wanted(&named("to", addresses), "example.org", &team);
    assert!(wanted(&["owner@example.org"]));
    assert!(wanted(&["owner@example.org", "x@other.example"]));
    // A Bcc, or a list's post, names none of the team's domains: kept, never dropped for good.
    assert!(wanted(&["list@lists.example"]));
    assert!(wanted(&[]));
    // Written to the team's other domain alone: left.
    assert!(!wanted(&["x@other.example"]));
    assert!(!is_wanted(&named("to", &["owner@example.org"]), "", &team));
}

#[test]
fn what_a_list_says_is_cut_to_what_is_used_and_the_whole_is_weighed() {
    let long = "x".repeat(5_000);
    let many: Vec<String> = (0..500).map(|n| format!("u{n}@d.example")).collect();
    let body = json!({"has_more": false, "data": [{
        "id": "a1", "created_at": "2026-10-09T14:37:40Z", "from": long, "subject": long, "to": many, "cc": many, "bcc": many,
        "html": long, "headers": {"X": long}, "attachments": [{"size": 5}, {"size": 7, "filename": long}]
    }]})
    .to_string();
    let (page, _) = read_page(body.as_bytes(), "example.org").unwrap();
    let said = &page[0].said;
    assert_eq!(said["from"].as_str().map(str::len), Some(MAX_TEXT));
    assert_eq!(said["to"].as_array().map(Vec::len), Some(MAX_SHOWN));
    assert!(said.get("bcc").is_none() && said.get("html").is_none() && said.get("headers").is_none());
    assert_eq!(said["attachments"][0]["size"], 12);
    assert!(page[0].weight() < 400_000, "{}", page[0].weight());
}

#[test]
fn one_listed_late_in_the_eighth_of_one_here_is_said_and_never_hidden() {
    let x = numbered(vec![listed("x", "2026-10-09T14:37:40.600Z")]).remove(0).0;
    // "x" was brought before its second settled, so the count stays short of it...
    let mut request = read_to(0);
    request.folders[0].known_uids = vec![x];
    // ...and "y", earlier in the same eighth, turns up in the list after it.
    let late = vec![listed("x", "2026-10-09T14:37:40.600Z"), listed("y", "2026-10-09T14:37:40.550Z")];
    let result = brought(&request, late, &mut |id: &str| Ok(mail(id)));
    assert!(subjects(&result).is_empty());
    assert!(result.errors.iter().any(|e| e.contains("1 of them weren't brought")), "{:?}", result.errors);
    // A message's UID comes from its own time alone, whatever else is listed.
    let alone = numbered(vec![listed("z", "2026-10-09T14:37:40.900Z")]).remove(0).0;
    let crowded = numbered(vec![listed("w", "2026-10-09T14:37:40.880Z"), listed("z", "2026-10-09T14:37:40.900Z")]);
    assert!(crowded.iter().all(|(uid, l)| l.id != "z" || *uid == alone));
}

#[test]
fn mail_for_another_domain_of_the_team_moves_the_count_without_being_brought() {
    let mut other = listed("other", "2026-10-09T14:37:41Z");
    other.wanted = false;
    let list = vec![listed("a", "2026-10-09T14:37:40Z"), other, listed("c", "2026-10-09T14:37:42Z")];
    let uids: Vec<u32> = numbered(list.clone()).iter().map(|(uid, _)| *uid).collect();
    let result = brought(&SyncRequest::default(), list, &mut |id: &str| Ok(mail(id)));
    assert_eq!(subjects(&result), ["a", "c"]);
    assert_eq!((result.folders[0].uid_seen, result.folders[0].complete), (uids[2], true));
    // At the end of the list, the count passes it all the same.
    let mut last = listed("last", "2026-10-09T14:37:43Z");
    last.wanted = false;
    let uid = numbered(vec![last.clone()])[0].0;
    let result = brought(&SyncRequest::default(), vec![last], &mut |id: &str| Ok(mail(id)));
    assert!(subjects(&result).is_empty());
    assert_eq!(result.folders[0].uid_seen, uid);
}

#[test]
fn a_message_for_another_domain_never_takes_the_uid_of_one_for_the_account() {
    let mut other = listed("a-other", "2026-10-09T14:37:40.000Z");
    other.wanted = false;
    let mine = listed("b-mine", "2026-10-09T14:37:40.050Z");
    let result = brought(&SyncRequest::default(), vec![other, mine], &mut |id: &str| Ok(mail(id)));
    assert_eq!(subjects(&result), ["b-mine"]);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
}
