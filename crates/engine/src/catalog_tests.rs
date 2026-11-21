// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The parity catalog: the floors that only rise, the Complete and Out of scope marks.

use mailcraft_time::{Date, DateTime};

use crate::{Session, catalog, sample};

/// The Complete count the floor test holds: it only rises, with `catalog::COMPLETE`.
const COMPLETE_FLOOR: usize = 0;

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

#[test]
#[allow(clippy::absurd_extreme_comparisons, reason = "floor starts at 0")]
fn parity_has_a_floor_that_only_rises() {
    let s = session();
    let p = catalog::parity(&s.registry);
    let percent = p["percent"].as_f64().unwrap();
    assert!(percent >= 60.0, "catalog parity fell to {percent}%");
    // Complete has a floor of its own, and never exceeds what has a command.
    let complete = p["complete"].as_u64().unwrap() as usize;
    assert!(complete >= COMPLETE_FLOOR, "Complete fell to {complete}");
    assert!(p["complete"].as_u64() <= p["implemented"].as_u64());
    // Every catalog id is spelled like a command id.
    for f in catalog::features() {
        assert!(f.id.contains('.') && !f.id.contains(' '), "{}", f.id);
    }
    assert!(catalog::parity_markdown(&s.registry).contains("## Missing"));
    assert!(catalog::parity_markdown(&s.registry).contains("## Out of scope"));
}

#[test]
fn out_of_scope_rows_have_reasons_and_leave_the_percentages() {
    let s = session();
    let p = catalog::parity(&s.registry);
    for (id, why) in catalog::OUT_OF_SCOPE {
        assert!(catalog::features().any(|f| f.id == id), "{id} is in the catalog");
        assert!(!why.is_empty() && !why.contains('\n'), "{id} has a one-line reason");
        let listed = |k: &str| p[k].as_array().unwrap().iter().any(|m| m["id"] == id);
        assert!(listed("outOfScope"), "{id} is listed as out of scope");
        assert!(!listed("missing"), "{id} isn't missing");
        assert!(s.registry.get(id).is_none(), "{id} has a command: take it off OUT_OF_SCOPE");
        assert!(!catalog::is_complete(id), "{id} is out of scope, so not Complete");
    }
    let rows = catalog::features().count() as u64;
    assert_eq!(p["total"].as_u64(), Some(rows));
    assert_eq!(p["inScope"].as_u64(), Some(rows - catalog::OUT_OF_SCOPE.len() as u64));
    assert_eq!(p["outOfScopeCount"].as_u64(), Some(catalog::OUT_OF_SCOPE.len() as u64));
    // Mistakes in the lists would hide as duplicates.
    let mut ids: Vec<_> = catalog::OUT_OF_SCOPE.iter().map(|(id, _)| *id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), catalog::OUT_OF_SCOPE.len());
}

#[test]
fn complete_rows_are_catalog_rows_with_a_command() {
    let s = session();
    for &id in catalog::COMPLETE {
        assert!(catalog::features().any(|f| f.id == id), "{id} is in the catalog");
        assert!(s.registry.get(id).is_some(), "{id} has a command");
        assert!(catalog::out_of_scope(id).is_none(), "{id} isn't out of scope");
    }
}
