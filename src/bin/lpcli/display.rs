//! Terminal display-width selection and table rendering.

use std::fmt;
use std::ops::{Deref, DerefMut};

use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL_CONDENSED};

/// Width used when stdout is not a terminal and no preference is supplied.
pub const DEFAULT_WIDTH: u16 = 100;

/// A resolved output width. Widths include table borders and cell padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Display {
    width: u16,
}

impl Display {
    /// Resolve the configured width, then the terminal width.
    pub fn new(configured_width: Option<u16>) -> Self {
        Self::resolve(configured_width, terminal_width())
    }

    /// Resolve width using an injected terminal width (useful for tests).
    fn resolve(configured_width: Option<u16>, detected_width: Option<u16>) -> Self {
        Self {
            width: configured_width
                .or(detected_width.filter(|width| *width > 0))
                .unwrap_or(DEFAULT_WIDTH),
        }
    }

    /// Construct a display with an explicit width.
    #[cfg(test)]
    pub fn from_width(width: u16) -> Self {
        Self { width }
    }

    /// The resolved maximum table width.
    #[cfg(test)]
    pub fn width(self) -> u16 {
        self.width
    }

    /// Start a consistently styled, width-aware table.
    pub fn table(self, headers: Vec<&str>) -> DisplayTable {
        let mut table = Table::new();
        table.load_style(UTF8_FULL_CONDENSED);
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.set_width(self.width);
        table.set_header(headers);
        DisplayTable {
            table,
            width: self.width,
        }
    }
}

/// Detect stdout's terminal width through comfy-table's existing tty support.
fn terminal_width() -> Option<u16> {
    Table::new().width()
}

/// A table with a compact stacked representation for widths that cannot fit
/// even one content column per table column.
pub struct DisplayTable {
    table: Table,
    width: u16,
}

impl Deref for DisplayTable {
    type Target = Table;

    fn deref(&self) -> &Self::Target {
        &self.table
    }
}

impl DerefMut for DisplayTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.table
    }
}

impl fmt::Display for DisplayTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let columns = self
            .table
            .header()
            .map(|row| row.cell_iter().count())
            .or_else(|| {
                self.table
                    .row_iter()
                    .next()
                    .map(|row| row.cell_iter().count())
            })
            .unwrap_or(0);
        // Each column needs at least one content character, two padding
        // characters, and a border. Below that, a grid would be wider than
        // the requested output or unreadably fragmented.
        let minimum_grid_width = columns.saturating_mul(3).saturating_add(1);
        if columns > 0 && usize::from(self.width) < minimum_grid_width {
            return self.fmt_stacked(f);
        }
        write!(f, "{}", self.table)
    }
}

impl DisplayTable {
    fn fmt_stacked(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<String> = self
            .table
            .header()
            .map(|row| row.cell_iter().map(|cell| cell.content()).collect())
            .unwrap_or_default();
        for (row_index, row) in self.table.row_iter().enumerate() {
            if row_index > 0 {
                writeln!(f)?;
            }
            for (index, cell) in row.cell_iter().enumerate() {
                let label = headers.get(index).map(String::as_str).unwrap_or("Value");
                writeln!(f, "{label}: {}", cell.content())?;
            }
        }
        if self.table.row_iter().next().is_none() {
            for header in headers {
                writeln!(f, "{header}:")?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_precedence_is_config_terminal_default() {
        assert_eq!(Display::resolve(Some(120), Some(180)).width(), 120);
        assert_eq!(Display::resolve(Some(120), None).width(), 120);
        assert_eq!(Display::resolve(None, Some(180)).width(), 180);
        assert_eq!(Display::resolve(None, Some(0)).width(), DEFAULT_WIDTH);
        assert_eq!(Display::resolve(None, None).width(), DEFAULT_WIDTH);
    }

    #[test]
    fn loaded_config_width_overrides_detected_terminal_width() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[display]\nwidth = 5\n").unwrap();
        let config = super::super::config::load_from(&path).unwrap();
        let display = Display::resolve(config.display.width, Some(120));
        let mut table = display.table(vec!["ID", "Title"]);
        table.add_row(vec!["1", "A complete title"]);
        assert_eq!(display.width(), 5);
        assert!(table.to_string().contains("Title: A complete title"));
    }

    #[test]
    fn wide_tables_keep_long_text_without_fixed_truncation() {
        let title = "A title that is deliberately much longer than fifty-five characters and must remain visible";
        let mut table = Display::from_width(150).table(vec!["ID", "Title", "Status"]);
        table.add_row(vec!["1771545", title, "Triaged"]);
        let rendered = table.to_string();
        assert!(rendered.contains(title), "{rendered}");
        assert!(!rendered.contains('…'), "{rendered}");
        assert!(rendered.lines().all(|line| line.chars().count() <= 150));
    }

    #[test]
    fn narrow_tables_wrap_instead_of_losing_text() {
        let title = "one two three four five six seven eight nine ten";
        let mut table = Display::from_width(35).table(vec!["ID", "Title"]);
        table.add_row(vec!["1", title]);
        let rendered = table.to_string();
        for word in title.split_whitespace() {
            assert!(rendered.contains(word), "missing {word}: {rendered}");
        }
        assert!(rendered.lines().count() > 3);
    }

    #[test]
    fn extremely_narrow_tables_use_stacked_rows() {
        let mut table = Display::from_width(5).table(vec!["ID", "Title"]);
        table.add_row(vec!["1", "A complete title"]);
        table.add_row(vec!["2", "Another title"]);
        let rendered = table.to_string();
        assert!(rendered.contains("ID: 1"));
        assert!(rendered.contains("Title: A complete title"));
        assert!(rendered.contains("ID: 2"));
        assert!(rendered.contains("Title: Another title"));
        assert!(!rendered.contains('┌'));
    }

    #[test]
    fn unicode_content_is_preserved() {
        let mut table = Display::from_width(40).table(vec!["Name", "Description"]);
        table.add_row(vec!["日本語", " café — naïve "]);
        let rendered = table.to_string();
        assert!(rendered.contains("日本語"));
        assert!(rendered.contains("café"));
        assert!(rendered.contains("naïve"));
    }
}
