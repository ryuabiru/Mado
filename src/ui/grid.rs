use std::collections::HashMap;

use crate::nvim::redraw::{CursorStyle, HighlightAttrs, RedrawEvent};

pub const DEFAULT_FOREGROUND: u32 = 0xd8dee9;
pub const DEFAULT_BACKGROUND: u32 = 0x1e222a;
const MAX_GRID_CELLS: usize = 4_000_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub highlight_id: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedHighlight {
    pub foreground: u32,
    pub background: u32,
    pub special: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub undercurl: bool,
    pub strikethrough: bool,
    pub overline: bool,
}

#[derive(Debug)]
pub struct GridState {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
    cursor: Cursor,
    cursor_grid: u64,
    foreground: u32,
    background: u32,
    special: u32,
    highlights: HashMap<u64, HighlightAttrs>,
    cursor_style_enabled: bool,
    cursor_styles: Vec<CursorStyle>,
    mode_index: usize,
    row_revisions: Vec<u64>,
    next_revision: u64,
}

impl Default for GridState {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            cells: Vec::new(),
            cursor: Cursor::default(),
            cursor_grid: 1,
            foreground: DEFAULT_FOREGROUND,
            background: DEFAULT_BACKGROUND,
            special: DEFAULT_FOREGROUND,
            highlights: HashMap::new(),
            cursor_style_enabled: false,
            cursor_styles: Vec::new(),
            mode_index: 0,
            row_revisions: Vec::new(),
            next_revision: 1,
        }
    }
}

