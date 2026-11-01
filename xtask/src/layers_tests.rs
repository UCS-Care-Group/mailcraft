// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn c(name: &str, deps: &[(&str, DepKind, bool)]) -> Crate {
    Crate { name: name.into(), deps: deps.iter().map(|(n, k, w)| Dep { name: (*n).into(), kind: *k, workspace: *w }).collect() }
}
use DepKind::*;

#[test]
fn clean_downward_graph_passes() {
    let g = [
        c("mailcraft-time", &[("kurbo", Normal, false)]),
        c("mailcraft-model", &[("mailcraft-time", Normal, true)]),
        c("mailcraft-engine", &[("mailcraft-model", Normal, true), ("mailcraft-testkit", Dev, true)]),
        c("mailcraft-ui-egui", &[("mailcraft-engine", Normal, true), ("egui", Normal, false)]),
        c("mailcraft-cli", &[("mailcraft-ui-egui", Normal, true)]),
    ];
    assert!(check(&g).is_empty(), "{:?}", check(&g));
}

#[test]
fn upward_dependency_flagged() {
    let v = check(&[c("mailcraft-model", &[("mailcraft-engine", Normal, true)])]);
    assert!(matches!(v[..], [Violation::Upward { from: 1, to: 4, .. }]));
}

#[test]
fn sideways_dependency_flagged() {
    let v = check(&[c("mailcraft-mime", &[("mailcraft-ical", Normal, true)])]);
    assert!(matches!(v[..], [Violation::Upward { from: 2, to: 2, .. }]));
}

#[test]
fn l0_foundation_chain_allowed_one_way() {
    assert!(check(&[c("mailcraft-model", &[("mailcraft-time", Normal, true)])]).is_empty());
    let v = check(&[c("mailcraft-time", &[("mailcraft-model", Normal, true)])]);
    assert!(matches!(v[..], [Violation::Upward { from: 0, to: 1, .. }]));
}

#[test]
fn self_dev_dependency_ignored() {
    assert!(check(&[c("mailcraft-model", &[("mailcraft-model", Dev, true)])]).is_empty());
}

#[test]
fn upward_dev_dependency_flagged() {
    let v = check(&[c("mailcraft-time", &[("mailcraft-model", Dev, true)])]);
    assert!(matches!(v[..], [Violation::Upward { kind: Dev, .. }]));
}

#[test]
fn ui_crates_below_l6_flagged() {
    for dep in ["egui", "eframe", "winit", "egui_kittest", "rfd", "bevy_ecs", "bevy"] {
        let v = check(&[c("mailcraft-engine", &[(dep, Normal, false)])]);
        assert!(matches!(v[..], [Violation::UiBelowL6 { layer: 4, .. }]), "{dep}");
    }
    assert!(check(&[c("mailcraft-engine", &[("egui_extras_not", Normal, false)])]).is_empty());
    assert!(!check(&[c("mailcraft-mcp", &[("winit", Normal, false)])]).is_empty());
}

#[test]
fn unregistered_crate_is_error() {
    let v = check(&[c("mailcraft-mystery", &[])]);
    assert!(matches!(&v[..], [Violation::Unregistered { krate }] if krate == "mailcraft-mystery"));
    assert!(v[0].to_string().contains("register"));
}

#[test]
fn testkit_only_as_dev_dependency() {
    let v = check(&[c("mailcraft-proto", &[("mailcraft-testkit", Normal, true)])]);
    assert!(matches!(v[..], [Violation::TestkitAsNormalDep { .. }]));
    assert!(check(&[c("mailcraft-proto", &[("mailcraft-testkit", Dev, true)])]).is_empty());
    // testkit itself may use anything up to L5 but not L6 crates.
    assert!(check(&[c("mailcraft-testkit", &[("mailcraft-engine", Normal, true)])]).is_empty());
    assert!(!check(&[c("mailcraft-testkit", &[("mailcraft-ui-egui", Normal, true)])]).is_empty());
}

#[test]
fn apps_and_xtask_exempt() {
    for app in ["mailcraft", "mailcraft-cli", "mailcraft-web", "xtask"] {
        assert!(check(&[c(app, &[("egui", Normal, false), ("mailcraft-ui-egui", Normal, true)])]).is_empty());
    }
}

