// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use proptest::prelude::*;

#[test]
fn namespaces_entities_cdata_and_failed_properties_stay_distinct() {
    let root = parse(br#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:e="evil"><d:response><d:href>/a&amp;b&#x2f;c</d:href><d:propstat><d:prop><e:getetag>wrong</e:getetag></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat><d:propstat><d:prop><d:getetag>&quot;v1&quot;</d:getetag><c:calendar-data><![CDATA[BEGIN:VCALENDAR]]></c:calendar-data><c:comp name="VEVENT"/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#).unwrap();
    let row = root.child(DAV, "response").unwrap();
    assert_eq!(row.value(DAV, "href"), Some("/a&b/c"));
    let props: Vec<_> = properties(row).collect();
    assert_eq!(props.len(), 3);
    assert!(props[0].is(DAV, "getetag"));
    assert_eq!(props[0].text, "\"v1\"");
    assert_eq!(props[1].text, "BEGIN:VCALENDAR");
    assert_eq!(props[2].component_name.as_deref(), Some("VEVENT"));
}

#[test]
fn dtd_unknown_entities_namespaces_and_incomplete_trees_fail_closed() {
    for text in [
        "<!DOCTYPE x [<!ENTITY a SYSTEM 'file:///not-read'>]><multistatus xmlns='DAV:'/>",
        "<multistatus xmlns='DAV:'>&unknown;</multistatus>",
        "<d:multistatus/>",
        "<multistatus xmlns='DAV:'><response>",
        "<multistatus xmlns='DAV:'><response></multistatus>",
        "<multistatus xmlns='DAV:'/><multistatus xmlns='DAV:'/>",
        "not XML<multistatus xmlns='DAV:'/>",
        "\u{a0}<multistatus xmlns='DAV:'/>",
        "<multistatus xmlns='wrong'/>",
        "<multistatus xmlns='DAV:'>\0</multistatus>",
        "<multistatus xmlns='DAV:'>&#0;</multistatus>",
        "<multistatus xmlns='DAV:' name='&#0;'/>",
        "",
    ] {
        assert!(parse(text.as_bytes()).is_err(), "{text}");
    }
    assert!(parse(&[0xff]).is_err());
    assert!(parse(&vec![b' '; MAX_BYTES + 1]).is_err());
}

#[test]
fn depth_and_node_counts_are_bounded() {
    let deep = format!("<multistatus xmlns='DAV:'>{}{}{}</multistatus>", "<x>".repeat(MAX_DEPTH), "text", "</x>".repeat(MAX_DEPTH));
    assert!(parse(deep.as_bytes()).is_err());
    let many = format!("<multistatus xmlns='DAV:'>{}</multistatus>", "<response/>".repeat(MAX_NODES));
    assert!(parse(many.as_bytes()).is_err());
}

#[test]
fn inherited_namespace_names_cannot_amplify_allocations() {
    let namespace = "x".repeat(MAX_NAME + 1);
    let document = format!("<d:multistatus xmlns:d='DAV:' xmlns:p='{namespace}'><p:property/><p:property/></d:multistatus>");
    assert!(parse(document.as_bytes()).is_err());
    let document = format!("<multistatus xmlns='DAV:'><comp name='{namespace}'/></multistatus>");
    assert!(parse(document.as_bytes()).is_err());
    let document = format!("<multistatus xmlns='DAV:'><{namespace}/></multistatus>");
    assert!(parse(document.as_bytes()).is_err());
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) { let _ = parse(&bytes); }
}
