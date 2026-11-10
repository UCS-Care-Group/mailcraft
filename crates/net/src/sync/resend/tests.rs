// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use mailcraft_model::sync::FolderState;

fn listed(id: &str, created: &str) -> Listed {
    let (secs, millis) = instant(created).unwrap();
    Listed { id: id.into(), secs, millis }
}

#[test]
fn a_page_of_resends_list_is_read_and_what_isnt_usable_left_out() {
    let body = br#"{"object":"list","has_more":true,"data":[
        {"id":"a39999a6-88e3-48b1-888b-beaabcde1b33","created_at":"2026-10-09T14:37:40.951Z","subject":"Hello"},
        {"id":"../../evil","created_at":"2026-10-09T14:37:40Z"},
        {"id":"b1","created_at":"not a date"},
        {"created_at":"2026-10-09T14:37:40Z"}]}"#;
    let (page, more) = read_page(body).unwrap();
    assert!(more);
    assert_eq!(page, vec![listed("a39999a6-88e3-48b1-888b-beaabcde1b33", "2026-10-09T14:37:40.951Z")]);
    assert_eq!(page[0].millis, 951);
    for junk in [&b""[..], b"[]", b"{\"data\":7}", b"\xff"] {
        assert!(read_page(junk).is_none());
    }
    assert_eq!(instant("2026-10-09T14:37:40.9Z").unwrap().1, 900);
}

#[test]
fn uids_follow_when_mail_came_and_stay_the_same_from_run_to_run() {
    let first =
        numbered(vec![listed("c", "2026-10-09T14:37:41.000Z"), listed("b", "2026-10-09T14:37:40.951Z"), listed("a", "2026-10-09T14:37:40.9Z")]);
    let ids: Vec<&str> = first.iter().map(|(_, l)| l.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    let uids: Vec<u32> = first.iter().map(|(uid, _)| *uid).collect();
    assert!(uids[0] < uids[1] && uids[1] < uids[2], "{uids:?}");
    // A later run that lists one more in the same second numbers the others as before.
    let later = numbered(vec![
        listed("d", "2026-10-09T14:37:41.500Z"),
        listed("c", "2026-10-09T14:37:41.000Z"),
        listed("b", "2026-10-09T14:37:40.951Z"),
        listed("a", "2026-10-09T14:37:40.9Z"),
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
    for elsewhere in ["https://resend.com.example/raw", "https://evilresend.com/raw", "http://127.0.0.1/raw"] {
        assert!(raw_may_come_from(&url(elsewhere)).is_err(), "{elsewhere}");
    }
    assert!(is_id("4ef9a417-02e9-4d39-ad75-9611e0fcc33c") && !is_id("a/b") && !is_id("") && !is_id(&"a".repeat(65)));
    // A key that can't be one is never sent.
    assert!(matches!(get("", "/emails/receiving"), Err(NetError::Proto(ProtoError::Auth(_)))));
    assert!(matches!(get("re_a b", "/emails/receiving"), Err(NetError::Proto(ProtoError::Auth(_)))));
}

/// `bring` as a run does it, with `fetch` standing in for Resend: the pieces it hands over, taken
/// in one after the other.
fn brought(request: &SyncRequest, listed: Vec<Listed>, fetch: &mut dyn FnMut(&str) -> Result<Got, NetError>) -> mailcraft_model::sync::SyncResult {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut whole = mailcraft_model::sync::SyncResult::default();
    let mut emit = |piece| whole.absorb(piece);
    let mut run = Run {
        request,
        limits: super::super::Limits::in_pieces(),
        cancel: &cancel,
        emit: &mut emit,
        out: super::super::blank(request),
        stopped: false,
        handed_over: std::time::Instant::now(),
        phase: mailcraft_model::sync::SyncPhase::Sending,
        asked: 0,
        counted: 0,
    };
    bring(&mut run, listed, fetch);
    run.finish();
    whole
}

fn mail(subject: &str) -> Got {
    Got::Whole(format!("From: noor@example.org\r\nTo: avery@lumen.example\r\nSubject: {subject}\r\n\r\nHello\r\n").into_bytes())
}

#[test]
fn a_message_that_cant_be_had_holds_the_count_back_for_the_next_run() {
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
    assert_eq!(inbox.new.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["a"]);
    assert_eq!((inbox.uid_seen, inbox.complete, inbox.remote.as_str()), (uids[0], false, "INBOX"));
    assert_eq!(result.errors.len(), 1);

    // The next run starts there and brings the rest.
    let mut request = request;
    request.folders = vec![FolderState {
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 1,
        uid_seen: uids[0],
        known_uids: vec![uids[0]],
        ..Default::default()
    }];
    let result = brought(&request, list(), &mut |id: &str| Ok(mail(id)));
    let inbox = &result.folders[0];
    assert_eq!(inbox.new.iter().map(|m| m.subject.as_str()).collect::<Vec<_>>(), ["b", "c"]);
    assert_eq!((inbox.uid_seen, inbox.complete), (uids[2], true));
}

#[test]
fn a_message_too_large_is_kept_as_its_header_and_a_ninth_in_one_second_is_said() {
    let request = SyncRequest::default();
    let said = serde_json::json!({
        "from": "Noor <noor@example.org>\r\nBcc: evil@example.org", "to": ["avery@lumen.example"], "subject": "Big",
        "created_at": "2026-10-09T14:37:40Z", "attachments": [{"size": 40_000_000u64}]
    });
    let mut list: Vec<Listed> = (0..10).map(|n| listed(&format!("m{n}"), &format!("2026-10-09T14:37:41.{n:03}Z"))).collect();
    list.push(listed("big", "2026-10-09T14:37:40Z"));
    let mut fetch = |id: &str| if id == "big" { Ok(Got::TooLarge(said.clone())) } else { Ok(mail(id)) };
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
