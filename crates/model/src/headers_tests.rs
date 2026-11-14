// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's header lines in two parts (HS): what is read by name stays on the message, the rest
//! goes to its block, and the two come back together as they came.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use proptest::prelude::*;

use crate::headers::{self, HeaderBlock, HeaderLine, MAX_BLOCK_LINES, MAX_HEADER_BLOCK, WORKING, is_working, is_working_name, merge, split};
use crate::{Mailbox, Message};

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
}

/// An Exchange-like header: trace and reports interleaved with what is read by name.
fn exchange() -> Vec<(String, String)> {
    pairs(&[
        (
            "Received",
            "from AM0PR01MB1234.eurprd01.prod.example.org (2603:10a6::1) by AM9PR01MB5678.eurprd01.prod.example.org with HTTPS; Mon, 1 Sep 2025 09:00:00 +0000",
        ),
        ("Authentication-Results", "mx.microsoft.com 1; spf=pass smtp.mailfrom=example.org; dmarc=pass header.from=example.org"),
        ("Received", "from mail.example.org (192.0.2.1) by DB1PEPF00000001.mail.protection.outlook.com (10.167.242.1) with Microsoft SMTP Server"),
        ("X-MS-Exchange-Organization-AuthAs", "Anonymous"),
        ("X-Microsoft-Antispam-Message-Info", "c29tZSBsb25nIGJhc2U2NCBzdHVmZg=="),
        ("DKIM-Signature", "v=1; a=rsa-sha256; d=example.org; s=s1; b=AAAA"),
        ("List-Id", "<news.example.org>"),
        ("X-MS-Exchange-Organization-MessageDirectionality", "Incoming"),
        ("Return-Path", "<bounce@example.org>"),
        ("X-MailCraft-Pst-Item", "abc:12"),
    ])
}

#[test]
fn what_is_read_by_name_stays_and_the_rest_goes_to_the_block_in_order() {
    let all = exchange();
    let (working, block) = split(all.clone());
    let names: Vec<&str> = working.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "Authentication-Results",
            "Received",
            "X-MS-Exchange-Organization-AuthAs",
            "List-Id",
            "X-MS-Exchange-Organization-MessageDirectionality",
            "X-MailCraft-Pst-Item"
        ]
    );
    // Only the hop at Exchange Online's edge stays of the trace.
    assert!(working.iter().any(|(n, v)| n == "Received" && v.contains(".mail.protection.outlook.com")));
    let lines = block.lines();
    let names: Vec<&str> = lines.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Received", "X-Microsoft-Antispam-Message-Info", "DKIM-Signature", "Return-Path"]);
    assert_eq!(lines.iter().map(|l| l.at).collect::<Vec<_>>(), [0, 4, 5, 8]);
    assert_eq!(merge(&working, &lines), all);
}

#[test]
fn names_are_matched_whatever_their_case() {
    assert!(is_working_name("list-id"));
    assert!(is_working_name("AUTHENTICATION-RESULTS"));
    assert!(is_working_name("x-mailcraft-anything"));
    assert!(!is_working_name("X-MailCraft"));
    assert!(!is_working_name("Received"));
    assert!(!is_working("Received", "from a.example.org by b.example.org"));
    assert!(!is_working_name("X-Microsoft-Antispam"));
    for name in WORKING {
        assert!(is_working_name(name), "{name}");
    }
}

#[test]
fn a_line_added_later_goes_after_the_rest_and_one_taken_off_leaves_the_others_in_order() {
    let all = exchange();
    let (mut working, block) = split(all.clone());
    working.push(("X-MailCraft-Answer".into(), "accepted".into()));
    let merged = merge(&working, &block.lines());
    assert_eq!(merged.get(..all.len()), Some(&all[..]));
    assert_eq!(merged.last().map(|(n, _)| n.as_str()), Some("X-MailCraft-Answer"));
    let mut m = Message { headers: all.clone(), ..Default::default() };
    m.split_headers();
    m.remove_header("list-id");
    let expected: Vec<_> = all.iter().filter(|(n, _)| n != "List-Id").cloned().collect();
    assert_eq!(m.all_headers(), expected);
    assert!(!m.has_header("List-Id"));
}

