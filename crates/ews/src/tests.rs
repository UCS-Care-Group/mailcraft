// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The request writer, the reader, the judgement of answers, and the client against the strict
//! stand-in.

use crate::standin::{AUTHORIZATION, BLOCKED_BY_POLICY, DISABLED, Hostile, StandIn, block_on};
use crate::*;

const URL: &str = "http://127.0.0.1:1/EWS/Exchange.asmx";

fn secret() -> Secret {
    Secret::new(AUTHORIZATION)
}

fn client() -> Client {
    Client::new(URL, secret()).unwrap()
}

fn check(standin: &StandIn) -> Result<FolderInfo, Error> {
    block_on(client().check(standin)).unwrap()
}

/// What the stand-in answers to a body written by hand.
fn raw(standin: &StandIn, body: &str) -> (u16, String) {
    let request = HttpRequest::soap(URL, &secret(), body.to_owned()).unwrap();
    let response = block_on(standin.exchange(&request)).unwrap().unwrap();
    (response.status(), String::from_utf8_lossy(response.body()).into_owned())
}

const HEAD: &str = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"><s:Header><t:RequestServerVersion Version=\"Exchange2013\"/></s:Header><s:Body>";
const TAIL: &str = "</s:Body></s:Envelope>";

fn hand(operation: &str) -> String {
    format!("{HEAD}{operation}{TAIL}")
}

const GOOD: &str = "<m:GetFolder><m:FolderShape><t:BaseShape>Default</t:BaseShape></m:FolderShape><m:FolderIds><t:DistinguishedFolderId Id=\"inbox\"/></m:FolderIds></m:GetFolder>";

// ---- the client against the stand-in ----

#[test]
fn the_inbox_can_be_had() {
    let standin = StandIn::new();
    let inbox = check(&standin).unwrap();
    assert_eq!(inbox.display_name, "Inbox");
    assert_eq!((inbox.total_count, inbox.unread_count, inbox.child_folder_count), (Some(12), Some(3), Some(0)));
    assert_eq!(inbox.change_key.as_deref(), Some("AQAAAA=="));
    let state = standin.state();
    assert_eq!(state.requests.len(), 1);
    assert_eq!((state.requests[0].operation.as_str(), state.requests[0].status), ("GetFolder", 200));
    assert!(state.writes.is_empty());
}

#[test]
fn several_folders_are_answered_in_order() {
    let standin = StandIn::new();
    let get = GetFolder::new(&[DistinguishedFolder::Drafts, DistinguishedFolder::SentItems, DistinguishedFolder::Inbox]);
    let all = block_on(client().get_folders(&standin, &get)).unwrap().unwrap();
    let names: Vec<_> = all.into_iter().map(|one| one.unwrap().display_name).collect();
    assert_eq!(names, ["Drafts", "Sent Items", "Inbox"]);
}

#[test]
fn a_folder_that_is_not_there_is_its_own_error() {
    let standin = StandIn::new();
    standin.state().folders.retain(|folder| folder.distinguished != "junkemail");
    let get = GetFolder::new(&[DistinguishedFolder::JunkEmail, DistinguishedFolder::Inbox]);
    let all = block_on(client().get_folders(&standin, &get)).unwrap().unwrap();
    assert_eq!(all[0], Err(Error::Server(ResponseCode::FolderNotFound)));
    assert!(all[1].is_ok());
}

#[test]
fn a_wrong_credential_is_unauthorized() {
    let standin = StandIn::new();
    let client = Client::new(URL, Secret::new("Basic bm9wZTpub3Bl")).unwrap();
    assert_eq!(block_on(client.check(&standin)).unwrap(), Err(Error::Unauthorized));
}

#[test]
fn an_organisation_that_shuts_the_service_is_told_apart() {
    let standin = StandIn::new();
    standin.state().blocked = Some(DISABLED.into());
    assert_eq!(check(&standin), Err(Error::Blocked(PolicyReason::Disabled)));
    standin.state().blocked = Some(BLOCKED_BY_POLICY.into());
    assert_eq!(check(&standin), Err(Error::Blocked(PolicyReason::Blocked)));
    standin.state().blocked = Some("something else".into());
    assert_eq!(check(&standin), Err(Error::Blocked(PolicyReason::Other)));
}

#[test]
fn a_busy_server_says_how_long_and_is_believed_only_so_far() {
    let standin = StandIn::new();
    standin.state().busy = (1, 2_500);
    assert_eq!(check(&standin), Err(Error::Busy { back_off_ms: 2_500 }));
    assert!(check(&standin).is_ok());
    standin.state().busy = (1, u64::MAX);
    assert_eq!(check(&standin), Err(Error::Busy { back_off_ms: error::MAX_BACK_OFF_MS }));
}

#[test]
fn a_busy_server_that_names_no_wait_gets_the_default() {
    let body = reply_with("ErrorServerBusy", "");
    assert_eq!(parse_folder_error(&body), Error::Busy { back_off_ms: error::DEFAULT_BACK_OFF_MS });
    let body = reply_with("ErrorServerBusy", "<m:BackOffMilliseconds>soon</m:BackOffMilliseconds>");
    assert_eq!(parse_folder_error(&body), Error::Busy { back_off_ms: error::DEFAULT_BACK_OFF_MS });
    let body = reply_with("ErrorServerBusy", "<m:BackOffMilliseconds>7000</m:BackOffMilliseconds>");
    assert_eq!(parse_folder_error(&body), Error::Busy { back_off_ms: 7_000 });
}

fn reply_with(code: &str, extra: &str) -> String {
    format!(
        "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><m:GetFolderResponse xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\">\
         <m:ResponseMessages><m:GetFolderResponseMessage ResponseClass=\"Error\"><m:ResponseCode>{code}</m:ResponseCode>{extra}</m:GetFolderResponseMessage></m:ResponseMessages>\
         </m:GetFolderResponse></s:Body></s:Envelope>"
    )
}

