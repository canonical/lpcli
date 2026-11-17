//! Output modes, style selection, and renderers for command results.
//!
//! Commands that use this layer build a mode-independent [`Report`]. The
//! [`Output`] resolved for the invocation picks one of four [`Renderer`]s:
//!
//! - `rich`: color, Unicode tables, and headings for sighted users;
//! - `plain`: labeled `Field: value` records with ASCII punctuation, no color,
//!   no box drawing, and words instead of symbols, for screen readers;
//! - `json`: Launchpad-compatible JSON;
//! - `tsv`: a header row, then exactly one tab-separated line per record.
//!
//! In `json` and `tsv` modes stdout carries data only; prose is omitted.
//! Styling is passed explicitly through [`Style`] rather than through the
//! process-wide override of the `colored` crate, so renderers are pure
//! functions that tests can run in parallel.

use std::io::{IsTerminal, Write};

use anstyle::{AnsiColor, Color, Style as AnsiStyle};
use clap::ValueEnum;
use serde::Deserialize;

use crate::display::Display;

/// Name of the environment variable that selects the output mode.
pub const FORMAT_ENV: &str = "LPCLI_FORMAT";

/// How command results are written to stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Color, Unicode tables, and headings.
    Rich,
    /// Accessible labeled records: ASCII punctuation, no color or box drawing.
    Plain,
    /// Launchpad-compatible JSON.
    Json,
    /// A header row, then one tab-separated record per line.
    Tsv,
}

impl OutputMode {
    /// The name used by `--format`, `LPCLI_FORMAT`, and `display.format`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rich => "rich",
            Self::Plain => "plain",
            Self::Json => "json",
            Self::Tsv => "tsv",
        }
    }

    /// Whether this mode writes machine-readable data rather than prose.
    pub fn is_structured(self) -> bool {
        matches!(self, Self::Json | Self::Tsv)
    }
}

/// When to use color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum ColorChoice {
    /// Follow `CLICOLOR_FORCE`, `NO_COLOR`, `TERM`, and whether the stream is
    /// a terminal.
    #[default]
    Auto,
    /// Always use color in rich output.
    Always,
    /// Never use color.
    Never,
}

/// The environment variables that affect output, captured once so that
/// mode and color selection are pure functions of their inputs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    /// `LPCLI_FORMAT`.
    pub format: Option<String>,
    /// `NO_COLOR`.
    pub no_color: Option<String>,
    /// `CLICOLOR_FORCE`.
    pub clicolor_force: Option<String>,
    /// `TERM`.
    pub term: Option<String>,
}

impl Environment {
    /// Read the relevant variables from the current process.
    pub fn from_process() -> Self {
        let var = |name: &str| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned());
        Self {
            format: var(FORMAT_ENV),
            no_color: var("NO_COLOR"),
            clicolor_force: var("CLICOLOR_FORCE"),
            term: var("TERM"),
        }
    }

    fn term_is_dumb(&self) -> bool {
        self.term.as_deref() == Some("dumb")
    }
}

/// Where the selected output mode came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeSource {
    /// The `--format` option.
    Flag,
    /// The `LPCLI_FORMAT` environment variable.
    Environment,
    /// `display.format` in `config.toml`.
    Config,
    /// Automatic detection.
    Automatic,
}

/// A selected output mode and where it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeChoice {
    pub mode: OutputMode,
    pub source: ModeSource,
}

/// Choose the output mode, from highest priority to lowest: `--format`,
/// `LPCLI_FORMAT`, `display.format`, then automatic detection.
///
/// An empty `LPCLI_FORMAT` counts as unset. An invalid one is an error, but
/// only when `--format` does not override it.
pub fn choose_mode(
    flag: Option<OutputMode>,
    env: &Environment,
    config_format: Option<OutputMode>,
    accessible: bool,
) -> Result<ModeChoice, String> {
    if let Some(mode) = flag {
        return Ok(ModeChoice {
            mode,
            source: ModeSource::Flag,
        });
    }
    if let Some(value) = env.format.as_deref().filter(|value| !value.is_empty()) {
        let mode = OutputMode::from_str(value, false).map_err(|_| {
            format!(
                "Invalid {FORMAT_ENV} value \"{}\": expected one of rich, plain, json, or tsv.",
                value.escape_debug()
            )
        })?;
        return Ok(ModeChoice {
            mode,
            source: ModeSource::Environment,
        });
    }
    if let Some(mode) = config_format {
        return Ok(ModeChoice {
            mode,
            source: ModeSource::Config,
        });
    }
    Ok(ModeChoice {
        mode: automatic_mode(env, accessible),
        source: ModeSource::Automatic,
    })
}

/// `plain` when the accessibility setting is on or `TERM=dumb`; otherwise
/// `rich`.
fn automatic_mode(env: &Environment, accessible: bool) -> OutputMode {
    if accessible || env.term_is_dumb() {
        OutputMode::Plain
    } else {
        OutputMode::Rich
    }
}

/// Decide whether to color one stream.
///
/// `--color always|never` overrides everything. Otherwise `CLICOLOR_FORCE`
/// set to anything other than `0` forces color on; a non-empty `NO_COLOR` or
/// `TERM=dumb` turns it off; and otherwise color follows whether the stream
/// is a terminal.
pub fn color_enabled(choice: ColorChoice, env: &Environment, is_terminal: bool) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            if env
                .clicolor_force
                .as_deref()
                .is_some_and(|value| !value.is_empty() && value != "0")
            {
                true
            } else if env
                .no_color
                .as_deref()
                .is_some_and(|value| !value.is_empty())
                || env.term_is_dumb()
            {
                false
            } else {
                is_terminal
            }
        }
    }
}

