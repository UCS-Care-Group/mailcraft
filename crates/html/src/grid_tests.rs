// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1966: a table's cells take their places in its grid by one rule, for the reading pane and the
//! phone alike: spans never overlap, and every one is bounded. The tables are made up.

use crate::{BlockKind, Cell, MAX_ROW_SPAN, MAX_TABLE_COLUMNS, Place, Row, Table, grid, parse};

fn first_table(html: &str) -> Table {
    parse(html)
        .into_iter()
        .find_map(|b| match b.kind {
            BlockKind::Table(t) => Some(t),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no table in {html}"))
}

fn at(row: usize, col: usize, rows: usize, cols: usize) -> Place {
    Place { row, col, rows, cols }
}

/// An order: the order number spans its two lines, the total spans the first three columns.
pub(crate) const ORDER: &str = "<table border=1>\
    <tr><th>Order</th><th>Item</th><th>Qty</th><th>Price</th></tr>\
    <tr><td rowspan=2>A-1042</td><td>Seed trays</td><td>4</td><td>€12.00</td></tr>\
    <tr><td>Twine, 100 m</td><td>2</td><td>€5.40</td></tr>\
    <tr><td colspan=3>Total</td><td>€17.40</td></tr></table>";

#[test]
fn an_order_table_with_row_and_column_spans_takes_its_places() {
    let g = grid(&first_table(ORDER));
    assert_eq!((g.columns, g.rows), (4, 4));
    let expect = [
        at(0, 0, 1, 1),
        at(0, 1, 1, 1),
        at(0, 2, 1, 1),
        at(0, 3, 1, 1),
        at(1, 0, 2, 1),
        at(1, 1, 1, 1),
        at(1, 2, 1, 1),
        at(1, 3, 1, 1),
        // The row under the order number starts at its second column.
        at(2, 1, 1, 1),
        at(2, 2, 1, 1),
        at(2, 3, 1, 1),
        at(3, 0, 1, 3),
        at(3, 3, 1, 1),
    ];
    assert_eq!(g.places, expect);
}

fn cell(colspan: u16, rowspan: u16) -> Cell {
    Cell { colspan, rowspan, ..Cell::default() }
}

fn table(columns: u16, rows: Vec<Vec<Cell>>) -> Table {
    Table { columns, rows: rows.into_iter().map(|cells| Row { cells }).collect(), ..Table::default() }
}

/// No two places share a slot of the grid, and every place lies inside it.
fn no_overlap(g: &crate::Grid) {
    let mut taken = vec![false; g.columns * g.rows];
    for p in &g.places {
        assert!(p.rows >= 1 && p.cols >= 1, "{p:?}");
        assert!(p.col + p.cols <= g.columns && p.row + p.rows <= g.rows, "{p:?} outside {}x{}", g.columns, g.rows);
        for r in p.row..p.row + p.rows {
            for c in p.col..p.col + p.cols {
                let slot = &mut taken[r * g.columns + c];
                assert!(!*slot, "{p:?} overlaps at {r},{c}");
                *slot = true;
            }
        }
    }
}

#[test]
fn a_column_span_stops_at_a_column_a_row_span_from_above_covers() {
    // The second row's first cell asks for three columns, but the third is still covered.
    let t = table(3, vec![vec![cell(1, 1), cell(1, 1), cell(1, 2)], vec![cell(3, 1)]]);
    let g = grid(&t);
    assert_eq!(g.places.get(3), Some(&at(1, 0, 1, 2)));
    no_overlap(&g);
}

#[test]
fn a_cell_with_no_room_left_in_its_row_goes_on_in_a_new_one() {
    let t = table(2, vec![vec![cell(1, 2), cell(1, 1)], vec![cell(1, 1), cell(1, 1)]]);
    let g = grid(&t);
    assert_eq!(g.places, [at(0, 0, 2, 1), at(0, 1, 1, 1), at(1, 1, 1, 1), at(2, 0, 1, 1)]);
    assert_eq!(g.rows, 3);
    no_overlap(&g);
}

#[test]
fn hostile_spans_are_held_to_the_grid() {
    let t = table(u16::MAX, vec![vec![cell(u16::MAX, u16::MAX), cell(0, 0)], vec![cell(9, 9)], vec![cell(1, 1); 200]]);
    let g = grid(&t);
    assert_eq!(g.columns, MAX_TABLE_COLUMNS);
    no_overlap(&g);
    // A table the parser reads keeps its spans within its caps too.
    let html = "<table border=1><tr><td colspan=99999 rowspan=99999>x</td><td rowspan=0>y</td></tr><tr><td>z</td></tr></table>";
    let t = first_table(html);
    assert!(t.rows.iter().flat_map(|r| &r.cells).all(|c| c.rowspan <= MAX_ROW_SPAN));
    no_overlap(&grid(&t));
    assert_eq!(grid(&Table::default()), crate::Grid { places: Vec::new(), columns: 1, rows: 0 });
}