fn parse_folder_error(body: &str) -> Error {
    let reply = Reply::parse(body.as_bytes()).unwrap();
    GetFolder::new(&[DistinguishedFolder::Inbox]).answer(&reply).unwrap().remove(0).unwrap_err()
}

#[test]
fn http_statuses_are_judged() {
    let judge = |response: HttpResponse| Reply::judge(&response, 1_000).unwrap_err();
    assert_eq!(judge(HttpResponse::new(401, vec![])), Error::Unauthorized);
    assert_eq!(judge(HttpResponse::new(403, vec![])), Error::Forbidden);
    assert_eq!(judge(HttpResponse::new(503, vec![]).with_field("retry-after", "3")), Error::Busy { back_off_ms: 3_000 });
    assert_eq!(judge(HttpResponse::new(429, vec![]).with_field("Retry-After", "99999")), Error::Busy { back_off_ms: error::MAX_BACK_OFF_MS });
    assert_eq!(
        judge(HttpResponse::new(429, vec![]).with_field("Retry-After", "Wed, 21 Oct 2026 07:28:00 GMT")),
        Error::Busy { back_off_ms: error::DEFAULT_BACK_OFF_MS }
    );
    assert_eq!(judge(HttpResponse::new(404, vec![])), Error::Status(404));
    assert_eq!(judge(HttpResponse::new(500, b"<html/>".to_vec())), Error::Status(500));
    assert_eq!(judge(HttpResponse::new(200, vec![b'x'; 2_000])), Error::TooLarge);
    assert_eq!(judge(HttpResponse::new(200, b"<html><body>Sign in</body></html>".to_vec())), Error::Unexpected);
    assert_eq!(judge(HttpResponse::new(200, b"not xml at all".to_vec())), Error::Malformed);
}

#[test]
fn a_fault_is_the_error_it_names() {
    let standin = StandIn::new();
    let (status, body) = raw(&standin, &hand("<m:GetFolder/>"));
    assert_eq!(status, 500);
    assert_eq!(Reply::parse(body.as_bytes()).unwrap_err(), Error::Server(ResponseCode::SchemaValidation));
    let (_, body) = raw(&standin, &hand("<m:NoSuchThing/>"));
    assert_eq!(Reply::parse(body.as_bytes()).unwrap_err(), Error::Server(ResponseCode::InvalidRequest));
}

#[test]
fn prefixes_are_the_documents_own_choice() {
    let body = "<a:Envelope xmlns:a=\"http://schemas.xmlsoap.org/soap/envelope/\"><a:Body>\
        <x:GetFolderResponse xmlns:x=\"http://schemas.microsoft.com/exchange/services/2006/messages\" xmlns:y=\"http://schemas.microsoft.com/exchange/services/2006/types\"><x:ResponseMessages>\
        <x:GetFolderResponseMessage ResponseClass=\"Success\"><x:ResponseCode>NoError</x:ResponseCode><x:Folders><y:Folder><y:FolderId Id=\"AbC+/=-_\"/><y:DisplayName>Inbox</y:DisplayName></y:Folder></x:Folders>\
        </x:GetFolderResponseMessage></x:ResponseMessages></x:GetFolderResponse></a:Body></a:Envelope>";
    let reply = Reply::parse(body.as_bytes()).unwrap();
    let got = GetFolder::new(&[DistinguishedFolder::Inbox]).answer(&reply).unwrap().remove(0).unwrap();
    assert_eq!(got.display_name, "Inbox");
    assert_eq!(got.total_count, None);
}

#[test]
fn an_element_in_the_wrong_namespace_is_not_the_one_asked_for() {
    let body = reply_with("NoError", "").replace("services/2006/messages", "services/2006/elsewhere");
    let reply = Reply::parse(body.as_bytes()).unwrap();
    assert_eq!(GetFolder::new(&[DistinguishedFolder::Inbox]).answer(&reply).unwrap_err(), Error::Unexpected);
}

#[test]
fn unknown_elements_are_skipped() {
    let extra = "<m:Surprise><m:Deeper>1</m:Deeper></m:Surprise><x:Other xmlns:x=\"urn:other\"/>";
    let body = reply_with("ErrorAccessDenied", extra);
    assert_eq!(parse_folder_error(&body), Error::Server(ResponseCode::AccessDenied));
}

#[test]
fn response_codes_are_kept_only_as_short_identifiers() {
    assert_eq!(ResponseCode::from_name(" ErrorServerBusy "), ResponseCode::ServerBusy);
    assert_eq!(ResponseCode::from_name("ErrorSomethingNew"), ResponseCode::Other("ErrorSomethingNew".into()));
    assert_eq!(ResponseCode::from_name("bad code\r\nX: y"), ResponseCode::Other(String::new()));
    assert_eq!(ResponseCode::from_name(&"A".repeat(500)), ResponseCode::Other(String::new()));
    assert_eq!(ResponseCode::from_name("bad\r\nX: y").to_string(), "an unnamed error");
}

#[test]
fn a_folder_id_that_is_not_an_id_is_not_taken() {
    let standin = StandIn::new();
    standin.state().hostile = Some(Hostile::BadId);
    assert_eq!(check(&standin), Err(Error::Unexpected));
    assert!(FolderId::new("AAMk+Tk/Zm9=").is_some());
    assert!(FolderId::new("").is_none());
    assert!(FolderId::new("a\u{1}b").is_none(), "no control character");
    assert!(FolderId::new("a b<c>&\"d").is_some(), "an id is opaque: any other character does");
    assert!(FolderId::new(&"a".repeat(2_000)).is_none());
}