/// Explicit styling for one output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// Emit ANSI color and emphasis.
    pub color: bool,
    /// Use Unicode symbols and box drawing; otherwise ASCII only.
    pub unicode: bool,
    /// Maximum table width, in columns.
    pub width: u16,
}

/// Everything needed to resolve the [`Output`] for one invocation.
pub struct Settings<'a> {
    /// `--format`.
    pub format: Option<OutputMode>,
    /// `--color`.
    pub color: ColorChoice,
    /// `display.format`.
    pub config_format: Option<OutputMode>,
    /// `display.accessible`.
    pub accessible: bool,
    /// The resolved table width.
    pub display: Display,
    /// Whether the command supports `json` output.
    pub json_output: bool,
    /// Whether the command supports `tsv` output.
    pub tsv_output: bool,
    /// Launchpad attribute names selected with `--fields`.
    pub fields: Option<Vec<String>>,
    /// The captured environment.
    pub env: &'a Environment,
    /// Whether stdout is a terminal.
    pub stdout_terminal: bool,
    /// Whether stderr is a terminal.
    pub stderr_terminal: bool,
}

/// The resolved output mode and styling for one invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    mode: OutputMode,
    stdout: Style,
    stderr: Style,
    pretty_json: bool,
    warning: Option<String>,
    fields: Option<Vec<String>>,
}

impl Output {
    /// Resolve the mode and per-stream styles.
    ///
    /// When `json` or `tsv` comes from `LPCLI_FORMAT` or `display.format` and
    /// the command does not support it yet, the automatic mode is used
    /// instead and [`Self::warning()`] explains why, so a global setting does
    /// not break other commands. An explicit `--format json|tsv` for such a
    /// command is rejected earlier, as a usage error.
    pub fn resolve(settings: Settings<'_>) -> Result<Self, String> {
        let choice = choose_mode(
            settings.format,
            settings.env,
            settings.config_format,
            settings.accessible,
        )?;
        let mut mode = choice.mode;
        let mut warning = None;
        let supported = match mode {
            OutputMode::Json => settings.json_output,
            OutputMode::Tsv => settings.tsv_output,
            OutputMode::Rich | OutputMode::Plain => true,
        };
        if !supported {
            let origin = match choice.source {
                ModeSource::Flag => {
                    return Err(format!(
                        "--format {} is not available for this command yet.",
                        mode.name()
                    ));
                }
                ModeSource::Environment => format!("{FORMAT_ENV}={}", mode.name()),
                ModeSource::Config => format!("display.format = \"{}\"", mode.name()),
                ModeSource::Automatic => {
                    unreachable!("automatic detection never picks json or tsv")
                }
            };
            let fallback = automatic_mode(settings.env, settings.accessible);
            warning = Some(format!(
                "{origin} is not supported by this command yet; using {} output.",
                fallback.name()
            ));
            mode = fallback;
        }
        if settings.fields.is_some() && mode != OutputMode::Json {
            return Err("--fields requires JSON output; add --format json.".into());
        }
        let width = settings.display.width();
        Ok(Self {
            mode,
            stdout: Style {
                color: mode == OutputMode::Rich
                    && color_enabled(settings.color, settings.env, settings.stdout_terminal),
                unicode: mode != OutputMode::Plain,
                width,
            },
            stderr: Style {
                color: mode != OutputMode::Plain
                    && color_enabled(settings.color, settings.env, settings.stderr_terminal),
                unicode: mode != OutputMode::Plain,
                width,
            },
            pretty_json: settings.stdout_terminal,
            warning,
            fields: settings.fields,
        })
    }

    /// An output with fixed settings, for tests.
    #[cfg(test)]
    pub fn fixed(mode: OutputMode, color: bool, width: u16) -> Self {
        let style = Style {
            color,
            unicode: mode != OutputMode::Plain,
            width,
        };
        Self {
            mode,
            stdout: style,
            stderr: style,
            pretty_json: true,
            warning: None,
            fields: None,
        }
    }

    /// Use `--fields` selection, for tests.
    #[cfg(test)]
    pub fn with_fields(mut self, fields: &[&str]) -> Self {
        self.fields = Some(fields.iter().map(|field| (*field).to_string()).collect());
        self
    }

    /// The effective output mode.
    pub fn mode(&self) -> OutputMode {
        self.mode
    }

    /// Styling for stdout.
    pub fn stdout_style(&self) -> Style {
        self.stdout
    }

    /// Prepare legacy, already-formatted text for stdout using this output's
    /// explicit stdout color decision.
    pub fn style_legacy_text(&self, text: &str) -> String {
        if self.stdout.color {
            text.to_string()
        } else {
            strip_sgr(text)
        }
    }

    /// Prepare legacy, already-formatted text for stderr using this output's
    /// explicit stderr color decision.
    pub fn style_legacy_stderr_text(&self, text: &str) -> String {
        if self.stderr.color {
            text.to_string()
        } else {
            strip_sgr(text)
        }
    }

    /// A notice about the mode selection, to show on stderr.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// The renderer for the effective mode.
    pub fn renderer(&self) -> Box<dyn Renderer> {
        match self.mode {
            OutputMode::Rich => Box::new(RichRenderer { style: self.stdout }),
            OutputMode::Plain => Box::new(PlainRenderer),
            OutputMode::Json => Box::new(JsonRenderer {
                pretty: self.pretty_json,
                fields: self.fields.clone(),
            }),
            OutputMode::Tsv => Box::new(TsvRenderer),
        }
    }