#[test]
fn places_that_make_no_sense_lose_no_line() {
    let working = pairs(&[("List-Id", "a"), ("Sender", "b")]);
    let block = vec![
        HeaderLine { at: usize::MAX, name: "X-A".into(), value: "1".into() },
        HeaderLine { at: 0, name: "X-B".into(), value: "2".into() },
        HeaderLine { at: 7, name: "X-C".into(), value: "3".into() },
    ];
    let merged = merge(&working, &block);
    assert_eq!(merged.len(), 5);
    for name in ["List-Id", "Sender", "X-A", "X-B", "X-C"] {
        assert!(merged.iter().any(|(n, _)| n == name), "{name}");
    }
}

#[test]
fn a_message_splits_and_joins() {
    let mut m = Message { headers: exchange(), ..Default::default() };
    m.split_headers();
    assert_eq!(m.headers.len(), 6);
    assert!(!m.header_block.none_in_memory());
    assert_eq!(m.all_headers(), exchange());
    assert_eq!(m.header("list-id"), Some("<news.example.org>"));
    assert!(m.header("Return-Path").is_none(), "only what stays on the message is looked up");
    // A line added since goes to the block too, the next time.
    m.headers.push(("X-Spam-Score".into(), "1.0".into()));
    m.split_headers();
    assert!(m.headers.iter().all(|(n, v)| is_working(n, v)));
    assert_eq!(m.all_headers().last().map(|(n, _)| n.as_str()), Some("X-Spam-Score"));
    m.join_headers();
    assert!(m.header_block.is_empty());
    assert_eq!(m.headers.len(), exchange().len() + 1);
}

#[test]
fn a_block_kept_outside_is_left_alone() {
    let kept = crate::ContentRef { sha256: [7; 32], len: 900 };
    let mut m = Message { headers: pairs(&[("List-Id", "a"), ("X-New", "b")]), ..Default::default() };
    m.header_block = HeaderBlock { data: Arc::default(), kept: Some(kept) };
    m.split_headers();
    assert_eq!(m.headers.len(), 2);
    assert_eq!(m.header_block.kept, Some(kept));
    assert!(!m.header_block.is_loaded());
    assert_eq!(m.header_block.size(), 900);
    m.join_headers();
    assert_eq!(m.header_block.kept, Some(kept), "nothing in memory to put back");
}

#[test]
fn a_message_round_trips_through_json_with_its_block() {
    let mut m = Message { id: 3, headers: exchange(), ..Default::default() };
    m.split_headers();
    let json = serde_json::to_string(&m).unwrap();
    assert!(json.contains("\"headerBlock\":[[0,\"Received\""), "{json}");
    let back: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(back, m);
    assert_eq!(back.all_headers(), exchange());
}

#[test]
fn a_mailbox_file_has_every_line_in_headers_as_before_and_is_split_when_read() {
    let mut mb = Mailbox::default();
    let mut m = Message { id: 1, subject: "Hi".into(), headers: exchange(), ..Default::default() };
    m.split_headers();
    mb.messages.insert(1, Arc::new(m.clone()));
    let json = serde_json::to_string(&mb).unwrap();
    assert!(!json.contains("headerBlock"), "a MailCraft from before reads every line: {json}");
    assert!(json.contains("X-Microsoft-Antispam-Message-Info"));
    let back: Mailbox = serde_json::from_str(&json).unwrap();
    let read = back.messages.get(&1).unwrap();
    assert_eq!(read.headers, m.headers);
    assert_eq!(read.header_block, m.header_block);
}

