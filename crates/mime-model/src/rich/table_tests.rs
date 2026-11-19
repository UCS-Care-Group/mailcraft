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

#[test]
fn header_weight_background_and_link_targets_survive_the_round_trip() {
    let html = r##"<table bgcolor="#ff0000"><tr><th style="font-weight:normal">Normal <b>Bold</b></th><td bgcolor="#0000ff"><a href="https://example.org/manual">Manual</a></td></tr></table>"##;
    let pasted = from_html_for_compose(html);
    let saved = to_html(&pasted);
    assert!(saved.contains("font-weight:normal;background-color:#ff0000\">Normal <b>Bold</b>"), "{saved}");
    assert!(saved.contains("background-color:#0000ff"), "{saved}");
    assert_eq!(pasted.to_plain(), "Normal Bold\tManual <https://example.org/manual>");
    assert_eq!(to_html(&from_html_with_pictures(&saved, &mut |_| None)), saved);
}

#[test]
fn zero_row_spans_stop_at_their_row_group() {
    use mailcraft_model::rich::PastedTable;

    let html = r#"<table><thead><tr><td rowspan="0">A</td><td>B</td><tr><td>C</td></thead><tbody><tr><td rowspan="0">D</td><td>E</td></tr><tr><td>F</td></tr><tr><td>G</td></tr></tbody><tbody><tr><td>H</td><td>I</td></tr></tbody></table>"#;
    let pasted = from_html_for_compose(html);
    let table: &PastedTable = pasted.paragraphs()[0].runs[0].table.as_deref().unwrap();
    assert_eq!(table.columns(), 2);
    assert_eq!(table.rows().len(), 6);
    assert_eq!(table.rows()[0][0].rowspan, 2);
    assert_eq!(table.rows()[2][0].rowspan, 3);
    assert_eq!(table.rows()[5][0].rowspan, 1);
    let saved = to_html(&pasted);
    assert_eq!(to_html(&from_html_with_pictures(&saved, &mut |_| None)), saved);
}

#[test]
fn implied_rows_stay_in_their_row_group() {
    let html = r#"<table><tbody><tr><td rowspan="0">A</td><td rowspan="2">B</td></tr></tbody><tbody><tr><td>C</td><td>D</td></tr></tbody></table>"#;
    let pasted = from_html_for_compose(html);
    let table = pasted.paragraphs()[0].runs[0].table.as_deref().unwrap();
    assert_eq!(table.columns(), 2);
    assert_eq!(table.rows().len(), 3);
    assert!(table.rows()[0].iter().all(|cell| cell.rowspan == 2));
    assert!(table.rows()[1].is_empty());
    assert_eq!(table.rows()[2].len(), 2);
    let saved = to_html(&pasted);
    assert_eq!(to_html(&from_html_with_pictures(&saved, &mut |_| None)), saved);
}

#[test]
fn replacing_text_keeps_a_table_in_the_same_paragraph() {
    use mailcraft_model::RichBody;

    let fragment = from_html_for_compose("<table><tr><td>Part</td><td>Price</td></tr></table>");
    let mut body = RichBody::from_plain("Hello world");
    body.select(6, 6).unwrap();
    body.paste(&fragment).unwrap();
    let before = body.text().clone();
    assert_eq!(before.paragraphs().len(), 1);
    assert!(before.find("Price", true, 0, before.len()).is_empty());
    assert!(before.find("o world", true, 0, before.len()).is_empty());
    assert!(body.replace_places(&[(6, 7)], "x").is_err());
    assert_eq!(body.text(), &before);
    let places = before.find("world", true, 0, before.len());
    assert_eq!(places, [(7, 12)]);
    body.replace_places(&places, "there").unwrap();
    assert_eq!(body.text().to_plain(), "Hello Part\tPricethere");
    assert!(to_html(body.text()).contains("<table "));
    assert!(body.undo());
    assert_eq!(body.text(), &before);
}

#[test]
fn empty_rows_obey_the_html_table_row_limit() {
    use mailcraft_html::{BlockKind, MAX_TABLE_ROWS, parse_text_with_tables};

    let html = format!("<table><tr><td>First</td></tr>{}</table>", "<tr></tr>".repeat(MAX_TABLE_ROWS + 1));
    let blocks = parse_text_with_tables(&html, None);
    let table = blocks.iter().find_map(|block| match &block.kind {
        BlockKind::Table(table) => Some(table),
        _ => None,
    });
    assert_eq!(table.unwrap().rows.len(), MAX_TABLE_ROWS);
    assert_eq!(from_html_for_compose(&html).to_plain(), "First");
}