    /// Render a report for stdout.
    pub fn render(&self, report: &Report) -> String {
        self.renderer().render(report)
    }

    /// Write a report to stdout.
    pub fn print(&self, report: &Report) -> lpcli::error::Result<()> {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(self.render(report).as_bytes())?;
        stdout.flush()?;
        Ok(())
    }

    /// Write an unchanged Launchpad or lpcli-defined JSON value to stdout.
    pub fn print_json(&self, value: &serde_json::Value) -> lpcli::error::Result<()> {
        self.print(&Report::json(value.clone()))
    }

    /// Write a mutation result's `self_link`, if it has one.
    pub fn print_quiet(&self, value: &serde_json::Value) -> lpcli::error::Result<()> {
        if let Some(link) = value.get("self_link").and_then(serde_json::Value::as_str) {
            let mut stdout = std::io::stdout().lock();
            writeln!(stdout, "{}", clean(link))?;
        }
        Ok(())
    }

    /// Format a runtime error for stderr.
    pub fn render_error(&self, error: &lpcli::error::LpError) -> String {
        let message = redact_secrets(&error.to_string());
        if self.mode == OutputMode::Json {
            let value = serde_json::json!({"error": {
                "kind": error.kind().name(),
                "message": message,
                "hint": error.hint(),
                "http_status": error.http_status(),
            }});
            return format!("{value}\n");
        }
        self.render_error_message(&message)
    }

    /// Format a text runtime error message for stderr.
    pub fn render_error_message(&self, message: &str) -> String {
        let message = clean(&redact_secrets(message));
        if self.mode == OutputMode::Plain {
            format!("Error: {message}\n")
        } else {
            format!("{} {message}\n", error_prefix(self.stderr.color))
        }
    }

    /// Format a warning for stderr.
    pub fn render_warning(&self, message: &str) -> String {
        let message = clean(&redact_secrets(message));
        if self.mode == OutputMode::Plain {
            format!("Warning: {message}\n")
        } else {
            format!(
                "{} {message}\n",
                paint("warning:", WARNING, self.stderr.color)
            )
        }
    }

    /// Write a warning to stderr.
    pub fn warn(&self, message: &str) -> lpcli::error::Result<()> {
        std::io::stderr()
            .lock()
            .write_all(self.render_warning(message).as_bytes())?;
        Ok(())
    }
}

/// Whether stdout and stderr are terminals.
pub fn terminals() -> (bool, bool) {
    (
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
    )
}

/// Remove credential-like values from error text before it is shown.
pub fn redact_secrets(message: &str) -> String {
    const KEYS: [&str; 8] = [
        "oauth_token_secret",
        "oauth_token",
        "oauth_signature",
        "oauth_consumer_key",
        "consumer_secret",
        "refresh_token_secret",
        "refresh_token",
        "access_token",
    ];
    let mut redacted = message.to_string();
    for key in KEYS {
        let key_lower = key.to_ascii_lowercase();
        let mut search = 0;
        loop {
            let lower = redacted.to_ascii_lowercase();
            let Some(found) = lower.get(search..).and_then(|tail| tail.find(&key_lower)) else {
                break;
            };
            let key_start = search + found;
            let mut cursor = key_start + key.len();
            let bytes = redacted.as_bytes();
            if bytes.get(cursor) == Some(&b'"') {
                cursor += 1;
            }
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if bytes.get(cursor) != Some(&b':') && bytes.get(cursor) != Some(&b'=') {
                search = key_start + key.len();
                continue;
            }
            cursor += 1;
            while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'"') || bytes.get(cursor) == Some(&b'\'') {
                let quote = bytes[cursor];
                let value_start = cursor + 1;
                let mut value_end = value_start;
                while value_end < bytes.len() {
                    if bytes[value_end] == b'\\' {
                        value_end = (value_end + 2).min(bytes.len());
                    } else if bytes[value_end] == quote {
                        break;
                    } else {
                        value_end += 1;
                    }
                }
                redacted.replace_range(value_start..value_end, "[redacted]");
                search = value_start + "[redacted]".len();
            } else {
                let value_start = cursor;
                let value_end = redacted[value_start..]
                    .find(|c: char| c.is_whitespace() || matches!(c, '&' | ',' | ';' | '}'))
                    .map_or(redacted.len(), |offset| value_start + offset);
                if value_start == value_end {
                    search = key_start + key.len();
                } else {
                    redacted.replace_range(value_start..value_end, "[redacted]");
                    search = value_start + "[redacted]".len();
                }
            }
        }
    }
    let mut search = 0;
    loop {
        let lower = redacted.to_ascii_lowercase();
        let Some(found) = lower
            .get(search..)
            .and_then(|tail| tail.find("authorization:"))
        else {
            break;
        };
        let start = search + found;
        let end = redacted[start..]
            .find('\n')
            .map_or(redacted.len(), |offset| start + offset);
        redacted.replace_range(start..end, "Authorization: [redacted]");
        search = start + "Authorization: [redacted]".len();
    }
    redacted
}

/// The `error:` prefix for runtime errors on stderr.
pub fn error_prefix(color: bool) -> String {
    paint("error:", ERROR, color)
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

/// A mode-independent command result.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    /// Content for the `rich` and `plain` modes, in order.
    pub blocks: Vec<Block>,
    /// The value printed in `json` mode.
    pub json: serde_json::Value,
    /// The records printed in `tsv` mode, and by [`Block::List`].
    pub records: Records,
}

