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
    /// Resolve explicit CLI width, then configuration, then terminal width.
    pub fn new(cli_width: Option<u16>, configured_width: Option<u16>) -> Self {
        Self::resolve(cli_width, configured_width, terminal_width())
    }

    /// Resolve width using an injected terminal width (useful for tests).
    pub fn resolve(
        cli_width: Option<u16>,
        configured_width: Option<u16>,
        detected_width: Option<u16>,
    ) -> Self {
        Self {
            width: cli_width
                .or(configured_width)
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

/// Parse a positive terminal width for clap.
pub fn parse_width(value: &str) -> Result<u16, String> {
    let width = value
        .trim()
        .parse::<u32>()
        .map_err(|_| "width must be a positive integer between 1 and 65535".to_string())?;
    u16::try_from(width)
        .ok()
        .filter(|width| *width > 0)
        .ok_or_else(|| "width must be a positive integer between 1 and 65535".to_string())
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
    fn width_precedence_is_cli_config_terminal_default() {
        assert_eq!(
            Display::resolve(Some(150), Some(120), Some(180)).width(),
            150
        );
        assert_eq!(Display::resolve(None, Some(120), Some(180)).width(), 120);
        assert_eq!(Display::resolve(None, None, Some(180)).width(), 180);
        assert_eq!(Display::resolve(None, None, Some(0)).width(), DEFAULT_WIDTH);
        assert_eq!(Display::resolve(None, None, None).width(), DEFAULT_WIDTH);
    }

    #[test]
    fn width_parser_rejects_invalid_values() {
        assert_eq!(parse_width("150").unwrap(), 150);
        assert_eq!(parse_width(" 1 ").unwrap(), 1);
        for value in ["0", "-1", "wide", "65536", ""] {
            assert!(parse_width(value).is_err(), "{value}");
        }
        assert_eq!(parse_width("65535").unwrap(), u16::MAX);
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