#[test]
fn hostile_answers_are_errors_never_panics() {
    for (how, expected) in [
        (Hostile::Dtd, Error::Malformed),
        (Hostile::CutShort, Error::Malformed),
        (Hostile::NotXml, Error::Malformed),
        (Hostile::Deep, Error::Malformed),
        (Hostile::Huge, Error::TooLarge),
        (Hostile::Fault, Error::Server(ResponseCode::Other(String::new()))),
        (Hostile::TooFew, Error::Unexpected),
    ] {
        let standin = StandIn::new();
        standin.state().hostile = Some(how);
        assert_eq!(check(&standin), Err(expected), "{how:?}");
    }
}

// ---- the request ----

#[test]
fn the_request_is_a_soap_envelope_with_the_version_and_one_operation() {
    let get = GetFolder::new(&[DistinguishedFolder::Inbox]);
    let text = soap::envelope(Version::Exchange2016, &get.element());
    let tree = xml::parse(text.as_bytes()).unwrap();
    assert!(tree.is(soap::SOAP, "Envelope"));
    let version = tree.child_in(soap::SOAP, "Header").unwrap().child_in(soap::TYPES, "RequestServerVersion").unwrap();
    assert_eq!(version.attribute("Version"), Some("Exchange2016"));
    let body = tree.child_in(soap::SOAP, "Body").unwrap();
    assert_eq!(body.children.len(), 1);
    assert!(body.children[0].is(soap::MESSAGES, "GetFolder"));
    let request = client().with_version(Version::Exchange2010Sp2).request(&get.element()).unwrap();
    assert_eq!(request.method(), Method::Post);
    assert!(String::from_utf8_lossy(request.body()).contains("Exchange2010_SP2"));
}

#[test]
fn nothing_written_can_break_out_of_its_place() {
    let nasty = "a\r\nX-Injected: 1\0</t:BaseShape><m:Evil/>\"'&";
    let element = Element::new("m:Op\r\n<x").attr("Na\nme", nasty).text(nasty).child(Element::new("t:Child").text(nasty));
    let text = element.write();
    // (A line feed in a text stays; a carriage return is a reference; an attribute has neither.)
    assert!(!text.contains(['\r', '\0']), "{text:?}");
    let attribute = text.split_once("Name=\"").and_then(|(_, rest)| rest.split_once('"')).map(|(value, _)| value).unwrap_or_default();
    assert!(!attribute.is_empty() && !attribute.contains(['\r', '\n', '\0']), "{text:?}");
    assert!(!text.contains("<m:Evil/>") && !text.contains("<x"));
    // What is written reads back as the one element, holding the text as text.
    let tree = xml::parse(soap::envelope(Version::default(), &element).as_bytes()).unwrap();
    let op = &tree.child_in(soap::SOAP, "Body").unwrap().children[0];
    assert_eq!(op.name, "Opx");
    assert!(op.text.contains("</t:BaseShape><m:Evil/>"));
    assert_eq!(op.children.len(), 1);
    let secret = Secret::new("Basic abc\r\nX-Injected: 1");
    assert_eq!(secret.expose(), "Basic abcX-Injected: 1");
    assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
}

#[test]
fn credentials_and_ids_are_not_in_debug_output() {
    let request = client().request(&GetFolder::new(&[DistinguishedFolder::Inbox]).element()).unwrap();
    let shown = format!("{request:?} {:?} {:?}", client(), FolderId::new("AAMkSecretId").unwrap());
    assert!(!shown.contains("standin.ews") && !shown.contains("SecretId") && !shown.contains("127.0.0.1"), "{shown}");
}

#[test]
fn the_transport_rule() {
    for ok in [
        "https://mail.example.org/EWS/Exchange.asmx",
        "HTTPS://mail.example.org",
        "http://127.0.0.1:8080/x",
        "http://localhost/x",
        "http://[::1]:80/x",
        "http://127.9.9.9/",
    ] {
        assert_eq!(http::check_endpoint(ok), Ok(()), "{ok}");
    }
    for (bad, why) in [
        ("http://mail.example.org/EWS", EndpointError::NotEncrypted),
        ("http://127.0.0.1.example.org/", EndpointError::NotEncrypted),
        ("http://localhost.example.org/", EndpointError::NotEncrypted),
        ("https://user:pw@mail.example.org/", EndpointError::Credentials),
        ("http://localhost@evil.example/", EndpointError::Credentials),
        ("ftp://mail.example.org/", EndpointError::Malformed),
        ("https://", EndpointError::Malformed),
        ("https://mail.example.org/a b", EndpointError::Malformed),
        ("https://mail.example.org/\r\nX: y", EndpointError::Malformed),
        ("mail.example.org", EndpointError::Malformed),
    ] {
        assert_eq!(http::check_endpoint(bad), Err(why), "{bad}");
    }
    assert_eq!(http::check_endpoint(&format!("https://a.example/{}", "a".repeat(3_000))), Err(EndpointError::Malformed));
}

// ---- the stand-in holds a request to the schema ----

#[test]
fn the_stand_in_takes_a_request_that_fits() {
    let (status, body) = raw(&StandIn::new(), &hand(GOOD));
    assert_eq!(status, 200, "{body}");
}

