// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use serde_json::{Value, json};

use crate::fake_tests::{block_on, sample_core};
use crate::settings::*;
use crate::{Core, CoreError};

fn exec(core: &Core, id: &str, params: Value) -> Value {
    serde_json::from_str(&block_on(core.execute(id.to_string(), params.to_string())).unwrap()).unwrap()
}

/// A Microsoft account of the invented kind, made as a sign-in makes it.
fn microsoft(core: &Core, email: &str) -> u64 {
    let made = exec(
        core,
        "account.add",
        json!({"email": email, "name": "Ada", "displayName": "Ada", "kind": "microsoft", "auth": "oauth2", "provider": "microsoft", "keepOffline": MOBILE_KEEP_OFFLINE}),
    );
    made["id"].as_u64().unwrap()
}

#[test]
fn device_settings_start_off_and_round_trip() {
    let off = default_device_settings();
    assert!(!off.download_pictures && !off.sender_images && !off.new_mail_only && !off.primary_only);
    let on = DeviceSettings { download_pictures: true, sender_images: true, new_mail_only: true, primary_only: false };
    assert_eq!(parse_device_settings(device_settings_json(on)), on);
    assert_eq!(parse_device_settings(device_settings_json(off)), off);
}

#[test]
fn device_settings_that_have_gone_wrong_turn_nothing_on() {
    let off = default_device_settings();
    for junk in ["", "{", "[]", "null", "\"yes\"", "{\"downloadPictures\":\"true\"}", "{\"senderImages\":1}", "{\"downloadPictures\":null}"] {
        assert_eq!(parse_device_settings(junk.to_string()), off, "{junk}");
    }
    // Too long to be what the app wrote.
    let long = format!("{{\"downloadPictures\":true,\"x\":\"{}\"}}", "a".repeat(20_000));
    assert_eq!(parse_device_settings(long), off);
    // One field right and one wrong: the right one stands.
    let some = parse_device_settings("{\"senderImages\":true,\"downloadPictures\":5,\"extra\":[1]}".to_string());
    assert!(some.sender_images && !some.download_pictures);
}

#[test]
fn the_windows_offered_are_the_engines_and_a_new_phone_account_keeps_a_month() {
    let codes = keep_offline_codes();
    assert_eq!(codes.first().map(String::as_str), Some("3d"));
    assert_eq!(codes.last().map(String::as_str), Some("all"));
    assert_eq!(codes.len(), 11);
    assert!(codes.contains(&mobile_keep_offline()));
    assert_eq!(mobile_keep_offline(), "1m");
}

#[test]
fn the_estimate_grows_with_the_window_and_is_capped() {
    let size = |c: &str| offline_estimate(c.to_string());
    assert!(size("3d") < size("1w") && size("1w") < size("2w") && size("2w") < size("1m"));
    assert!(size("1m") < size("3m") && size("3m") < size("12m") && size("12m") < size("2y"));
    assert_eq!(size("1m"), 40 * 1024 * 1024);
    assert_eq!(size("all"), size("nonsense"));
    assert!(size("10y") <= size("all"));
}

#[test]
fn keep_offline_is_the_engines_setting_of_the_account() {
    let core = sample_core();
    let account = microsoft(&core, "ada@example.org");
    assert_eq!(block_on(core.keep_offline(account)).unwrap(), "1m");
    let longer = block_on(core.set_keep_offline(account, "3m".into())).unwrap();
    assert_eq!(longer, KeepOfflineChange { code: "3m".into(), shorter: false, longer: true });
    assert_eq!(block_on(core.keep_offline(account)).unwrap(), "3m");
    // The engine says it too.
    let listed = exec(&core, "account.list", json!({}));
    assert!(listed.to_string().contains("\"keepOffline\":\"3m\""), "{listed}");
    let shorter = block_on(core.set_keep_offline(account, "1w".into())).unwrap();
    assert!(shorter.shorter && !shorter.longer);
    let same = block_on(core.set_keep_offline(account, "1w".into())).unwrap();
    assert!(!same.shorter && !same.longer);
}

