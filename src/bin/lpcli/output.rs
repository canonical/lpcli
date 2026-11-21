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
//! Styling is passed explicitly through [`Style`] rather than through a
//! process-wide override, so renderers are pure functions that tests can run
//! in parallel.

use std::io::{IsTerminal, Write};

use anstyle::{AnsiColor, Color, Style as AnsiStyle};
use clap::ValueEnum;
use comfy_table::{
    ColumnConstraint, ContentArrangement, ContentLineStyle, LineStyle, Table, TableStyle, Width,
};
use serde::Deserialize;

use crate::display::{ASCII_LIGHT_TABLE, Display};

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

/// When and how to report download progress on stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum ProgressChoice {
    /// Follow the output mode and whether stderr is a terminal.
    #[default]
    Auto,
    /// Start line, occasional milestones, and a summary, with no redrawing.
    Plain,
    /// Animated progress bars on stderr.
    Rich,
    /// Do not report progress.
    None,
}

/// Choose the download progress mode for an invocation.
pub fn resolve_progress(
    choice: ProgressChoice,
    mode: OutputMode,
    stderr_terminal: bool,
) -> lpcli::download::ProgressMode {
    match choice {
        ProgressChoice::None => lpcli::download::ProgressMode::None,
        ProgressChoice::Plain => lpcli::download::ProgressMode::Plain,
        ProgressChoice::Rich => lpcli::download::ProgressMode::Rich,
        ProgressChoice::Auto => {
            if mode == OutputMode::Plain {
                lpcli::download::ProgressMode::Plain
            } else if mode.is_structured() {
                lpcli::download::ProgressMode::None
            } else if !stderr_terminal {
                lpcli::download::ProgressMode::Plain
            } else {
                lpcli::download::ProgressMode::Rich
            }
        }
    }
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
    /// `PAGER`.
    pub pager: Option<String>,
    /// `LINES`.
    pub lines: Option<String>,
    /// Whether `SUDO_*` environment variables indicate running under `sudo`.
    pub sudo: bool,
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
            pager: var("PAGER"),
            lines: var("LINES"),
            sudo: ["SUDO_USER", "SUDO_UID", "SUDO_GID"]
                .iter()
                .any(|name| std::env::var_os(name).is_some()),
        }
    }

    fn term_is_dumb(&self) -> bool {
        self.term.as_deref() == Some("dumb")
    }
}

/// Default line count threshold above which human output is sent to `$PAGER`
/// when `LINES` and terminal height detection are unavailable.
pub const DEFAULT_PAGER_LINES: usize = 24;

/// Whether a configured `$PAGER` command is empty or invokes `cat`.
fn is_noop_pager(pager: &str) -> bool {
    let trimmed = pager.trim();
    if trimmed.is_empty() {
        return true;
    }
    split_command_words(trimmed).is_some_and(|words| {
        std::path::Path::new(&words[0]).file_name() == Some(std::ffi::OsStr::new("cat"))
    })
}

/// Choose the line threshold above which output is piped to `$PAGER`.
///
/// Prefers a positive `LINES` override, then the detected terminal height in
/// rows, and falls back to [`DEFAULT_PAGER_LINES`].
pub fn resolve_pager_lines(env_lines: Option<&str>, terminal_rows: Option<u16>) -> usize {
    env_lines
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&lines| lines > 0)
        .or_else(|| terminal_rows.filter(|&rows| rows > 0).map(usize::from))
        .unwrap_or(DEFAULT_PAGER_LINES)
}

/// Resolve the pager command for an invocation, if paging is enabled.
///
/// Paging is used only when stdout is a non-dumb terminal, `--no-pager` was
/// not passed, the process is not running under `sudo`, the mode is
/// human-facing (`rich` or `plain`), and `PAGER` is set to a non-empty command
/// whose program is not `cat`.
pub fn resolve_pager(
    no_pager: bool,
    mode: OutputMode,
    env: &Environment,
    stdout_terminal: bool,
) -> Option<String> {
    if no_pager || !stdout_terminal || env.term_is_dumb() || env.sudo || mode.is_structured() {
        return None;
    }
    let pager = env.pager.as_deref()?.trim();
    if is_noop_pager(pager) {
        return None;
    }
    Some(pager.to_string())
}

/// Whether `rendered` is long enough to be piped through `pager`.
pub fn should_page(pager: Option<&str>, rendered: &str, max_lines: usize) -> bool {
    pager.is_some_and(|cmd| !is_noop_pager(cmd)) && rendered.lines().count() > max_lines
}

/// Split a shell-like command string (such as `less -FRX` or `"/usr/bin/less" -R`)
/// into program and arguments without invoking a shell.
fn split_command_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '\\') => {
                let next = chars.next()?;
                current.push(next);
                in_word = true;
            }
            (Some('"'), '\\') => match chars.next() {
                Some(next @ ('"' | '\\' | '$' | '`')) => current.push(next),
                Some(next) => {
                    current.push('\\');
                    current.push(next);
                }
                None => return None,
            },
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (Some(q), c) if c == q => {
                quote = None;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            (_, c) => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push(current);
    }
    (!words.is_empty()).then_some(words)
}

fn write_all_ignore_broken_pipe(writer: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    match writer.write_all(bytes).and_then(|()| writer.flush()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err),
    }
}

fn pager_exited_cleanly(status: std::process::ExitStatus) -> bool {
    if status.success() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        // Treat SIGINT (2) and SIGPIPE (13) from user/pipe termination as clean.
        if status.code().is_none() && matches!(status.signal(), Some(2 | 13)) {
            return true;
        }
    }
    false
}

fn write_with_pager_io<W: Write, E: Write>(
    rendered: &str,
    pager: Option<&str>,
    max_lines: usize,
    stdout: &mut W,
    stderr: &mut E,
    format_warning: impl Fn(&str) -> String,
) -> lpcli::error::Result<bool> {
    if should_page(pager, rendered, max_lines)
        && let Some(pager_cmd) = pager
    {
        match split_command_words(pager_cmd) {
            Some(words) => {
                let (program, args) = (&words[0], &words[1..]);
                match std::process::Command::new(program)
                    .args(args)
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(mut child) => {
                        let mut pipe_error = None;
                        if let Some(mut stdin) = child.stdin.take() {
                            match stdin
                                .write_all(rendered.as_bytes())
                                .and_then(|()| stdin.flush())
                            {
                                Ok(()) => {}
                                Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {}
                                Err(err) => {
                                    pipe_error = Some(err);
                                }
                            }
                        }
                        let status = child.wait()?;
                        if let Some(err) = pipe_error {
                            return Err(err.into());
                        }
                        if !pager_exited_cleanly(status) {
                            return Err(lpcli::error::LpError::Io(std::io::Error::other(format!(
                                "Pager '{program}' exited unsuccessfully ({status})."
                            ))));
                        }
                        return Ok(true);
                    }
                    Err(err) => {
                        let warning = format_warning(&format!(
                            "failed to start pager '{program}': {err}; writing directly to stdout."
                        ));
                        let _ = stderr.write_all(warning.as_bytes());
                    }
                }
            }
            None => {
                let warning =
                    format_warning("ignoring invalid PAGER command; writing directly to stdout.");
                let _ = stderr.write_all(warning.as_bytes());
            }
        }
    }
    write_all_ignore_broken_pipe(stdout, rendered.as_bytes())?;
    Ok(false)
}