#[test]
fn the_stand_in_refuses_what_does_not_fit() {
    let cases = [
        ("an unknown element", GOOD.replace("<m:FolderShape>", "<m:Extra/><m:FolderShape>")),
        ("a wrong namespace", GOOD.replace("<t:BaseShape>Default</t:BaseShape>", "<m:BaseShape>Default</m:BaseShape>")),
        ("a missing FolderIds", "<m:GetFolder><m:FolderShape><t:BaseShape>Default</t:BaseShape></m:FolderShape></m:GetFolder>".into()),
        ("a missing BaseShape", "<m:GetFolder><m:FolderShape/><m:FolderIds><t:DistinguishedFolderId Id=\"inbox\"/></m:FolderIds></m:GetFolder>".into()),
        ("a wrong order", "<m:GetFolder><m:FolderIds><t:DistinguishedFolderId Id=\"inbox\"/></m:FolderIds><m:FolderShape><t:BaseShape>Default</t:BaseShape></m:FolderShape></m:GetFolder>".into()),
        ("an unknown attribute", GOOD.replace("Id=\"inbox\"", "Id=\"inbox\" Odd=\"1\"")),
        ("a missing Id", GOOD.replace("Id=\"inbox\"", "")),
        ("an unknown folder name", GOOD.replace("inbox", "archive")),
        ("an unknown shape", GOOD.replace("Default", "Everything")),
        ("no folders", "<m:GetFolder><m:FolderShape><t:BaseShape>Default</t:BaseShape></m:FolderShape><m:FolderIds/></m:GetFolder>".into()),
        ("two operations", format!("{GOOD}{GOOD}")),
    ];
    for (what, operation) in cases {
        let (status, body) = raw(&StandIn::new(), &hand(&operation));
        assert_eq!(status, 500, "{what}: {body}");
        assert!(body.contains("ErrorSchemaValidation") || body.contains("ErrorInvalidRequest"), "{what}: {body}");
    }
}

#[test]
fn the_stand_in_refuses_a_bad_envelope() {
    let standin = StandIn::new();
    for (what, text) in [
        ("no version", hand(GOOD).replace("<t:RequestServerVersion Version=\"Exchange2013\"/>", "")),
        ("a version the schema lacks", hand(GOOD).replace("Exchange2013", "Exchange2099")),
        ("the version in the wrong namespace", hand(GOOD).replace("<t:RequestServerVersion", "<m:RequestServerVersion")),
        ("a wrong envelope namespace", hand(GOOD).replace("http://schemas.xmlsoap.org/soap/envelope/", "urn:other")),
        ("not xml", "hello".to_owned()),
        ("a DTD", format!("<!DOCTYPE s [<!ENTITY a \"b\">]>{}", hand(GOOD))),
    ] {
        let (status, body) = raw(&standin, &text);
        assert_eq!(status, 500, "{what}: {body}");
    }
}

#[test]
fn the_stand_in_notes_and_refuses_anything_that_would_change_the_mailbox() {
    let standin = StandIn::new();
    let (status, body) = raw(&standin, &hand("<m:EmptyFolder DeleteType=\"HardDelete\"/>"));
    assert_eq!(status, 200);
    assert!(body.contains("ErrorAccessDenied"));
    assert_eq!(standin.state().writes, ["EmptyFolder"]);
}

// ---- the reader's cost ----

#[test]
fn a_flood_of_elements_with_a_wide_namespace_costs_a_small_multiple_of_the_body() {
    // 1.6 MB: a namespace of 2,000 characters and 399,000 empty elements. Held as the nodes
    // would be (a copy of the namespace on each), this took 870 MB.
    let flood = format!("<a xmlns=\"{}\">{}</a>", "n".repeat(2_000), "<b/>".repeat(399_000));
    assert!(flood.len() > 1_500_000);
    assert_eq!(xml::parse(flood.as_bytes()).unwrap_err(), xml::Malformed);
    // The budget is a small multiple of the body, and has an end.
    assert!(xml::budget(flood.len()) < 30 * 1024 * 1024);
    assert_eq!(xml::budget(usize::MAX), 64 * 1024 * 1024);
    // What fits holds the namespace once: the nodes point to the one copy.
    let fits = format!("<a xmlns=\"{}\">{}</a>", "n".repeat(2_000), "<b>x</b>".repeat(1_000));
    let tree = xml::parse(fits.as_bytes()).unwrap();
    assert_eq!(tree.children.len(), 1_000);
    assert!(tree.children.iter().all(|child| std::sync::Arc::ptr_eq(&child.namespace, &tree.namespace)));
}

#[test]
fn a_flood_of_different_namespaces_is_paid_for_once_each() {
    let flood: String = (0..400).map(|n| format!("<b xmlns=\"{n}{}\"/>", "u".repeat(1_900))).collect();
    // Each is as big as its place in the body: no more, and the whole is within the budget.
    assert!(xml::parse(format!("<a>{flood}</a>").as_bytes()).is_ok());
    // A body that is nothing but elements is turned away long before 100,000 of them.
    let thin = format!("<a>{}</a>", "<b/>".repeat(60_000));
    assert!(xml::parse(thin.as_bytes()).is_err());
}

#[test]
fn what_xml_forbids_never_reaches_the_tree() {
    let tree = xml::parse("<a b=\"1&#2;2&#x1B;3\u{1}4\">x&#1;y&#x1B;z\u{1}w\u{FFFE}\u{FFFF}&#xFFFE;v\tu\n</a>".as_bytes()).unwrap();
    assert_eq!(tree.text, "xyzwv\tu\n");
    assert_eq!(tree.attribute("b"), Some("1234"));
    // ... nor into what a caller reads from an answer.
    let body = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><m:GetFolderResponse xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"><m:ResponseMessages>\
        <m:GetFolderResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode><m:Folders><t:Folder><t:FolderId Id=\"AbC\"/><t:DisplayName>In&#1;bo&#x1B;x\u{2}</t:DisplayName></t:Folder></m:Folders></m:GetFolderResponseMessage>\
        </m:ResponseMessages></m:GetFolderResponse></s:Body></s:Envelope>";
    let reply = Reply::parse(body.as_bytes()).unwrap();
    let got = GetFolder::new(&[DistinguishedFolder::Inbox]).answer(&reply).unwrap().remove(0).unwrap();
    assert_eq!(got.display_name, "Inbox");
}

