// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn calls(text: &str) -> Vec<(usize, &'static str)> {
    scan(text, false).calls
}

#[test]
fn a_bare_clock_call_is_found() {
    let text = "fn poll() {\n    let started = std::time::Instant::now();\n    while started.elapsed() < BUDGET {}\n}\n";
    assert_eq!(calls(text), vec![(2, "Instant::now"), (3, ".elapsed()")]);
    assert_eq!(calls("fn f() { std::thread::sleep(D); let t = SystemTime::now(); }"), vec![(1, "thread::sleep"), (1, "SystemTime::now")]);
}

#[test]
fn native_only_code_is_left_alone() {
    // A statement, a block, an item and a whole module under a native cfg.
    let text = r#"
fn poll() {
    #[cfg(not(target_arch = "wasm32"))]
    let started = std::time::Instant::now();
    #[cfg(target_os = "macos")]
    {
        let now = std::time::SystemTime::now();
    }
    #[cfg(not(target_arch = "wasm32"))]
    let over = f(a, started.elapsed());
    let after = 1;
}
#[cfg(not(target_arch = "wasm32"))]
fn native() -> u64 {
    std::thread::sleep(D);
    Instant::now().elapsed().as_secs()
}
#[cfg(test)]
mod tests {
    fn t() { let t = Instant::now(); }
}
"#;
    assert_eq!(calls(text), vec![]);
}

#[test]
fn a_cfg_covers_only_what_it_is_on() {
    let text = r#"
struct S {
    #[cfg(not(target_arch = "wasm32"))]
    started: std::time::Instant,
    other: u8,
}
fn f() {
    #[cfg(not(target_arch = "wasm32"))]
    let a = 1;
    let b = Instant::now();
    #[cfg(target_arch = "wasm32")]
    let c = Instant::now();
    #[cfg(not(target_arch = "wasm32"))]
    fn inner() {}
    let d = x.elapsed();
}
"#;
    assert_eq!(calls(text), vec![(10, "Instant::now"), (12, "Instant::now"), (15, ".elapsed()")]);
}

#[test]
fn comments_strings_and_other_names_are_not_calls() {
    let text = r##"
// Instant::now() panics on wasm32
/* thread::sleep( */
fn f() {
    let a = "Instant::now() { {";
    let b = r#"SystemTime::now()"#;
    let c = '{';
    let d = '\'';
    let e = Stopwatch::start().so_far();
    let f = MyInstant::now();
    let g: &'static str = "";
}
fn h() { let t = Instant::now(); }
"##;
    assert_eq!(calls(text), vec![(13, "Instant::now")]);
}

#[test]
fn modules_are_found_with_their_cfg_and_path() {
    let text = r#"
mod plain;
pub mod public;
#[cfg(not(target_arch = "wasm32"))]
pub mod store;
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod fake;
#[cfg(not(target_arch = "wasm32"))]
#[path = "store_index.rs"]
mod index;
#[path = "mailbox_bulk.rs"]
mod bulk;
mod inline { fn f() {} }
"#;
    let mods = scan(text, false).mods;
    let got: Vec<(&str, Option<&str>, bool)> = mods.iter().map(|m| (m.name.as_str(), m.path.as_deref(), m.native)).collect();
    assert_eq!(
        got,
        vec![
            ("plain", None, false),
            ("public", None, false),
            ("store", None, true),
            ("fake", None, true),
            ("index", Some("store_index.rs"), true),
            ("bulk", Some("mailbox_bulk.rs"), false),
        ]
    );
}

#[test]
fn a_file_under_an_inner_native_cfg_is_native() {
    assert_eq!(calls("#![cfg(not(target_arch = \"wasm32\"))]\nfn f() { Instant::now(); }"), vec![]);
}

#[test]
fn native_cfgs_are_told_from_the_rest() {
    for native in [
        "not(target_arch = \"wasm32\")",
        "test",
        "all(test, not(target_arch = \"wasm32\"))",
        "windows",
        "unix",
        "target_os = \"macos\"",
        "any(target_os = \"linux\", target_os = \"freebsd\")",
        "all(unix, not(target_os = \"macos\"))",
    ] {
        assert!(is_native_cfg(native), "{native}");
    }
    for browser in ["target_arch = \"wasm32\"", "not(test)", "feature = \"x\"", "any(test, feature = \"test-support\")", "not(windows)"] {
        assert!(!is_native_cfg(browser), "{browser}");
    }
}

#[test]
fn a_cfg_is_native_only_when_it_is_false_on_wasm_whatever_else_holds() {
    // True on wasm32 too: `not(any(unix, windows))` holds there, and so may the feature.
    assert!(!is_native_cfg("any(target_os = \"macos\", not(any(unix, windows)))"));
    assert!(!is_native_cfg("any(feature = \"x\", not(target_arch = \"wasm32\"))"));
    // `all` is native when one arm is, `any` when every arm is.
    assert!(is_native_cfg("all(feature = \"x\", not(target_arch = \"wasm32\"))"));
    assert!(is_native_cfg("any(unix, windows)"));
    assert!(is_native_cfg("not(target_family = \"wasm\")"));
    assert!(!is_native_cfg("not(target_os = \"macos\")"));
    // What can't be read is not native.
    assert!(!is_native_cfg("all(test"));
    assert!(!is_native_cfg("weird(test)"));
}

#[test]
fn waits_with_a_time_out_are_calls_too() {
    let text = "fn f() {\n    rx.recv_timeout(D);\n    let g = cv.wait_timeout(g, D);\n    std::thread::park_timeout(D);\n}\n";
    assert_eq!(calls(text), vec![(2, ".recv_timeout("), (3, ".wait_timeout("), (4, "park_timeout(")]);
}

#[test]
fn a_call_that_says_why_the_browser_never_reaches_it_is_allowed() {
    let text = "fn f() {\n    // wasm-clock: only the desktop app makes this\n    rx.recv_timeout(D);\n    rx.recv_timeout(D); // wasm-clock: the same\n    // wasm-clock:\n    rx.recv_timeout(D);\n    rx.recv_timeout(D);\n}\n";
    assert_eq!(calls(text), vec![(6, ".recv_timeout("), (7, ".recv_timeout(")]);
}
