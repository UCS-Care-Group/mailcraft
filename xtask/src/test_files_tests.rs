// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn finds_an_inline_test_module_only_under_cfg_test() {
    assert!(has_inline_tests("fn a() {}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n}\n"));
    assert!(has_inline_tests("#[cfg(test)]\n#[allow(clippy::x)]\n  pub mod checks {\n}\n"));
    // A test module in a file of its own, and a module that isn't a test's.
    assert!(!has_inline_tests("#[cfg(test)]\nmod tests;\n"));
    assert!(!has_inline_tests("#[cfg(test)]\n#[path = \"a_tests.rs\"]\nmod tests;\n"));
    assert!(!has_inline_tests("mod inner {\n}\n#[cfg(test)]\nuse x;\nmod other {\n}\n"));
}

#[test]
fn test_files_are_not_checked() {
    for path in ["crates/a/src/tests.rs", "crates/a/src/b_tests.rs", "crates/a/tests/c.rs", "tests/d.rs"] {
        assert!(is_test_file(path), "{path}");
    }
    assert!(!is_test_file("crates/a/src/lib.rs"));
}

#[test]
fn the_list_only_shrinks() {
    let (added, gone) = problems(&["a.rs", "new.rs"], &["a.rs", "moved.rs"]);
    assert_eq!((added, gone), (vec!["new.rs"], vec!["moved.rs"]));
}
