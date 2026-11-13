// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The TNEF reader against synthetic streams, and against arbitrary bytes.

use proptest::prelude::*;

use crate::test_writer::{self as w, Prop, Writer};
use crate::{Error, LeftOut, Limits, Tnef, read};

fn tnef(bytes: &[u8]) -> Tnef {
    read(bytes, &Limits::default()).unwrap()
}

#[test]
fn a_file_with_its_long_name_type_and_data() {
    let bytes = Writer::new().file("Quarterly report.pdf", "application/pdf", b"%PDF-1.7 made up").finish();
    let t = tnef(&bytes);
    assert!(t.is_complete(), "{:?}", t.left_out);
    assert_eq!(t.files.len(), 1);
    let f = &t.files[0];
    assert_eq!(f.name.as_deref(), Some("Quarterly report.pdf"));
    assert_eq!(f.mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(f.data, b"%PDF-1.7 made up");
    assert_eq!(t.body, None);
}

#[test]
fn files_come_in_the_streams_order() {
    let bytes = Writer::new().file("one.txt", "text/plain", b"1").file("two.txt", "text/plain", b"22").legacy_file(b"THREE.TXT", b"333").finish();
    let t = tnef(&bytes);
    let names: Vec<_> = t.files.iter().map(|f| f.name.clone().unwrap()).collect();
    assert_eq!(names, ["one.txt", "two.txt", "THREE.TXT"]);
    assert_eq!(t.files[2].data, b"333");
    assert!(t.is_complete());
}

#[test]
fn a_legacy_title_is_read_in_the_streams_code_page() {
    // "Отчёт.txt" in Windows-1251.
    let title = [0xCE, 0xF2, 0xF7, 0xB8, 0xF2, b'.', b't', b'x', b't'];
    let bytes = Writer::with_codepage(1251).legacy_file(&title, b"data").finish();
    assert_eq!(tnef(&bytes).files[0].name.as_deref(), Some("Отчёт.txt"));
}

#[test]
fn without_an_oem_code_page_the_internet_code_page_is_used() {
    let title = [0xCE, 0xF2, 0xF7, 0xB8, 0xF2];
    let bytes = Writer::with_codepage(0).message_props(&[Prop::Int32(0x3FDE, 1251)]).legacy_file(&title, b"data").finish();
    assert_eq!(tnef(&bytes).files[0].name.as_deref(), Some("Отчёт"));
}

#[test]
fn the_long_name_wins_over_the_legacy_title() {
    let bytes = Writer::new().file("A long name.docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document", b"PK").finish();
    assert_eq!(tnef(&bytes).files[0].name.as_deref(), Some("A long name.docx"));
}

#[test]
fn names_lose_control_characters_and_content_ids_their_brackets() {
    let bytes = Writer::new()
        .attachment(
            1,
            &[
                Prop::Unicode(0x3707, "bad\r\nname\u{7}.png".to_string()),
                Prop::Unicode(0x3712, " <image001.png@01DA0000.00000000> ".to_string()),
                Prop::Binary(0x3701, b"\x89PNG".to_vec()),
            ],
        )
        .finish();
    let f = &tnef(&bytes).files[0];
    assert_eq!(f.name.as_deref(), Some("badname.png"));
    assert_eq!(f.content_id.as_deref(), Some("image001.png@01DA0000.00000000"));
}

#[test]
fn an_attachment_without_its_rend_data_is_still_read() {
    let mut writer = Writer::new();
    writer.attribute(w::ATTACHMENT, w::ATTACH_TITLE, b"loose.bin\0").attribute(w::ATTACHMENT, w::ATTACH_DATA, b"xyz");
    let t = tnef(&writer.finish());
    assert_eq!(t.files.len(), 1);
    assert_eq!(t.files[0].name.as_deref(), Some("loose.bin"));
}

#[test]
fn bytes_that_are_not_tnef_are_refused() {
    assert_eq!(read(b"PK\x03\x04 a zip file", &Limits::default()), Err(Error::NotTnef));
    assert_eq!(read(b"", &Limits::default()), Err(Error::NotTnef));
    assert!(!crate::is_tnef(b"\x78\x9F\x3E"));
    assert!(crate::is_tnef(&Writer::new().finish()));
}

#[test]
fn another_version_is_refused() {
    // The version's data is at 15..19 (signature, key, level, id, length), its checksum at 19.
    let mut bytes = Writer::new().finish();
    assert_eq!(&bytes[15..19], [0, 0, 1, 0]);
    bytes[17] = 2;
    bytes[19] = 2;
    assert_eq!(read(&bytes, &Limits::default()), Err(Error::UnsupportedVersion));
    // A version whose checksum is wrong is no better.
    let mut bytes = Writer::new().finish();
    bytes[19] = 9;
    assert_eq!(read(&bytes, &Limits::default()), Err(Error::UnsupportedVersion));
}

#[test]
fn a_wrong_checksum_leaves_out_its_attribute_and_nothing_else() {
    let mut writer = Writer::new();
    writer.rend_data(1).attribute(w::ATTACHMENT, w::ATTACH_TITLE, b"broken.txt\0").attribute_with_checksum(w::ATTACHMENT, w::ATTACH_DATA, b"abc", 7);
    writer.file("fine.txt", "text/plain", b"ok");
    let t = tnef(&writer.finish());
    assert_eq!(t.files.len(), 1);
    assert_eq!(t.files[0].name.as_deref(), Some("fine.txt"));
    assert_eq!(t.left_out, [LeftOut::BadChecksum, LeftOut::NoData]);
}

#[test]
fn a_message_class_with_a_wrong_checksum_is_no_harm() {
    let mut writer = Writer::new();
    writer.attribute_with_checksum(w::MESSAGE, w::MESSAGE_CLASS, b"IPM.Microsoft Mail.Note\0", 0);
    writer.file("a.txt", "text/plain", b"a");
    let t = tnef(&writer.finish());
    assert!(t.is_complete(), "{:?}", t.left_out);
    assert_eq!(t.files.len(), 1);
}

#[test]
fn an_attached_message_is_left_shut() {
    let inner = Writer::new().file("inside.txt", "text/plain", b"never opened").finish();
    let bytes = Writer::new()
        .attachment(1, &[Prop::Int32(0x3705, 5), Prop::Unicode(0x3001, "Forwarded".into()), Prop::Object(0x3701, w::IID_MESSAGE, inner)])
        .file("kept.txt", "text/plain", b"kept")
        .finish();
    let t = tnef(&bytes);
    assert_eq!(t.files.len(), 1);
    assert_eq!(t.files[0].name.as_deref(), Some("kept.txt"));
    assert_eq!(t.left_out, [LeftOut::AttachedMessage]);
}

#[test]
fn an_ole_object_or_a_reference_is_no_file() {
    let bytes = Writer::new()
        .attachment(2, &[Prop::Binary(0x3701, b"ole".to_vec())])
        .attachment(1, &[Prop::Int32(0x3705, 2), Prop::Unicode(0x3707, "\\\\server\\share\\x.doc".into())])
        .finish();
    let t = tnef(&bytes);
    assert!(t.files.is_empty());
    assert_eq!(t.left_out, [LeftOut::Object]);
}

#[test]
fn an_attachment_with_no_data_is_named() {
    let bytes = Writer::new().attachment(1, &[Prop::Unicode(0x3707, "empty.txt".into())]).finish();
    let t = tnef(&bytes);
    assert!(t.files.is_empty());
    assert_eq!(t.left_out, [LeftOut::NoData]);
}

#[test]
fn a_stream_cut_short_keeps_what_came_before() {
    let mut bytes = Writer::new().file("first.txt", "text/plain", b"1").file("second.txt", "text/plain", b"2").finish();
    bytes.truncate(bytes.len() - 5);
    let t = tnef(&bytes);
    assert_eq!(t.files.len(), 2, "the second's legacy data came before the cut");
    assert!(t.left_out.contains(&LeftOut::Truncated));
}

#[test]
fn named_and_multi_valued_properties_are_read_past() {
    let bytes = Writer::new()
        .attachment(
            1,
            &[
                Prop::NamedInt32 { name: "Made up name".into(), value: 7 },
                Prop::MvInt32(0x6000, vec![1, 2, 3]),
                Prop::MvUnicode(0x6001, vec!["a".into(), "bcd".into()]),
                Prop::Bool(0x7FFE, false),
                Prop::Unicode(0x3707, "after.txt".into()),
                Prop::Binary(0x3701, b"after".to_vec()),
            ],
        )
        .finish();
    let t = tnef(&bytes);
    assert!(t.is_complete(), "{:?}", t.left_out);
    assert_eq!(t.files[0].name.as_deref(), Some("after.txt"));
    assert_eq!(t.files[0].data, b"after");
}

#[test]
fn a_property_of_no_known_size_stops_its_list() {
    let bytes = Writer::new()
        .attachment(
            1,
            &[Prop::Unicode(0x3707, "before.txt".into()), Prop::Binary(0x3701, b"data".to_vec()), Prop::Raw(0x0099, 0x6000, vec![1, 2, 3, 4])],
        )
        .finish();
    let t = tnef(&bytes);
    assert_eq!(t.files[0].name.as_deref(), Some("before.txt"));
    assert_eq!(t.left_out, [LeftOut::Properties]);
}

#[test]
fn the_body_from_its_property_its_attribute_or_its_rich_text() {
    let bytes = Writer::new().message_props(&[Prop::Unicode(0x1000, "Hello from the body\r\n".into())]).finish();
    assert_eq!(tnef(&bytes).body.as_deref(), Some("Hello from the body\r\n"));

    let mut writer = Writer::new();
    writer.attribute(w::MESSAGE, w::BODY, b"Caf\xE9\0");
    assert_eq!(tnef(&writer.finish()).body.as_deref(), Some("Café"));

    let rtf = w::uncompressed_rtf(b"{\\rtf1\\ansi Rich {\\b text}\\par}");
    let bytes = Writer::new().message_props(&[Prop::Binary(0x1009, rtf)]).finish();
    assert_eq!(tnef(&bytes).body.as_deref().map(str::trim), Some("Rich text"));

    // An empty body is none.
    let bytes = Writer::new().message_props(&[Prop::Unicode(0x1000, "  ".into())]).finish();
    assert_eq!(tnef(&bytes).body, None);
}

#[test]
fn the_caps_hold() {
    let bytes = Writer::new().file("1", "text/plain", b"1").file("2", "text/plain", b"2").file("3", "text/plain", b"3").finish();
    let limits = Limits { max_files: 2, ..Limits::default() };
    let t = read(&bytes, &limits).unwrap();
    assert_eq!(t.files.iter().map(|f| f.name.clone().unwrap()).collect::<Vec<_>>(), ["1", "2"]);
    assert_eq!(t.left_out, [LeftOut::Cap]);

    let limits = Limits { max_attributes: 3, ..Limits::default() };
    let t = read(&bytes, &limits).unwrap();
    assert!(t.left_out.contains(&LeftOut::Cap));

    let long = "x".repeat(5_000);
    let bytes = Writer::new().attachment(1, &[Prop::Unicode(0x3707, long), Prop::Binary(0x3701, vec![1])]).finish();
    assert_eq!(tnef(&bytes).files[0].name.as_ref().map(|n| n.chars().count()), Some(1_024));

    let bytes = Writer::new().message_props(&[Prop::Unicode(0x1000, "é".repeat(10))]).finish();
    let t = read(&bytes, &Limits { max_body: 5, ..Limits::default() }).unwrap();
    assert_eq!(t.body.as_deref(), Some("éé"));

    let bytes = Writer::new().attachment(1, &[Prop::MvInt32(0x6000, vec![0; 10]), Prop::Binary(0x3701, vec![1])]).finish();
    let t = read(&bytes, &Limits { max_values: 9, ..Limits::default() }).unwrap();
    assert_eq!(t.left_out, [LeftOut::Properties, LeftOut::NoData]);
}

#[test]
fn many_property_lists_keep_at_most_the_cap_of_properties_and_say_so() {
    let mut writer = Writer::new();
    for _ in 0..10 {
        writer.message_props(&[Prop::Int32(0x6000, 1), Prop::Int32(0x6001, 2)]);
    }
    let t = read(&writer.finish(), &Limits { max_properties: 3, ..Limits::default() }).unwrap();
    assert_eq!(t.left_out, [LeftOut::Cap]);

    // A body in a later list that had no room is missing, and that is said, not passed over.
    let bytes = Writer::new().message_props(&[Prop::Int32(0x6000, 1)]).message_props(&[Prop::Unicode(0x1000, "The body".into())]).finish();
    let t = read(&bytes, &Limits { max_properties: 1, ..Limits::default() }).unwrap();
    assert_eq!(t.body, None);
    assert!(!t.is_complete());
    assert_eq!(t.left_out, [LeftOut::Cap]);
}

#[test]
fn named_properties_count_against_the_cap() {
    let named: Vec<Prop> = (0..5).map(|i| Prop::NamedInt32 { name: format!("n{i}"), value: i }).collect();
    let bytes = Writer::new().message_props(&named).finish();
    assert!(read(&bytes, &Limits::default()).unwrap().is_complete());
    let t = read(&bytes, &Limits { max_properties: 4, ..Limits::default() }).unwrap();
    assert_eq!(t.left_out, [LeftOut::Properties]);
}

#[test]
fn a_body_cut_at_its_cap_is_said() {
    let bytes = Writer::new().message_props(&[Prop::Unicode(0x1000, "abcdef".into())]).finish();
    let t = read(&bytes, &Limits { max_body: 4, ..Limits::default() }).unwrap();
    assert_eq!(t.body.as_deref(), Some("abcd"));
    assert_eq!(t.left_out, [LeftOut::Cap]);
    // One that fits is whole and complete.
    let t = read(&bytes, &Limits { max_body: 6, ..Limits::default() }).unwrap();
    assert_eq!(t.body.as_deref(), Some("abcdef"));
    assert!(t.is_complete());

    // Rich text bigger than the cap isn't read at all, and that is said too.
    let rtf = w::uncompressed_rtf(b"{\\rtf1\\ansi A rich text body longer than the cap\\par}");
    let bytes = Writer::new().message_props(&[Prop::Binary(0x1009, rtf)]).finish();
    let t = read(&bytes, &Limits { max_body: 8, ..Limits::default() }).unwrap();
    assert_eq!(t.body, None);
    assert_eq!(t.left_out, [LeftOut::Cap]);
}

/// A valid stream with a few files, to mutate.
fn sample() -> Vec<u8> {
    Writer::new()
        .message_props(&[Prop::Unicode(0x1000, "text".into()), Prop::Int32(0x3FDE, 1252)])
        .file("a.txt", "text/plain", b"aaaa")
        .legacy_file(b"B.BIN", b"bb")
        .attachment(1, &[Prop::NamedInt32 { name: "n".into(), value: 1 }, Prop::Unicode(0x3707, "c".into()), Prop::Binary(0x3701, b"c".to_vec())])
        .finish()
}

fn check(bytes: &[u8]) {
    if let Ok(t) = read(bytes, &Limits::default()) {
        let out: usize = t.files.iter().map(|f| f.data.len()).sum();
        assert!(out <= bytes.len(), "{out} bytes of files from {} bytes", bytes.len());
        assert!(t.files.len() <= Limits::default().max_files);
    }
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        check(&bytes);
    }

    #[test]
    fn arbitrary_bytes_after_the_signature_never_panic(tail in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let mut bytes = crate::SIGNATURE.to_vec();
        bytes.extend_from_slice(&tail);
        check(&bytes);
    }

    #[test]
    fn a_damaged_stream_never_panics(flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..8), cut in any::<usize>()) {
        let mut bytes = sample();
        for (at, b) in flips {
            let at = at % bytes.len();
            bytes[at] = b;
        }
        bytes.truncate(cut % (bytes.len() + 1));
        check(&bytes);
    }
}

/// Writes the fuzz target's seed files (`fuzz/seeds/tnef/`): `cargo test -p mailcraft-tnef --
/// --ignored write_fuzz_seeds`.
#[test]
#[ignore]
fn write_fuzz_seeds() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/tnef");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("sample.dat"), sample()).unwrap();
    let rtf = w::uncompressed_rtf(b"{\\rtf1\\ansi Rich {\\b text}\\par}");
    std::fs::write(dir.join("rtf-body.dat"), Writer::new().message_props(&[Prop::Binary(0x1009, rtf)]).finish()).unwrap();
}
