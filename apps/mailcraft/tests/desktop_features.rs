// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The resolved desktop clipboard features: plain fields must be able to copy, without a
//! second Wayland data device taking the bounded reader's selection (#1217, #1708).
//! No actual clipboard or account credentials are accessed.

#[test]
fn desktop_text_fields_enable_the_platform_clipboard() {
    // Inspect the resolved graph, including features enabled transitively: checking just the
    // direct dependency would miss another crate turning the competing reader back on.
    let output = std::process::Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["tree", "--locked", "--offline", "-p", "mailcraft", "-e", "features", "-i", "egui-winit", "--prefix", "none"])
        .output()
        .expect("read the resolved clipboard features");
    assert!(output.status.success(), "cargo tree: {}", String::from_utf8_lossy(&output.stderr));
    let features = String::from_utf8(output.stdout).expect("cargo tree emits UTF-8");
    assert!(features.contains("egui-winit feature \"arboard\""), "native text fields need a system clipboard for X11 and copying: {features}");
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    assert!(
        !features.contains("egui-winit feature \"smithay-clipboard\""),
        "a second wl_data_device can steal selections from the bounded Wayland reader: {features}"
    );
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    assert!(features.contains("egui-winit feature \"clipboard\""), "other desktop platforms need eframe's normal clipboard: {features}");
}