#[test]
fn a_prefix_or_a_declaration_over_its_cap_is_no_document() {
    let long = "p".repeat(65);
    assert_eq!(xml::parse(format!("<{long}:a xmlns:{long}=\"u\"/>").as_bytes()).unwrap_err(), xml::Malformed);
    assert_eq!(xml::parse(format!("<a xmlns:{long}=\"u\"/>").as_bytes()).unwrap_err(), xml::Malformed);
    assert_eq!(xml::parse(format!("<a xmlns=\"{}\"/>", "u".repeat(2_049)).as_bytes()).unwrap_err(), xml::Malformed);
    assert_eq!(xml::parse(b"<a xmlns:p=\"&bogus;\"/>").unwrap_err(), xml::Malformed);
    let many: String = (0..65).map(|n| format!(" a{n}=\"1\"")).collect();
    assert_eq!(xml::parse(format!("<a{many}/>").as_bytes()).unwrap_err(), xml::Malformed);
    // At the caps, they are read; and a declaration after the twelfth attribute still counts.
    let twelve: String = (0..13).map(|n| format!(" a{n}=\"1\"")).collect();
    let tree = xml::parse(format!("<a{twelve} xmlns:p=\"{}\"><p:c/></a>", "u".repeat(2_048)).as_bytes()).unwrap();
    assert_eq!(tree.attributes.len(), 12);
    assert_eq!(tree.children[0].namespace.len(), 2_048);
    assert!(xml::parse(format!("<{p}:a xmlns:{p}=\"u\"/>", p = "p".repeat(64)).as_bytes()).is_ok());
}

#[test]
fn debug_output_shows_nothing_a_server_chose() {
    let body = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><m:GetFolderResponse xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"><m:ResponseMessages>\
        <m:GetFolderResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode><m:Folders><t:Folder><t:FolderId Id=\"SecretId\" ChangeKey=\"SecretKey\"/><t:DisplayName>Secret Name</t:DisplayName></t:Folder></m:Folders></m:GetFolderResponseMessage>\
        </m:ResponseMessages></m:GetFolderResponse></s:Body></s:Envelope>";
    let reply = Reply::parse(body.as_bytes()).unwrap();
    let node = xml::parse(body.as_bytes()).unwrap();
    let messages = reply.messages("GetFolder").unwrap();
    let info = GetFolder::new(&[DistinguishedFolder::Inbox]).answer(&reply).unwrap().remove(0).unwrap();
    let shown = format!("{reply:?} {node:?} {messages:?} {info:?} {:?}", HttpResponse::new(200, body.as_bytes().to_vec()));
    assert!(!shown.contains("Secret") && !shown.contains("Folders"), "{shown}");
}

// ---- busy servers ----

#[test]
fn a_busy_fault_with_its_wait_in_the_types_namespace_is_read() {
    let standin = StandIn::new();
    {
        let mut state = standin.state();
        state.busy_fault = true;
        state.busy.1 = 297_749;
    }
    assert_eq!(check(&standin), Err(Error::Busy { back_off_ms: 297_749 }));
    assert_eq!(standin.state().requests[0].status, 500);
    assert!(check(&standin).is_ok());
    // Over the cap it is the cap.
    {
        let mut state = standin.state();
        state.busy_fault = true;
        state.busy.1 = 9_999_999;
    }
    assert_eq!(check(&standin), Err(Error::Busy { back_off_ms: error::MAX_BACK_OFF_MS }));
}

#[test]
fn a_busy_server_inside_an_internal_server_error_is_busy() {
    let standin = StandIn::new();
    {
        let mut state = standin.state();
        state.busy = (1, 4_000);
        state.busy_inner = true;
    }
    assert_eq!(check(&standin), Err(Error::Busy { back_off_ms: 4_000 }));
    assert_eq!(standin.state().requests[0].code, "ErrorInternalServerError");
    // Without the inner error it is the server's own error, and not a reason to wait.
    let body =
        reply_with("ErrorInternalServerError", "<m:MessageXml><t:Value xmlns:t=\"x\" Name=\"BackOffMilliseconds\">4000</t:Value></m:MessageXml>");
    assert_eq!(parse_folder_error(&body), Error::Server(ResponseCode::InternalServerError));
    // The same in a fault.
    let fault = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><s:Fault><faultcode>s:Server</faultcode><detail>\
        <e:ResponseCode xmlns:e=\"http://schemas.microsoft.com/exchange/services/2006/errors\">ErrorInternalServerError</e:ResponseCode>\
        <e:MessageXml xmlns:e=\"http://schemas.microsoft.com/exchange/services/2006/errors\"><e:Value Name=\"InnerErrorResponseCode\">ErrorServerBusy</e:Value><e:Value Name=\"BackOffMilliseconds\">2500</e:Value></e:MessageXml></detail></s:Fault></s:Body></s:Envelope>";
    assert_eq!(Reply::parse(fault.as_bytes()).unwrap_err(), Error::Busy { back_off_ms: 2_500 });
}

#[test]
fn a_wait_of_nothing_is_a_wait() {
    let standin = StandIn::new();
    standin.state().busy = (1, 0);
    assert_eq!(check(&standin), Err(Error::Busy { back_off_ms: error::MIN_BACK_OFF_MS }));
    let response = HttpResponse::new(503, vec![]).with_field("Retry-After", "0");
    assert_eq!(Reply::judge(&response, 10).unwrap_err(), Error::Busy { back_off_ms: error::MIN_BACK_OFF_MS });
    let body = reply_with("ErrorServerBusy", "<m:BackOffMilliseconds>0</m:BackOffMilliseconds>");
    assert_eq!(parse_folder_error(&body), Error::Busy { back_off_ms: error::MIN_BACK_OFF_MS });
}