impl Report {
    /// A report for JSON-only results.
    pub fn json(json: serde_json::Value) -> Self {
        Self {
            blocks: Vec::new(),
            json,
            records: Records {
                columns: Vec::new(),
                rows: Vec::new(),
            },
        }
    }
}

/// Tabular records with a fixed set of columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Records {
    pub columns: Vec<Column>,
    /// One value per column; `None` marks a missing value.
    pub rows: Vec<Vec<Option<String>>>,
}

/// A record column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// The label shown in tables and `plain` records.
    pub label: &'static str,
    /// The `tsv` header, a Launchpad attribute name where one exists.
    pub key: &'static str,
}

/// The singular and plural names of the records in a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Noun {
    pub singular: &'static str,
    pub plural: &'static str,
}

impl Noun {
    fn count(self, n: usize) -> String {
        let noun = if n == 1 { self.singular } else { self.plural };
        format!("{n} {noun} found.")
    }
}

/// Status levels, shown as symbols in `rich` and as words in `plain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warning,
    Error,
}

/// A piece of inline text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    /// Ordinary text.
    Text(String),
    /// A Launchpad name, such as an account.
    Name(String),
    /// A command the user can run.
    Command(String),
}

/// One piece of `rich` or `plain` content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A section heading.
    Heading(String),
    /// A status line.
    Status(Level, Vec<Span>),
    /// A detail sentence that belongs to the preceding status.
    Note(Vec<Span>),
    /// A labeled detail that belongs to the preceding status.
    Field(&'static str, String),
    /// The report's [`Records`], with a count before (`plain`) or after
    /// (`rich`) them, an optional hint, and a message for an empty list.
    List {
        noun: Noun,
        empty: String,
        hint: Option<String>,
    },
    /// A blank line.
    Break,
}

// ---------------------------------------------------------------------------
// Renderers
// ---------------------------------------------------------------------------

/// Turns a [`Report`] into the text written to stdout.
pub trait Renderer {
    fn render(&self, report: &Report) -> String;
}

/// Color, Unicode tables, and headings.
pub struct RichRenderer {
    pub style: Style,
}

/// Labeled records with ASCII punctuation, no color, and no box drawing.
pub struct PlainRenderer;

/// Launchpad-compatible JSON, pretty-printed for terminals.
pub struct JsonRenderer {
    pub pretty: bool,
    pub fields: Option<Vec<String>>,
}

/// Keep only selected attributes in Launchpad entries. Collection metadata
/// and lpcli-defined wrappers are retained; unknown attributes are omitted.
pub fn select_fields(value: &serde_json::Value, fields: &[String]) -> serde_json::Value {
    fn is_entry(map: &serde_json::Map<String, serde_json::Value>) -> bool {
        map.contains_key("self_link") || map.contains_key("resource_type_link")
    }
    match value {
        serde_json::Value::Object(map) if is_entry(map) => fields
            .iter()
            .filter_map(|field| map.get(field).map(|value| (field.clone(), value.clone())))
            .collect::<serde_json::Map<_, _>>()
            .into(),
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(key, value)| {
                let selected = if key == "lpcli" {
                    value.clone()
                } else {
                    select_fields(value, fields)
                };
                (key.clone(), selected)
            })
            .collect::<serde_json::Map<_, _>>()
            .into(),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| select_fields(value, fields))
            .collect::<Vec<_>>()
            .into(),
        value => value.clone(),
    }
}

/// A header row, then one tab-separated record per line.
pub struct TsvRenderer;

const BOLD: AnsiStyle = AnsiStyle::new().bold();
const NAME: AnsiStyle = AnsiStyle::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
const OK: AnsiStyle = AnsiStyle::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Green)));
const WARNING: AnsiStyle = AnsiStyle::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
const ERROR: AnsiStyle = AnsiStyle::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)));

fn paint(text: &str, style: AnsiStyle, color: bool) -> String {
    if color {
        format!("{}{text}{}", style.render(), style.render_reset())
    } else {
        text.to_string()
    }
}

/// Make Launchpad text safe to show on a terminal: control characters other
/// than newlines (for example, escape sequences in a title) are replaced, and
/// tabs become spaces.
/// Remove ANSI SGR styling sequences (`ESC [ digits/semicolons m`) emitted
/// by legacy formatters. Anything else, including other escape sequences,
/// is kept unchanged for the renderer's own sanitizing.
fn strip_sgr(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("\u{1b}[") {
        stripped.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let params = after
            .find(|c: char| !(c.is_ascii_digit() || c == ';'))
            .unwrap_or(after.len());
        if after[params..].starts_with('m') {
            rest = &after[params + 1..];
        } else {
            stripped.push_str("\u{1b}[");
            rest = after;
        }
    }
    stripped.push_str(rest);
    stripped
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' => '\n',
            '\t' => ' ',
            c if c.is_control() => '\u{FFFD}',
            c => c,
        })
        .collect()
}

impl RichRenderer {
    fn spans(&self, spans: &[Span]) -> String {
        spans
            .iter()
            .map(|span| match span {
                Span::Text(text) => clean(text),
                Span::Name(name) => paint(&clean(name), NAME, self.style.color),
                Span::Command(command) => {
                    paint(&format!("`{}`", clean(command)), BOLD, self.style.color)
                }
            })
            .collect()
    }

