// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where each cell of a table sits in its grid: the one rule the reading pane and the phone both
//! draw by (#1966).

use crate::{MAX_TABLE_COLUMNS, Table};

/// A cell's place: its first row and column, and the rows and columns it covers (at least 1 each).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub row: usize,
    pub col: usize,
    pub rows: usize,
    pub cols: usize,
}

/// The places of a table's cells, in the order of its rows and their cells, and the grid's size.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grid {
    pub places: Vec<Place>,
    pub columns: usize,
    pub rows: usize,
}

/// Give every cell of `table` its place, minding the cells that span rows and columns. A span is
/// held to the columns there are and stops at a column a cell from above still covers, and a cell
/// that finds no room left in its row goes on in a new one: nothing is ever dropped or laid over
/// another. A row span is held to the rows there are.
pub fn grid(table: &Table) -> Grid {
    let columns = usize::from(table.columns).clamp(1, MAX_TABLE_COLUMNS);
    // Per column, how many rows a cell from above still covers.
    let mut covered = vec![0usize; columns];
    let mut places = Vec::new();
    let mut row = 0usize;
    let next_row = |covered: &mut Vec<usize>, row: &mut usize| {
        covered.iter_mut().for_each(|c| *c = c.saturating_sub(1));
        *row = row.saturating_add(1);
    };
    for source in &table.rows {
        let mut col = 0usize;
        for cell in &source.cells {
            loop {
                while covered.get(col).is_some_and(|c| *c > 0) {
                    col += 1;
                }
                if col < columns {
                    break;
                }
                next_row(&mut covered, &mut row);
                col = 0;
            }
            let wanted = usize::from(cell.colspan).clamp(1, columns.saturating_sub(col).max(1));
            let cols = covered.iter().skip(col).take(wanted).take_while(|c| **c == 0).count().max(1);
            let rows = usize::from(cell.rowspan).max(1);
            covered.iter_mut().skip(col).take(cols).for_each(|c| *c = rows);
            places.push(Place { row, col, rows, cols });
            col = col.saturating_add(cols);
        }
        next_row(&mut covered, &mut row);
    }
    for place in &mut places {
        place.rows = place.rows.min(row.saturating_sub(place.row)).max(1);
    }
    Grid { places, columns, rows: row }
}