// ---- text and fields ----

#[test]
fn a_text_keeps_its_lines() {
    let text = "first\r\nsecond\tindented\nthird\0\u{1}\u{FFFE}<&>";
    let written = Element::new("t:Body").text(text).write();
    assert_eq!(written, "<t:Body>first&#13;\nsecond\tindented\nthird&lt;&amp;&gt;</t:Body>");
    assert!(!written.contains(['\r', '\0']));
    let tree = xml::parse(soap::envelope(Version::default(), &Element::new("m:Op").text(text)).as_bytes()).unwrap();
    let op = &tree.child_in(soap::SOAP, "Body").unwrap().children[0];
    assert_eq!(op.text, "first\r\nsecond\tindented\nthird<&>");
    // An attribute, or a header, keeps none of it.
    assert_eq!(xml::escape("a\r\nb\tc\0"), "abc");
}

#[test]
fn the_response_keeps_a_small_capped_set_of_fields() {
    let response = HttpResponse::new(401, vec![])
        .with_field("WWW-Authenticate", "Negotiate")
        .with_field("www-authenticate", "NTLM")
        .with_field("LOCATION", " https://mail.example.org/ews ")
        .with_field("Set-Cookie", "a=b\r\nX: y")
        .with_field("X-Tabbed", "one\ttwo")
        .with_field("bad name", "x")
        .with_field("X-Long", &"v".repeat(http::MAX_FIELD_VALUE_LEN + 1));
    assert_eq!(response.headers_named("WWW-Authenticate").collect::<Vec<_>>(), ["Negotiate", "NTLM"]);
    assert_eq!(response.header("location"), Some("https://mail.example.org/ews"));
    assert_eq!((response.header("bad name"), response.header("x-long")), (None, None));
    // A control character is a space (a tab stays); the value is not lost.
    assert_eq!(response.header("set-cookie"), Some("a=b  X: y"));
    assert_eq!(response.header("x-tabbed"), Some("one\ttwo"));
    let mut many = HttpResponse::new(200, vec![]);
    for n in 0..100 {
        many = many.with_field(&format!("x-{n}"), "1");
    }
    assert_eq!((0..100).filter(|n| many.header(&format!("x-{n}")).is_some()).count(), http::MAX_FIELDS);
    assert_eq!(many.with_field(&"n".repeat(65), "1").header(&"n".repeat(65)), None);
}

#[test]
fn a_request_can_be_any_get_or_post() {
    let get = HttpRequest::new(
        Method::Get,
        "https://mail.example.org/autodiscover",
        &[("Accept", "text/xml\r\nX-Injected: 1"), ("bad name", "x"), ("X-A\r\nB", "y")],
        vec![],
    )
    .unwrap();
    assert_eq!(get.method(), Method::Get);
    assert_eq!(get.method().as_str(), "GET");
    assert_eq!(get.fields().collect::<Vec<_>>(), [("Accept", "text/xmlX-Injected: 1")]);
    assert_eq!(get.field("accept"), Some("text/xmlX-Injected: 1"));
    assert!(HttpRequest::new(Method::Post, "http://mail.example.org/", &[], vec![]).is_err());
    let soap = HttpRequest::soap(URL, &secret(), "<x/>".into()).unwrap();
    assert_eq!((soap.method(), soap.field("content-type")), (Method::Post, Some("text/xml; charset=utf-8")));
    assert_eq!(soap.field("authorization"), Some(AUTHORIZATION));
}

// ---- the stand-in, further ----

#[test]
fn the_stand_in_refuses_stray_content() {
    let cases = [
        (
            "a child in a folder name",
            GOOD.replace("<t:DistinguishedFolderId Id=\"inbox\"/>", "<t:DistinguishedFolderId Id=\"inbox\"><t:X/></t:DistinguishedFolderId>"),
        ),
        (
            "text in a folder name",
            GOOD.replace("<t:DistinguishedFolderId Id=\"inbox\"/>", "<t:DistinguishedFolderId Id=\"inbox\">more</t:DistinguishedFolderId>"),
        ),
        ("text between elements", GOOD.replace("<m:FolderIds>", "<m:FolderIds>stray")),
        ("text in the operation", GOOD.replace("<m:GetFolder>", "<m:GetFolder>stray")),
        ("an element beside a shape's name", GOOD.replace("Default", "Default<t:X/>")),
    ];
    for (what, operation) in cases {
        let (status, body) = raw(&StandIn::new(), &hand(&operation));
        assert_eq!(status, 500, "{what}: {body}");
    }
    let (status, _) = raw(&StandIn::new(), &hand(GOOD).replace("<s:Body>", "stray<s:Body>"));
    assert_eq!(status, 500);
    let (status, _) = raw(&StandIn::new(), &hand(GOOD).replace("<s:Header>", "<s:Header stray=\"1\">"));
    assert_eq!(status, 500);
    let (status, _) = raw(&StandIn::new(), &hand(GOOD).replace("<s:Body>", "<s:Body Id=\"1\">"));
    assert_eq!(status, 500);
    let (status, _) = raw(
        &StandIn::new(),
        &hand(GOOD).replace(
            "<t:RequestServerVersion Version=\"Exchange2013\"/>",
            "<t:RequestServerVersion Version=\"Exchange2013\">x</t:RequestServerVersion>",
        ),
    );
    assert_eq!(status, 500);
    // White space between elements is no stray text.
    let (status, body) = raw(&StandIn::new(), &hand(&GOOD.replace("><", ">\n  <")));
    assert_eq!(status, 200, "{body}");
}

// ---- the second review ----

