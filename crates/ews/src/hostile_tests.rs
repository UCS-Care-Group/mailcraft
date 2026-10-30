// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Property tests: every reader takes any bytes without panicking, and what the writer writes
//! is the same whatever it is given.

use proptest::prelude::*;

use crate::standin::{AUTHORIZATION, StandIn, block_on};
use crate::*;

/// An answer that is nearly right, to be broken a byte at a time.
fn honest() -> Vec<u8> {
    let standin = StandIn::new();
    let request = Client::new("https://mail.example.org/EWS/Exchange.asmx", Secret::new(AUTHORIZATION))
        .and_then(|client| client.request(&GetFolder::new(&[DistinguishedFolder::Inbox]).element()))
        .unwrap();
    let request = HttpRequest::soap("http://127.0.0.1/", &Secret::new(AUTHORIZATION), String::from_utf8(request.body().to_vec()).unwrap()).unwrap();
    block_on(standin.exchange(&request)).unwrap().unwrap().body().to_vec()
}

/// Nearly right answers to the mail operations: `SyncFolderHierarchy`, `SyncFolderItems` (a first
/// read and a later one) and `GetItem`, from a stand-in holding a little mail.
fn honest_mail() -> Vec<Vec<u8>> {
    let standin = StandIn::new();
    let (id, folder) = {
        let mut state = standin.state();
        let id = state.deliver("inbox", "Kickoff", false);
        state.deliver("inbox", "Budget", true);
        state.set_read(&id, true);
        state.edit_subject(&id, "Kickoff (moved)");
        state.remove(&id);
        (state.deliver("inbox", "Agenda", false), state.folder_id("inbox").unwrap_or_default())
    };
    let client = Client::new("https://mail.example.org/EWS/Exchange.asmx", Secret::new(AUTHORIZATION)).unwrap();
    let delta = format!("S0|d|{folder}|0");
    let operations = [
        SyncFolderHierarchy::new("").element(),
        SyncFolderItems::new(&folder, "", 200, Shape::Full).element(),
        SyncFolderItems::new(&folder, &delta, 200, Shape::Full).element(),
        GetItemMime::new(&id).element(),
    ];
    operations
        .iter()
        .filter_map(|operation| {
            let request = client.request(operation).ok()?;
            let request =
                HttpRequest::soap("http://127.0.0.1/", &Secret::new(AUTHORIZATION), String::from_utf8(request.body().to_vec()).ok()?).ok()?;
            block_on(standin.exchange(&request)).and_then(Result::ok).map(|response| response.body().to_vec())
        })
        .collect()
}

/// Every reader of the mail operations, over a parsed answer.
fn read_mail(reply: &Reply) {
    let _ = SyncFolderHierarchy::new("").answer(reply);
    let _ = SyncFolderItems::new("AAMk", "", 200, Shape::Full).answer(reply);
    let _ = GetItemMime::new("AAMk").answer(reply);
    let _ = reply.busy();
}

