// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dependency layering rules (docs/architecture.md).
//!
//! The rule engine works on a small, metadata-independent model so it can be
//! unit-tested; `from_metadata` builds that model from `cargo metadata`.

use serde_json::Value;

/// Where a workspace crate sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Regular layered crate.
    Layer(u8),
    /// Layer 0, and additionally may depend on no workspace crate at all.
    Standalone,
    /// Test tooling: may depend on anything up to L5 (it sits at L6 for rule
    /// purposes); other crates may use it only as a dev-dependency.
    Testkit,
    /// Binaries and build tooling: exempt from the rules.
    Exempt,
}

impl Class {
    fn layer(self) -> Option<u8> {
        match self {
            Class::Layer(l) => Some(l),
            Class::Standalone => Some(0),
            Class::Testkit => Some(6),
            Class::Exempt => None,
        }
    }
}

/// The layering table. Names are package names without the `mailcraft-`
/// prefix.
pub const TABLE: &[(&str, Class)] = &[
    ("time", Class::Layer(0)),
    ("control-key", Class::Standalone),
    ("charset", Class::Standalone),
    ("link", Class::Standalone),
    ("i18n", Class::Standalone),
    ("model", Class::Layer(1)),
    ("html", Class::Layer(1)),
    ("mime", Class::Layer(2)),
    ("ical", Class::Layer(2)),
    ("vcard", Class::Layer(2)),
    ("proto", Class::Layer(3)),
    // JMAP over any HTTP, beside `proto` and `net`: no sockets and no browser APIs, so it builds for the browser too.
    ("jmap", Class::Layer(3)),
    ("net", Class::Layer(3)),
    ("engine", Class::Layer(4)),
    ("mcp", Class::Layer(5)),
    ("ui-egui", Class::Layer(6)),
    ("testkit", Class::Testkit),
    // apps and tooling
    ("mailcraft", Class::Exempt),
    ("cli", Class::Exempt),
    ("web", Class::Exempt),
    ("xtask", Class::Exempt),
];

/// Explicit orderings *within* a layer (earlier may be used by later): the socket transport
/// (`net`) drives the protocol state machines (`proto`).
pub const INTRA_LAYER_ORDER: &[&[&str]] = &[&["proto", "net"]];

fn intra_layer_allowed(from: &str, to: &str) -> bool {
    let (from, to) = (short_name(from), short_name(to));
    INTRA_LAYER_ORDER.iter().any(|chain| match (chain.iter().position(|n| *n == from), chain.iter().position(|n| *n == to)) {
        (Some(f), Some(t)) => t < f,
        _ => false,
    })
}

/// External crates that constitute a UI toolkit / windowing dependency.
/// Entries ending in `*` are prefixes.
pub const UI_CRATES: &[&str] = &["egui", "eframe", "winit", "egui_kittest", "rfd", "bevy*"];

/// External crates only the apps (the `Exempt` binaries) may use: dispatch2 (the main queue, used to
/// quit the platform's way, `-[NSApplication terminate:]` on macOS) because ending the process is
/// the desktop app's decision, never a library's.
pub const APP_ONLY_CRATES: &[&str] = &["dispatch2"];

/// First layer allowed to use UI crates.
pub const UI_MIN_LAYER: u8 = 6;

pub fn short_name(pkg: &str) -> &str {
    pkg.strip_prefix("mailcraft-").unwrap_or(pkg)
}

pub fn classify(pkg: &str) -> Option<Class> {
    let s = short_name(pkg);
    TABLE.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
}

