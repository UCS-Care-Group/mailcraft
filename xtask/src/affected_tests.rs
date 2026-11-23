// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn pkg(name: &str, dir: &str, deps: &[&str], dev: &[&str]) -> Package {
    let build_deps: Vec<String> = deps.iter().map(|d| d.to_string()).collect();
    let mut all = build_deps.clone();
    all.extend(dev.iter().map(|d| d.to_string()));
    Package { name: name.into(), dir: dir.into(), deps: all, build_deps }
}

/// A small workspace: model at the bottom, i18n beside it, engine on model, the binding on engine,
/// the UI and the CLI on top, and xtask.
fn workspace() -> Vec<Package> {
    vec![
        pkg("mailcraft-model", "crates/model", &[], &[]),
        pkg("mailcraft-i18n", "crates/i18n", &[], &[]),
        pkg("mailcraft-mime", "crates/mime", &[], &[]),
        pkg("mailcraft-smime", "crates/smime", &[], &[]),
        pkg("mailcraft-engine", "crates/engine", &["mailcraft-model", "mailcraft-i18n", "mailcraft-smime"], &[]),
        pkg("mailcraft-mobile", "crates/mobile", &["mailcraft-engine"], &[]),
        pkg("mailcraft-ui-egui", "crates/ui-egui", &["mailcraft-engine"], &[]),
        pkg("mailcraft-cli", "apps/mailcraft-cli", &["mailcraft-engine"], &["mailcraft-ui-egui"]),
        pkg("xtask", "xtask", &["mailcraft-i18n"], &[]),
    ]
}

fn files(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn pr(list: &[&str]) -> Plan {
    plan("pull_request", false, Some(&files(list)), &workspace())
}

fn always() -> String {
    TREE_WIDE.iter().map(|(_, t)| *t).collect::<Vec<_>>().join(" | ")
}

#[test]
fn a_leaf_crate_runs_its_own_tests_and_its_dependents() {
    let p = pr(&["crates/ui-egui/src/reading.rs", "docs/gaps.md"]);
    assert_eq!(p.full, None);
    assert_eq!(p.selected, ["mailcraft-ui-egui"]);
    assert_eq!(p.filter(), format!("rdeps(=mailcraft-ui-egui) | {}", always()));
    assert!(p.rust && !p.linux_only);
    assert_eq!(p.matrix(), ALL_OS);
    // The UI isn't part of the phone binding.
    assert!(!p.android_rust && !p.android_app);
    assert!(p.why.iter().any(|w| w == "crates/ui-egui/src/reading.rs -> mailcraft-ui-egui"), "{:?}", p.why);
}

#[test]
fn a_core_crate_selects_itself_and_reaches_the_phone_binding() {
    let p = pr(&["crates/model/src/lib.rs", "crates/model/Cargo.toml"]);
    assert_eq!(p.full, None);
    assert_eq!(p.selected, ["mailcraft-model"]);
    assert!(p.filter().starts_with("rdeps(=mailcraft-model) | "));
    // The binding builds with the model through the engine, so its check runs; the app doesn't.
    assert!(p.android_rust);
    assert!(!p.android_app);
}

#[test]
fn several_packages_are_joined_in_the_filter() {
    let p = pr(&["crates/mime/src/a.rs", "crates/engine/src/b.rs"]);
    assert_eq!(p.selected, ["mailcraft-engine", "mailcraft-mime"]);
    assert_eq!(p.filter(), format!("rdeps(=mailcraft-engine) | rdeps(=mailcraft-mime) | {}", always()));
}

#[test]
fn the_root_manifest_and_other_global_files_run_everything() {
    for f in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        ".cargo/config.toml",
        ".config/nextest.toml",
        "xtask/src/main.rs",
        ".github/workflows/ci.yml",
        ".github/scripts/apt-mirrors.sh",
    ] {
        let p = pr(&["crates/ui-egui/src/a.rs", f]);
        assert!(p.full.as_deref().is_some_and(|r| r.contains(f)), "{f}: {:?}", p.full);
        assert_eq!(p.filter(), "", "{f}");
        assert_eq!(p.matrix(), ALL_OS, "{f}");
        assert!(p.rust && p.android_rust, "{f}");
    }
    // A member's own manifest is that member's.
    assert_eq!(pr(&["crates/engine/Cargo.toml"]).selected, ["mailcraft-engine"]);
}

