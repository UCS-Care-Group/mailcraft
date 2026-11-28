// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Typed authentication results remain authoritative when streaming pieces are combined.

use crate::sync::SyncResult;

#[test]
fn auth_verdict_absorbs_refusal_without_overriding_unknown_legacy_errors() {
    for first in [None, Some(false), Some(true)] {
        for last in [None, Some(false), Some(true)] {
            for legacy_errors in [false, true] {
                let mut result = SyncResult { auth_refused: first, ..Default::default() };
                let errors = if legacy_errors { vec!["legacy failure".into()] } else { Vec::new() };
                result.absorb(SyncResult { auth_refused: last, errors, ..Default::default() });
                let expected = if first == Some(true) || last == Some(true) {
                    Some(true)
                } else if last.is_none() && legacy_errors {
                    None
                } else {
                    first.or(last)
                };
                assert_eq!(result.auth_refused, expected);
            }
        }
    }
    let mut legacy = SyncResult { errors: vec!["legacy sign-in failure".into()], ..Default::default() };
    legacy.absorb(SyncResult { auth_refused: Some(false), ..Default::default() });
    assert_eq!(legacy.auth_refused, None);
    legacy.absorb(SyncResult { auth_refused: Some(true), ..Default::default() });
    legacy.absorb(SyncResult { auth_refused: Some(false), ..Default::default() });
    assert_eq!(legacy.auth_refused, Some(true));
    let decoded: SyncResult = serde_json::from_str(r#"{"authRefused":false}"#).unwrap();
    assert_eq!(decoded.auth_refused, Some(false));
}
