// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{from_html_for_compose, from_html_with_pictures, to_html};
use mailcraft_model::rich::{MAX_PASTED_CELLS, MAX_PASTED_COLUMNS, MAX_PASTED_ROWS, MAX_PASTED_SPANS};

#[test]
fn spreadsheet_paste_keeps_cells_and_styles_when_a_draft_reopens() {
    let html = r##"<table><tr><th colspan="2" style="background-color:#f0d080">Parts</th><th>Price</th></tr>
<tr><td><b>Washer</b></td><td></td><td style="text-align:right">12,30</td></tr></table>"##;
    let pasted = from_html_for_compose(html);
    let saved = to_html(&pasted);
    assert!(saved.contains("colspan=\"2\"") && saved.contains("background-color:#f0d080"), "{saved}");
    assert!(saved.contains("<b>Washer</b>") && saved.contains("></td>") && saved.contains("align=\"right\""), "{saved}");
    assert_eq!(pasted.to_plain(), "Parts\tPrice\nWasher\t\t12,30");
    let reopened = from_html_with_pictures(&saved, &mut |_| None);
    assert_eq!(to_html(&reopened), saved);
    assert_eq!(serde_json::from_str::<mailcraft_model::rich::RichText>(&serde_json::to_string(&pasted).unwrap()).unwrap(), pasted);
}

fn range(rows: usize, cols: usize) -> String {
    let row = format!("<tr>{}</tr>", "<td>cell</td>".repeat(cols));
    format!("<table>{}</table>", row.repeat(rows))
}

#[test]
fn reopening_a_draft_keeps_inline_pictures_from_table_cells() {
    let html = r#"<table><tr><td>Before<img src="cid:photo@example.org">After</td></tr></table>"#;
    let reopened = from_html_with_pictures(html, &mut |_| Some(7));
    let runs = &reopened.paragraphs()[0].runs;
    assert_eq!(runs.iter().find_map(|run| run.picture), Some(7));
    assert_eq!(runs.first().unwrap().text, "Before");
    assert_eq!(runs.last().unwrap().text, "After");
}

#[test]
fn oversized_ranges_fall_back_to_text() {
    for html in [
        range(MAX_PASTED_ROWS + 1, 2),
        range(2, MAX_PASTED_COLUMNS + 1),
        range(MAX_PASTED_CELLS / MAX_PASTED_COLUMNS + 1, MAX_PASTED_COLUMNS),
        range(mailcraft_html::MAX_TABLE_ROWS * 2, mailcraft_html::MAX_TABLE_COLUMNS + 1),
    ] {
        let result = from_html_for_compose(&html);
        assert!(result.to_plain().contains("cell"));
        assert!(!to_html(&result).contains("<table"));
    }
}

#[test]
fn a_cell_with_too_many_styled_spans_falls_back_to_text() {
    // The HTML reader bounds blocks independently of the composer: spread the runs over
    // several cells so its own per-block bound cannot hide a missing composer bound.
    let contents = "<b>x</b><i>y</i>".repeat(MAX_PASTED_SPANS / 16 + 1);
    let row = format!("<tr>{}</tr>", format!("<td>{contents}</td>").repeat(8));
    let result = from_html_for_compose(&format!("<table>{row}{row}</table>"));
    assert!(result.to_plain().contains("xy"));
    assert!(!to_html(&result).contains("<table"));
}

#[test]
fn deeply_nested_tables_and_giant_spans_remain_bounded() {
    let html = format!(
        "{}<b>leaf</b><script>danger()</script>{}<table><tr><td rowspan=\"9999999\" colspan=\"9999999\">tail</td></tr></table>",
        "<table><tr><td>".repeat(mailcraft_html::MAX_TABLE_DEPTH * 30),
        "</td></tr></table>".repeat(mailcraft_html::MAX_TABLE_DEPTH * 30),
    );
    let result = from_html_for_compose(&html);
    let saved = to_html(&result);
    assert!(result.to_plain().contains("leaf"));
    assert!(result.to_plain().contains("tail"));
    assert!(!saved.contains("danger()"));
    assert!(!saved.contains("9999999"));
    assert!(saved.matches("<table").count() <= 2);
}
