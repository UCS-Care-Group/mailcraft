// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The signature read from the options of Outlook on the web (#484): the request as the schema has
//! it, the reader over answers made by hand (the shape Exchange Server 2019 answered with, and
//! hostile ones), and the whole round trip against the strict stand-in.

use mailcraft_model::sync::{EwsSignature, MAX_SIGNATURE_HTML, MAX_SIGNATURE_TEXT};

use crate::standin::{AUTHORIZATION, StandIn, block_on, error_message, response};
use crate::standin_mail::ok_message;
use crate::*;

const URL: &str = "http://127.0.0.1:1/EWS/Exchange.asmx";

fn client() -> Client {
    Client::new(URL, Secret::new(AUTHORIZATION)).unwrap()
}

fn entry(key: &str, kind: &str, values: &[&str]) -> (String, String, Vec<String>) {
    (key.to_owned(), kind.to_owned(), values.iter().map(|v| (*v).to_owned()).collect())
}

/// What the stand-in answers to the request, read.
fn asked(standin: &StandIn) -> Result<Option<EwsSignature>, Error> {
    let op = GetUserOptions;
    let reply = block_on(client().call(standin, &op.element())).unwrap()?;
    op.answer(&reply)
}

/// A reply made by hand: the `dictionary` in the shape the server wrote it.
fn read(dictionary: &str) -> Result<Option<EwsSignature>, Error> {
    let inner = format!(
        "<m:UserConfiguration><t:UserConfigurationName Name=\"OWA.UserOptions\"/><t:Dictionary>{dictionary}</t:Dictionary></m:UserConfiguration>"
    );
    let body = response("GetUserConfiguration", &ok_message("GetUserConfiguration", &inner));
    GetUserOptions.answer(&Reply::parse(body.as_bytes()).unwrap())
}

fn dictionary_entry(key: &str, kind: &str, value: &str) -> String {
    format!(
        "<t:DictionaryEntry><t:DictionaryKey><t:Type>String</t:Type><t:Value>{key}</t:Value></t:DictionaryKey>\
         <t:DictionaryValue><t:Type>{kind}</t:Type><t:Value>{value}</t:Value></t:DictionaryValue></t:DictionaryEntry>"
    )
}

#[test]
fn the_request_names_the_options_in_the_root_folder_and_asks_for_the_dictionary() {
    let request = client().request(&GetUserOptions.element()).unwrap();
    let body = String::from_utf8_lossy(request.body()).into_owned();
    assert!(body.contains("<m:GetUserConfiguration>"), "{body}");
    assert!(
        body.contains("<m:UserConfigurationName Name=\"OWA.UserOptions\"><t:DistinguishedFolderId Id=\"root\"/></m:UserConfigurationName>"),
        "{body}"
    );
    assert!(body.contains("<m:UserConfigurationProperties>Dictionary</m:UserConfigurationProperties>"), "{body}");
}

#[test]
fn a_signature_the_owner_saved_is_read_with_when_the_server_adds_it() {
    let standin = StandIn::new();
    standin.state().user_options = Some(vec![
        entry("timezone", "String", &["Central Standard Time"]),
        entry("FavoriteFolders", "StringArray", &["AAA=", "BBB="]),
        entry("signaturehtml", "String", &["<div>Ada Lovelace<br><b>Analytical Engines</b></div>"]),
        entry("signaturetext", "String", &["Ada Lovelace\nAnalytical Engines"]),
        entry("autoaddsignature", "Boolean", &["true"]),
        entry("autoaddsignatureonreply", "Boolean", &["false"]),
    ]);
    let found = asked(&standin).unwrap().unwrap();
    assert_eq!(found.html, "<div>Ada Lovelace<br><b>Analytical Engines</b></div>");
    assert_eq!(found.text, "Ada Lovelace\nAnalytical Engines");
    assert_eq!((found.on_new, found.on_reply), (true, false));
    assert!(standin.state().writes.is_empty(), "reading changes nothing");
}

/// The shape Exchange Server 2019 answered with for a mailbox whose owner never
/// saved a signature: the dictionary holds a few options and none is a signature. (The values are
/// invented; the entry names and types are what the server wrote.)
#[test]
fn the_options_of_a_mailbox_without_a_signature_are_no_signature() {
    let seen = [
        dictionary_entry("timezone", "String", "Central Standard Time"),
        dictionary_entry("IsOptimizedForAccessibility", "Boolean", "false"),
        dictionary_entry("UserOptionsMigrationState", "Integer32", "5"),
    ]
    .concat();
    assert_eq!(read(&seen), Ok(None));
}

#[test]
fn a_mailbox_with_no_options_at_all_is_no_signature_and_not_an_error() {
    let standin = StandIn::new();
    assert_eq!(asked(&standin), Ok(None));
}

#[test]
fn an_empty_signature_is_none() {
    let both = [dictionary_entry("signaturehtml", "String", ""), dictionary_entry("signaturetext", "String", "   ")].concat();
    assert_eq!(read(&both), Ok(None));
}

#[test]
fn an_entry_of_another_type_is_not_a_signature() {
    let odd = [
        dictionary_entry("signaturehtml", "Integer32", "7"),
        dictionary_entry("signaturetext", "StringArray", "text"),
        dictionary_entry("autoaddsignature", "String", "true"),
    ]
    .concat();
    assert_eq!(read(&odd), Ok(None));
}

