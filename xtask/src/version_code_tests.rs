// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn maps_releases_and_release_candidates() {
    assert_eq!(code("1.2.3"), Ok(1_020_399));
    assert_eq!(code("1.2.3-rc.1"), Ok(1_020_301));
    assert_eq!(code("1.2.3-rc.98"), Ok(1_020_398));
    assert_eq!(code("0.12.0"), Ok(120_099));
    assert_eq!(code("0.0.0-rc.1"), Ok(1));
    assert_eq!(code("2099.99.99"), Ok(2_099_999_999));
    assert!(code("2099.99.99").is_ok_and(|c| c <= PLAY_MAX));
}

/// Every version gets its own code, and they rise in release order: Play takes each code once and
/// wants every upload's higher than the last.
#[test]
fn codes_are_distinct_and_rise_in_release_order() {
    let order = [
        "0.9.0",
        "0.12.0-rc.1",
        "0.12.0-rc.2",
        "0.12.0-rc.10",
        "0.12.0",
        "0.12.1-rc.1",
        "0.12.1",
        "0.13.0-rc.1",
        "0.13.0",
        "0.99.99",
        "1.0.0-rc.1",
        "1.0.0",
        "1.2.3-rc.98",
        "1.2.3",
        "1.2.4",
        "2.0.0",
    ];
    let codes: Vec<u32> = order.iter().map(|v| code(v).unwrap()).collect();
    for (pair, versions) in codes.windows(2).zip(order.windows(2)) {
        assert!(pair[0] < pair[1], "{versions:?}: {pair:?}");
    }
}

#[test]
fn refuses_what_has_no_code_of_its_own() {
    for bad in [
        "",
        "1.2",
        "v1.2.3",
        "1.2.3+build",
        "1.2.3-rc",
        "1.2.3-rc.0",
        "1.2.3-rc.99",
        "1.2.3-rc.01",
        "1.2.3-rc.1.1",
        "1.2.3-beta.1",
        "1.2.3-alpha",
        "1.100.0",
        "1.0.100",
        "2100.0.0",
        "99999999999.0.0",
    ] {
        assert!(code(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_workspace_version_has_a_code() {
    let text = std::fs::read_to_string(crate::root().join("Cargo.toml")).unwrap();
    code(&crate::version::read(&text).unwrap()).unwrap();
}