/// Write `rendered` through `pager` when `should_page` holds, or directly to
/// stdout otherwise. Returns `true` if output was sent to the pager process.
#[cfg(test)]
pub fn write_with_pager(
    rendered: &str,
    pager: Option<&str>,
    max_lines: usize,
) -> lpcli::error::Result<bool> {
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    write_with_pager_io(
        rendered,
        pager,
        max_lines,
        &mut stdout,
        &mut stderr,
        |msg| format!("warning: {msg}\n"),
    )
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

/// How relative timestamp hints (such as `2 days ago`) are attached to `rich`
/// single-value timestamps and comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeTime {
    /// Use `jiff::Timestamp::now()`.
    System,
    /// Use an injected timestamp (for deterministic tests).
    #[cfg(test)]
    Fixed(jiff::Timestamp),
    /// Omit relative hints (default for `Output::fixed`).
    #[cfg(test)]
    Disabled,
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
    progress: lpcli::download::ProgressMode,
    pager: Option<String>,
    pager_lines: usize,
    time_zone: jiff::tz::TimeZone,
    relative_time: RelativeTime,
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
        let terminal_rows = settings
            .stdout_terminal
            .then(crate::display::terminal_height)
            .flatten();
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
            progress: resolve_progress(ProgressChoice::Auto, mode, settings.stderr_terminal),
            pager: resolve_pager(false, mode, settings.env, settings.stdout_terminal),
            pager_lines: resolve_pager_lines(settings.env.lines.as_deref(), terminal_rows),
            time_zone: jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC),
            relative_time: RelativeTime::System,
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
            progress: lpcli::download::ProgressMode::None,
            pager: None,
            pager_lines: DEFAULT_PAGER_LINES,
            time_zone: jiff::tz::TimeZone::UTC,
            relative_time: RelativeTime::Disabled,
        }
    }

    /// Override the time zone used for `rich` timestamp formatting, for tests.
    #[cfg(test)]
    pub fn with_time_zone(mut self, time_zone: jiff::tz::TimeZone) -> Self {
        self.time_zone = time_zone;
        self
    }

    /// Override relative timestamp hints for `rich` rendering, for tests.
    #[cfg(test)]
    pub fn with_relative_time(mut self, relative_time: RelativeTime) -> Self {
        self.relative_time = relative_time;
        self
    }

    /// Select the download progress mode on stderr from `choice` and `stderr_terminal`.
    pub fn with_progress(mut self, choice: ProgressChoice, stderr_terminal: bool) -> Self {
        self.progress = resolve_progress(choice, self.mode, stderr_terminal);
        self
    }

    /// The resolved download progress mode for stderr.
    pub fn progress(&self) -> lpcli::download::ProgressMode {
        self.progress
    }

    /// Select whether long human output goes through `$PAGER`.
    pub fn with_pager(mut self, no_pager: bool, env: &Environment, stdout_terminal: bool) -> Self {
        self.pager = resolve_pager(no_pager, self.mode, env, stdout_terminal);
        self.pager_lines = resolve_pager_lines(
            env.lines.as_deref(),
            stdout_terminal
                .then(crate::display::terminal_height)
                .flatten(),
        );
        self
    }

    /// The resolved pager command, if paging is enabled for this invocation.
    #[cfg(test)]
    pub fn pager(&self) -> Option<&str> {
        self.pager.as_deref()
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
    #[cfg(test)]
    pub fn stdout_style(&self) -> Style {
        self.stdout
    }

    /// Prepare legacy, already-formatted text for stdout using this output's
    /// explicit stdout color decision.
    #[cfg(test)]
    pub fn style_legacy_text(&self, text: &str) -> String {
        if self.stdout.color {
            text.to_string()
        } else {
            strip_sgr(text)
        }
    }

    /// Prepare legacy, already-formatted text for stderr using this output's
    /// explicit stderr color decision.
    #[cfg(test)]
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
            OutputMode::Rich => Box::new(RichRenderer {
                style: self.stdout,
                time_zone: self.time_zone.clone(),
                relative_time: self.relative_time,
            }),
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

    /// Render a human-facing report using stderr styling, or an empty string
    /// in structured (`json`/`tsv`) modes.
    pub fn render_stderr_report(&self, report: &Report) -> String {
        match self.mode {
            OutputMode::Rich => RichRenderer {
                style: self.stderr,
                time_zone: self.time_zone.clone(),
                relative_time: self.relative_time,
            }
            .render(report),
            OutputMode::Plain => PlainRenderer.render(report),
            OutputMode::Json | OutputMode::Tsv => String::new(),
        }
    }

    /// Write a human-facing report directly to stderr without paging.
    pub fn write_stderr_report(&self, report: &Report) -> lpcli::error::Result<()> {
        let rendered = self.render_stderr_report(report);
        if rendered.is_empty() {
            return Ok(());
        }
        let mut stderr = std::io::stderr().lock();
        stderr.write_all(rendered.as_bytes())?;
        stderr.flush()?;
        Ok(())
    }

    /// Format a redacted, mode-sanitized diagnostic line for stderr.
    pub fn render_stderr_line(&self, message: &str) -> String {
        let redacted = redact_secrets(message);
        if self.mode == OutputMode::Plain {
            format!("{}\n", clean_plain(&redacted))
        } else {
            format!("{}\n", clean(&redacted))
        }
    }

    /// Write a redacted, mode-sanitized diagnostic line to stderr.
    pub fn write_stderr_line(&self, message: &str) -> lpcli::error::Result<()> {
        let rendered = self.render_stderr_line(message);
        let mut stderr = std::io::stderr().lock();
        stderr.write_all(rendered.as_bytes())?;
        stderr.flush()?;
        Ok(())
    }

    /// Write pre-rendered text to stdout, piping through `$PAGER` when enabled
    /// and the text exceeds the terminal height threshold.
    pub fn write_paged(&self, rendered: &str) -> lpcli::error::Result<()> {
        let mut stdout = std::io::stdout().lock();
        let mut stderr = std::io::stderr().lock();
        write_with_pager_io(
            rendered,
            self.pager.as_deref(),
            self.pager_lines,
            &mut stdout,
            &mut stderr,
            |msg| self.render_warning(msg),
        )?;
        Ok(())
    }

    /// Write raw pre-formatted text directly to stdout without paging or
    /// re-rendering (for `skills`, `--quiet`, and one-time access token secrets).
    pub fn write_raw(&self, text: &str) -> lpcli::error::Result<()> {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(text.as_bytes())?;
        stdout.flush()?;
        Ok(())
    }

    /// Write a single raw line directly to stdout without paging or copying
    /// the string (used for zeroized secrets such as `hardware-token issue --print`).
    pub fn write_raw_line(&self, line: &str) -> lpcli::error::Result<()> {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(line.as_bytes())?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
        Ok(())
    }

    /// Write a report to stdout, piping through `$PAGER` when appropriate.
    pub fn print(&self, report: &Report) -> lpcli::error::Result<()> {
        self.write_paged(&self.render(report))
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
        let redacted = redact_secrets(message);
        if self.mode == OutputMode::Plain {
            format!("Error: {}\n", clean_plain(&redacted))
        } else {
            format!("{} {}\n", error_prefix(self.stderr.color), clean(&redacted))
        }
    }

    /// Format a warning for stderr.
    pub fn render_warning(&self, message: &str) -> String {
        let redacted = redact_secrets(message);
        if self.mode == OutputMode::Plain {
            format!("Warning: {}\n", clean_plain(&redacted))
        } else {
            format!(
                "{} {}\n",
                paint("warning:", WARNING, self.stderr.color),
                clean(&redacted)
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
    lpcli::error::redact_secrets(message)
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

/// How a column's cell value should be formatted for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnFormat {
    /// Display the stored text as-is.
    #[default]
    Text,
    /// Parse RFC 3339 timestamps and format as `YYYY-MM-DD` (local in `rich`, UTC in `plain`/`tsv`).
    Date,
    /// Parse RFC 3339 timestamps and format as `YYYY-MM-DD HH:MM` (local in `rich`, UTC in `plain`/`tsv`).
    DateTime,
}

/// Precision for a single [`Block::Timestamp`] value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampPrecision {
    Date,
    DateTime,
}

/// A comment timestamp, either a parsed `Timestamp` or a raw timestamp string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentDate {
    Timestamp(jiff::Timestamp),
    Text(String),
}

/// A record column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// The label shown in tables and `plain` records.
    pub label: &'static str,
    /// The `tsv` header, a Launchpad attribute name where one exists.
    pub key: &'static str,
    /// Optional timestamp formatting applied per renderer.
    pub format: ColumnFormat,
    /// Whether `rich` tables omit this column, for values such as long URLs
    /// that leave too little room for the other columns.
    pub hide_in_rich: bool,
}

impl Column {
    /// Construct a plain text column.
    pub const fn new(label: &'static str, key: &'static str) -> Self {
        Self {
            label,
            key,
            format: ColumnFormat::Text,
            hide_in_rich: false,
        }
    }