#[test]
fn the_flags_alone_do_not_make_a_signature() {
    let flags = [dictionary_entry("autoaddsignature", "Boolean", "true"), dictionary_entry("autoaddsignatureonreply", "Boolean", "true")].concat();
    assert_eq!(read(&flags), Ok(None));
}

#[test]
fn markup_is_returned_as_the_server_wrote_it_for_the_reader_of_html_to_judge() {
    // (Escaped in the XML, as the server writes HTML; nothing here runs or is fetched.)
    let hostile =
        dictionary_entry("signaturehtml", "String", "&lt;script&gt;alert(1)&lt;/script&gt;&lt;img src=&quot;http://x.example/p.png&quot;&gt;ok");
    let found = read(&hostile).unwrap().unwrap();
    assert_eq!(found.html, "<script>alert(1)</script><img src=\"http://x.example/p.png\">ok");
}

#[test]
fn a_signature_past_the_sizes_is_none_never_a_cut_one() {
    let big_html = "x".repeat(MAX_SIGNATURE_HTML + 1);
    let standin = StandIn::new();
    standin.state().user_options = Some(vec![entry("signaturehtml", "String", &[&big_html])]);
    assert_eq!(asked(&standin), Ok(None));
    let big_text = "y".repeat(MAX_SIGNATURE_TEXT + 1);
    standin.state().user_options = Some(vec![entry("signaturetext", "String", &[&big_text])]);
    assert_eq!(asked(&standin), Ok(None));
    // At the limit it is taken: the reader's text cap for a dictionary value has room for it.
    let at_limit = "z".repeat(MAX_SIGNATURE_HTML);
    standin.state().user_options = Some(vec![entry("signaturehtml", "String", &[&at_limit])]);
    assert_eq!(asked(&standin).unwrap().map(|s| s.html.len()), Some(MAX_SIGNATURE_HTML));
}

#[test]
fn a_dictionary_of_thousands_of_entries_is_looked_at_up_to_a_bound() {
    let many: String = (0..5_000).map(|n| dictionary_entry(&format!("Option{n}"), "String", "v")).collect();
    let past = dictionary_entry("signaturetext", "String", "late");
    assert_eq!(read(&format!("{many}{past}")), Ok(None), "the signature is past the entries looked at");
    assert_eq!(read(&dictionary_entry("signaturetext", "String", "early")).unwrap().unwrap().text, "early");
}

#[test]
fn a_refusal_is_the_servers_and_a_missing_configuration_is_no_signature() {
    let refused = response("GetUserConfiguration", &error_message("GetUserConfiguration", "ErrorAccessDenied"));
    assert_eq!(GetUserOptions.answer(&Reply::parse(refused.as_bytes()).unwrap()), Err(Error::Server(ResponseCode::AccessDenied)));
    let gone = response("GetUserConfiguration", &error_message("GetUserConfiguration", "ErrorItemNotFound"));
    assert_eq!(GetUserOptions.answer(&Reply::parse(gone.as_bytes()).unwrap()), Ok(None));
}

#[test]
fn two_answers_for_one_request_are_not_an_answer() {
    let one = ok_message("GetUserConfiguration", "<m:UserConfiguration><t:Dictionary/></m:UserConfiguration>");
    let body = response("GetUserConfiguration", &format!("{one}{one}"));
    assert_eq!(GetUserOptions.answer(&Reply::parse(body.as_bytes()).unwrap()), Err(Error::Unexpected));
}

#[test]
fn the_stand_in_holds_the_request_to_the_schema() {
    let standin = StandIn::new();
    let wrong = |operation: &str| {
        let head = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" xmlns:m=\"http://schemas.microsoft.com/exchange/services/2006/messages\" xmlns:t=\"http://schemas.microsoft.com/exchange/services/2006/types\"><s:Header><t:RequestServerVersion Version=\"Exchange2013\"/></s:Header><s:Body>";
        let request = HttpRequest::soap(URL, &Secret::new(AUTHORIZATION), format!("{head}{operation}</s:Body></s:Envelope>")).unwrap();
        let response = block_on(standin.exchange(&request)).unwrap().unwrap();
        String::from_utf8_lossy(response.body()).into_owned()
    };
    // No properties asked for, a folder in the wrong namespace, a property the schema lacks.
    let named = "<m:UserConfigurationName Name=\"OWA.UserOptions\"><t:DistinguishedFolderId Id=\"root\"/></m:UserConfigurationName>";
    let misplaced = "<m:UserConfigurationName Name=\"OWA.UserOptions\"><m:DistinguishedFolderId Id=\"root\"/></m:UserConfigurationName>";
    let properties = |what: &str| format!("<m:UserConfigurationProperties>{what}</m:UserConfigurationProperties>");
    assert!(wrong(&format!("<m:GetUserConfiguration>{named}</m:GetUserConfiguration>")).contains("ErrorSchemaValidation"));
    assert!(
        wrong(&format!("<m:GetUserConfiguration>{misplaced}{}</m:GetUserConfiguration>", properties("Dictionary"))).contains("ErrorSchemaValidation")
    );
    assert!(
        wrong(&format!("<m:GetUserConfiguration>{named}{}</m:GetUserConfiguration>", properties("Everything"))).contains("ErrorSchemaValidation")
    );
}

#[test]
fn the_names_are_matched_whatever_their_case() {
    let odd = [dictionary_entry("SignatureText", "String", "Ada"), dictionary_entry("AUTOADDSIGNATURE", "Boolean", "TRUE")].concat();
    let found = read(&odd).unwrap().unwrap();
    assert_eq!((found.text.as_str(), found.on_new, found.on_reply), ("Ada", true, false));
}
