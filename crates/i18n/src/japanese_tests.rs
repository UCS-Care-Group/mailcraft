// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{Lang, date};

#[test]
fn japanese_is_available_and_selected_for_matching_locales() {
    let japanese = Lang::Japanese;
    assert_eq!(japanese.code(), "ja");
    assert!(japanese.has_catalogue());
    let expected = if cfg!(target_arch = "wasm32") { Lang::English } else { japanese };
    assert_eq!(Lang::available().contains(&japanese), !cfg!(target_arch = "wasm32"));
    for code in ["ja", "ja-JP", "ja_JP.UTF-8", "ja_JP.eucJP", "JA_jp"] {
        assert_eq!(Lang::from_code(code), Some(japanese), "{code}");
        assert_eq!(Lang::from_pref("auto", Some(code)), expected, "{code}");
    }
    assert_eq!(Lang::from_pref("ja", None), expected);
    // The browser has no CJK font yet: Japanese is offered on the desktop only.
    assert!(japanese.supported_on(false));
    assert!(!japanese.supported_on(true));
}

#[test]
fn japanese_catalogue_translates_text_plurals_and_dates() {
    let japanese = Lang::Japanese;
    assert_eq!(japanese.t("Inbox"), "受信トレイ");
    assert_eq!(japanese.picker_name(), "日本語 (プレビュー)");
    for n in [0, 1, 2, 100] {
        assert_eq!(japanese.tn(n, "{n} message", "{n} messages"), "{n} 通のメッセージ");
    }
    let day = date::Day { year: 2026, month: 10, day: 11, weekday: 6 };
    assert_eq!(date::long(&japanese, day), "2026年10月11日 (日曜日)");
    assert_eq!(date::month_day(&japanese, 2, 3), "2月3日");
    assert_eq!(date::numeric(&japanese, day), "2026/10/11");
    assert_eq!(date::time(&japanese, 21, 5, japanese.conventions().twelve_hour), "21:05");
    // The week starts on Sunday in Japan.
    assert_eq!(japanese.conventions().week_start, 6);
}
