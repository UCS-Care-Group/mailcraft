// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Windows of the system take the main window's icon and application id (#519).

use std::sync::Arc;

use crate::windows::{WindowIdentity, like_main, remember_identity};

#[test]
fn a_window_takes_the_hosts_icon_and_app_id() {
    let ctx = egui::Context::default();
    let icon = Arc::new(egui::IconData { rgba: vec![255; 4], width: 1, height: 1 });
    remember_identity(&ctx, Some(&WindowIdentity { icon: Some(icon.clone()), app_id: Some("example.org.app".into()) }));
    let b = like_main(&ctx, egui::ViewportBuilder::default());
    assert_eq!(b.app_id.as_deref(), Some("example.org.app"));
    assert_eq!(b.icon, Some(icon));
}

#[test]
fn without_an_identity_the_builder_is_left_alone() {
    let ctx = egui::Context::default();
    let b = like_main(&ctx, egui::ViewportBuilder::default().with_title("x"));
    assert!(b.app_id.is_none() && b.icon.is_none());
    assert_eq!(b.title.as_deref(), Some("x"));
}