#[test]
fn spreadsheet_clipboard_keeps_spacer_rows_columns_and_row_spans() {
    for html in [
        r#"<style>.xl65 { text-align:right }</style><table><colgroup><col span="3"></colgroup><tr><td rowspan="2">Part</td><td></td><td class="xl65">12,30</td></tr><tr><td></td><td></td></tr><tr><td></td><td></td><td></td></tr></table>"#,
        r#"<table cellspacing="0"><colgroup><col width="80"><col width="20"><col width="40"></colgroup><tr><td rowspan="2">Part</td><td><br></td><td align="right">12,30</td></tr><tr><td><br></td><td><br></td></tr><tr><td><br></td><td><br></td><td><br></td></tr></table>"#,
        r#"<google-sheets-html-origin><table><colgroup><col span="3"></colgroup><tbody><tr><td rowspan="2">Part</td><td></td><td style="text-align:right" data-sheets-value="{}">12,30</td></tr><tr><td></td><td></td></tr><tr><td></td><td></td><td></td></tr></tbody></table></google-sheets-html-origin>"#,
    ] {
        let pasted = from_html_for_compose(html);
        let saved = to_html(&pasted);
        assert!(saved.contains("rowspan=\"2\"") && saved.contains("align=\"right\">12,30"), "{saved}");
        assert_eq!(saved.matches("<tr>").count(), 3, "{saved}");
        assert_eq!(saved.matches("<td ").count(), 8, "{saved}");
        assert_eq!(to_html(&from_html_with_pictures(&saved, &mut |_| None)), saved);
    }
    let span = "<table><tr><td rowspan=\"2\">Part</td></tr><tr></tr></table>";
    let saved = to_html(&from_html_for_compose(span));
    assert!(saved.contains("rowspan=\"2\"") && saved.contains("<tr></tr>"), "{saved}");
    assert_eq!(to_html(&from_html_with_pictures(&saved, &mut |_| None)), saved);
}

#[test]
fn reopening_a_draft_only_preserves_geometry_of_marked_tables() {
    let html = "<table role=\"presentation\"><tr><td>Signature</td><td>Contact</td></tr></table><table border=\"1\"><tr><td>Foreign</td><td>Grid</td></tr></table>";
    let reopened = from_html_with_pictures(html, &mut |_| None);
    let plain = reopened.to_plain();
    for value in ["Signature", "Contact", "Foreign", "Grid"] {
        assert!(plain.contains(value), "{plain}");
    }
    assert!(!to_html(&reopened).contains("<table"));
}

#[test]
fn inserting_a_signature_respects_the_table_storage_budget() {
    use mailcraft_model::rich::{ListKind, MAX_CHARS, Paragraph, RichText, Run};

    let html = "<table><tr><td>x</td></tr></table>";
    let table = from_html_for_compose(html).paragraphs()[0].runs[0].clone();
    let mut message = RichText::from_paragraphs(vec![Paragraph::new(vec![table, Run::plain("y".repeat(MAX_CHARS - 20))], ListKind::None)]);
    let before = message.clone();
    assert!(message.put_signature(&RichText::from_plain("1234567890123456"), 0).is_err());
    assert_eq!(message, before);
    assert!(message.put_signature(&RichText::from_plain("123456789012"), 0).is_ok());
    assert!(message.put_signature(&RichText::from_plain("abcdefghijkl"), 0).is_ok());
    assert!(message.paragraphs().iter().flat_map(|p| &p.runs).any(|run| run.table.is_some()));
}

#[test]
fn undo_charges_shared_table_storage_once() {
    use mailcraft_model::RichBody;
    use mailcraft_model::rich::{CellAlignment, CellText, ListKind, MAX_CHARS, Paragraph, PastedTable, RichText, Run, Style, TableCell};

    let cell = TableCell {
        lines: vec![vec![CellText { text: "x".repeat(MAX_CHARS / 2), style: Style::default() }]],
        header: false,
        colspan: 1,
        rowspan: 1,
        background: None,
        alignment: CellAlignment::Left,
    };
    let table = std::sync::Arc::new(PastedTable::new(1, vec![vec![cell]]).unwrap());
    let mut body = RichBody::new(RichText::from_paragraphs(vec![Paragraph::new(vec![Run::table(table)], ListKind::None)]));
    assert_eq!(body.text().len(), 1);
    assert_eq!(body.text().to_plain().len(), MAX_CHARS / 2);
    for _ in 0..20 {
        body.insert_at(0, "a", &Style::default()).unwrap();
    }
    for _ in 0..20 {
        assert!(body.undo());
    }
    assert!(!body.undo());
    assert_eq!(body.text().len(), 1);
}

#[test]
fn malformed_saved_table_keeps_other_runs() {
    use mailcraft_model::rich::{OBJECT, RichText};

    let saved = serde_json::json!({"paragraphs": [{"runs": [
        {"text": "Before"}, {"text": OBJECT.to_string(), "table": {"columns": 1, "rows": [[{
            "lines": [], "header": false, "colspan": 1, "rowspan": 1, "alignment": "Left"
        }]]}}, {"text": "After"}
    ]}]});
    fn table(body: &mut serde_json::Value) -> &mut serde_json::Value {
        &mut body["paragraphs"][0]["runs"][1]["table"]
    }
    let mut wrong_type = saved.clone();
    *table(&mut wrong_type) = serde_json::json!(17);
    let mut unknown_alignment = saved.clone();
    table(&mut unknown_alignment)["rows"][0][0]["alignment"] = serde_json::json!("Justify");
    let mut missing_span = saved;
    table(&mut missing_span)["rows"][0][0].as_object_mut().unwrap().remove("colspan");
    for malformed in [wrong_type, unknown_alignment, missing_span] {
        let reopened: RichText = serde_json::from_value(malformed).unwrap();
        assert_eq!(reopened.text(), "BeforeAfter");
    }
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