/// Whether every vector in the tree holds exactly what it has in it.
fn tight(node: &xml::Node) -> bool {
    node.children.capacity() == node.children.len()
        && node.namespaces.capacity() == node.namespaces.len()
        && node.attributes.capacity() == node.attributes.len()
        && node.text.capacity() == node.text.len()
        && node.children.iter().all(tight)
}

#[test]
fn deep_chains_cost_what_they_hold() {
    // 2,560 chains, each 39 elements deep: 0.7 MB. A node's `children` has room for four before
    // it holds one, and the 99,840 nodes took 37 MB more than their sizes: 55 times the body.
    let chain = format!("<c>{}{}</c>", "<d>".repeat(38), "</d>".repeat(38));
    let doc = format!("<a>{}</a>", chain.repeat(2_560));
    assert!(doc.len() < 800_000);
    assert_eq!(xml::parse(doc.as_bytes()).unwrap_err(), xml::Malformed);
    // A node costs its size, and the budget is a multiple of the body.
    assert!(xml::budget(doc.len()) < 16 * doc.len() + 300_000);
    // What fits has no room to spare in any of its vectors.
    let fits = format!("<a xmlns:p=\"u\" b=\"1\">{}</a>", "<p:c e=\"1\">text<d/><d/><d/></p:c>".repeat(200));
    let tree = xml::parse(fits.as_bytes()).unwrap();
    assert_eq!(tree.children.len(), 200);
    assert!(tight(&tree));
}

#[test]
fn a_namespace_declared_again_is_still_one_copy() {
    let tree = xml::parse(b"<a><b xmlns=\"urn:same\"><c/></b><d xmlns=\"urn:same\"><e xmlns:p=\"urn:same\"/></d></a>").unwrap();
    let (b, d) = (&tree.children[0], &tree.children[1]);
    assert!(std::sync::Arc::ptr_eq(&b.namespace, &d.namespace));
    assert!(std::sync::Arc::ptr_eq(&b.namespaces[0].1, &d.namespaces[0].1));
    assert!(std::sync::Arc::ptr_eq(&b.children[0].namespace, &d.children[0].namespaces[0].1));
}

#[test]
fn a_prefix_over_its_cap_is_no_document_even_undeclared() {
    assert_eq!(xml::parse(format!("<{}:a/>", "p".repeat(65)).as_bytes()).unwrap_err(), xml::Malformed);
    assert!(xml::parse(format!("<{}:a/>", "p".repeat(64)).as_bytes()).is_ok());
}

#[test]
fn the_fields_that_are_read_are_kept_whatever_came_first() {
    let mut response = HttpResponse::new(403, vec![]);
    for n in 0..40 {
        response = response.with_field(&format!("X-Noise-{n}"), "1");
    }
    let response = response
        .with_field("X-EWS-Policy-Reason", DISABLED)
        .with_field("Retry-After", "7")
        .with_field("WWW-Authenticate", "NTLM")
        .with_field("Location", "https://mail.example.org/ews");
    assert_eq!(response.policy_reason(), Some(DISABLED));
    assert_eq!(
        (response.retry_after(), response.header("www-authenticate"), response.header("location")),
        (Some("7"), Some("NTLM"), Some("https://mail.example.org/ews"))
    );
    // ... so a 403 is a block and a 503 is the wait the server named.
    assert_eq!(Reply::judge(&response, 10).unwrap_err(), Error::Blocked(PolicyReason::Disabled));
    let mut busy = HttpResponse::new(503, vec![]);
    for n in 0..40 {
        busy = busy.with_field(&format!("X-Noise-{n}"), "1");
    }
    assert_eq!(Reply::judge(&busy.with_field("Retry-After", "7"), 10).unwrap_err(), Error::Busy { back_off_ms: 7_000 });
    // Those have a cap of their own, too.
    let mut flood = HttpResponse::new(401, vec![]);
    for _ in 0..100 {
        flood = flood.with_field("WWW-Authenticate", "Basic");
    }
    assert_eq!(flood.headers_named("www-authenticate").count(), http::read_field_cap("www-authenticate"));
    // ... and one cannot push out another.
    let flood = flood.with_field("Retry-After", "7").with_field("Retry-After", "8").with_field("Retry-After", "9");
    assert_eq!(flood.headers_named("retry-after").collect::<Vec<_>>(), ["7", "8"]);
    assert_eq!(flood.headers_named("www-authenticate").count(), 16);
}

#[test]
fn an_element_shows_no_text_in_debug_output() {
    let element = Element::new("t:Body").attr("Id", "SecretId").text("Secret mail").child(Element::new("t:C").text("Secret too"));
    let shown = format!("{element:?}");
    assert!(!shown.contains("Secret"), "{shown}");
}

#[test]
fn small_strings_are_charged_what_the_allocator_charges() {
    // 82,952 elements with twelve one-byte attributes each: 6.3 MB. Each string is a block of at
    // least 32 bytes, which a charge of one byte did not see: it peaked at 134 MB.
    let element = format!("<b{}/>", (0..12).map(|n| format!(" {}=\"1\"", (b'a' + n) as char)).collect::<String>());
    let doc = format!("<a>{}</a>", element.repeat(82_952));
    assert!(doc.len() < 7_000_000);
    assert_eq!(xml::parse(doc.as_bytes()).unwrap_err(), xml::Malformed);
    // A few of them are a document (the budget is 16 times the body, and has to cover every
    // block the tree holds).
    let doc = format!("<a>{}</a>", element.repeat(500));
    assert!(xml::parse(doc.as_bytes()).is_ok());
    // Text is charged the same way: a flood of one-character texts.
    let doc = format!("<a>{}</a>", "<b>x</b>".repeat(60_000));
    assert_eq!(xml::parse(doc.as_bytes()).unwrap_err(), xml::Malformed);
}