    fn symbol(&self, level: Level) -> String {
        let (unicode, ascii, style) = match level {
            Level::Ok => ("✓", "OK:", OK),
            Level::Warning => ("~", "Warning:", WARNING),
            Level::Error => ("✗", "Error:", ERROR),
        };
        let symbol = if self.style.unicode { unicode } else { ascii };
        paint(symbol, style, self.style.color)
    }

    fn list(
        &self,
        out: &mut String,
        records: &Records,
        noun: Noun,
        empty: &str,
        hint: Option<&str>,
    ) {
        if records.rows.is_empty() {
            out.push_str(&clean(empty));
            out.push('\n');
            return;
        }
        let missing = if self.style.unicode { "—" } else { "none" };
        let mut table = Display::from_width(self.style.width)
            .table(records.columns.iter().map(|column| column.label).collect());
        for row in &records.rows {
            table.add_row(
                row.iter()
                    .map(|value| value.as_deref().map_or_else(|| missing.to_string(), clean))
                    .collect::<Vec<_>>(),
            );
        }
        out.push_str(&format!("{table}\n"));
        out.push_str(&noun.count(records.rows.len()));
        if let Some(hint) = hint {
            out.push(' ');
            out.push_str(&clean(hint));
        }
        out.push('\n');
    }
}

impl Renderer for RichRenderer {
    fn render(&self, report: &Report) -> String {
        let mut out = String::new();
        let rule = if self.style.unicode { "─" } else { "-" };
        for block in &report.blocks {
            match block {
                Block::Heading(text) => {
                    out.push_str(&paint(&clean(text), BOLD, self.style.color));
                    out.push('\n');
                    out.push_str(&rule.repeat(40));
                    out.push('\n');
                }
                Block::Status(level, spans) => {
                    out.push_str(&format!("{} {}\n", self.symbol(*level), self.spans(spans)));
                }
                Block::Note(spans) => out.push_str(&format!("  {}\n", self.spans(spans))),
                Block::Field(label, value) => {
                    out.push_str(&format!("  {label}: {}\n", clean(value)));
                }
                Block::List { noun, empty, hint } => {
                    self.list(&mut out, &report.records, *noun, empty, hint.as_deref());
                }
                Block::Break => out.push('\n'),
            }
        }
        out
    }
}

impl PlainRenderer {
    fn spans(spans: &[Span]) -> String {
        spans
            .iter()
            .map(|span| match span {
                Span::Text(text) | Span::Name(text) => clean(text),
                Span::Command(command) => format!("\"{}\"", clean(command)),
            })
            .collect()
    }

    /// `Label: value`, with continuation lines indented so each record stays
    /// one block of lines.
    fn field(out: &mut String, label: &str, value: Option<&str>) {
        let value = value.map_or_else(|| "none".to_string(), clean);
        out.push_str(label);
        out.push_str(": ");
        out.push_str(&value.replace('\n', "\n  "));
        out.push('\n');
    }

    fn list(out: &mut String, records: &Records, noun: Noun, empty: &str, hint: Option<&str>) {
        if records.rows.is_empty() {
            out.push_str(&clean(empty));
            out.push('\n');
            return;
        }
        out.push_str(&noun.count(records.rows.len()));
        if let Some(hint) = hint {
            out.push(' ');
            out.push_str(&clean(hint));
        }
        out.push('\n');
        for row in &records.rows {
            out.push('\n');
            for (column, value) in records.columns.iter().zip(row) {
                Self::field(out, column.label, value.as_deref());
            }
        }
    }
}

impl Renderer for PlainRenderer {
    fn render(&self, report: &Report) -> String {
        let mut out = String::new();
        for block in &report.blocks {
            match block {
                Block::Heading(text) => {
                    out.push_str(&clean(text));
                    out.push('\n');
                }
                Block::Status(level, spans) => {
                    let word = match level {
                        Level::Ok => "OK",
                        Level::Warning => "Warning",
                        Level::Error => "Error",
                    };
                    out.push_str(&format!("{word}: {}\n", Self::spans(spans)));
                }
                Block::Note(spans) => out.push_str(&format!("{}\n", Self::spans(spans))),
                Block::Field(label, value) => Self::field(&mut out, label, Some(value)),
                Block::List { noun, empty, hint } => {
                    Self::list(&mut out, &report.records, *noun, empty, hint.as_deref());
                }
                Block::Break => out.push('\n'),
            }
        }
        out
    }
}

impl Renderer for JsonRenderer {
    fn render(&self, report: &Report) -> String {
        // Serializing a `serde_json::Value` cannot fail: its map keys are
        // always strings.
        let value = self.fields.as_ref().map_or_else(
            || report.json.clone(),
            |fields| select_fields(&report.json, fields),
        );
        let mut text = if self.pretty {
            serde_json::to_string_pretty(&value)
        } else {
            serde_json::to_string(&value)
        }
        .expect("a JSON value always serializes");
        text.push('\n');
        text
    }
}

/// Escape a TSV field so that every record stays on one line and the column
/// count never changes: backslash, tab, newline, and carriage return become
/// `\\`, `\t`, `\n`, and `\r`. Other control characters and Unicode line
/// separators use `\u{hex}` escapes, preventing terminal control injection.
pub fn escape_tsv(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                escaped.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
            }
            c => escaped.push(c),
        }
    }
    escaped
}