#[test]
fn nothing_the_reader_kept_is_left_out_by_the_split() {
    // More lines than a block holds, and more bytes (escaped six times over in its JSON), with a
    // line read by name among them: the block is within its caps, and the rest stays on the message.
    let mut all: Vec<(String, String)> = (0..MAX_BLOCK_LINES + 50).map(|i| ("X-A".to_string(), format!("v{i}"))).collect();
    all.insert(700, ("List-Id".into(), "<l.example.org>".into()));
    all.extend((0..100).map(|_| ("X-B".to_string(), "\u{1}".repeat(8_000))));
    let (working, block) = split(all.clone());
    assert!(block.lines().len() <= MAX_BLOCK_LINES && block.data.len() <= MAX_HEADER_BLOCK);
    assert_eq!(working.len() + block.lines().len(), all.len());
    assert_eq!(merge(&working, &block.lines()), all);
    let mut m = Message { headers: all.clone(), ..Default::default() };
    m.split_headers();
    assert_eq!(m.header("List-Id"), Some("<l.example.org>"));
    assert_eq!(m.all_headers(), all);
    assert!(HeaderBlock::of(&[]).is_empty());
}

#[test]
fn a_line_taken_off_while_the_block_is_kept_outside_leaves_the_others_in_their_places() {
    let all: Vec<(String, String)> = [("List-Id", "<l>"), ("Received", "from a.example.org"), ("Sender", "s@example.org"), ("X-A", "v")]
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    let (headers, block) = split(all);
    let mut m = Message {
        headers,
        header_block: HeaderBlock { data: Arc::default(), kept: Some(crate::ContentRef { sha256: [7; 32], len: 9 }) },
        ..Default::default()
    };
    m.remove_header("List-Id");
    assert!(!m.has_header("List-Id") && m.all_headers().iter().all(|(n, _)| !n.is_empty()));
    // Brought back in: every other line where it was.
    m.header_block.data = block.data.clone();
    let names: Vec<String> = m.all_headers().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["Received", "Sender", "X-A"]);
    // Written whole, the empty line is not among them.
    m.header_block = HeaderBlock::default();
    let written = serde_json::to_string(&crate::Messages::read(std::collections::BTreeMap::from([(1, Arc::new(m))]))).unwrap();
    assert!(!written.contains("[\"\",\"\"]"), "{written}");
}

#[test]
fn bytes_that_are_no_block_read_as_none() {
    for junk in [&b""[..], b"[", b"{}", b"[[1,2,3]]", b"[[\"a\",\"b\",\"c\"]]", b"null", b"[[-1,\"a\",\"b\"]]"] {
        assert!(HeaderBlock::read(junk).is_none(), "{:?}", String::from_utf8_lossy(junk));
    }
    let too_long = format!("[[0,\"X-A\",\"{}\"]]", "a".repeat(MAX_HEADER_BLOCK));
    assert!(HeaderBlock::read(too_long.as_bytes()).is_none());
    assert_eq!(HeaderBlock::read(b"[[2,\"X-A\",\"v\"]]").map(|b| b.lines()), Some(vec![HeaderLine { at: 2, name: "X-A".into(), value: "v".into() }]));
    // Bytes in memory that are damaged are no lines, not a failure.
    let damaged = HeaderBlock { data: Arc::new(b"[[0,".to_vec()), kept: None };
    assert!(damaged.lines().is_empty());
    assert_eq!(serde_json::to_string(&damaged).unwrap(), "[]");
}