// ---- real answers fit ----

const ENVELOPE_OPEN: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Header>\
    <h:ServerVersionInfo MajorVersion=\"15\" MinorVersion=\"20\" MajorBuildNumber=\"7933\" MinorBuildNumber=\"10\" Version=\"V2018_01_08\" xmlns:h=\"http://schemas.microsoft.com/exchange/services/2006/types\" xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"/>\
    </s:Header><s:Body>";
const ENVELOPE_CLOSE: &str = "</s:Body></s:Envelope>";

/// An id the way a server writes one: base64 of about `len` characters.
fn id(kind: &str, n: usize, len: usize) -> String {
    let seed = format!("{kind}{n:08}");
    seed.chars().cycle().take(len).map(|c| if c.is_ascii_alphanumeric() { c } else { 'A' }).collect()
}

fn sync_item(n: usize) -> String {
    format!(
        "<t:Create><t:Message><t:ItemId Id=\"{}\" ChangeKey=\"{}\"/><t:ParentFolderId Id=\"{}\" ChangeKey=\"AQAAAA==\"/>\
         <t:ItemClass>IPM.Note</t:ItemClass><t:Subject>Quarterly figures, part {n} &amp; what comes next</t:Subject>\
         <t:DateTimeReceived>2026-10-2{}T09:{:02}:11Z</t:DateTimeReceived><t:Size>{}</t:Size><t:Importance>Normal</t:Importance>\
         <t:IsRead>{}</t:IsRead><t:HasAttachments>false</t:HasAttachments><t:InternetMessageId>&lt;{}@mail.example.org&gt;</t:InternetMessageId>\
         <t:From><t:Mailbox><t:Name>Sender Number {n}</t:Name><t:EmailAddress>sender{n}@example.org</t:EmailAddress><t:RoutingType>SMTP</t:RoutingType></t:Mailbox></t:From>\
         </t:Message></t:Create>",
        id("item", n, 152),
        id("key", n, 44),
        id("folder", 1, 152),
        n % 10,
        n % 60,
        4_000 + n,
        n.is_multiple_of(2),
        id("mid", n, 36)
    )
}

#[test]
fn a_realistic_sync_folder_items_answer_fits_with_room_to_spare() {
    let items: String = (0..512).map(sync_item).collect();
    let body = format!(
        "{ENVELOPE_OPEN}<m:SyncFolderItemsResponse xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\
         <m:ResponseMessages><m:SyncFolderItemsResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode>\
         <m:SyncState>{}</m:SyncState><m:IncludesLastItemInRange>false</m:IncludesLastItemInRange><m:Changes>{items}</m:Changes>\
         </m:SyncFolderItemsResponseMessage></m:ResponseMessages></m:SyncFolderItemsResponse>{ENVELOPE_CLOSE}",
        id("state", 1, 800)
    );
    let (tree, cost) = xml::parse_with_cost(body.as_bytes()).unwrap();
    let budget = xml::budget(body.len());
    // At most half of the budget, so that a longer name or a few more fields do not tip it.
    assert!(cost * 2 <= budget, "{cost} of {budget} for {} bytes", body.len());
    let reply = Reply::parse(body.as_bytes()).unwrap();
    let messages = reply.messages("SyncFolderItems").unwrap();
    let changes = messages[0].ok().unwrap().child_in(soap::MESSAGES, "Changes").unwrap();
    assert_eq!(changes.children.len(), 512);
    let first = changes.children[0].child_in(soap::TYPES, "Message").unwrap();
    assert_eq!(first.child_in(soap::TYPES, "ItemId").unwrap().attribute("Id").map(str::len), Some(152));
    assert_eq!(first.child_in(soap::TYPES, "From").unwrap().child_in(soap::TYPES, "Mailbox").unwrap().text_of("EmailAddress"), "sender0@example.org");
    assert!(tree.is(soap::SOAP, "Envelope"));
}

#[test]
fn a_get_item_answer_with_ten_megabytes_of_mime_fits() {
    let mime = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVowMTIzNDU2Nzg5".repeat(10 * 1024 * 1024 / 48);
    let body = format!(
        "{ENVELOPE_OPEN}<m:GetItemResponse xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\">\
         <m:ResponseMessages><m:GetItemResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode><m:Items><t:Message>\
         <t:MimeContent CharacterSet=\"UTF-8\">{mime}</t:MimeContent><t:ItemId Id=\"{}\" ChangeKey=\"{}\"/></t:Message></m:Items>\
         </m:GetItemResponseMessage></m:ResponseMessages></m:GetItemResponse>{ENVELOPE_CLOSE}",
        id("item", 1, 152),
        id("key", 1, 44)
    );
    assert!(body.len() > 10 * 1024 * 1024 && body.len() < http::RESPONSE_LIMIT);
    let (tree, cost) = xml::parse_with_cost(body.as_bytes()).unwrap();
    assert!(cost * 2 <= xml::budget(body.len()), "{cost} of {}", xml::budget(body.len()));
    let message = tree.find(&["Body", "GetItemResponse", "ResponseMessages", "GetItemResponseMessage", "Items", "Message"]).unwrap();
    assert_eq!(message.child("MimeContent").unwrap().text.len(), mime.len());
    // Another element's text is still capped.
    let other = format!("<a><b>{}</b></a>", "x".repeat(xml::MAX_TEXT + 1));
    assert_eq!(xml::parse(other.as_bytes()).unwrap().children[0].text, "");
    // ... and the room for a message has an end.
    let huge = format!("<a><MimeContent>{}</MimeContent></a>", "x".repeat(xml::MAX_MIME_TEXT + 1));
    assert_eq!(xml::parse(huge.as_bytes()).unwrap().children[0].text, "");
}