impl Renderer for TsvRenderer {
    fn render(&self, report: &Report) -> String {
        let records = &report.records;
        let mut out = records
            .columns
            .iter()
            .map(|column| column.key)
            .collect::<Vec<_>>()
            .join("\t");
        out.push('\n');
        for row in &records.rows {
            let line = row
                .iter()
                .map(|value| value.as_deref().map(escape_tsv).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\t");
            out.push_str(&line);
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn env() -> Environment {
        Environment::default()
    }

    fn settings(env: &Environment) -> Settings<'_> {
        Settings {
            format: None,
            color: ColorChoice::Auto,
            config_format: None,
            accessible: false,
            display: Display::from_width(80),
            json_output: true,
            tsv_output: true,
            fields: None,
            env,
            stdout_terminal: true,
            stderr_terminal: true,
        }
    }

    fn sample() -> Report {
        Report {
            blocks: vec![
                Block::Heading("Section".into()),
                Block::Status(
                    Level::Ok,
                    vec![
                        Span::Text("Signed in as ".into()),
                        Span::Name("~someone".into()),
                    ],
                ),
                Block::Note(vec![
                    Span::Text("Run ".into()),
                    Span::Command("lpcli login".into()),
                    Span::Text(" again.".into()),
                ]),
                Block::Field("Endpoint", "https://api.launchpad.net/devel/".into()),
                Block::Break,
                Block::List {
                    noun: Noun {
                        singular: "item",
                        plural: "items",
                    },
                    empty: "No items found.".into(),
                    hint: Some("A hint.".into()),
                },
            ],
            json: serde_json::json!({"total_size": 2, "start": 0, "entries": []}),
            records: Records {
                columns: vec![
                    Column {
                        label: "Name",
                        key: "name",
                    },
                    Column {
                        label: "Summary",
                        key: "summary",
                    },
                ],
                rows: vec![
                    vec![Some("first".into()), Some("tab\there\nnew \\ line".into())],
                    vec![Some("second".into()), None],
                ],
            },
        }
    }

    #[test]
    fn mode_priority_is_flag_env_config_automatic() {
        let mut env = env();
        let choose = |env: &Environment, flag, config, accessible| {
            choose_mode(flag, env, config, accessible).unwrap()
        };
        assert_eq!(
            choose(&env, None, None, false),
            ModeChoice {
                mode: OutputMode::Rich,
                source: ModeSource::Automatic
            }
        );
        assert_eq!(choose(&env, None, None, true).mode, OutputMode::Plain);
        assert_eq!(
            choose(&env, None, Some(OutputMode::Tsv), true),
            ModeChoice {
                mode: OutputMode::Tsv,
                source: ModeSource::Config
            }
        );
        env.format = Some("json".into());
        assert_eq!(
            choose(&env, None, Some(OutputMode::Tsv), true),
            ModeChoice {
                mode: OutputMode::Json,
                source: ModeSource::Environment
            }
        );
        assert_eq!(
            choose(&env, Some(OutputMode::Plain), Some(OutputMode::Tsv), false),
            ModeChoice {
                mode: OutputMode::Plain,
                source: ModeSource::Flag
            }
        );
    }

    #[test]
    fn dumb_terminal_selects_plain_and_empty_env_is_unset() {
        let env = Environment {
            term: Some("dumb".into()),
            format: Some(String::new()),
            ..env()
        };
        assert_eq!(
            choose_mode(None, &env, None, false).unwrap(),
            ModeChoice {
                mode: OutputMode::Plain,
                source: ModeSource::Automatic
            }
        );
    }

    #[test]
    fn invalid_env_format_is_a_clear_error_unless_overridden() {
        for value in ["xml", "JSON", " json", "text\u{1b}[31m"] {
            let env = Environment {
                format: Some(value.into()),
                ..env()
            };
            let error = choose_mode(None, &env, None, false).unwrap_err();
            assert!(error.contains(FORMAT_ENV), "{error}");
            assert!(error.contains("rich, plain, json, or tsv"), "{error}");
            assert!(
                !error.contains('\u{1b}'),
                "escape sequences must not be echoed: {error}"
            );
            assert_eq!(
                choose_mode(Some(OutputMode::Json), &env, None, false)
                    .unwrap()
                    .mode,
                OutputMode::Json
            );
        }
    }

    #[test]
    fn color_rules_follow_flag_then_conventions_then_terminal() {
        let base = env();
        assert!(color_enabled(ColorChoice::Auto, &base, true));
        assert!(!color_enabled(ColorChoice::Auto, &base, false));
        assert!(color_enabled(ColorChoice::Always, &base, false));
        assert!(!color_enabled(ColorChoice::Never, &base, true));

        let no_color = Environment {
            no_color: Some("1".into()),
            ..env()
        };
        assert!(!color_enabled(ColorChoice::Auto, &no_color, true));
        assert!(color_enabled(ColorChoice::Always, &no_color, true));
        let empty_no_color = Environment {
            no_color: Some(String::new()),
            ..env()
        };
        assert!(color_enabled(ColorChoice::Auto, &empty_no_color, true));

        let dumb = Environment {
            term: Some("dumb".into()),
            ..env()
        };
        assert!(!color_enabled(ColorChoice::Auto, &dumb, true));

        for (force, expected) in [("1", true), ("yes", true), ("0", false), ("", false)] {
            let forced = Environment {
                clicolor_force: Some(force.into()),
                no_color: Some("1".into()),
                ..env()
            };
            assert_eq!(
                color_enabled(ColorChoice::Auto, &forced, false),
                expected,
                "CLICOLOR_FORCE={force:?}"
            );
            assert!(!color_enabled(ColorChoice::Never, &forced, true));
        }
    }

    #[test]
    fn color_is_decided_per_stream_and_only_rich_colors_stdout() {
        let env = env();
        let output = Output::resolve(Settings {
            stdout_terminal: false,
            stderr_terminal: true,
            ..settings(&env)
        })
        .unwrap();
        assert!(!output.stdout.color);
        assert!(output.stderr.color);
        assert!(!output.pretty_json);

        for mode in [OutputMode::Plain, OutputMode::Json, OutputMode::Tsv] {
            let output = Output::resolve(Settings {
                format: Some(mode),
                color: ColorChoice::Always,
                ..settings(&env)
            })
            .unwrap();
            assert!(!output.stdout.color, "{mode:?} stdout must not be colored");
            assert_eq!(output.stderr.color, mode != OutputMode::Plain);
        }
    }

    #[test]
    fn unsupported_structured_modes_fall_back_only_when_implicit() {
        let env = Environment {
            format: Some("json".into()),
            ..env()
        };
        let output = Output::resolve(Settings {
            json_output: false,
            tsv_output: false,
            ..settings(&env)
        })
        .unwrap();
        assert_eq!(output.mode(), OutputMode::Rich);
        assert!(output.warning().unwrap().contains("LPCLI_FORMAT=json"));

        let output = Output::resolve(Settings {
            json_output: false,
            tsv_output: false,
            accessible: true,
            config_format: Some(OutputMode::Tsv),
            ..settings(&Environment::default())
        })
        .unwrap();
        assert_eq!(output.mode(), OutputMode::Plain);
        assert!(
            output
                .warning()
                .unwrap()
                .contains("display.format = \"tsv\"")
        );
        assert_eq!(
            output.render_warning(output.warning().unwrap()),
            "Warning: display.format = \"tsv\" is not supported by this command yet; using plain output.\n"
        );

        let error = Output::resolve(Settings {
            json_output: false,
            tsv_output: false,
            format: Some(OutputMode::Tsv),
            ..settings(&env)
        })
        .unwrap_err();
        assert!(error.contains("--format tsv"), "{error}");

        let output = Output::resolve(Settings {
            json_output: false,
            tsv_output: false,
            format: Some(OutputMode::Plain),
            ..settings(&env)
        })
        .unwrap();
        assert_eq!((output.mode(), output.warning()), (OutputMode::Plain, None));
    }

    #[test]
    fn rich_rendering_without_color_has_no_escape_codes() {
        let rendered = Output::fixed(OutputMode::Rich, false, 80).render(&sample());
        insta::assert_snapshot!(rendered);
        assert!(!rendered.contains('\u{1b}'));
    }

    #[test]
    fn rich_rendering_with_color_styles_symbols_and_names() {
        let rendered = Output::fixed(OutputMode::Rich, true, 80).render(&sample());
        assert!(rendered.contains("\u{1b}[1m\u{1b}[32m✓"), "{rendered:?}");
        assert!(
            rendered.contains("\u{1b}[1m\u{1b}[36m~someone"),
            "{rendered:?}"
        );
    }

    #[test]
    fn plain_rendering_is_ascii_labeled_records() {
        let rendered = Output::fixed(OutputMode::Plain, true, 80).render(&sample());
        insta::assert_snapshot!(rendered);
        assert_plain_invariants(&rendered);
    }

    #[test]
    fn tsv_escapes_terminal_controls_reversibly() {
        let mut report = sample();
        let payload = "\u{1b}]52;c;payload\u{7}\u{0}\u{85}\u{2028}\u{2029}";
        report.records.rows[0][0] = Some(payload.into());
        let rendered = Output::fixed(OutputMode::Tsv, false, 80).render(&report);
        assert_tsv_invariants(&rendered, 2);
        assert!(rendered.contains("\\u{1b}]52;c;payload\\u{7}\\u{0}\\u{85}\\u{2028}\\u{2029}"));
        assert!(
            !rendered
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        );
        assert_eq!(escape_tsv("\\u{1b}"), "\\\\u{1b}");
    }

    #[test]
    fn tsv_has_one_line_per_record_and_escapes_separators() {
        let rendered = Output::fixed(OutputMode::Tsv, true, 80).render(&sample());
        assert_eq!(
            rendered,
            "name\tsummary\nfirst\ttab\\there\\nnew \\\\ line\nsecond\t\n"
        );
        assert_tsv_invariants(&rendered, 2);
    }

    #[test]
    fn json_is_the_report_value_pretty_only_for_terminals() {
        let report = sample();
        let pretty = JsonRenderer {
            pretty: true,
            fields: None,
        }
        .render(&report);
        let compact = JsonRenderer {
            pretty: false,
            fields: None,
        }
        .render(&report);
        assert_eq!(compact, "{\"entries\":[],\"start\":0,\"total_size\":2}\n");
        assert!(pretty.contains("\n  \"entries\""));
        for text in [pretty, compact] {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&text).unwrap(),
                report.json
            );
        }
    }