#[test]
fn a_file_outside_every_package_runs_everything() {
    for f in [
        "assets/fonts/Inter-Regular.ttf",
        "fuzz/seeds/html/a.html",
        "contributors/people.toml",
        "deny.toml",
        "ATTRIBUTION.md",
        "packaging/linux/mailcraft.desktop",
        "apps/mailcraft-android/bindgen/src/main.rs",
    ] {
        let p = pr(&[f]);
        assert!(p.full.as_deref().is_some_and(|r| r.contains("outside every package")), "{f}: {:?}", p.full);
        assert_eq!(p.filter(), "", "{f}");
    }
}

#[test]
fn a_missing_or_empty_diff_runs_everything() {
    let p = plan("pull_request", false, None, &workspace());
    assert!(p.full.as_deref().is_some_and(|r| r.contains("could not be read")));
    assert_eq!(p.filter(), "");
    assert!(p.rust && p.android_app && p.android_rust);
    assert!(plan("pull_request", false, Some(&[]), &workspace()).full.is_some());
}

#[test]
fn the_label_and_every_other_event_run_everything() {
    let p = plan("pull_request", true, Some(&files(&["crates/ui-egui/src/a.rs"])), &workspace());
    assert!(p.full.as_deref().is_some_and(|r| r.contains("full-ci")));
    for event in ["push", "schedule", "workflow_dispatch"] {
        let p = plan(event, false, None, &workspace());
        assert!(p.full.as_deref().is_some_and(|r| r.contains(event)), "{event}");
        assert_eq!(p.filter(), "");
        assert_eq!(p.matrix(), ALL_OS);
        assert!(p.rust && p.android_app && p.android_rust, "{event}");
    }
}

#[test]
fn the_merge_queue_runs_everything_on_linux_alone() {
    let p = plan("merge_group", false, None, &workspace());
    assert!(p.full.is_some());
    assert_eq!(p.filter(), "");
    assert_eq!(p.matrix(), LINUX_ONLY);
    assert!(p.rust && p.android_app && p.android_rust);
}

#[test]
fn documentation_alone_runs_no_rust() {
    let p = pr(&[
        "docs/gaps.md",
        "README.md",
        "docs/log.d/ROADMAP/progress/1.md",
        "packaging/release-notes.d/fixes/1.md",
        ".github/ISSUE_TEMPLATE/bug.yml",
    ]);
    assert!(p.full.is_none() && !p.rust && !p.android_app && !p.android_rust);
    assert_eq!(p.filter(), "");
}

#[test]
fn a_translation_runs_on_linux_alone() {
    let p = pr(&["crates/i18n/po/de.po", "crates/i18n/po/messages.pot", "docs/gaps.md"]);
    assert_eq!(p.full, None);
    assert!(p.rust && p.linux_only);
    assert_eq!(p.matrix(), LINUX_ONLY);
    assert_eq!(p.selected, ["mailcraft-i18n"]);
    // Code beside it puts the other systems back.
    let p = pr(&["crates/i18n/po/de.po", "crates/i18n/src/lib.rs"]);
    assert!(!p.linux_only);
    assert_eq!(p.matrix(), ALL_OS);
}