impl GridState {
    pub fn apply(&mut self, event: &RedrawEvent) -> bool {
        match event {
            RedrawEvent::GridResize {
                grid: 1,
                width,
                height,
            } => match (usize::try_from(*width), usize::try_from(*height)) {
                (Ok(width), Ok(height)) => self.resize(width, height),
                _ => tracing::warn!(width, height, "ignoring unrepresentable Neovim grid size"),
            },
            RedrawEvent::GridLine {
                grid: 1,
                row,
                col_start,
                cells,
                ..
            } => {
                let Ok(row) = usize::try_from(*row) else {
                    tracing::warn!(row, "ignoring unrepresentable grid row");
                    return false;
                };
                let Ok(mut col) = usize::try_from(*col_start) else {
                    tracing::warn!(col_start, "ignoring unrepresentable grid column");
                    return false;
                };
                let mut highlight_id = 0;
                for cell in cells {
                    if col >= self.width {
                        break;
                    }
                    if let Some(id) = cell.highlight_id {
                        highlight_id = id;
                    }
                    let repeat = usize::try_from(cell.repeat)
                        .unwrap_or(usize::MAX)
                        .min(self.width - col);
                    for _ in 0..repeat {
                        if let Some(target) = self.cell_mut(row, col) {
                            target.text.clone_from(&cell.text);
                            target.highlight_id = highlight_id;
                        }
                        col += 1;
                    }
                }
                self.mark_row_dirty(row);
            }
            RedrawEvent::GridClear { grid: 1 } => {
                self.cells.fill(Cell::default());
                self.mark_all_rows_dirty();
            }
            RedrawEvent::GridCursorGoto { grid, row, col } => {
                self.cursor_grid = *grid;
                self.cursor = Cursor {
                    row: *row as usize,
                    col: *col as usize,
                };
            }
            RedrawEvent::GridScroll {
                grid: 1,
                top,
                bottom,
                left,
                right,
                rows,
                cols,
            } => self.scroll(
                *top as usize,
                *bottom as usize,
                *left as usize,
                *right as usize,
                *rows,
                *cols,
            ),
            RedrawEvent::DefaultColorsSet {
                foreground,
                background,
                special,
            } => {
                if *foreground >= 0 {
                    self.foreground = *foreground as u32;
                }
                if *background >= 0 {
                    self.background = *background as u32;
                }
                if *special >= 0 {
                    self.special = *special as u32;
                }
                self.mark_all_rows_dirty();
            }
            RedrawEvent::HighlightDefine { id, attrs } => {
                self.highlights.insert(*id, attrs.clone());
                self.mark_all_rows_dirty();
            }
            RedrawEvent::ModeInfoSet {
                cursor_style_enabled,
                styles,
            } => {
                self.cursor_style_enabled = *cursor_style_enabled;
                self.cursor_styles.clone_from(styles);
            }
            RedrawEvent::ModeChange { mode_index, .. } => {
                self.mode_index = *mode_index as usize;
            }
            RedrawEvent::Flush => return true,
            _ => {}
        }
        false
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        (row < self.height && col < self.width).then(|| &self.cells[row * self.width + col])
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn cursor_is_on_main_grid(&self) -> bool {
        self.cursor_grid == 1
    }

    pub fn background(&self) -> u32 {
        self.background
    }

    pub fn cursor_style(&self) -> CursorStyle {
        if !self.cursor_style_enabled {
            return CursorStyle::default();
        }
        self.cursor_styles
            .get(self.mode_index)
            .cloned()
            .unwrap_or_default()
    }

    pub fn row_revision(&self, row: usize) -> u64 {
        self.row_revisions.get(row).copied().unwrap_or(0)
    }

    pub fn resolve_highlight(&self, id: u64) -> ResolvedHighlight {
        let attrs = self.highlights.get(&id).cloned().unwrap_or_default();
        let mut foreground = attrs.foreground.unwrap_or(self.foreground);
        let mut background = attrs.background.unwrap_or(self.background);
        if attrs.reverse {
            std::mem::swap(&mut foreground, &mut background);
        }
        ResolvedHighlight {
            foreground,
            background,
            special: attrs.special.unwrap_or(self.special),
            bold: attrs.bold,
            italic: attrs.italic,
            underline: attrs.underline
                || attrs.underdouble
                || attrs.underdotted
                || attrs.underdashed,
            undercurl: attrs.undercurl,
            strikethrough: attrs.strikethrough,
            overline: attrs.overline,
        }
    }

    fn resize(&mut self, width: usize, height: usize) {
        let Some(cell_count) = width.checked_mul(height) else {
            tracing::warn!(width, height, "ignoring overflowing Neovim grid size");
            return;
        };
        if cell_count > MAX_GRID_CELLS {
            tracing::warn!(
                width,
                height,
                cell_count,
                "ignoring excessively large Neovim grid"
            );
            return;
        }
        let mut cells = vec![Cell::default(); cell_count];
        for row in 0..height.min(self.height) {
            for col in 0..width.min(self.width) {
                cells[row * width + col] = self.cells[row * self.width + col].clone();
            }
        }
        self.width = width;
        self.height = height;
        self.cells = cells;
        self.row_revisions.resize(height, 0);
        self.mark_all_rows_dirty();
    }

    fn cell_mut(&mut self, row: usize, col: usize) -> Option<&mut Cell> {
        (row < self.height && col < self.width).then(|| &mut self.cells[row * self.width + col])
    }

    fn scroll(
        &mut self,
        top: usize,
        bottom: usize,
        left: usize,
        right: usize,
        rows: i64,
        cols: i64,
    ) {
        let bottom = bottom.min(self.height);
        let right = right.min(self.width);
        if top >= bottom || left >= right {
            return;
        }
        if rows < 0 {
            for row in (top..bottom).rev() {
                for col in left..right {
                    self.scroll_cell(row, col, top, bottom, left, right, rows, cols);
                }
            }
        } else if rows == 0 && cols < 0 {
            for row in top..bottom {
                for col in (left..right).rev() {
                    self.scroll_cell(row, col, top, bottom, left, right, rows, cols);
                }
            }
        } else {
            for row in top..bottom {
                for col in left..right {
                    self.scroll_cell(row, col, top, bottom, left, right, rows, cols);
                }
            }
        }
        for row in top..bottom {
            self.mark_row_dirty(row);
        }
    }

    fn mark_row_dirty(&mut self, row: usize) {
        let Some(revision) = self.row_revisions.get_mut(row) else {
            return;
        };
        *revision = self.next_revision;
        self.next_revision = self.next_revision.wrapping_add(1).max(1);
    }

    fn mark_all_rows_dirty(&mut self) {
        for row in 0..self.row_revisions.len() {
            self.mark_row_dirty(row);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn scroll_cell(
        &mut self,
        row: usize,
        col: usize,
        top: usize,
        bottom: usize,
        left: usize,
        right: usize,
        rows: i64,
        cols: i64,
    ) {
        let source_row = row as i64 + rows;
        let source_col = col as i64 + cols;
        let target = row * self.width + col;
        if source_row >= top as i64
            && source_row < bottom as i64
            && source_col >= left as i64
            && source_col < right as i64
        {
            let source = source_row as usize * self.width + source_col as usize;
            clone_cell(&mut self.cells, target, source);
        } else {
            self.cells[target].text.clear();
            self.cells[target].highlight_id = 0;
        }
    }
}

fn clone_cell(cells: &mut [Cell], target: usize, source: usize) {
    if target == source {
        return;
    }
    if target < source {
        let (before, after) = cells.split_at_mut(source);
        before[target].clone_from(&after[0]);
    } else {
        let (before, after) = cells.split_at_mut(target);
        after[0].clone_from(&before[source]);
    }
}

#[cfg(test)]
mod tests {
    use crate::nvim::redraw::GridCell;

    use super::*;

    #[test]
    fn expands_repeat_and_inherits_highlight() {
        let mut grid = GridState::default();
        grid.apply(&RedrawEvent::GridResize {
            grid: 1,
            width: 5,
            height: 1,
        });
        grid.apply(&RedrawEvent::GridLine {
            grid: 1,
            row: 0,
            col_start: 0,
            cells: vec![
                GridCell {
                    text: "x".into(),
                    highlight_id: Some(3),
                    repeat: 2,
                },
                GridCell {
                    text: "y".into(),
                    highlight_id: None,
                    repeat: 1,
                },
            ],
            wrap: false,
        });

        assert_eq!(grid.cell(0, 0).unwrap().text, "x");
        assert_eq!(grid.cell(0, 1).unwrap().highlight_id, 3);
        assert_eq!(grid.cell(0, 2).unwrap().highlight_id, 3);
    }

    #[test]
    fn scrolls_region_up() {
        let mut grid = GridState::default();
        grid.resize(1, 3);
        for (row, text) in ["a", "b", "c"].into_iter().enumerate() {
            grid.cell_mut(row, 0).unwrap().text = text.into();
        }
        grid.scroll(0, 3, 0, 1, 1, 0);
        assert_eq!(grid.cell(0, 0).unwrap().text, "b");
        assert_eq!(grid.cell(1, 0).unwrap().text, "c");
        assert_eq!(grid.cell(2, 0).unwrap().text, "");
    }

    #[test]
    fn scrolls_region_down_without_overwriting_sources() {
        let mut grid = GridState::default();
        grid.resize(1, 3);
        for (row, text) in ["a", "b", "c"].into_iter().enumerate() {
            grid.cell_mut(row, 0).unwrap().text = text.into();
        }
        grid.scroll(0, 3, 0, 1, -1, 0);
        assert_eq!(grid.cell(0, 0).unwrap().text, "");
        assert_eq!(grid.cell(1, 0).unwrap().text, "a");
        assert_eq!(grid.cell(2, 0).unwrap().text, "b");
    }

    #[test]
    fn rejects_excessively_large_grids() {
        let mut grid = GridState::default();
        grid.resize(80, 24);
        grid.resize(usize::MAX, 2);
        assert_eq!((grid.width(), grid.height()), (80, 24));

        grid.resize(MAX_GRID_CELLS + 1, 1);
        assert_eq!((grid.width(), grid.height()), (80, 24));
    }

    #[test]
    fn clamps_cell_repeat_to_the_visible_grid() {
        let mut grid = GridState::default();
        grid.resize(2, 1);
        grid.apply(&RedrawEvent::GridLine {
            grid: 1,
            row: 0,
            col_start: 0,
            cells: vec![GridCell {
                text: "x".into(),
                highlight_id: None,
                repeat: u64::MAX,
            }],
            wrap: false,
        });
        assert_eq!(
            grid.cells()
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<Vec<_>>(),
            ["x", "x"]
        );
    }

    #[test]
    fn revisions_only_the_rows_changed_by_line_updates() {
        let mut grid = GridState::default();
        grid.resize(3, 2);
        let first_row = grid.row_revision(0);
        let second_row = grid.row_revision(1);

        grid.apply(&RedrawEvent::GridLine {
            grid: 1,
            row: 1,
            col_start: 0,
            cells: vec![GridCell {
                text: "changed".into(),
                highlight_id: Some(1),
                repeat: 1,
            }],
            wrap: false,
        });

        assert_eq!(grid.row_revision(0), first_row);
        assert_ne!(grid.row_revision(1), second_row);
    }
}
