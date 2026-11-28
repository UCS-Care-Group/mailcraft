// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{Lang, date};

#[test]
fn korean_is_available_and_selected_for_matching_locales() {
    let korean = Lang::Korean;
    assert_eq!(korean.code(), "ko");
    assert!(korean.has_catalogue());
    let expected = if cfg!(target_arch = "wasm32") { Lang::English } else { korean };
    assert_eq!(Lang::available().contains(&korean), !cfg!(target_arch = "wasm32"));
    for code in ["ko", "ko-KR", "ko_KR.UTF-8", "KO_kr"] {
        assert_eq!(Lang::from_code(code), Some(korean), "{code}");
        assert_eq!(Lang::from_pref("auto", Some(code)), expected, "{code}");
    }
    assert_eq!(Lang::from_pref("ko", None), expected);
    assert!(korean.supported_on(false));
    assert!(!korean.supported_on(true));
}

#[test]
fn korean_catalogue_translates_text_plurals_and_dates() {
    let korean = Lang::Korean;
    assert_eq!(korean.t("Inbox"), "받은 편지함");
    assert_eq!(korean.picker_name(), "한국어 (미리 보기)");
    for n in [0, 1, 2, 100] {
        assert_eq!(korean.tn(n, "{n} message", "{n} messages"), "{n} 메시지");
    }
    let day = date::Day { year: 2026, month: 10, day: 11, weekday: 6 };
    assert_eq!(date::long(&korean, day), "2026년 10월 11일 일요일");
    assert_eq!(date::month_day(&korean, 2, 3), "2월 3일");
    assert_eq!(date::numeric(&korean, day), "2026. 10. 11.");
    assert_eq!(date::time(&korean, 21, 5, korean.conventions().twelve_hour), "오후 9:05");
    assert_eq!(korean.conventions().week_start, 6);
    assert!(korean.conventions().twelve_hour);
}