    #[test]
    fn empty_lists_print_the_empty_message() {
        let mut report = sample();
        report.records.rows.clear();
        for mode in [OutputMode::Rich, OutputMode::Plain] {
            let rendered = Output::fixed(mode, false, 80).render(&report);
            assert!(rendered.ends_with("No items found.\n"), "{rendered}");
            assert!(!rendered.contains("A hint."));
        }
        assert_eq!(
            Output::fixed(OutputMode::Tsv, false, 80).render(&report),
            "name\tsummary\n"
        );
    }

    #[test]
    fn terminal_control_sequences_in_data_are_neutralised() {
        let mut report = sample();
        report.records.rows[0][0] = Some("evil\u{1b}]52;c;payload\u{7}\r".into());
        report.blocks[1] = Block::Status(Level::Ok, vec![Span::Name("~a\u{1b}[2J".into())]);
        for mode in [OutputMode::Rich, OutputMode::Plain] {
            let rendered = Output::fixed(mode, false, 200).render(&report);
            assert!(!rendered.contains('\u{1b}'), "{mode:?}: {rendered:?}");
            assert!(!rendered.contains('\u{7}'), "{mode:?}: {rendered:?}");
            assert!(!rendered.contains('\r'), "{mode:?}: {rendered:?}");
        }
        let json = Output::fixed(OutputMode::Json, false, 80).render(&Report {
            json: serde_json::json!({"title": "evil\u{1b}[2J"}),
            ..report.clone()
        });
        assert!(
            !json.contains('\u{1b}'),
            "JSON escapes control characters: {json:?}"
        );
    }

