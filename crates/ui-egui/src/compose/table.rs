// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Layout of the table objects pasted into a message.

use std::sync::Arc;

use egui::{
    Color32, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2,
    text::{Galley, LayoutJob},
    vec2,
};
use mailcraft_model::rich::{CellAlignment, PastedTable, Style};

use crate::theme::Tokens;

const PADDING: f32 = 4.0;
const MIN_ROW: f32 = 24.0;

struct Cell {
    rect: Rect,
    text: Arc<Galley>,
    background: Option<Color32>,
}

pub(super) struct TableLayout {
    pub size: Vec2,
    cells: Vec<Cell>,
}

impl TableLayout {
    pub fn new(ctx: &egui::Context, table: &PastedTable, tokens: &Tokens, room: f32) -> Self {
        let columns = usize::from(table.columns());
        let mut occupied = vec![0; columns];
        let mut widths = vec![24.0_f32; columns];
        let mut heights = vec![MIN_ROW; table.rows().len()];
        let mut cells = Vec::new();
        let families = ctx.fonts(|fonts| fonts.families());
        for (row_index, row) in table.rows().iter().enumerate() {
            let mut column = 0;
            for cell in row {
                while occupied.get(column).is_some_and(|end| *end > row_index) {
                    column += 1;
                }
                if column >= columns {
                    break;
                }
                let end_column = (column + usize::from(cell.colspan)).min(columns);
                let end_row = (row_index + usize::from(cell.rowspan)).min(table.rows().len());
                if end_column <= column || end_row <= row_index {
                    continue;
                }
                let mut job = LayoutJob {
                    halign: match cell.alignment {
                        CellAlignment::Left => egui::Align::Min,
                        CellAlignment::Center => egui::Align::Center,
                        CellAlignment::Right => egui::Align::Max,
                    },
                    ..Default::default()
                };
                let background =
                    cell.background.as_deref().and_then(mailcraft_model::rich::safe_color).and_then(|color| Color32::from_hex(&color).ok());
                let format = |style: &Style| {
                    let mut style = style.clone().clean();
                    style.bold |= cell.header;
                    let mut format = super::text_format(&style, false, tokens, &|family| families.contains(family));
                    let paper = if format.background != Color32::TRANSPARENT { format.background } else { background.unwrap_or(tokens.surface) };
                    let ink = if style.link.is_some() {
                        tokens.accent_text
                    } else {
                        style.color.as_deref().and_then(|c| Color32::from_hex(c).ok()).unwrap_or(tokens.text)
                    };
                    format.color = if tokens.high_contrast {
                        crate::reading::colors::readable_strong(ink, paper)
                    } else {
                        crate::reading::colors::readable(ink, paper)
                    };
                    format.underline.color = format.color;
                    format
                };
                for (i, line) in cell.lines.iter().enumerate() {
                    if i > 0 {
                        job.append("\n", 0.0, format(&Style::default()));
                    }
                    for run in line {
                        job.append(&run.text, 0.0, format(&run.style));
                    }
                }
                let wanted = ctx.fonts_mut(|f| f.layout_job(job.clone()).size().x).min(440.0) + 2.0 * PADDING;
                let per_column = wanted / (end_column - column) as f32;
                for (slot, width) in occupied.iter_mut().zip(&mut widths).take(end_column).skip(column) {
                    *slot = (*slot).max(end_row);
                    *width = width.max(per_column);
                }
                cells.push((row_index, end_row, column, end_column, job, background));
                column = end_column;
            }
        }
        let total = widths.iter().sum::<f32>();
        let scale = (room / total).min(1.0);
        widths.iter_mut().for_each(|w| *w *= scale);
        let edges = |sizes: &[f32]| {
            std::iter::once(0.0)
                .chain(sizes.iter().scan(0.0, |sum, size| {
                    *sum += size;
                    Some(*sum)
                }))
                .collect::<Vec<_>>()
        };
        let xs = edges(&widths);
        let mut laid = Vec::new();
        for (row, end_row, column, end_column, mut job, background) in cells {
            let (Some(&left), Some(&right)) = (xs.get(column), xs.get(end_column)) else { continue };
            job.wrap.max_width = (right - left - 2.0 * PADDING).max(1.0);
            let text = ctx.fonts_mut(|f| f.layout_job(job));
            let Some(spanned) = heights.get_mut(row..end_row) else { continue };
            let missing = (text.size().y + 2.0 * PADDING - spanned.iter().sum::<f32>()).max(0.0);
            if let Some(last) = spanned.last_mut() {
                *last += missing;
            }
            laid.push((row, end_row, left, right, text, background));
        }
        let ys = edges(&heights);
        let cells = laid
            .into_iter()
            .filter_map(|(row, end_row, left, right, text, background)| {
                let rect = Rect::from_min_max(pos2(left, *ys.get(row)?), pos2(right, *ys.get(end_row)?));
                Some(Cell { rect, text, background })
            })
            .collect();
        Self { size: vec2(total * scale, heights.iter().sum()), cells }
    }

    pub fn paint(&self, painter: &Painter, at: Pos2, tokens: &Tokens) {
        for cell in &self.cells {
            let rect = cell.rect.translate(at.to_vec2());
            if let Some(background) = cell.background {
                painter.rect_filled(rect, 0.0, background);
            }
            painter.rect_stroke(rect, 0.0, Stroke::new(1.0, tokens.border), StrokeKind::Inside);
            let width = cell.text.size().x;
            let x = match cell.text.job.halign {
                egui::Align::Min => rect.left() + PADDING,
                egui::Align::Center => rect.center().x - width / 2.0,
                egui::Align::Max => rect.right() - PADDING - width,
            };
            painter.with_clip_rect(rect).galley(pos2(x, rect.top() + PADDING), cell.text.clone(), tokens.text);
        }
    }
}
