//! A small table builder for the CLI's tabular output.
//!
//! Centralizes the visual language every command shares: auto column widths,
//! per-column alignment, a dim/bold header, and per-cell color. Color is emitted
//! only when stdout is an interactive terminal, so `dotfiles … | cat` and the
//! convergence harness see clean, uncolored text.

use std::io::IsTerminal;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const RED: &str = "\x1b[31m";
pub const CYAN: &str = "\x1b[36m";

/// Header row style: dim + underline.
const HEADER: &str = "\x1b[2;4m";

/// Column (and cell) alignment.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// One table cell: text plus an optional foreground color.
pub struct Cell {
    text: String,
    fg: Option<&'static str>,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Self {
        Cell { text: text.into(), fg: None }
    }

    /// Color this cell's text (applied only on a terminal).
    pub fn fg(mut self, color: &'static str) -> Self {
        self.fg = Some(color);
        self
    }

    fn width(&self) -> usize {
        self.text.chars().count()
    }
}

/// Convenience constructor: `cell("zsh")`.
pub fn cell(text: impl Into<String>) -> Cell {
    Cell::new(text)
}

/// A table: an optional title, aligned headers, and rows of cells.
#[derive(Default)]
pub struct Table {
    title: Option<String>,
    columns: Vec<(String, Align)>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn new() -> Self {
        Table::default()
    }

    /// A bold/cyan heading printed above the table.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Add a column with a header and alignment.
    pub fn column(mut self, header: impl Into<String>, align: Align) -> Self {
        self.columns.push((header.into(), align));
        self
    }

    /// Append a row. Extra cells beyond the column count are ignored; missing
    /// trailing cells render empty.
    pub fn row(&mut self, cells: Vec<Cell>) {
        self.rows.push(cells);
    }

    /// Render the table to stdout.
    pub fn print(&self) {
        let color = std::io::stdout().is_terminal();
        let n = self.columns.len();

        // Column widths = max(header, widest cell).
        let mut widths: Vec<usize> = self.columns.iter().map(|(h, _)| h.chars().count()).collect();
        for row in &self.rows {
            for (i, c) in row.iter().enumerate().take(n) {
                widths[i] = widths[i].max(c.width());
            }
        }

        if let Some(t) = &self.title {
            if color {
                println!("{BOLD}{CYAN}{t}{RESET}\n");
            } else {
                println!("{t}\n");
            }
        }

        // Header row (dim + bold).
        let header_cells: Vec<String> = self
            .columns
            .iter()
            .enumerate()
            .map(|(i, (h, align))| {
                render(h, if color { Some(HEADER) } else { None }, widths[i], *align, color)
            })
            .collect();
        println!("{}", join(&header_cells));

        // Body rows.
        for row in &self.rows {
            let cells: Vec<String> = (0..n)
                .map(|i| {
                    let (text, fg) = row.get(i).map_or(("", None), |c| (c.text.as_str(), c.fg));
                    render(text, fg, widths[i], self.columns[i].1, color)
                })
                .collect();
            println!("{}", join(&cells));
        }
    }
}

/// Render one cell: pad to `width`, with color (when enabled) wrapping only the
/// text so trailing padding stays plain and trimmable.
fn render(text: &str, fg: Option<&'static str>, width: usize, align: Align, color: bool) -> String {
    let fill = " ".repeat(width.saturating_sub(text.chars().count()));
    let body = match (color, fg) {
        (true, Some(c)) => format!("{c}{text}{RESET}"),
        _ => text.to_string(),
    };
    match align {
        Align::Left => format!("{body}{fill}"),
        Align::Right => format!("{fill}{body}"),
    }
}

/// Join rendered cells with a two-space gutter; trim trailing padding.
fn join(cells: &[String]) -> String {
    cells.join("  ").trim_end().to_string()
}