fn is_ui_crate(name: &str) -> bool {
    UI_CRATES.iter().any(|p| match p.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == *p,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

#[derive(Debug, Clone)]
pub struct Dep {
    pub name: String,
    pub kind: DepKind,
    /// `true` if the dependency is a workspace member.
    pub workspace: bool,
}

#[derive(Debug, Clone)]
pub struct Crate {
    pub name: String,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    Unregistered { krate: String },
    Upward { krate: String, dep: String, from: u8, to: u8, kind: DepKind },
    StandaloneHasWorkspaceDep { krate: String, dep: String },
    TestkitAsNormalDep { krate: String },
    UiBelowL6 { krate: String, dep: String, layer: u8 },
    AppOnly { krate: String, dep: String },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::Unregistered { krate } => {
                write!(f, "{krate}: unknown workspace crate; register it in xtask/src/layers.rs TABLE (see docs/architecture.md)")
            }
            Violation::Upward { krate, dep, from, to, kind } => {
                write!(f, "{krate} (L{from}) -> {dep} (L{to}) [{kind:?}]: may only depend on strictly lower layers")
            }
            Violation::StandaloneHasWorkspaceDep { krate, dep } => {
                write!(f, "{krate}: standalone crate must not depend on workspace crate {dep}")
            }
            Violation::TestkitAsNormalDep { krate } => {
                write!(f, "{krate}: mailcraft-testkit may only be a dev-dependency")
            }
            Violation::UiBelowL6 { krate, dep, layer } => {
                write!(f, "{krate} (L{layer}) depends on UI crate `{dep}`; UI toolkits are only allowed in L6+")
            }
            Violation::AppOnly { krate, dep } => {
                write!(f, "{krate} depends on `{dep}`, which only the apps may use")
            }
        }
    }
}

/// Check all rules. Returns violations sorted for stable output.
pub fn check(crates: &[Crate]) -> Vec<Violation> {
    let mut out = Vec::new();
    for c in crates {
        let Some(class) = classify(&c.name) else {
            out.push(Violation::Unregistered { krate: c.name.clone() });
            continue;
        };
        if class == Class::Exempt {
            continue;
        }
        let layer = class.layer().unwrap_or(0);
        for d in &c.deps {
            // Self dev-dependencies (e.g. to enable features in tests) are fine.
            if d.name == c.name {
                continue;
            }
            if d.workspace {
                if class == Class::Standalone {
                    out.push(Violation::StandaloneHasWorkspaceDep { krate: c.name.clone(), dep: d.name.clone() });
                    continue;
                }
                match classify(&d.name) {
                    // Unregistered deps are reported on their own entry.
                    None => {}
                    Some(Class::Testkit) => {
                        if d.kind != DepKind::Dev {
                            out.push(Violation::TestkitAsNormalDep { krate: c.name.clone() });
                        }
                    }
                    Some(dc) => {
                        let to = dc.layer().unwrap_or(u8::MAX);
                        if to >= layer && !(to == layer && intra_layer_allowed(&c.name, &d.name)) {
                            out.push(Violation::Upward {
                                krate: c.name.clone(),
                                dep: d.name.clone(),
                                from: layer,
                                to: if to == u8::MAX { 7 } else { to },
                                kind: d.kind,
                            });
                        }
                    }
                }
            } else if APP_ONLY_CRATES.contains(&d.name.as_str()) {
                out.push(Violation::AppOnly { krate: c.name.clone(), dep: d.name.clone() });
            } else if layer < UI_MIN_LAYER && is_ui_crate(&d.name) {
                out.push(Violation::UiBelowL6 { krate: c.name.clone(), dep: d.name.clone(), layer });
            }
        }
    }
    out.sort_by_key(|v| v.to_string());
    out.dedup();
    out
}

/// Build the model from `cargo metadata --format-version 1 --no-deps`.
pub fn from_metadata(meta: &Value) -> Result<Vec<Crate>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let members: Vec<&str> = pkgs.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in pkgs {
        let name = p["name"].as_str().ok_or("package without name")?.to_owned();
        let mut deps = Vec::new();
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let dname = d["name"].as_str().unwrap_or_default().to_owned();
            let kind = match d["kind"].as_str() {
                Some("dev") => DepKind::Dev,
                Some("build") => DepKind::Build,
                _ => DepKind::Normal,
            };
            let workspace = members.contains(&dname.as_str()) || d["path"].is_string();
            deps.push(Dep { name: dname, kind, workspace });
        }
        out.push(Crate { name, deps });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn describe(class: Option<Class>) -> String {
    match class {
        Some(Class::Layer(l)) => format!("L{l}"),
        Some(Class::Standalone) => "L0 standalone".into(),
        Some(Class::Testkit) => "testkit".into(),
        Some(Class::Exempt) => "exempt".into(),
        None => "UNREGISTERED".into(),
    }
}

#[cfg(test)]
mod tests {
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
}
