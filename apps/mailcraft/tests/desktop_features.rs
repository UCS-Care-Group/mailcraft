// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native text fields need eframe's platform clipboard, independently of the rich HTML
//! clipboard used by the composer. The workspace disables eframe's default features.
//! This manifest guard needs no access to the actual clipboard or account credentials.

#[test]
fn desktop_text_fields_enable_the_platform_clipboard() {
    let manifest = include_str!("../Cargo.toml");
    let dependency =
        manifest.lines().find(|line| line.trim_start().starts_with("eframe = ")).expect("desktop eframe dependency must be declared explicitly");
    assert!(
        dependency.contains("\"clipboard\""),
        "eframe defaults are disabled: explicitly enable its clipboard feature so native text fields can paste"
    );
}
