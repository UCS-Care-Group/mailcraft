// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{dictionary_copyright_notice, documents_missing_dictionary_notice};

const FIXTURE: &str = "\
intro mentions Build Date: too early\r\n\
\r\n\
COPYRIGHT, SOURCES, and CREDITS:\r\n\
\r\n\
Copyright 2000-2026 by Kevin Atkinson\r\n\
\r\n\
  Copyright 2016 by Benjamin Titze\r\n\
\r\n\
  Copyright 1993, Geoff Kuenning, Granada Hills, CA\r\n\
\r\n\
  SUCH DAMAGE.\r\n\
\r\n\
Build Date: yesterday\r\n";

#[test]
fn notice_stops_before_build_date_and_drops_trailing_blanks() {
    let notice = dictionary_copyright_notice(FIXTURE).unwrap();
    assert!(notice.starts_with("COPYRIGHT, SOURCES, and CREDITS:\n"));
    assert!(notice.ends_with("SUCH DAMAGE."));
    assert!(!notice.ends_with('\n'));
    assert!(!notice.contains('\r'));
    assert!(!notice.contains("Build Date"));
    assert!(!notice.contains("intro"));
    assert!(notice.contains("Copyright 2000-2026 by Kevin Atkinson"));
    assert!(notice.contains("Copyright 2016 by Benjamin Titze"));
    assert!(notice.contains("Copyright 1993, Geoff Kuenning, Granada Hills, CA"));
}

#[test]
fn markers_only_count_at_the_start_of_a_line() {
    let text = "\
see COPYRIGHT, SOURCES, and CREDITS: in prose
COPYRIGHT, SOURCES, and CREDITS:
Copyright 2000-2026 by Kevin Atkinson
Copyright 2016 by Benjamin Titze
Copyright 1993, Geoff Kuenning, Granada Hills, CA
not a Build Date: mention

Build Date: real
";
    let notice = dictionary_copyright_notice(text).unwrap();
    assert!(notice.contains("not a Build Date: mention"));
    assert!(!notice.contains("in prose"));
    assert!(!notice.contains("real"));
}

#[test]
fn a_document_must_contain_the_whole_notice() {
    let notice = dictionary_copyright_notice(FIXTURE).unwrap();
    assert!(documents_missing_dictionary_notice(&notice, &[("NOTICE", &notice)]).is_empty());
    let crlf = notice.replace('\n', "\r\n");
    assert!(documents_missing_dictionary_notice(&notice, &[("NOTICE", &crlf)]).is_empty());
    let broken = notice.replacen("Copyright 2016 by Benjamin Titze", "Copyright 2016 by someone else", 1);
    let missing = documents_missing_dictionary_notice(&notice, &[("NOTICE", &broken)]);
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(missing[0].contains("NOTICE"));
    assert!(missing[0].contains("Copyright 2016 by Benjamin Titze"));
}

#[test]
fn a_changed_notice_is_reported_even_when_every_copyright_line_remains() {
    let notice = dictionary_copyright_notice(FIXTURE).unwrap();
    let broken = notice.replacen("SUCH DAMAGE.", "SUCH DAMAGE", 1);
    let missing = documents_missing_dictionary_notice(&notice, &[("ATTRIBUTION.md", &broken)]);
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(missing[0].contains("verbatim"));
}

#[test]
fn missing_markers_and_a_missing_copyright_line_fail() {
    let no_start = dictionary_copyright_notice("no notice here\nBuild Date: x\n").unwrap_err();
    assert!(no_start.contains("COPYRIGHT, SOURCES, and CREDITS:"));
    let no_end = dictionary_copyright_notice(
        "COPYRIGHT, SOURCES, and CREDITS:\nCopyright 2000-2026 by Kevin Atkinson\nCopyright 2016 by Benjamin Titze\nCopyright 1993, Geoff Kuenning, Granada Hills, CA\n",
    )
    .unwrap_err();
    assert!(no_end.contains("Build Date:"));
    let dropped = dictionary_copyright_notice(
        "COPYRIGHT, SOURCES, and CREDITS:\nCopyright 2000-2026 by Kevin Atkinson\nCopyright 1993, Geoff Kuenning, Granada Hills, CA\n\nBuild Date: x\n",
    )
    .unwrap_err();
    assert!(dropped.contains("Copyright 2016 by Benjamin Titze"));
}

#[test]
fn the_shipped_notice_is_in_attribution_and_notice() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let readme = std::fs::read_to_string(root.join("assets/dictionaries/en_US/README_en_US.txt")).unwrap();
    let notice = dictionary_copyright_notice(&readme).unwrap();
    assert!(!notice.contains("Build Date:"));
    assert!(notice.contains("Copyright 2000-2026 by Kevin Atkinson"));
    let attribution = std::fs::read_to_string(root.join("ATTRIBUTION.md")).unwrap();
    let notice_file = std::fs::read_to_string(root.join("NOTICE")).unwrap();
    let missing = documents_missing_dictionary_notice(&notice, &[("ATTRIBUTION.md", &attribution), ("NOTICE", &notice_file)]);
    assert!(missing.is_empty(), "{missing:?}");
}
