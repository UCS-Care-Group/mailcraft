// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn detects_unattributed_assets() {
    let files = vec!["assets/icons/a.svg".to_string(), "crates/x/src/lib.rs".into(), "docs/images/b.png".into(), "tests/fixture.png".into()];
    let md = "| `assets/icons/a.svg` | me | here | MIT |";
    assert_eq!(missing(&files, md), vec!["docs/images/b.png".to_string(), "tests/fixture.png".into()]);
    assert!(!is_asset("crates/x/src/lib.rs"));
    assert!(is_asset("assets/fonts/OFL.txt"));
}

#[test]
fn trademark_files_need_a_row_pointing_at_trademarks_md() {
    let files = vec!["assets/trademarks/a.svg".to_string(), "assets/trademarks/b.png".into(), "assets/trademarks/c.png".into()];
    let md =
        "| `assets/trademarks/a.svg` | UCS | own | trademark, see TRADEMARKS.md |\n| `assets/trademarks/b.png` | UCS | own | MIT OR Apache-2.0 |";
    assert_eq!(missing(&files, md), vec!["assets/trademarks/b.png".to_string(), "assets/trademarks/c.png".into()]);
}