#[test]
fn the_android_apps_own_files_run_its_job_and_no_rust_tests() {
    let p = pr(&["apps/mailcraft-android/app/src/main/kotlin/Main.kt", "apps/mailcraft-android/app/build.gradle.kts", "docs/mobile.md"]);
    assert_eq!(p.full, None);
    assert!(!p.rust && p.android_app && p.android_rust);
    assert!(p.reason().contains("Android app"));
    // With Rust beside them, the Rust tests run too.
    let p = pr(&["apps/mailcraft-android/app/src/main/kotlin/Main.kt", "crates/ui-egui/src/a.rs"]);
    assert!(p.rust && p.android_app && !p.linux_only);
}

#[test]
fn the_binding_and_the_host_crate_run_the_apps_job() {
    assert!(pr(&["crates/mobile/src/lib.rs"]).android_app);
    // A global file the app is built from: the whole suite and the app.
    let p = pr(&["xtask/src/mobile.rs"]);
    assert!(p.full.is_some() && p.android_app);
    // A global file it isn't: the whole suite without the app's job.
    let p = pr(&["Cargo.lock"]);
    assert!(p.full.is_some() && !p.android_app && p.android_rust);
}

#[test]
fn test_data_read_by_another_package_selects_that_package_too() {
    let p = pr(&["crates/smime/testdata/opaque.eml"]);
    assert_eq!(p.selected, ["mailcraft-mime", "mailcraft-smime"]);
}

#[test]
fn a_nested_package_wins_over_its_parent_folder() {
    let mut ws = workspace();
    ws.push(pkg("inner", "crates/engine/inner", &[], &[]));
    let p = plan("pull_request", false, Some(&files(&["crates/engine/inner/src/a.rs", "crates/engineer.rs"])), &ws);
    // `crates/engineer.rs` is not inside `crates/engine`.
    assert!(p.full.as_deref().is_some_and(|r| r.contains("crates/engineer.rs")));
    let p = plan("pull_request", false, Some(&files(&["crates/engine/inner/src/a.rs"])), &ws);
    assert_eq!(p.selected, ["inner"]);
}

#[test]
fn packages_are_read_from_cargo_metadata() {
    let meta = serde_json::json!({ "packages": [
        { "name": "a", "manifest_path": "/w/crates/a/Cargo.toml", "dependencies": [
            { "name": "b", "kind": null }, { "name": "c", "kind": "dev" }, { "name": "serde", "kind": null } ] },
        { "name": "b", "manifest_path": "/w/crates/b/Cargo.toml", "dependencies": [] },
        { "name": "c", "manifest_path": "/w/apps/c/Cargo.toml", "dependencies": [] },
    ]});
    let p = packages(&meta, Path::new("/w")).unwrap();
    assert_eq!(p[0], Package { name: "a".into(), dir: "crates/a".into(), deps: vec!["b".into(), "c".into()], build_deps: vec!["b".into()] });
    assert_eq!(p[2].dir, "apps/c");
    assert!(packages(&serde_json::json!({ "packages": [{ "name": "x", "manifest_path": "/elsewhere/Cargo.toml" }] }), Path::new("/w")).is_err());
}

/// Every test that reads the whole tree's source (it walks the root it finds two folders up) is in
/// `TREE_WIDE`, so a pull request that changes some other package still runs it.
#[test]
fn every_test_that_reads_the_whole_tree_is_known() {
    let root = crate::root();
    let mut found = Vec::new();
    for top in ["crates", "apps"] {
        walk(&root.join(top), &mut |path| {
            let Ok(text) = std::fs::read_to_string(path) else { return };
            if text.contains(r#"env!("CARGO_MANIFEST_DIR")).join("../..")"#) {
                let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                found.push(rel);
            }
        });
    }
    found.sort();
    let mut known: Vec<String> = TREE_WIDE.iter().map(|(f, _)| f.to_string()).collect();
    known.sort();
    assert_eq!(found, known, "a test that reads the whole tree: add it to affected::TREE_WIDE");
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name();
        if path.is_dir() {
            if name != "target" && name != "mailcraft-android" {
                walk(&path, f);
            }
        } else if path.extension().is_some_and(|x| x == "rs") {
            f(&path);
        }
    }
}