/// Everything that reads an answer, over `bytes`.
fn read_everything(bytes: &[u8], status: u16) {
    let _ = xml::parse(bytes);
    let _ = Reply::parse(bytes);
    let response = HttpResponse::new(status, bytes.to_vec())
        .with_field("Retry-After", &String::from_utf8_lossy(bytes))
        .with_field("X-EWS-Policy-Reason", &String::from_utf8_lossy(bytes));
    if let Ok(reply) = Reply::judge(&response, 1 << 20) {
        if let Ok(all) = reply.messages("GetFolder") {
            for one in &all {
                let _ = (one.class(), one.code().to_string(), one.ok().is_ok());
            }
        }
        let _ = GetFolder::new(&[DistinguishedFolder::Inbox, DistinguishedFolder::Drafts]).answer(&reply);
        read_mail(&reply);
    }
    let _ = Error::from(xml::Malformed).to_string();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_bytes_are_never_a_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096), status in prop_oneof![Just(200u16), Just(500), any::<u16>()]) {
        read_everything(&bytes, status);
    }

    #[test]
    fn xml_shaped_text_is_never_a_panic(text in "(<[a-z:]{0,6}( [a-z:]{1,4}=\"[a-z&;#0-9]{0,6}\")?/?>|</[a-z:]{0,6}>|[a-z &;#0-9]{0,8}|<!\\[CDATA\\[|\\]\\]>|<!--|-->|<\\?|\\?>|<!DOCTYPE|&[a-z]{0,5};){0,80}") {
        read_everything(text.as_bytes(), 200);
    }

    #[test]
    fn a_nearly_right_answer_broken_anywhere_is_never_a_panic(at in 0usize..4096, byte in any::<u8>(), cut in 0usize..4096) {
        let mut body = honest();
        let place = at % body.len().max(1);
        if let Some(slot) = body.get_mut(place) {
            *slot = byte;
        }
        read_everything(&body, 200);
        body.truncate(cut % (body.len() + 1));
        read_everything(&body, 200);
    }

    #[test]
    fn a_nearly_right_mail_answer_broken_anywhere_is_never_a_panic(which in 0usize..4, at in 0usize..16384, byte in any::<u8>(), cut in 0usize..16384) {
        let all = honest_mail();
        prop_assert!(all.len() == 4);
        let mut body = all.get(which % all.len().max(1)).cloned().unwrap_or_default();
        let place = at % body.len().max(1);
        if let Some(slot) = body.get_mut(place) {
            *slot = byte;
        }
        read_everything(&body, 200);
        body.truncate(cut % (body.len() + 1));
        read_everything(&body, 200);
    }

    #[test]
    fn base64_and_ids_and_times_take_any_text(text in "\\PC{0,300}") {
        let _ = crate::b64::decode(&text, 1 << 20);
        let _ = SyncFolderItems::new(&text, &text, 0, Shape::Lean).element().write();
        let _ = SyncFolderHierarchy::new(&text).element().write();
        let _ = GetItemMime::new(&text).element().write();
    }

    #[test]
    fn deep_and_wide_documents_are_capped(depth in 0usize..200, width in 0usize..2000) {
        let deep = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
        read_everything(deep.as_bytes(), 200);
        let wide = format!("<a>{}</a>", "<b/>".repeat(width));
        read_everything(wide.as_bytes(), 200);
        prop_assert!(depth <= xml::MAX_DEPTH || xml::parse(deep.as_bytes()).is_err());
    }

    #[test]
    fn the_other_readers_take_any_text(text in "\\PC{0,300}") {
        let _ = http::check_endpoint(&text);
        let _ = ResponseCode::from_name(&text);
        let _ = ResponseClass::from_name(&text);
        let _ = PolicyReason::from_header(&text);
        let _ = FolderId::new(&text);
        let _ = Secret::new(&text);
    }

    #[test]
    fn the_stand_in_takes_any_request(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let standin = StandIn::new();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if let Ok(request) = HttpRequest::soap("http://127.0.0.1/", &Secret::new(AUTHORIZATION), text) {
            let _ = block_on(standin.exchange(&request));
        }
    }

    #[test]
    fn nothing_written_holds_a_control_character_or_a_stray_tag(name in "\\PC{0,20}", value in "\\PC{0,60}", control in "[\\x00-\\x1f]{0,6}") {
        let nasty = format!("{control}{value}{control}");
        let text = Element::new(&format!("m:{name}")).attr(&name, &nasty).text(&nasty).child(Element::new("t:C").text(&nasty)).write();
        // (Text keeps its tabs and line feeds; nothing else of the controls gets through.)
        prop_assert!(!text.chars().any(|c| c.is_control() && c != '\t' && c != '\n'));
        let attribute = Element::new("m:A").attr(&name, &nasty).write();
        prop_assert!(!attribute.chars().any(char::is_control));
        // The only tags are the two it has: a value never opens another.
        prop_assert!(text.matches('<').count() <= 4);
        let secret = Secret::new(&nasty);
        prop_assert!(!secret.expose().chars().any(char::is_control));
    }
}