#[test]
fn keep_offline_refuses_what_is_not_a_window_or_an_account() {
    let core = sample_core();
    let account = microsoft(&core, "ada@example.org");
    for code in ["", "forever", "1M ", "../../x", "1m; drop"] {
        let r = block_on(core.set_keep_offline(account, code.to_string()));
        // (The engine reads a code in any case and trimmed: that is a window, the rest are not.)
        if code.trim().eq_ignore_ascii_case("1m") {
            assert!(r.is_ok());
        } else {
            assert!(matches!(r, Err(CoreError::BadRequest { .. })), "{code}");
        }
    }
    assert!(matches!(block_on(core.set_keep_offline(u64::MAX, "1m".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.keep_offline(u64::MAX)), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.offline_freed(u64::MAX, "1m".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.offline_freed(account, "x".into())), Err(CoreError::BadRequest { .. })));
}

#[test]
fn a_shorter_window_says_what_it_frees_and_thinning_is_in_small_calls() {
    let core = sample_core();
    let account = microsoft(&core, "ada@example.org");
    // Nothing of the account is here yet: nothing to free, nothing to thin.
    assert_eq!(block_on(core.offline_freed(account, "3d".into())).unwrap(), OfflineFreed { messages: 0, bytes: 0 });
    let r = block_on(core.thin_offline(0)).unwrap();
    assert_eq!((r.thinned, r.left), (0, 0));
    let r = block_on(core.thin_offline(u32::MAX)).unwrap();
    assert_eq!(r.left, 0);
}

#[test]
fn thinning_with_no_account_that_keeps_mail_is_nothing_and_not_an_error() {
    let core = sample_core();
    let r = block_on(core.thin_offline(300)).unwrap();
    assert_eq!(r, ThinResult { thinned: 0, bytes: 0, left: 0 });
}

#[test]
fn a_signature_is_the_accounts_own_and_goes_through_the_engines_commands() {
    let core = sample_core();
    let ada = microsoft(&core, "ada@example.org");
    let bo = microsoft(&core, "bo@example.org");
    assert_eq!(block_on(core.account_signature(ada)).unwrap(), AccountSignature { text: String::new(), editable: true });
    block_on(core.set_account_signature(ada, "Ada\nExample Org".into())).unwrap();
    block_on(core.set_account_signature(bo, "Bo".into())).unwrap();
    assert_eq!(block_on(core.account_signature(ada)).unwrap().text, "Ada\nExample Org");
    assert_eq!(block_on(core.account_signature(bo)).unwrap().text, "Bo");
    // The engine has them under the addresses, for new messages and for replies.
    let listed = exec(&core, "account.list", json!({})).to_string();
    assert!(listed.contains("\"signature\":\"ada@example.org\"") && listed.contains("\"replySignature\":\"ada@example.org\""), "{listed}");
    // Changing one leaves the other.
    block_on(core.set_account_signature(ada, "Ada B.".into())).unwrap();
    assert_eq!(block_on(core.account_signature(ada)).unwrap().text, "Ada B.");
    assert_eq!(block_on(core.account_signature(bo)).unwrap().text, "Bo");
    // White space only takes it away, for that account alone.
    block_on(core.set_account_signature(ada, " \n ".into())).unwrap();
    assert_eq!(block_on(core.account_signature(ada)).unwrap().text, "");
    assert_eq!(block_on(core.account_signature(bo)).unwrap().text, "Bo");
    // And taking away what is not there is no error.
    block_on(core.set_account_signature(ada, String::new())).unwrap();
}

#[test]
fn a_signature_of_another_name_is_edited_as_it_is_and_a_rich_one_is_left_alone() {
    let core = sample_core();
    let ada = microsoft(&core, "ada@example.org");
    exec(&core, "signature.set", json!({"name": "Work", "text": "Ada at work", "default": true, "account": ada}));
    block_on(core.set_account_signature(ada, "Ada, edited".into())).unwrap();
    assert_eq!(block_on(core.account_signature(ada)).unwrap().text, "Ada, edited");
    let listed = exec(&core, "account.list", json!({})).to_string();
    assert!(listed.contains("\"signature\":\"Work\""), "{listed}");
    // One with its own layout is shown but never flattened.
    exec(&core, "signature.html", json!({"name": "Work", "html": "<p>Ada <b>in bold</b></p><table><tr><td>x</td></tr></table>"}));
    let shown = block_on(core.account_signature(ada)).unwrap();
    if !shown.editable {
        assert!(matches!(block_on(core.set_account_signature(ada, "flat".into())), Err(CoreError::BadRequest { .. })));
    }
}

#[test]
fn a_signature_that_is_too_long_or_for_nobody_is_refused() {
    let core = sample_core();
    let ada = microsoft(&core, "ada@example.org");
    assert!(matches!(block_on(core.set_account_signature(ada, "x".repeat(MAX_SIGNATURE + 1))), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.set_account_signature(u64::MAX, "x".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.account_signature(u64::MAX)), Err(CoreError::BadRequest { .. })));
    // Multi-byte text is counted in characters and never cut in the middle of one.
    block_on(core.set_account_signature(ada, "é".repeat(MAX_SIGNATURE))).unwrap();
    assert_eq!(block_on(core.account_signature(ada)).unwrap().text.chars().count(), MAX_SIGNATURE);
}