    /// Construct a column whose values `rich` tables omit.
    ///
    /// `plain`, `tsv`, and `json` still include the column, so long URLs
    /// remain available for copying and machine consumption.
    pub const fn rich_hidden(label: &'static str, key: &'static str) -> Self {
        Self {
            label,
            key,
            format: ColumnFormat::Text,
            hide_in_rich: true,
        }
    }

    /// Construct a `YYYY-MM-DD` date column backed by raw RFC 3339 timestamps.
    pub const fn date(label: &'static str, key: &'static str) -> Self {
        Self {
            label,
            key,
            format: ColumnFormat::Date,
            hide_in_rich: false,
        }
    }

    /// Construct a `YYYY-MM-DD HH:MM` timestamp column backed by raw RFC 3339 timestamps.
    pub const fn datetime(label: &'static str, key: &'static str) -> Self {
        Self {
            label,
            key,
            format: ColumnFormat::DateTime,
            hide_in_rich: false,
        }
    }
}

/// The singular and plural names of the records in a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Noun {
    pub singular: &'static str,
    pub plural: &'static str,
}

impl Noun {
    pub(crate) fn count(self, n: usize) -> String {
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

/// An attachment displayed beneath a [`CommentEntry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentAttachment {
    /// Attachment title or filename.
    pub title: String,
    /// Attachment classification such as `"Patch"` or `"Unspecified"`.
    pub kind: String,
    /// Resolved direct download URL when available.
    pub url: Option<String>,
}

/// A single comment card rendered as a structured entry list in `plain` mode
/// and a width-aware table card in `rich` mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentEntry {
    /// Primary record field label in `plain` and stacked modes (e.g. `"Comment"`, `"Message"`, `"Location"`).
    pub label_field: &'static str,
    /// Comment sequence or location label such as `"#0"` or `"lib/model.py:5"`.
    pub label: String,
    /// Normalized author account name, if known.
    pub author: Option<String>,
    /// Comment timestamp, if known.
    pub date: Option<CommentDate>,
    /// Additional structured fields shown below the header in `rich` and after `Date` in `plain`.
    pub details: Vec<(&'static str, String)>,
    /// Content field label in `plain` and stacked modes (e.g. `"Content"`, `"Text"`).
    pub content_field: &'static str,
    /// Raw comment body text.
    pub content: String,
    /// Attachments associated with this comment, in Launchpad order.
    pub attachments: Vec<CommentAttachment>,
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
    /// A labeled timestamp rendered in local time with an optional relative hint
    /// in `rich` mode and UTC in `plain` mode.
    Timestamp(&'static str, jiff::Timestamp, TimestampPrecision),
    /// The report's [`Records`], with a leading count in both `rich` and `plain`,
    /// an optional hint, and a message for an empty list.
    List {
        noun: Noun,
        empty: String,
        hint: Option<String>,
    },
    /// A table of records for `rich` and `plain` output whose columns or
    /// rows are independent of [`Report::records`] (used when a report has
    /// multiple tables or when `tsv` uses raw Launchpad attributes).
    Table {
        records: Records,
        noun: Noun,
        empty: String,
        hint: Option<String>,
    },
    /// A table shown only in `rich` output.
    ///
    /// This is for supplementary detail, such as long URLs, that `plain`,
    /// `tsv`, and `json` already carry elsewhere and that would make the
    /// main `rich` table too narrow.
    RichTable {
        records: Records,
        noun: Noun,
        empty: String,
        hint: Option<String>,
    },
    /// A sequence of threaded comments rendered as table-like cards in `rich`
    /// mode and labeled `Field: value` records in `plain` mode.
    Comments {
        entries: Vec<CommentEntry>,
        noun: Noun,
        empty: String,
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
    pub time_zone: jiff::tz::TimeZone,
    pub relative_time: RelativeTime,
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

fn paint_lines(text: &str, style: AnsiStyle, color: bool) -> String {
    if !color {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    for (idx, line) in text.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if !line.is_empty() {
            out.push_str(&paint(line, style, true));
        }
    }
    out
}

/// Extract a Launchpad username or resource name from the final path segment
/// of a link, stripping any leading `~`.
pub fn link_name(link: Option<&str>) -> Option<String> {
    link.and_then(|url| url.rsplit('/').next())
        .map(|segment| segment.trim_start_matches('~'))
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
}

/// Format a raw timestamp string as UTC `YYYY-MM-DD HH:MM` when it parses as
/// RFC 3339, or return the trimmed input string as-is.
pub fn format_timestamp_str(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed.parse::<jiff::Timestamp>() {
        Ok(ts) => format_utc_timestamp(ts, TimestampPrecision::DateTime),
        Err(_) => trimmed.to_string(),
    }
}

/// Format a concise relative-time hint (such as `2 days ago` or `in 3 hours`)
/// relative to `now`.
pub fn format_relative_hint(timestamp: jiff::Timestamp, now: jiff::Timestamp) -> String {
    let delta = now.as_second() - timestamp.as_second();
    let abs = delta.unsigned_abs();
    if abs < 60 {
        return "just now".to_string();
    }
    let unit = |count: u64, singular: &str, plural: &str| {
        if count == 1 {
            format!("1 {singular}")
        } else {
            format!("{count} {plural}")
        }
    };
    let amount = if abs < 3_600 {
        unit(abs / 60, "minute", "minutes")
    } else if abs < 86_400 {
        unit(abs / 3_600, "hour", "hours")
    } else if abs < 30 * 86_400 {
        unit(abs / 86_400, "day", "days")
    } else if abs < 365 * 86_400 {
        unit(abs / (30 * 86_400), "month", "months")
    } else {
        unit(abs / (365 * 86_400), "year", "years")
    };
    if delta >= 0 {
        format!("{amount} ago")
    } else {
        format!("in {amount}")
    }
}

/// Format `timestamp` in `tz` at `precision`, appending `(relative hint)` when
/// `now` is provided.
pub fn format_local_timestamp(
    timestamp: jiff::Timestamp,
    precision: TimestampPrecision,
    tz: &jiff::tz::TimeZone,
    now: Option<jiff::Timestamp>,
) -> String {
    let fmt = match precision {
        TimestampPrecision::Date => "%Y-%m-%d",
        TimestampPrecision::DateTime => "%Y-%m-%d %H:%M",
    };
    let base = timestamp
        .to_zoned_datetime(tz.clone())
        .strftime(fmt)
        .to_string();
    match now {
        Some(now) => format!("{base} ({})", format_relative_hint(timestamp, now)),
        None => base,
    }
}

fn format_utc_timestamp(timestamp: jiff::Timestamp, precision: TimestampPrecision) -> String {
    let fmt = match precision {
        TimestampPrecision::Date => "%Y-%m-%d",
        TimestampPrecision::DateTime => "%Y-%m-%d %H:%M",
    };
    timestamp.strftime(fmt).to_string()
}

fn format_column_value(
    value: &str,
    format: ColumnFormat,
    tz: Option<&jiff::tz::TimeZone>,
) -> String {
    let precision = match format {
        ColumnFormat::Text => return value.to_string(),
        ColumnFormat::Date => TimestampPrecision::Date,
        ColumnFormat::DateTime => TimestampPrecision::DateTime,
    };
    let Ok(ts) = value.trim().parse::<jiff::Timestamp>() else {
        return value.to_string();
    };
    match tz {
        Some(tz) => format_local_timestamp(ts, precision, tz, None),
        None => format_utc_timestamp(ts, precision),
    }
}

impl CommentDate {
    fn format_utc(&self) -> String {
        match self {
            Self::Timestamp(ts) => format_utc_timestamp(*ts, TimestampPrecision::DateTime),
            Self::Text(raw) => format_timestamp_str(raw),
        }
    }
}

/// Color style for a Launchpad status label, chosen to work on both light and
/// dark terminal themes.
pub fn status_style(status: &str) -> Option<AnsiStyle> {
    match status {
        "Fix Released"
        | "Fix Committed"
        | "Resolved"
        | "Answered"
        | "Merged"
        | "Accepted"
        | "Done"
        | "Published"
        | "Supported"
        | "Current Stable Release"
        | "Active"
        | "active"
        | "Approved"
        | "Good progress"
        | "Beta available"
        | "Implemented"
        | "Online"
        | "Started" => Some(OK),
        "In Progress" | "Triaged" | "Confirmed" | "Needs review" | "Needs Review"
        | "Needs Approval" | "Pending" | "Unapproved" | "Incomplete" | "Work in progress"
        | "Slow progress" | "Blocked" => Some(WARNING),
        "Rejected" | "Failed" | "Expired" | "Invalid" | "Won't Fix" | "revoked" | "Unreachable"
        | "Abandoned" | "Superseded" | "Obsolete" | "Deactivated" => Some(ERROR),
        "New" | "Opinion" | "Open" | "Needs information" | "Unknown" | "open" | "Not started"
        | "Informational" | "Deferred" | "Entry" | "Development Focus" | "Pre-release Freeze" => {
            Some(NAME)
        }
        _ => None,
    }
}

/// Color style for a bug importance or specification priority label.
pub fn importance_style(importance: &str) -> Option<AnsiStyle> {
    match importance {
        "Critical" => Some(ERROR),
        "High" | "Medium" => Some(WARNING),
        "Low" | "Wishlist" | "Undecided" => Some(NAME),
        _ => None,
    }
}

/// Map a field/column label and its sanitized value to an optional semantic color style.
fn field_value_style(label: &str, value: &str) -> Option<AnsiStyle> {
    match label {
        "Status" | "Lifecycle" | "Implementation" | "Impl" | "Upload Status" | "Definition" => {
            status_style(value)
        }
        "Importance" | "Priority" => importance_style(value),
        "Tags" | "Author" | "Person" | "Subscribed By" | "Assignee" | "Member" | "Maintainer" => {
            (!matches!(value, "none" | "—" | "unknown" | "(none)")).then_some(NAME)
        }
        _ => None,
    }
}

/// Render a colored lightweight `comfy-table` grid whose columns and logical
/// rows are delimited by sentinel `cell_sep` (`\x1f`) and `row_sep` (`\x1e`)
/// control characters, painting bold headers and semantic body cells with
/// pure `anstyle` SGR codes.
fn colorize_grid_with_sentinels(
    out: &mut String,
    raw: &str,
    row_styles: &[Vec<Option<AnsiStyle>>],
    cell_sep: char,
    row_sep: char,
    unicode: bool,
) {
    let out_sep = if unicode { '┆' } else { '|' };
    let (sep_fill, sep_j) = if unicode { ('─', '┼') } else { ('-', '+') };
    let mut in_body = false;
    let mut row_idx = 0usize;
    for line in raw.split_inclusive('\n') {
        let (body, newline) = match line.strip_suffix('\n') {
            Some(stripped) => (stripped, "\n"),
            None => (line, ""),
        };
        if !in_body && !body.is_empty() && body.chars().all(|c| c == sep_fill || c == sep_j) {
            in_body = true;
            row_idx = 0;
            out.push_str(line);
            continue;
        }
        if in_body && body.starts_with(row_sep) {
            row_idx += 1;
            continue;
        }
        let styles = if in_body {
            row_styles.get(row_idx).map(Vec::as_slice).unwrap_or(&[])
        } else {
            &[]
        };
        push_delimited_row(out, body, cell_sep, out_sep, |col_idx| {
            if in_body {
                styles.get(col_idx).copied().flatten()
            } else {
                Some(BOLD)
            }
        });
        out.push_str(newline);
    }
}

/// Remove ANSI SGR styling sequences (`ESC [ digits/semicolons m`) emitted
/// by legacy formatters. Anything else, including other escape sequences,
/// is kept unchanged for the renderer's own sanitizing.
#[cfg(test)]
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

fn is_unsafe_unicode_format(c: char) -> bool {
    matches!(
        c,
        '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

fn clean(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' => '\n',
            '\t' => ' ',
            c if c.is_control() || is_unsafe_unicode_format(c) => '\u{FFFD}',
            c => c,
        })
        .collect()
}

/// Sanitize text for `plain` mode: keep ASCII punctuation and words rather than
/// decorative symbols, em/en dashes, box drawing, or emoji, and neutralize
/// control and bidi/line-separator format characters.
pub(crate) fn clean_plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' => out.push('\n'),
            '\r' => {}
            '\t' => out.push(' '),
            '—' | '–' => out.push('-'),
            '✓' => out.push_str("OK"),
            '✗' => out.push_str("Error"),
            '●'
            | 'ℹ'
            | '\u{FE0E}'
            | '\u{FE0F}'
            | '\u{200D}'
            | '\u{2500}'..='\u{257F}'
            | '\u{2600}'..='\u{27BF}'
            | '\u{1F300}'..='\u{1FAFF}' => {}
            c if c.is_control() || is_unsafe_unicode_format(c) => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
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

    fn resolved_now(&self) -> Option<jiff::Timestamp> {
        match self.relative_time {
            RelativeTime::System => Some(jiff::Timestamp::now()),
            #[cfg(test)]
            RelativeTime::Fixed(now) => Some(now),
            #[cfg(test)]
            RelativeTime::Disabled => None,
        }
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
        out.push_str(&noun.count(records.rows.len()));
        if let Some(hint) = hint {
            out.push(' ');
            out.push_str(&clean(hint));
        }
        out.push_str("\n\n");

        let missing = "none";
        let display = Display::from_width(self.style.width);
        if display.is_stacked(records.columns.len()) {
            for (row_index, row) in records.rows.iter().enumerate() {
                if row_index > 0 {
                    out.push('\n');
                }
                for (index, column) in records.columns.iter().enumerate() {
                    if column.hide_in_rich {
                        continue;
                    }
                    let cleaned = row
                        .get(index)
                        .and_then(Option::as_deref)
                        .map(|raw| format_column_value(raw, column.format, Some(&self.time_zone)))
                        .map(|val| clean(&val))
                        .unwrap_or_else(|| missing.to_string());
                    let formatted = if self.style.color
                        && let Some(style) = field_value_style(column.label, &cleaned)
                    {
                        paint_lines(&cleaned, style, true)
                    } else {
                        cleaned
                    };
                    out.push_str(column.label);
                    out.push_str(": ");
                    out.push_str(&formatted);
                    out.push('\n');
                }
            }
        } else {
            let mut table = display.table(
                records
                    .columns
                    .iter()
                    .filter(|column| !column.hide_in_rich)
                    .map(|column| column.label)
                    .collect(),
            );
            if !self.style.unicode {
                table.load_style(ASCII_LIGHT_TABLE);
            }
            table.force_no_tty();
            let mut row_styles = Vec::with_capacity(records.rows.len());
            for row in &records.rows {
                let mut cells = Vec::with_capacity(records.columns.len());
                let mut styles = Vec::with_capacity(records.columns.len());
                for (col_idx, column) in records.columns.iter().enumerate() {
                    if column.hide_in_rich {
                        continue;
                    }
                    let value = row.get(col_idx);
                    let cleaned = value
                        .and_then(Option::as_deref)
                        .map(|raw| format_column_value(raw, column.format, Some(&self.time_zone)))
                        .map(|val| clean(&val))
                        .unwrap_or_else(|| missing.to_string());
                    let style = self
                        .style
                        .color
                        .then(|| field_value_style(column.label, &cleaned))
                        .flatten();
                    cells.push(cleaned);
                    styles.push(style);
                }
                table.add_row(cells);
                row_styles.push(styles);
            }
            if self.style.color {
                const CELL_SEP: char = '\x1f';
                const ROW_SEP: char = '\x1e';
                let style = table.style_mut();
                style.header_lines.junction = Some(CELL_SEP);
                style.content_lines.junction = Some(CELL_SEP);
                style.row_separator = LineStyle::none().fill(ROW_SEP).junction(ROW_SEP);
                let raw = format!("{table}\n");
                colorize_grid_with_sentinels(
                    out,
                    &raw,
                    &row_styles,
                    CELL_SEP,
                    ROW_SEP,
                    self.style.unicode,
                );
            } else {
                out.push_str(&format!("{table}\n"));
            }
        }
    }

    fn prepare_comments(&self, entries: &[CommentEntry], missing: &str) -> Vec<PreparedComment> {
        let now = self.resolved_now();
        entries
            .iter()
            .map(|entry| {
                let label = clean(&entry.label);
                let author = entry
                    .author
                    .as_deref()
                    .map(clean)
                    .unwrap_or_else(|| missing.to_string());
                let author_style = self
                    .style
                    .color
                    .then(|| field_value_style("Author", &author))
                    .flatten();
                let date = match &entry.date {
                    Some(CommentDate::Timestamp(ts)) => format_local_timestamp(
                        *ts,
                        TimestampPrecision::DateTime,
                        &self.time_zone,
                        now,
                    ),
                    Some(CommentDate::Text(raw)) => {
                        let trimmed = raw.trim();
                        if let Ok(ts) = trimmed.parse::<jiff::Timestamp>() {
                            format_local_timestamp(
                                ts,
                                TimestampPrecision::DateTime,
                                &self.time_zone,
                                now,
                            )
                        } else {
                            let cleaned = clean(trimmed);
                            if cleaned.is_empty() {
                                missing.to_string()
                            } else {
                                cleaned
                            }
                        }
                    }
                    None => missing.to_string(),
                };
                let details: Vec<(&'static str, String)> = entry
                    .details
                    .iter()
                    .map(|(k, v)| (*k, clean(&v.replace("\r\n", "\n"))))
                    .collect();
                let detail_lines: Vec<String> =
                    details.iter().map(|(k, v)| format!("{k}: {v}")).collect();
                let normalized = entry.content.replace("\r\n", "\n");
                let cleaned_body = clean(&normalized);
                let trimmed_body = cleaned_body.trim_matches('\n');
                let content = (!trimmed_body.trim().is_empty()).then(|| trimmed_body.to_string());
                let box_body = match (detail_lines.is_empty(), &content) {
                    (true, None) => entry.attachments.is_empty().then(|| missing.to_string()),
                    (true, Some(text)) => Some(text.clone()),
                    (false, None) => Some(detail_lines.join("\n")),
                    (false, Some(text)) => Some(format!("{}\n\n{text}", detail_lines.join("\n"))),
                };
                let attachment_lines = entry
                    .attachments
                    .iter()
                    .map(|att| format!("Attachment: {}", clean(&format_attachment_text(att))))
                    .collect();
                PreparedComment {
                    label_field: entry.label_field,
                    label,
                    author,
                    author_style,
                    date,
                    details,
                    content_field: entry.content_field,
                    content,
                    box_body,
                    attachment_lines,
                }
            })
            .collect()
    }

    fn render_stacked_comments(
        &self,
        out: &mut String,
        prepared: &[PreparedComment],
        missing: &str,
    ) {
        for (idx, entry) in prepared.iter().enumerate() {
            if idx > 0 {
                out.push('\n');
            }
            let label = paint_lines(&entry.label, BOLD, self.style.color);
            let author = entry.author_style.map_or_else(
                || entry.author.clone(),
                |style| paint_lines(&entry.author, style, true),
            );
            let body = entry.content.as_deref().unwrap_or(missing);
            out.push_str(&format!("{}: {label}\n", entry.label_field));
            out.push_str(&format!("Author: {author}\n"));
            out.push_str(&format!("Date: {}\n", entry.date));
            for (k, v) in &entry.details {
                out.push_str(&format!("{k}: {v}\n"));
            }
            out.push_str(&format!("{}: {body}\n", entry.content_field));
            for att in &entry.attachment_lines {
                if let Some(rest) = att.strip_prefix("Attachment:") {
                    out.push_str(&paint("Attachment:", BOLD, self.style.color));
                    out.push_str(rest);
                } else {
                    out.push_str(att);
                }
                out.push('\n');
            }
        }
    }

    fn render_comment_card(
        &self,
        out: &mut String,
        entry: &PreparedComment,
        layout: &CommentLayout,
    ) {
        let attachments_cell =
            (!entry.attachment_lines.is_empty()).then(|| entry.attachment_lines.join("\n"));
        let has_content = entry.box_body.is_some();
        let has_attachments = attachments_cell.is_some();

        if layout.three_col_header {
            let mut header_table = Table::new();
            header_table.load_style(comment_header_style(self.style.unicode, self.style.color));
            header_table.force_no_tty();
            header_table.set_content_arrangement(ContentArrangement::Dynamic);
            header_table.set_width(layout.box_w);
            header_table.add_row([
                entry.label.as_str(),
                entry.author.as_str(),
                entry.date.as_str(),
            ]);
            header_table.set_constraints([
                ColumnConstraint::Absolute(Width::Fixed(layout.col0_w)),
                ColumnConstraint::Absolute(Width::Fixed(layout.col1_w)),
                ColumnConstraint::Absolute(Width::Fixed(layout.col2_w)),
            ]);
            let header_raw = format!("{header_table}\n");
            if self.style.color {
                colorize_comment_header_table(
                    out,
                    &header_raw,
                    [Some(BOLD), entry.author_style, None],
                    self.style.unicode,
                );
            } else {
                out.push_str(&header_raw);
            }

            let mut body_table = Table::new();
            body_table.load_style(comment_body_style(self.style.unicode, self.style.color));
            body_table.force_no_tty();
            body_table.set_content_arrangement(ContentArrangement::Dynamic);
            body_table.set_width(layout.box_w);
            if let Some(body) = &entry.box_body {
                body_table.add_row([body.as_str()]);
            }
            if let Some(att) = &attachments_cell {
                body_table.add_row([att.as_str()]);
            }
            body_table.set_constraints([ColumnConstraint::Absolute(Width::Fixed(layout.box_w))]);
            let body_raw = format!("{body_table}\n");
            if self.style.color {
                colorize_comment_single_col(
                    out,
                    &body_raw,
                    false,
                    has_content,
                    has_attachments,
                    self.style.unicode,
                );
            } else {
                out.push_str(&body_raw);
            }
        } else {
            let sep = if self.style.unicode { " · " } else { " | " };
            let mut card = Table::new();
            card.load_style(comment_fallback_card_style(
                self.style.unicode,
                self.style.color,
            ));
            card.force_no_tty();
            card.set_content_arrangement(ContentArrangement::Dynamic);
            card.set_width(layout.box_w);
            card.set_header([format!(
                "{}{sep}{}{sep}{}",
                entry.label, entry.author, entry.date
            )]);
            if let Some(body) = &entry.box_body {
                card.add_row([body.as_str()]);
            }
            if let Some(att) = &attachments_cell {
                card.add_row([att.as_str()]);
            }
            card.set_constraints([ColumnConstraint::Absolute(Width::Fixed(layout.box_w))]);
            let raw = format!("{card}\n");
            if self.style.color {
                colorize_comment_single_col(
                    out,
                    &raw,
                    true,
                    has_content,
                    has_attachments,
                    self.style.unicode,
                );
            } else {
                out.push_str(&raw);
            }
        }
    }

    fn comments(&self, out: &mut String, entries: &[CommentEntry], noun: Noun, empty: &str) {
        if entries.is_empty() {
            out.push_str(&clean(empty));
            out.push('\n');
            return;
        }
        out.push_str(&noun.count(entries.len()));
        out.push_str("\n\n");
        let missing = "none";
        let prepared = self.prepare_comments(entries, missing);
        if self.style.width < 10 {
            self.render_stacked_comments(out, &prepared, missing);
        } else {
            let layout = comment_card_metrics(&prepared, self.style.width);
            for (idx, entry) in prepared.iter().enumerate() {
                if idx > 0 {
                    out.push('\n');
                }
                self.render_comment_card(out, entry, &layout);
            }
        }
    }
}

struct PreparedComment {
    label_field: &'static str,
    label: String,
    author: String,
    author_style: Option<AnsiStyle>,
    date: String,
    details: Vec<(&'static str, String)>,
    content_field: &'static str,
    content: Option<String>,
    box_body: Option<String>,
    attachment_lines: Vec<String>,
}

struct CommentLayout {
    box_w: u16,
    col0_w: u16,
    col1_w: u16,
    col2_w: u16,
    three_col_header: bool,
}

fn format_attachment_text(attachment: &CommentAttachment) -> String {
    match &attachment.url {
        Some(url) => format!("{} [{}] {url}", attachment.title, attachment.kind),
        None => format!("{} [{}]", attachment.title, attachment.kind),
    }
}

fn comment_card_metrics(prepared: &[PreparedComment], max_width: u16) -> CommentLayout {
    let mut header_measure = Table::new();
    let mut body_measure = Table::new();
    for entry in prepared {
        header_measure.add_row([
            entry.label.as_str(),
            entry.author.as_str(),
            entry.date.as_str(),
        ]);
        if let Some(body) = &entry.box_body {
            body_measure.add_row([body.as_str()]);
        }
        if !entry.attachment_lines.is_empty() {
            body_measure.add_row([entry.attachment_lines.join("\n")]);
        }
    }
    let header_widths = header_measure.column_max_content_widths();
    let label_w = header_widths.first().copied().unwrap_or(1);
    let author_w = header_widths.get(1).copied().unwrap_or(1);
    let date_w = header_widths.get(2).copied().unwrap_or(1);
    let body_max_w = body_measure
        .column_max_content_widths()
        .first()
        .copied()
        .unwrap_or(1);

    // Borderless sides: three columns have 2 inner junctions and 6 padding
    // spaces (8); a single column has 2 padding spaces (2).
    let natural_header_w =
        8usize + usize::from(label_w) + usize::from(author_w) + usize::from(date_w);
    let natural_body_w = 2usize + usize::from(body_max_w);
    let box_w = natural_header_w
        .max(natural_body_w)
        .min(usize::from(max_width));

    let fixed_chrome = 8usize + usize::from(label_w) + usize::from(date_w);
    let min_author_col = usize::from(author_w).clamp(1, 12);
    let three_col_header = box_w >= fixed_chrome.saturating_add(min_author_col);
    let (col0_w, col1_w, col2_w) = if three_col_header {
        (
            u16::try_from(usize::from(label_w) + 2).unwrap_or(u16::MAX),
            u16::try_from((box_w - fixed_chrome) + 2).unwrap_or(u16::MAX),
            u16::try_from(usize::from(date_w) + 2).unwrap_or(u16::MAX),
        )
    } else {
        (0, 0, 0)
    };

    CommentLayout {
        box_w: u16::try_from(box_w).unwrap_or(u16::MAX),
        col0_w,
        col1_w,
        col2_w,
        three_col_header,
    }
}

fn comment_header_style(unicode: bool, color: bool) -> TableStyle {
    let junction = if color {
        '\x1f'
    } else if unicode {
        '┆'
    } else {
        '|'
    };
    if unicode {
        TableStyle::new()
            .top_border(LineStyle::none().fill('─').junction('┬'))
            .content_lines(ContentLineStyle::none().junction(junction))
            .bottom_border(LineStyle::none().fill('─').junction('┴'))
    } else {
        TableStyle::new()
            .top_border(LineStyle::none().fill('-').junction('+'))
            .content_lines(ContentLineStyle::none().junction(junction))
            .bottom_border(LineStyle::none().fill('-').junction('+'))
    }
}

fn comment_body_style(unicode: bool, color: bool) -> TableStyle {
    let h = if unicode { '─' } else { '-' };
    let row_sep = if color {
        '\x1e'
    } else if unicode {
        '╌'
    } else {
        '-'
    };
    TableStyle::new()
        .top_border(LineStyle::none())
        .content_lines(ContentLineStyle::none())
        .row_separator(LineStyle::none().fill(row_sep))
        .bottom_border(LineStyle::none().fill(h))
}

fn comment_fallback_card_style(unicode: bool, color: bool) -> TableStyle {
    let h = if unicode { '─' } else { '-' };
    let header_sep = if color { '\x1d' } else { h };
    let row_sep = if color {
        '\x1e'
    } else if unicode {
        '╌'
    } else {
        '-'
    };
    TableStyle::new()
        .top_border(LineStyle::none().fill(h))
        .header_lines(ContentLineStyle::none())
        .header_separator(LineStyle::none().fill(header_sep))
        .content_lines(ContentLineStyle::none())
        .row_separator(LineStyle::none().fill(row_sep))
        .bottom_border(LineStyle::none().fill(h))
}

fn push_delimited_row(
    out: &mut String,
    row: &str,
    in_sep: char,
    out_sep: char,
    style_for_col: impl Fn(usize) -> Option<AnsiStyle>,
) {
    for (col_idx, cell) in row.split(in_sep).enumerate() {
        if col_idx > 0 {
            out.push(out_sep);
        }
        if let Some(style) = style_for_col(col_idx) {
            push_painted_cell(out, cell, style);
        } else {
            out.push_str(cell);
        }
    }
}

fn push_painted_cell(out: &mut String, cell: &str, style: AnsiStyle) {
    let trimmed = cell.trim();
    if !trimmed.is_empty()
        && let Some(pos) = cell.find(trimmed)
    {
        out.push_str(&cell[..pos]);
        out.push_str(&paint(trimmed, style, true));
        out.push_str(&cell[pos + trimmed.len()..]);
    } else {
        out.push_str(cell);
    }
}

fn push_attachment_cell(out: &mut String, cell: &str) {
    let trimmed = cell.trim_start();
    if let Some(rest) = trimmed.strip_prefix("Attachment:") {
        let prefix_len = cell.len() - trimmed.len();
        out.push_str(&cell[..prefix_len]);
        out.push_str(&paint("Attachment:", BOLD, true));
        out.push_str(rest);
    } else {
        out.push_str(cell);
    }
}

fn colorize_comment_header_table(
    out: &mut String,
    raw: &str,
    styles: [Option<AnsiStyle>; 3],
    unicode: bool,
) {
    let out_sep = if unicode { '┆' } else { '|' };
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let last_idx = lines.len().saturating_sub(1);
    for (idx, line) in lines.iter().enumerate() {
        if idx == 0 || idx == last_idx {
            out.push_str(line);
            continue;
        }
        let (body, newline) = line.strip_suffix('\n').map_or((*line, ""), |s| (s, "\n"));
        push_delimited_row(out, body, '\x1f', out_sep, |col_idx| {
            styles.get(col_idx).copied().flatten()
        });
        out.push_str(newline);
    }
}

fn colorize_comment_single_col(
    out: &mut String,
    raw: &str,
    has_header: bool,
    has_content: bool,
    has_attachments: bool,
    unicode: bool,
) {
    let header_fill = if unicode { "─" } else { "-" };
    let row_fill = if unicode { "╌" } else { "-" };
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let last_idx = lines.len().saturating_sub(1);
    let mut in_header = has_header;
    let mut in_attachments = !has_header && !has_content && has_attachments;
    for (idx, line) in lines.iter().enumerate() {
        if (has_header && idx == 0) || idx == last_idx {
            out.push_str(line);
            continue;
        }
        let (body, newline) = line.strip_suffix('\n').map_or((*line, ""), |s| (s, "\n"));
        if in_header && body.starts_with('\x1d') {
            in_header = false;
            in_attachments = !has_content && has_attachments;
            out.push_str(&header_fill.repeat(body.chars().count()));
            out.push_str(newline);
            continue;
        }
        if !in_header && body.starts_with('\x1e') {
            in_attachments = has_attachments;
            out.push_str(&row_fill.repeat(body.chars().count()));
            out.push_str(newline);
            continue;
        }
        if in_header {
            push_painted_cell(out, body, BOLD);
            out.push_str(newline);
        } else if in_attachments {
            push_attachment_cell(out, body);
            out.push_str(newline);
        } else {
            out.push_str(line);
        }
    }
}

impl Renderer for RichRenderer {
    fn render(&self, report: &Report) -> String {
        let mut out = String::new();
        let rule = if self.style.unicode { "─" } else { "-" };
        let now = self.resolved_now();
        for block in &report.blocks {
            match block {
                Block::Heading(text) => {
                    let cleaned = clean(text);
                    let rule_w = crate::display::text_display_width(&cleaned)
                        .clamp(1, usize::from(self.style.width).max(1));
                    out.push_str(&paint(&cleaned, BOLD, self.style.color));
                    out.push('\n');
                    out.push_str(&rule.repeat(rule_w));
                    out.push('\n');
                }
                Block::Status(level, spans) => {
                    out.push_str(&format!("{} {}\n", self.symbol(*level), self.spans(spans)));
                }
                Block::Note(spans) => out.push_str(&format!("  {}\n", self.spans(spans))),
                Block::Field(label, value) => {
                    let cleaned = clean(value);
                    let formatted = if self.style.color
                        && let Some(style) = field_value_style(label, &cleaned)
                    {
                        paint_lines(&cleaned, style, true)
                    } else {
                        cleaned
                    };
                    out.push_str(&format!("  {label}: {formatted}\n"));
                }
                Block::Timestamp(label, timestamp, precision) => {
                    let formatted =
                        format_local_timestamp(*timestamp, *precision, &self.time_zone, now);
                    out.push_str(&format!("  {label}: {formatted}\n"));
                }
                Block::List { noun, empty, hint } => {
                    self.list(&mut out, &report.records, *noun, empty, hint.as_deref());
                }
                Block::Table {
                    records,
                    noun,
                    empty,
                    hint,
                } => {
                    self.list(&mut out, records, *noun, empty, hint.as_deref());
                }
                Block::RichTable {
                    records,
                    noun,
                    empty,
                    hint,
                } => {
                    self.list(&mut out, records, *noun, empty, hint.as_deref());
                }
                Block::Comments {
                    entries,
                    noun,
                    empty,
                } => {
                    self.comments(&mut out, entries, *noun, empty);
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
                Span::Text(text) | Span::Name(text) => clean_plain(text),
                Span::Command(command) => format!("\"{}\"", clean_plain(command)),
            })
            .collect()
    }

    /// `Label: value`, with continuation lines indented so each record stays
    /// one block of lines.
    fn field(out: &mut String, label: &str, value: Option<&str>) {
        let value = value.map_or_else(|| "none".to_string(), clean_plain);
        out.push_str(label);
        out.push_str(": ");
        out.push_str(&value.replace('\n', "\n  "));
        out.push('\n');
    }

    fn list(out: &mut String, records: &Records, noun: Noun, empty: &str, hint: Option<&str>) {
        if records.rows.is_empty() {
            out.push_str(&clean_plain(empty));
            out.push('\n');
            return;
        }
        out.push_str(&noun.count(records.rows.len()));
        if let Some(hint) = hint {
            out.push(' ');
            out.push_str(&clean_plain(hint));
        }
        out.push('\n');
        for row in &records.rows {
            out.push('\n');
            for (column, value) in records.columns.iter().zip(row) {
                if value.as_deref() == Some("") {
                    continue;
                }
                let formatted = value
                    .as_deref()
                    .map(|raw| format_column_value(raw, column.format, None));
                Self::field(out, column.label, formatted.as_deref());
            }
        }
    }

    fn comments(out: &mut String, entries: &[CommentEntry], noun: Noun, empty: &str) {
        if entries.is_empty() {
            out.push_str(&clean_plain(empty));
            out.push('\n');
            return;
        }
        out.push_str(&noun.count(entries.len()));
        out.push('\n');
        for entry in entries {
            out.push('\n');
            Self::field(out, entry.label_field, Some(&entry.label));
            Self::field(out, "Author", entry.author.as_deref());
            let date = entry.date.as_ref().map(CommentDate::format_utc);
            Self::field(out, "Date", date.as_deref());
            for (label, value) in &entry.details {
                Self::field(out, label, Some(value));
            }
            Self::field(out, entry.content_field, Some(&entry.content));
            for att in &entry.attachments {
                let attachment = format_attachment_text(att);
                Self::field(out, "Attachment", Some(&attachment));
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
                    out.push_str(&clean_plain(text));
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
                Block::Timestamp(label, timestamp, precision) => {
                    let formatted = format_utc_timestamp(*timestamp, *precision);
                    Self::field(&mut out, label, Some(&formatted));
                }
                Block::List { noun, empty, hint } => {
                    Self::list(&mut out, &report.records, *noun, empty, hint.as_deref());
                }
                Block::Table {
                    records,
                    noun,
                    empty,
                    hint,
                } => {
                    Self::list(&mut out, records, *noun, empty, hint.as_deref());
                }
                Block::RichTable { .. } => {}
                Block::Comments {
                    entries,
                    noun,
                    empty,
                } => {
                    Self::comments(&mut out, entries, *noun, empty);
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
            let line = records
                .columns
                .iter()
                .zip(row.iter())
                .map(|(column, value)| {
                    value
                        .as_deref()
                        .map(|raw| escape_tsv(&format_column_value(raw, column.format, None)))
                        .unwrap_or_default()
                })
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
                    Column::new("Name", "name"),
                    Column::new("Summary", "summary"),
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

    #[test]
    fn clean_and_clean_plain_neutralize_bidi_line_separators_and_variation_selectors() {
        let hostile =
            "hello\u{202E}dlrow\u{2066}iso\u{2028}sep\u{2029}para \u{2139}\u{FE0F}\u{200D}\u{FE0E}";
        let rich = clean(hostile);
        for c in ['\u{202E}', '\u{2066}', '\u{2028}', '\u{2029}'] {
            assert!(!rich.contains(c), "clean leaked {c:?}: {rich:?}");
        }

        let plain = clean_plain(hostile);
        assert_plain_invariants(&plain);
        for c in [
            '\u{202E}', '\u{2066}', '\u{2028}', '\u{2029}', '\u{2139}', '\u{FE0F}', '\u{200D}',
            '\u{FE0E}',
        ] {
            assert!(!plain.contains(c), "clean_plain leaked {c:?}: {plain:?}");
        }
    }

    #[test]
    fn pager_resolution_lines_and_io_fallback_behave_safely() {
        // Sudo and all variants of `cat` disable paging.
        let sudo_env = Environment {
            pager: Some("less -FRX".into()),
            term: Some("xterm-256color".into()),
            sudo: true,
            ..env()
        };
        assert_eq!(
            resolve_pager(false, OutputMode::Rich, &sudo_env, true),
            None
        );

        for cat_cmd in [
            "cat",
            "/bin/cat",
            "/usr/bin/cat",
            "cat -u",
            "'/usr/bin/cat' -n",
        ] {
            let cat_env = Environment {
                pager: Some(cat_cmd.into()),
                term: Some("xterm-256color".into()),
                ..env()
            };
            assert_eq!(
                resolve_pager(false, OutputMode::Rich, &cat_env, true),
                None,
                "{cat_cmd}"
            );
            assert!(!should_page(Some(cat_cmd), "1\n2\n3\n4\n", 1), "{cat_cmd}");
        }

        // Line threshold precedence: LINES > detected terminal height > default (24).
        assert_eq!(resolve_pager_lines(Some("40"), Some(60)), 40);
        assert_eq!(resolve_pager_lines(Some(" 0 "), Some(55)), 55);
        assert_eq!(resolve_pager_lines(Some("bad"), Some(55)), 55);
        assert_eq!(resolve_pager_lines(None, Some(0)), DEFAULT_PAGER_LINES);
        assert_eq!(resolve_pager_lines(None, None), DEFAULT_PAGER_LINES);

        let long_text = "1\n2\n3\n4\n";
        let fmt_warn = |msg: &str| format!("Warning: {msg}\n");

        // Failing pager must surface an error rather than swallowing output.
        let mut out = Vec::new();
        let mut err = Vec::new();
        let res = write_with_pager_io(
            long_text,
            Some("sh -c 'exit 1'"),
            2,
            &mut out,
            &mut err,
            fmt_warn,
        );
        assert!(res.is_err(), "failing pager must return Err");
        assert!(out.is_empty());

        // Early-exiting pager that succeeds (e.g. `q` or `head`) returns Ok(true).
        let mut out = Vec::new();
        let mut err = Vec::new();
        let paged = write_with_pager_io(
            long_text,
            Some("sh -c 'exit 0'"),
            2,
            &mut out,
            &mut err,
            fmt_warn,
        )
        .unwrap();
        assert!(paged);
        assert!(err.is_empty());

        // Nonexistent pager warns on stderr and falls back to stdout.
        let mut out = Vec::new();
        let mut err = Vec::new();
        let paged = write_with_pager_io(
            long_text,
            Some("/nonexistent/lpcli-missing-pager-binary"),
            2,
            &mut out,
            &mut err,
            fmt_warn,
        )
        .unwrap();
        assert!(!paged);
        assert_eq!(String::from_utf8(out).unwrap(), long_text);
        let warn = String::from_utf8(err).unwrap();
        assert!(warn.starts_with("Warning: failed to start pager"), "{warn}");

        // Unbalanced quotes warn on stderr and fall back to stdout.
        let mut out = Vec::new();
        let mut err = Vec::new();
        let paged = write_with_pager_io(
            long_text,
            Some("less 'unterminated"),
            2,
            &mut out,
            &mut err,
            fmt_warn,
        )
        .unwrap();
        assert!(!paged);
        assert_eq!(String::from_utf8(out).unwrap(), long_text);
        let warn = String::from_utf8(err).unwrap();
        assert!(warn.contains("ignoring invalid PAGER command"), "{warn}");

        // Direct stdout BrokenPipe is ignored cleanly, while other I/O errors fail.
        struct FailingWriter(std::io::ErrorKind);
        impl Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(self.0, "simulated"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut err = Vec::new();
        assert!(
            !write_with_pager_io(
                "short\n",
                None,
                10,
                &mut FailingWriter(std::io::ErrorKind::BrokenPipe),
                &mut err,
                fmt_warn
            )
            .unwrap()
        );
        assert!(
            write_with_pager_io(
                "short\n",
                None,
                10,
                &mut FailingWriter(std::io::ErrorKind::PermissionDenied),
                &mut err,
                fmt_warn
            )
            .is_err()
        );
    }

    #[test]
    fn headings_scale_with_text_and_terminal_width() {
        let report = Report {
            blocks: vec![
                Block::Heading("Short".into()),
                Block::Heading("A heading much wider than twenty columns".into()),
                Block::Heading("日本語".into()),
            ],
            json: serde_json::Value::Null,
            records: Records {
                columns: Vec::new(),
                rows: Vec::new(),
            },
        };
        let wide = Output::fixed(OutputMode::Rich, false, 100).render(&report);
        assert_eq!(
            wide,
            concat!(
                "Short\n─────\n",
                "A heading much wider than twenty columns\n",
                "────────────────────────────────────────\n",
                "日本語\n──────\n",
            )
        );
        let narrow = Output::fixed(OutputMode::Rich, false, 15).render(&report);
        let lines: Vec<&str> = narrow.lines().collect();
        assert_eq!(lines[1], "─────");
        assert_eq!(lines[3], "───────────────");
        assert_eq!(lines[5], "──────");
    }

    #[test]
    fn timestamps_format_in_local_time_with_relative_hints_in_rich_and_utc_in_plain_and_tsv() {
        let ts: jiff::Timestamp = "2026-11-16T02:30:00Z".parse().unwrap();
        let now: jiff::Timestamp = "2026-11-18T02:30:00Z".parse().unwrap();

        assert_eq!(
            format_relative_hint(
                "2026-11-18T02:29:30Z".parse().unwrap(),
                "2026-11-18T02:30:00Z".parse().unwrap()
            ),
            "just now"
        );
        assert_eq!(
            format_relative_hint("2026-11-18T02:25:00Z".parse().unwrap(), now),
            "5 minutes ago"
        );
        assert_eq!(
            format_relative_hint("2026-11-17T23:30:00Z".parse().unwrap(), now),
            "3 hours ago"
        );
        assert_eq!(format_relative_hint(ts, now), "2 days ago");
        assert_eq!(
            format_relative_hint("2026-08-15T02:30:00Z".parse().unwrap(), now),
            "3 months ago"
        );
        assert_eq!(
            format_relative_hint("2024-11-10T02:30:00Z".parse().unwrap(), now),
            "2 years ago"
        );
        assert_eq!(
            format_relative_hint("2026-11-20T02:30:00Z".parse().unwrap(), now),
            "in 2 days"
        );

        let report = Report {
            blocks: vec![
                Block::Timestamp("Created", ts, TimestampPrecision::DateTime),
                Block::List {
                    noun: Noun {
                        singular: "item",
                        plural: "items",
                    },
                    empty: "None.".into(),
                    hint: None,
                },
            ],
            json: serde_json::Value::Null,
            records: Records {
                columns: vec![
                    Column::date("Day", "date_created"),
                    Column::datetime("Exact", "exact"),
                ],
                rows: vec![vec![Some(ts.to_string()), Some(ts.to_string())]],
            },
        };

        // UTC-5 rolls 2026-11-16T02:30:00Z back to 2026-11-15 21:30.
        let tz_minus5 = jiff::tz::TimeZone::fixed(jiff::tz::Offset::constant(-5));
        let rich = Output::fixed(OutputMode::Rich, false, 100)
            .with_time_zone(tz_minus5)
            .with_relative_time(RelativeTime::Fixed(now))
            .render(&report);
        assert!(
            rich.contains("Created: 2026-11-15 21:30 (2 days ago)"),
            "{rich}"
        );
        assert!(rich.contains("2026-11-15 ┆ 2026-11-15 21:30"), "{rich}");

        // Plain and TSV stay deterministic UTC without relative hints even with local tz set.
        let plain = Output::fixed(OutputMode::Plain, false, 100)
            .with_time_zone(jiff::tz::TimeZone::fixed(jiff::tz::Offset::constant(-5)))
            .with_relative_time(RelativeTime::Fixed(now))
            .render(&report);
        assert!(plain.contains("Created: 2026-11-16 02:30\n"), "{plain}");
        assert!(
            plain.contains("Day: 2026-11-16\nExact: 2026-11-16 02:30\n"),
            "{plain}"
        );

        let tsv = Output::fixed(OutputMode::Tsv, false, 100)
            .with_time_zone(jiff::tz::TimeZone::fixed(jiff::tz::Offset::constant(-5)))
            .render(&report);
        assert_eq!(tsv, "date_created\texact\n2026-11-16\t2026-11-16 02:30\n");
    }

    #[test]
    fn stderr_helpers_render_reports_for_stderr_and_sanitize_lines() {
        let env = env();
        let report = Report {
            blocks: vec![Block::Heading("Downloading 2 file(s)".into())],
            json: serde_json::json!({"ok": true}),
            records: Records {
                columns: Vec::new(),
                rows: Vec::new(),
            },
        };
        let rich_stderr = Output::resolve(Settings {
            stdout_terminal: false,
            stderr_terminal: true,
            ..settings(&env)
        })
        .unwrap();
        let rendered = rich_stderr.render_stderr_report(&report);
        assert!(
            rendered.contains("\u{1b}[1mDownloading 2 file(s)\u{1b}[0m"),
            "{rendered:?}"
        );
        assert!(rendered.contains("─────────────────────"), "{rendered}");

        let plain_stderr = Output::fixed(OutputMode::Plain, false, 80);
        assert_eq!(
            plain_stderr.render_stderr_report(&report),
            "Downloading 2 file(s)\n"
        );
        for mode in [OutputMode::Json, OutputMode::Tsv] {
            assert_eq!(
                Output::fixed(mode, false, 80).render_stderr_report(&report),
                ""
            );
        }

        let hostile = "Issued to 'key\u{1b}[31m\u{202E}\rname' oauth_token_secret=topsecret";
        let line_rich = Output::fixed(OutputMode::Rich, true, 80).render_stderr_line(hostile);
        assert!(!line_rich.contains('\u{1b}'), "{line_rich:?}");
        assert!(!line_rich.contains('\u{202E}'), "{line_rich:?}");
        assert!(!line_rich.contains('\r'), "{line_rich:?}");
        assert!(!line_rich.contains("topsecret"), "{line_rich:?}");
        let line_plain = plain_stderr.render_stderr_line(hostile);
        assert_plain_invariants(&line_plain);
        assert!(!line_plain.contains("topsecret"), "{line_plain:?}");
    }

    /// `plain` output: ASCII decoration only, no ANSI codes, no box drawing,
    /// and no unsafe bidi, line-separator, or stray symbol-variation controls.
    pub fn assert_plain_invariants(rendered: &str) {
        assert!(!rendered.contains('\u{1b}'), "ANSI escape in plain output");
        for c in rendered.chars() {
            assert!(
                !matches!(
                    c,
                    '\u{2500}'..='\u{257F}'
                        | '\u{2600}'..='\u{27BF}'
                        | '\u{1F300}'..='\u{1FAFF}'
                        | '—'
                        | '–'
                        | '●'
                        | 'ℹ'
                        | '\u{2028}'
                        | '\u{2029}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{FE0E}'
                        | '\u{FE0F}'
                        | '\u{200D}'
                ),
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
