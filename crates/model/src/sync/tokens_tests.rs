// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! When an access token counts as fresh: the margin before renewing never exceeds half its life,
//! so a provider's short-lived tokens (Keycloak's five minutes) are used, not renewed in a loop.

use super::{FRESH_MARGIN, Tokens};

const NOW: i64 = 1_791_993_600;

fn living(seconds: i64) -> Tokens {
    Tokens { access: "a".into(), expires_at: NOW + seconds, issued_at: NOW, ..Tokens::default() }
}

#[test]
fn a_token_just_handed_out_is_fresh_however_short_it_lives() {
    for life in [60, 300, 600, 900, 3_600] {
        let t = living(life);
        assert!(t.fresh(NOW) && t.fresh_for(NOW, 600), "a token of {life} s just handed out");
    }
}

#[test]
fn a_long_lived_token_keeps_the_whole_margin() {
    // An hour (Microsoft's, Google's): renewed once fewer than the margin and the wish remain.
    let t = living(3_600);
    assert!(t.fresh_for(NOW + 3_600 - FRESH_MARGIN - 600 - 1, 600));
    assert!(!t.fresh_for(NOW + 3_600 - FRESH_MARGIN - 600, 600));
}

#[test]
fn a_short_lived_token_is_renewed_after_half_its_life() {
    let t = living(300);
    assert!(t.fresh_for(NOW + 149, 600));
    assert!(!t.fresh_for(NOW + 150, 600));
    assert!(!t.fresh(NOW + 300));
}

#[test]
fn a_token_whose_start_isnt_known_keeps_the_old_rule() {
    // As every token made before `issued_at` was: the whole margin.
    let t = Tokens { issued_at: 0, ..living(3_600) };
    assert!(t.fresh_for(NOW + 3_600 - FRESH_MARGIN - 600 - 1, 600));
    assert!(!Tokens { issued_at: 0, ..living(300) }.fresh(NOW));
    // Nonsense (it ran out before it began) is no shorter margin either.
    assert!(!Tokens { issued_at: NOW + 10_000, ..living(300) }.fresh(NOW));
    // No access token is never fresh.
    assert!(!Tokens { access: String::new(), ..living(3_600) }.fresh(NOW));
}
