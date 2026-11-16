// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn what_cargo_run_sets_is_taken_out_and_cargos_settings_stay() {
    for key in ["CARGO_PKG_NAME", "CARGO_PKG_VERSION", "CARGO_MANIFEST_DIR", "CARGO_MANIFEST_PATH", "CARGO_CRATE_NAME", "CARGO_BIN_NAME", "OUT_DIR"] {
        assert!(set_by_cargo_run(key), "{key}");
    }
    for key in [
        "CARGO",
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_JOBS",
        "CARGO_NET_RETRY",
        "CARGO_INCREMENTAL",
        "RUSTFLAGS",
        "RUSTC_WRAPPER",
        "PATH",
        "XTASK_TEST_FILTER",
    ] {
        assert!(!set_by_cargo_run(key), "{key}");
    }
}

#[test]
fn the_cargo_it_runs_doesnt_inherit_xtasks_own_variables() {
    // The test itself runs under cargo, which sets CARGO_MANIFEST_DIR for it.
    let c = cargo();
    let removed: Vec<_> = c.get_envs().filter(|(_, v)| v.is_none()).map(|(k, _)| k.to_string_lossy().into_owned()).collect();
    if std::env::var_os("CARGO_MANIFEST_DIR").is_some() {
        assert!(removed.iter().any(|k| k == "CARGO_MANIFEST_DIR"), "{removed:?}");
    }
    assert!(!removed.iter().any(|k| k == "CARGO_HOME" || k == "CARGO_TARGET_DIR"));
}
