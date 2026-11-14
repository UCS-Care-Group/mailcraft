// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Never crash: every registered command and every query, given junk, answers `Ok` or `Err`.
//! The idea of the engine's `hostile_params_never_panic`, at the binding: through `Core`, so that a
//! panic that escaped everything would show as `CoreError::Internal`, which is asserted against.

use serde_json::{Value, json};

use crate::CoreError;
use crate::fake_tests::{block_on, sample_core};

fn junk() -> Vec<Value> {
    vec![
        json!(null),
        json!({}),
        json!([]),
        json!("text"),
        json!(-1),
        json!({"id": -1, "ids": [-1, 0, 1.5, "x", null, 18446744073709551615u64], "folder": 0, "index": 1e300, "value": {"deep": [[[]]]}}),
        json!({"id": 18446744073709551615u64, "folder": 18446744073709551615u64, "parent": "x", "account": 1e19, "calendar": -5}),
        json!({"id": 1, "name": "", "title": "", "category": "", "query": "\"\"\"", "by": "nothing", "value": "nothing", "response": "maybe"}),
        json!({"name": "\u{0}\r\n/\\..", "title": "x".repeat(100_000), "category": "\u{202e}", "to": "a@b,;;<<>>\"", "subject": "\r\nBcc: x@example.org", "body": "\u{0}"}),
        json!({"start": "9999-12-31T23:59", "end": "0001-01-01", "date": "9999-12-31", "due": "0000-00-00", "from": "9999-01-01", "occurrence": "x", "minutes": 1e300}),
        json!({"data": "!!!!", "name": "../../etc/passwd", "mime": "\r\n"}),
        json!({"ids": (0..10_000u64).collect::<Vec<_>>()}),
        json!({"extend": true, "rows": 18446744073709551615u64}),
        json!({"account": 1, "download": {"account": 1, "complete": true, "lists": [{"id": "\u{0}"}, null], "tasks": [{"id": "t\r\n", "list": "L", "title": "\u{0}"}, {}, null]}}),
        json!({"compose": 18446744073709551615u64, "body": "\u{0}"}),
        json!({"keep": 1, "others": [1, 1, 0, -1, "x", 1e300], "fields": {"first": "x", "": null}}),
    ]
}

/// Raw texts that are not even JSON, or are JSON of the wrong shape, or enormous.
fn raw() -> Vec<String> {
    vec![
        String::new(),
        "{".into(),
        "null".into(),
        "[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[".into(),
        "\u{0}".into(),
        format!("{{\"id\": {}}}", "9".repeat(5_000)),
        "\"\\ud800\"".into(),
    ]
}

fn not_internal<T: std::fmt::Debug>(what: &str, r: &Result<T, CoreError>) {
    assert!(!matches!(r, Err(CoreError::Internal)), "{what}: a panic was caught");
    // `Session::run` turns a command's panic into this error, so that is caught here too.
    if let Err(CoreError::Failed { reason }) = r {
        assert!(!reason.contains("failed unexpectedly"), "{what}: a command's panic was caught by Session::run");
    }
}

#[test]
fn every_command_with_junk_answers_and_never_panics() {
    let core = sample_core();
    let ids: Vec<&'static str> = mailcraft_engine::cmd::registry().all().iter().map(|c| c.id).collect();
    assert!(ids.len() > 100);
    for round in 0..2 {
        for id in &ids {
            for j in junk() {
                // File paths stay away from the disk: only the in-memory forms are tried.
                if id.starts_with("file.") && j.get("path").is_some() {
                    continue;
                }
                let r = block_on(core.execute((*id).to_string(), j.to_string()));
                not_internal(id, &r);
            }
            for text in raw() {
                let r = block_on(core.execute((*id).to_string(), text));
                not_internal(id, &r);
            }
        }
        if round == 0 {
            // Reachable state for the second pass: the sample again, and something selected.
            let _ = block_on(core.execute("file.new".into(), json!({"sample": true}).to_string()));
        }
        // Whatever happened, the core still answers.
        assert!(block_on(core.folders(0)).is_ok(), "round {round}");
    }
}

#[test]
fn every_query_with_junk_answers_and_never_panics() {
    let core = sample_core();
    let numbers = [0u64, 1, 2, 7, u64::MAX, u64::MAX - 1];
    let counts = [0u32, 1, 5, 200, 201, u32::MAX];
    let texts = ["", "all", "unread", "flagged", "attachments", "toMe", "ALL", "nonsense", "\u{0}", "../..", "\u{202e}x", "x\r\nBcc: y"];
    let dates = ["", "2026-10-14", "9999-12-31", "0000-00-00", "0001-01-01", "2026-13-45", "x", "20261014", "2026-10-14T10:00", "-2026-10-14"];
    let widths = [0.0f32, -1.0, 1.0, 390.0, 16_384.0, 1e30, f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
    let huge = "x".repeat(200_000);

    for n in numbers {
        not_internal("folders", &block_on(core.folders(n)));
        for t in texts {
            for off in counts {
                for c in counts {
                    not_internal("messages", &block_on(core.messages(n, t.to_string(), off, c)));
                    not_internal("people", &block_on(core.people(t.to_string(), off, c)));
                    not_internal("tasks", &block_on(core.tasks(n, t.to_string(), c % 2 == 0, off, c)));
                }
            }
        }
        for w in widths {
            for dark in [false, true] {
                not_internal("message", &block_on(core.message(n, w, dark)));
            }
        }
        for t in texts {
            not_internal("picture", &block_on(core.picture(n, t.to_string())));
        }
        not_internal("picture", &block_on(core.picture(n, huge.clone())));
    }
    for a in dates {
        for b in dates {
            not_internal("calendar", &block_on(core.calendar(a.to_string(), b.to_string())));
        }
    }
    not_internal("people", &block_on(core.people(huge.clone(), 0, 5)));
    not_internal("messages", &block_on(core.messages(1, huge.clone(), 0, 5)));
    not_internal("tasks", &block_on(core.tasks(0, huge, true, 0, 5)));
    not_internal("task_lists", &block_on(core.task_lists()));
    not_internal("today", &block_on(core.today()));

    // Every real message of the sample reads, at every width and theme, in the ways a phone asks.
    let folder = block_on(core.folders(0)).unwrap().folders.iter().find(|f| f.kind == "inbox").map(|f| f.id).unwrap_or(0);
    for row in block_on(core.messages(folder, "all".into(), 0, 200)).unwrap().rows {
        for w in widths {
            for dark in [false, true] {
                assert!(block_on(core.message(row.id, w, dark)).is_ok());
            }
        }
    }
}

#[test]
fn the_text_helpers_never_cut_inside_a_character() {
    for s in ["", "a", "é", "日本語のテキスト", "a\u{301}b\u{301}", "😀😀😀"] {
        for max in 0..8 {
            let cut = crate::text::clip(s, max);
            assert!(s.starts_with(&cut) && cut.chars().count() <= max);
        }
    }
    assert_eq!(crate::text::window(0), 1);
    assert_eq!(crate::text::window(u32::MAX), 200);
}