#[test]
fn a_hostile_or_damaged_block_in_json_is_no_block_and_the_message_reads() {
    let lines: Vec<String> = (0..MAX_BLOCK_LINES + 10).map(|i| format!("[{i},\"X-A\",\"v\"]")).collect();
    let too_many = format!("{{\"subject\":\"s\",\"headerBlock\":[{}]}}", lines.join(","));
    for json in [
        &too_many[..],
        r#"{"subject":"s","headerBlock":[[1]]}"#,
        r#"{"subject":"s","headerBlock":"x"}"#,
        r#"{"subject":"s","headerBlock":{"a":[1]}}"#,
        r#"{"subject":"s","headerBlock":[[0,"X-A","v"],7]}"#,
    ] {
        let m: Message = serde_json::from_str(json).unwrap();
        assert!(m.header_block.is_empty() && m.subject == "s", "{json}");
    }
    let m: Message = serde_json::from_str(r#"{"headerBlock":[[3,"X-A","v"]]}"#).unwrap();
    assert_eq!(m.header_block.lines(), vec![HeaderLine { at: 3, name: "X-A".into(), value: "v".into() }]);
}

proptest! {
    #[test]
    fn any_bytes_read_as_a_block_or_none(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        if let Some(block) = HeaderBlock::read(&bytes) {
            prop_assert!(block.data.len() <= MAX_HEADER_BLOCK);
            let _ = block.lines();
        }
    }

    #[test]
    fn split_then_merge_is_what_was_split(list in proptest::collection::vec((prop::sample::select(vec!["Received", "List-Id", "X-A", "Sender", "x-mailcraft-z", "DKIM-Signature", "Authentication-Results"]), "[ -~]{0,40}"), 0..40)) {
        let all: Vec<(String, String)> = list.into_iter().map(|(n, v)| (n.to_string(), v)).collect();
        let (working, block) = split(all.clone());
        prop_assert!(working.iter().all(|(n, v)| is_working(n, v)));
        prop_assert_eq!(merge(&working, &block.lines()), all);
    }
}

// ---- every name read is one that stays on the message ----

/// The workspace's root, two folders up from this crate.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The source files of the program (not its tests, examples, benches or fuzz targets).
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if !matches!(name.as_str(), "tests" | "examples" | "benches" | "target" | "fuzz" | ".git" | "node_modules") {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" && name != "test_support.rs" {
            out.push(path);
        }
    }
}

/// The value of a constant a lookup names, by its last path segment.
fn constant(name: &str) -> Option<&'static str> {
    Some(match name {
        "AUTH_AS" => crate::auth_results::AUTH_AS,
        "DIRECTIONALITY" => crate::auth_results::DIRECTIONALITY,
        "RESULTS" => "Authentication-Results",
        "NOT_DOWNLOADED" => crate::NOT_DOWNLOADED,
        "OPENED_FROM" | "OPENED_FILE_HEADER" => "X-MailCraft-Opened-File-SHA256",
        "PST_ITEM_HEADER" => "X-MailCraft-Pst-Item",
        "PLACE_HEADER" => "X-MailCraft-Place",
        "ANSWER_HEADER" => "X-MailCraft-Meeting-Response",
        _ => return None,
    })
}

/// What a lookup's first argument names: a literal, or a known constant.
fn named(arg: &str) -> Result<String, String> {
    let arg = arg.trim();
    if let Some(literal) = arg.strip_prefix('"') {
        return literal.split('"').next().map(str::to_string).ok_or_else(|| arg.to_string());
    }
    let last = arg.rsplit("::").next().unwrap_or(arg);
    constant(last).map(str::to_string).ok_or_else(|| format!("{arg} (add it to `constant` in headers_tests.rs if it names a header)"))
}

/// Where a message's own lines are read directly (a file, and what the line says), and why that
/// is right there.
const READS_ITS_OWN: &[(&str, &str)] = &[
    // Whether a message read has lines to split off.
    ("model/src/messages.rs", "m.headers.iter()"),
    // Whether a message written whole has a line taken off to leave out.
    ("model/src/messages.rs", "self.0.headers.iter()"),
    // The Exchange Online edge hop is found by its place among the lines, which stays on the message.
    ("model/src/auth_results.rs", "m.headers.iter()"),
    // The size of a message from a .pst file: its lines and its block (the next line) both counted.
    ("engine/src/import_pst/item.rs", "m.headers.iter().map(|(k, v)| text(k)"),
];

/// Lookups that are an HTTP request's or answer's, not a message's (a file, and what the line says).
const NOT_A_MESSAGE: &[(&str, &str)] =
    &[("ews/src/autodiscover.rs", "response.header("), ("apps/mailcraft/src/graph_folders_flow.rs", "asked.header(")];