    #[test]
    fn credential_redaction_covers_json_values_form_values_and_every_header() {
        let input = concat!(
            "{\"oauth_token_secret\": \"json-secret\", \"oauth_token\":\"json-token\"} ",
            "oauth_signature=form-secret&refresh_token: 'refresh-secret'\n",
            "Authorization: OAuth oauth_token=header-one\n",
            "authorization: Bearer header-two"
        );
        let output = redact_secrets(input);
        for value in [
            "json-secret",
            "json-token",
            "form-secret",
            "refresh-secret",
            "header-one",
            "header-two",
        ] {
            assert!(!output.contains(value), "{value} leaked: {output}");
        }
        assert_eq!(
            output.matches("Authorization: [redacted]").count(),
            2,
            "{output}"
        );
        assert!(output.contains("\"oauth_token_secret\": \"[redacted]\""));
    }

    #[test]
    fn warnings_and_runtime_messages_also_redact_secrets() {
        for rendered in [
            Output::fixed(OutputMode::Plain, false, 80)
                .render_error_message("OAuth failure: {\"oauth_token_secret\":\"s3cret\"}"),
            Output::fixed(OutputMode::Plain, false, 80)
                .render_warning("response Authorization: Bearer topsecret"),
        ] {
            assert!(!rendered.contains("s3cret"), "{rendered}");
            assert!(!rendered.contains("topsecret"), "{rendered}");
        }
    }

    #[test]
    fn legacy_text_honours_explicit_color_decision() {
        let text = "\u{1b}[1;32m✓\u{1b}[0m done\n";
        assert_eq!(
            Output::fixed(OutputMode::Rich, false, 80).style_legacy_text(text),
            "✓ done\n"
        );
        assert_eq!(
            Output::fixed(OutputMode::Rich, true, 80).style_legacy_text(text),
            text
        );
    }

    #[test]
    fn legacy_stdout_and_stderr_text_use_their_own_stream_decision() {
        let env = Environment::default();
        let output = Output::resolve(Settings {
            stdout_terminal: true,
            stderr_terminal: false,
            ..settings(&env)
        })
        .unwrap();
        let text = "\u{1b}[33mwarning\u{1b}[0m";
        assert_eq!(output.style_legacy_text(text), text);
        assert_eq!(output.style_legacy_stderr_text(text), "warning");
    }

    #[test]
    fn sgr_stripping_never_alters_non_sgr_text() {
        for (input, expected) in [
            ("plain text, no escapes", "plain text, no escapes"),
            ("\u{1b}[mreset", "reset"),
            (
                "\u{1b}[2J clear, then more text",
                "\u{1b}[2J clear, then more text",
            ),
            ("unterminated \u{1b}[12", "unterminated \u{1b}[12"),
            ("\u{1b}]52;c;payload\u{7}", "\u{1b}]52;c;payload\u{7}"),
            ("café \u{1b}[1m✓\u{1b}[0m", "café ✓"),
        ] {
            assert_eq!(strip_sgr(input), expected, "{input:?}");
        }
    }

    #[test]
    fn rich_warnings_are_colored_only_when_stderr_allows() {
        let output = Output::fixed(OutputMode::Rich, false, 80);
        assert_eq!(output.render_warning("careful"), "warning: careful\n");
        let output = Output::fixed(OutputMode::Json, true, 80);
        assert!(output.render_warning("careful").starts_with("\u{1b}["));
    }

    /// `plain` output: ASCII decoration only, no ANSI codes, no box drawing.
    pub fn assert_plain_invariants(rendered: &str) {
        assert!(!rendered.contains('\u{1b}'), "ANSI escape in plain output");
        for c in rendered.chars() {
            assert!(
                !matches!(c, '\u{2500}'..='\u{257F}' | '✓' | '✗' | '—' | '\u{1F300}'..='\u{1FAFF}'),
                "decorative character {c:?} in plain output:\n{rendered}"
            );
        }
    }

    /// `tsv` output: a header plus exactly one line per record, each with
    /// the header's column count.
    pub fn assert_tsv_invariants(rendered: &str, records: usize) {
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), records + 1, "{rendered}");
        let columns = lines[0].split('\t').count();
        for line in &lines {
            assert_eq!(line.split('\t').count(), columns, "{line:?}");
        }
        assert!(!rendered.contains('\u{1b}'));
    }
}
