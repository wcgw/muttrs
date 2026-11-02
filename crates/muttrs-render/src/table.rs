//! Tables of data, drawn as grids: columns as wide as their content, or shared out when the width is short, and
//! linearised when not even their longest words fit.

use ratatui_core::text::{Line, Span};

use crate::ir::Table;
use crate::layout::{self, Cx, Fragment};

/// Between columns.
const SEPARATOR: &str = " │ ";
const SEPARATOR_WIDTH: usize = 3;

/// As wide as anything gets, for measuring.
const UNBOUNDED: u16 = 4_000;

pub(crate) fn layout(cx: &Cx<'_>, table: &Table, width: u16) -> Fragment {
    let cx = cx.without_pictures();
    let columns = columns(table);
    if columns == 0 {
        return Fragment::default();
    }
    let (min, max) = measure(&cx, table, columns);
    let available = usize::from(width).saturating_sub(SEPARATOR_WIDTH * (columns - 1));
    let (sum_min, sum_max) = (min.iter().sum::<usize>(), max.iter().sum::<usize>());
    if sum_min > available || available < columns {
        return linear(&cx, table, width);
    }
    let widths = if sum_max <= available {
        max
    } else {
        share(&min, &max, available)
    };

    let styles = &cx.document.styles;
    let mut fragment = Fragment::default();
    for (index, row) in table.rows.iter().enumerate() {
        // Each cell's lines, and its width.
        let mut cells: Vec<(usize, Vec<Line<'static>>)> = Vec::new();
        let mut column = 0;
        for cell in &row.cells {
            if column >= columns {
                break;
            }
            let span = cell.span.min(columns - column);
            let cell_width =
                widths[column..column + span].iter().sum::<usize>() + SEPARATOR_WIDTH * (span - 1);
            let laid = cx.blocks(&cell.blocks, u16::try_from(cell_width).unwrap_or(width));
            cells.push((cell_width, laid.lines));
            column += span;
        }
        // Cells missing at the end of a short row.
        for &missing in &widths[column..] {
            cells.push((missing, Vec::new()));
        }
        let height = cells
            .iter()
            .map(|(_, lines)| lines.len())
            .max()
            .unwrap_or(0)
            .max(1);
        for line_index in 0..height {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (cell_index, (cell_width, lines)) in cells.iter().enumerate() {
                if cell_index > 0 {
                    spans.push(Span::styled(SEPARATOR, styles.rule));
                }
                let drawn = lines.get(line_index).cloned().unwrap_or_default();
                let padding = cell_width.saturating_sub(layout::line_width(&drawn));
                spans.extend(drawn.spans);
                // The last cell needs no padding: nothing follows it.
                if cell_index + 1 < cells.len() && padding > 0 {
                    spans.push(Span::raw(" ".repeat(padding)));
                }
            }
            fragment.push(Line::from(spans));
        }
        let next_is_body = table.rows.get(index + 1).is_some_and(|next| !next.header);
        if row.header && next_is_body {
            let rule = widths
                .iter()
                .map(|&width| "─".repeat(width))
                .collect::<Vec<_>>()
                .join("─┼─");
            fragment.push(Line::from(Span::styled(rule, styles.rule)));
        }
    }
    fragment
}

/// The width a table can't be laid out narrower than: its columns' longest words, and the separators.
pub(crate) fn min_width(table: &Table) -> u16 {
    let columns = columns(table);
    if columns == 0 {
        return 0;
    }
    let mut min = vec![0usize; columns];
    for row in &table.rows {
        let mut column = 0;
        for cell in &row.cells {
            if cell.span == 1 && column < columns {
                min[column] = min[column].max(usize::from(layout::min_width(&cell.blocks)));
            }
            column += cell.span;
        }
    }
    u16::try_from(min.iter().sum::<usize>() + SEPARATOR_WIDTH * (columns - 1)).unwrap_or(u16::MAX)
}

fn columns(table: &Table) -> usize {
    table
        .rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.span).sum::<usize>())
        .max()
        .unwrap_or(0)
}

/// Each column's narrowest (its longest word) and widest (its longest line unwrapped), from its single-column cells.
fn measure(cx: &Cx<'_>, table: &Table, columns: usize) -> (Vec<usize>, Vec<usize>) {
    let mut min = vec![1usize; columns];
    let mut max = vec![1usize; columns];
    for row in &table.rows {
        let mut column = 0;
        for cell in &row.cells {
            if cell.span == 1 && column < columns {
                min[column] = min[column].max(usize::from(layout::min_width(&cell.blocks)));
                let natural = cx
                    .measuring()
                    .blocks(&cell.blocks, UNBOUNDED)
                    .lines
                    .iter()
                    .map(layout::line_width)
                    .max()
                    .unwrap_or(0);
                max[column] = max[column].max(natural);
            }
            column += cell.span;
        }
    }
    for (max, min) in max.iter_mut().zip(&min) {
        *max = (*max).max(*min);
    }
    (min, max)
}

/// Widths adding up to `available`: each column its narrowest, plus a share of what's left in proportion to how
/// much wider it would like to be, the cells rounding leaves going to the largest remainders.
fn share(min: &[usize], max: &[usize], available: usize) -> Vec<usize> {
    let extra = available - min.iter().sum::<usize>();
    let slack: usize = max.iter().zip(min).map(|(max, min)| max - min).sum();
    if slack == 0 {
        return min.to_vec();
    }
    let wants: Vec<usize> = max
        .iter()
        .zip(min)
        .map(|(max, min)| extra * (max - min))
        .collect();
    let mut widths: Vec<usize> = min
        .iter()
        .zip(&wants)
        .map(|(min, want)| min + want / slack)
        .collect();
    let mut by_remainder: Vec<usize> = (0..widths.len()).collect();
    by_remainder.sort_by_key(|&column| std::cmp::Reverse(wants[column] % slack));
    let mut left = available - widths.iter().sum::<usize>();
    for column in by_remainder.into_iter().cycle().take(widths.len() * 2) {
        if left == 0 {
            break;
        }
        if widths[column] < max[column] {
            widths[column] += 1;
            left -= 1;
        }
    }
    widths
}

/// A table too wide for a grid: its rows one after the other, each cell on lines of its own.
fn linear(cx: &Cx<'_>, table: &Table, width: u16) -> Fragment {
    let mut fragment = Fragment::default();
    for row in &table.rows {
        let mut drawn = Fragment::default();
        for cell in &row.cells {
            drawn.append(cx.blocks(&cell.blocks, width));
        }
        fragment.append(drawn);
        fragment.append(gap());
    }
    fragment
}

fn gap() -> Fragment {
    let mut gap = Fragment::default();
    gap.push_gap();
    gap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_go_to_the_columns_that_want_them() {
        assert_eq!(share(&[4, 4], &[4, 20], 14), [4, 10]);
        assert_eq!(share(&[2, 2, 2], &[10, 10, 4], 13), [5, 5, 3]);
    }
}