#[test]
fn dispatch2_is_for_the_apps_alone() {
    assert!(check(&[c("mailcraft", &[("dispatch2", Normal, false)])]).is_empty());
    for krate in ["mailcraft-ui-egui", "mailcraft-engine", "mailcraft-time"] {
        let v = check(&[c(krate, &[("dispatch2", Normal, false)])]);
        assert!(matches!(&v[..], [Violation::AppOnly { dep, .. }] if dep == "dispatch2"), "{krate}: {v:?}");
    }
}

#[test]
fn the_jmap_client_sits_at_layer_3() {
    assert_eq!(classify("mailcraft-jmap"), Some(Class::Layer(3)));
    // It may use what is below it...
    let below =
        [("mailcraft-time", Normal, true), ("mailcraft-model", Normal, true), ("mailcraft-mime", Normal, true), ("serde_json", Normal, false)];
    assert!(check(&[c("mailcraft-jmap", &below)]).is_empty());
    // ... and nothing at its own layer or above it, in the code or in the tests.
    for (dep, kind, to) in
        [("mailcraft-net", Normal, 3), ("mailcraft-proto", Normal, 3), ("mailcraft-engine", Dev, 4), ("mailcraft-ui-egui", Normal, 6)]
    {
        let v = check(&[c("mailcraft-jmap", &[(dep, kind, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 3, to: t, .. }] if t == to), "{dep}: {v:?}");
    }
    // The engine and the front end may use it.
    assert!(check(&[c("mailcraft-engine", &[("mailcraft-jmap", Normal, true)])]).is_empty());
    assert!(check(&[c("mailcraft-ui-egui", &[("mailcraft-jmap", Normal, true)])]).is_empty());
}

#[test]
fn the_ews_client_sits_at_layer_3() {
    assert_eq!(classify("mailcraft-ews"), Some(Class::Layer(3)));
    // It may use what is below it...
    let below = [("mailcraft-model", Normal, true), ("mailcraft-mime", Normal, true), ("quick-xml", Normal, false)];
    assert!(check(&[c("mailcraft-ews", &below)]).is_empty());
    // ... and nothing at its own layer or above it, in the code or in the tests.
    for (dep, kind, to) in
        [("mailcraft-net", Normal, 3), ("mailcraft-jmap", Normal, 3), ("mailcraft-engine", Dev, 4), ("mailcraft-ui-egui", Normal, 6)]
    {
        let v = check(&[c("mailcraft-ews", &[(dep, kind, true)])]);
        assert!(matches!(v[..], [Violation::Upward { from: 3, to: t, .. }] if t == to), "{dep}: {v:?}");
    }
    // The engine may use it.
    assert!(check(&[c("mailcraft-engine", &[("mailcraft-ews", Normal, true)])]).is_empty());
}

#[test]
fn link_uses_names_and_not_the_other_way() {
    assert_eq!(classify("mailcraft-link"), Some(Class::Layer(0)));
    assert!(check(&[c("mailcraft-link", &[("mailcraft-names", Normal, true)])]).is_empty());
    // `names` stays standalone: it may use no workspace crate.
    let v = check(&[c("mailcraft-names", &[("mailcraft-link", Normal, true)])]);
    assert!(matches!(&v[..], [Violation::StandaloneHasWorkspaceDep { dep, .. }] if dep == "mailcraft-link"), "{v:?}");
    // The order is for these two alone: another crate at layer 0 may not use `link`.
    let v = check(&[c("mailcraft-time", &[("mailcraft-link", Normal, true)])]);
    assert!(matches!(v[..], [Violation::Upward { from: 0, to: 0, .. }]), "{v:?}");
}

#[test]
fn metadata_parsing() {
    let meta: Value = serde_json::from_str(
        r#"{"packages":[
            {"name":"mailcraft-model","dependencies":[
                {"name":"mailcraft-time","kind":null,"path":"/x/crates/time"},
                {"name":"serde","kind":null},
                {"name":"proptest","kind":"dev"}]},
            {"name":"mailcraft-time","dependencies":[]}
        ]}"#,
    )
    .unwrap();
    let g = from_metadata(&meta).unwrap();
    assert_eq!(g.len(), 2);
    let doc = g.iter().find(|c| c.name == "mailcraft-model").unwrap();
    assert!(doc.deps[0].workspace && !doc.deps[1].workspace);
    assert_eq!(doc.deps[2].kind, Dev);
    assert!(check(&g).is_empty());
}
