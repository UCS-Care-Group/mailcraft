// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{Lang, date};

#[test]
fn simplified_chinese_is_available_and_selected_for_matching_locales() {
    let chinese = Lang::ChineseSimplified;
    assert_eq!(chinese.code(), "zh_CN");
    assert!(chinese.has_catalogue());
    let expected = if cfg!(target_arch = "wasm32") { Lang::English } else { chinese };
    assert_eq!(Lang::available().contains(&chinese), !cfg!(target_arch = "wasm32"));
    for code in ["zh", "zh-CN", "zh-SG", "zh-Hans", "zh_Hans_CN.UTF-8", "ZH_cn"] {
        assert_eq!(Lang::from_code(code), Some(chinese), "{code}");
        assert_eq!(Lang::from_pref("auto", Some(code)), expected, "{code}");
    }
    assert_eq!(Lang::from_pref("zh_CN", None), expected);
    assert!(chinese.supported_on(false));
    assert!(!chinese.supported_on(true));
    for lang in [Lang::English, Lang::French, Lang::German, Lang::Spanish, Lang::Italian, Lang::Dutch, Lang::Romanian, Lang::Pseudo] {
        assert_eq!(lang.supported_on(true), lang.supported_on(false), "{lang:?}");
    }
}

#[test]
fn traditional_chinese_locales_do_not_select_simplified_chinese() {
    for code in ["zh-TW", "zh-HK", "zh-MO", "zh-Hant", "zh_Hant_CN.UTF-8"] {
        assert_eq!(Lang::from_code(code), None, "{code}");
        assert_eq!(Lang::from_pref("auto", Some(code)), Lang::English, "{code}");
    }
    assert_eq!(Lang::from_code("zh-Hans-HK"), Some(Lang::ChineseSimplified));
}

#[test]
fn chinese_catalogue_translates_text_plurals_and_dates() {
    let chinese = Lang::ChineseSimplified;
    assert_eq!(chinese.t("Inbox"), "收件箱");
    assert_eq!(chinese.picker_name(), "简体中文（预览）");
    for n in [0, 1, 2, 100] {
        assert_eq!(chinese.tn(n, "{n} message", "{n} messages"), "{n} 封邮件");
    }
    let day = date::Day { year: 2026, month: 10, day: 11, weekday: 6 };
    assert_eq!(date::long(&chinese, day), "2026年10月11日，周日");
    assert_eq!(date::month_day(&chinese, 2, 3), "2月3日");
    assert_eq!(date::numeric(&chinese, day), "2026/10/11");
    assert_eq!(date::time(&chinese, 21, 5, chinese.conventions().twelve_hour), "21:05");
    assert_eq!(chinese.conventions().week_start, 0);
}