#[test]
fn every_name_the_program_reads_stays_on_the_message() {
    let mut files = Vec::new();
    // The crates that read a mailbox's messages (the others' headers are HTTP's, a catalogue's or
    // another format's).
    for top in
        ["crates/model", "crates/mime-model", "crates/engine", "crates/ui-egui", "crates/mcp", "crates/net", "crates/jmap", "crates/ews", "apps"]
    {
        sources(&workspace().join(top), &mut files);
    }
    // Stand-ins for servers, for the tests, and HTTP.
    files.retain(|f| {
        !f.components().any(|c| c.as_os_str() == "standin") && !f.ends_with("standin.rs") && !f.ends_with("http.rs") && !f.ends_with("https.rs")
    });
    assert!(files.len() > 100, "the sources weren't found");
    let mut problems = Vec::new();
    let mut seen = 0usize;
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        // (With `/` between its parts on every system.)
        let shown = file.strip_prefix(workspace()).unwrap_or(file).display().to_string().replace('\\', "/");
        // This module defines the lookups.
        if shown.ends_with("model/src/headers.rs") {
            continue;
        }
        // Tests make messages as they like; so does what follows `#[cfg(test)]` in a file.
        let test_file = shown.ends_with("tests.rs") || shown.contains("/tests/");
        let mut in_tests = test_file;
        for (n, line) in text.lines().enumerate() {
            in_tests |= line.trim_start().starts_with("#[cfg(test)]");
            // A message's own lines read past the lookups (a line on it taken by place, looked
            // for, or gone through): whatever shape it has, it misses what went to the block.
            // Writing them (a push, a whole new list) is no read.
            let direct = [
                ".headers.iter(",
                ".headers.get(",
                ".headers.first(",
                ".headers.last(",
                ".headers.contains(",
                ".headers.retain(",
                ".headers.into_iter(",
                ".headers[",
                "in &m.headers",
                ".headers.windows(",
            ];
            if !in_tests
                && !READS_ITS_OWN.iter().any(|(f, says)| shown.ends_with(f) && line.contains(says))
                && direct.iter().any(|d| line.contains(d))
                && !line.trim_start().starts_with("//")
            {
                problems.push(format!("{shown}:{}: reads a message's header lines past `header`, `header_values` and `all_headers`", n + 1));
            }
            if NOT_A_MESSAGE.iter().any(|(f, says)| shown.ends_with(f) && line.contains(says)) {
                continue;
            }
            for call in [".header(", ".header_values(", ".has_header(", ".remove_header("] {
                let mut rest = line;
                while let Some(at) = rest.find(call) {
                    let after = rest.get(at + call.len()..).unwrap_or("");
                    let arg = after.split([')', ',']).next().unwrap_or("");
                    seen += 1;
                    match named(arg) {
                        Ok(name) if is_working_name(&name) => {}
                        Ok(name) => problems.push(format!("{shown}:{}: {name} isn't in WORKING", n + 1)),
                        Err(what) => problems.push(format!("{shown}:{}: can't tell which header {what} is", n + 1)),
                    }
                    rest = after;
                }
            }
            // A message's headers looked through by name directly, past the lookups.
            if line.contains(".headers") && line.contains("eq_ignore_ascii_case(") && !line.trim_start().starts_with("//") {
                for piece in line.split("eq_ignore_ascii_case(").skip(1) {
                    let arg = piece.split(')').next().unwrap_or("");
                    // (A name compared with another variable is not a lookup by name.)
                    if !(arg.starts_with('"') || arg.chars().next().is_some_and(|c| c.is_ascii_uppercase()) || arg.contains("::")) {
                        continue;
                    }
                    seen += 1;
                    match named(arg) {
                        Ok(name) if is_working_name(&name) || name == "Received" => {}
                        Ok(name) => problems.push(format!("{shown}:{}: {name} isn't in WORKING", n + 1)),
                        Err(what) => problems.push(format!("{shown}:{}: can't tell which header {what} is", n + 1)),
                    }
                }
            }
        }
    }
    assert!(seen > 10, "no lookups found: the scan is broken");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn the_constants_the_scan_knows_are_the_ones_in_the_code() {
    assert_eq!(constant("NOT_DOWNLOADED"), Some(crate::NOT_DOWNLOADED));
    assert_eq!(headers::OWN_PREFIX, "X-MailCraft-");
}
