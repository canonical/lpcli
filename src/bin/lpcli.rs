//! lpcli — command-line interface for Launchpad.net
//!
//! Parse command-line arguments with `clap` and dispatch to the `lpcli` library.

use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::str::FromStr;
use std::sync::Arc;

#[path = "lpcli/config.rs"]
mod config;
#[path = "lpcli/display.rs"]
mod display;
#[path = "lpcli/output.rs"]
mod output;
#[path = "lpcli/commands"]
mod commands {
    pub(crate) mod bug;
    pub(crate) mod builds;
    pub(crate) mod git;
    pub(crate) mod identity;
    pub(crate) mod queue;
    pub(crate) mod read_domains;
    pub(crate) mod webhook_snap_token;
}

use commands::bug::*;
use commands::builds::*;
use commands::git::*;
use commands::identity::*;
use commands::queue::*;
use commands::read_domains::*;
use commands::webhook_snap_token::*;

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};

use display::Display;
use output::{
    Block, ColorChoice, Column, CommentAttachment, CommentDate, CommentEntry, Level, Noun, Output,
    OutputMode, ProgressChoice, Records, Report, Span, TimestampPrecision,
};
use ubuntu_info_rust::{Architecture, Pocket, SeriesInfo};

use lpcli::{
    access_tokens, auth,
    bugs::{self, BugSearchParams},
    client::{Collection, LaunchpadClient, Raw},
    cves::{self, CveSearchParams},
    download, git,
    hardware_key::{Authenticator, UsbAuthenticator},
    hardware_tokens::{
        self, GrantStore, HardwareLogin, HardwareTokenGrant, IssueGrantRequest, OAuthPermission,
        RegisteredAuthenticator, StoredIn,
    },
    packages::{self, SourceSearchParams},
    people, projects,
    questions::{self, QuestionSearchParams},
    queue::{self, QueueSearchParams},
    snaps, specifications, status, translations, webhooks,
};

// ---------------------------------------------------------------------------
// Ubuntu series and pocket argument parsing
// ---------------------------------------------------------------------------

/// Validate a comma-separated list of Ubuntu series arguments.
fn validate_ubuntu_series_list(value: &str) -> Result<(), String> {
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        validate_ubuntu_series(part)?;
    }
    Ok(())
}

/// Parse an Ubuntu archive pocket argument for clap.
///
/// Matching is case-insensitive so existing values such as `Release` and
/// `Security` continue to work.
fn parse_pocket(s: &str) -> Result<Pocket, String> {
    Pocket::from_str(&s.trim().to_ascii_lowercase()).map_err(|_| {
        let valid: Vec<&str> = Pocket::ALL.iter().map(|pocket| pocket.as_str()).collect();
        format!(
            "invalid Ubuntu archive pocket '{s}'; valid pockets are: {}",
            valid.join(", ")
        )
    })
}

/// Upload-queue statuses, in Launchpad's API spelling.
const QUEUE_STATUSES: [&str; 5] = ["New", "Unapproved", "Accepted", "Done", "Rejected"];

/// Parse an upload-queue status argument for clap.
///
/// Matching is case-insensitive (e.g. `new`, `New` and `NEW` are all
/// accepted), and the result is Launchpad's canonical spelling (e.g. `New`),
/// which the `getPackageUploads` API requires.
fn parse_queue_status(s: &str) -> Result<String, String> {
    let wanted = s.trim();
    QUEUE_STATUSES
        .iter()
        .find(|status| status.eq_ignore_ascii_case(wanted))
        .map(|status| (*status).to_string())
        .ok_or_else(|| {
            format!(
                "invalid upload-queue status '{s}'; valid statuses are: {}",
                QUEUE_STATUSES.map(str::to_ascii_lowercase).join(", ")
            )
        })
}

/// Return the Launchpad API spelling of a pocket (e.g. `Release`).
pub(crate) fn pocket_launchpad_name(pocket: Pocket) -> &'static str {
    match pocket {
        Pocket::Release => "Release",
        Pocket::Security => "Security",
        Pocket::Updates => "Updates",
        Pocket::Proposed => "Proposed",
        Pocket::Backports => "Backports",
    }
}

/// A package architecture accepted by the upload-queue commands.
///
/// In addition to the machine architectures defined by `ubuntu-info-rust`,
/// Launchpad's upload queue uses the special values `source` and `all`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueArch {
    /// Source package files.
    Source,
    /// Architecture-independent binary packages.
    All,
    /// A machine architecture.
    Arch(Architecture),
}

impl QueueArch {
    /// Return the canonical identifier used by Launchpad.
    fn as_str(self) -> &'static str {
        match self {
            QueueArch::Source => "source",
            QueueArch::All => "all",
            QueueArch::Arch(arch) => arch.as_str(),
        }
    }
}

impl std::fmt::Display for QueueArch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for QueueArch {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let value = s.trim();
        if value.eq_ignore_ascii_case("source") {
            return Ok(QueueArch::Source);
        }
        if value.eq_ignore_ascii_case("all") {
            return Ok(QueueArch::All);
        }
        Architecture::from_str(&value.to_ascii_lowercase())
            .map(QueueArch::Arch)
            .map_err(|_| invalid_arch_error(value))
    }
}

/// Parse an upload-queue architecture argument for clap.
fn parse_queue_arch(s: &str) -> Result<QueueArch, String> {
    s.parse()
}

/// Parse a build architecture argument for `package builds`.
fn parse_build_arch(s: &str) -> Result<String, String> {
    let value = s.trim();
    Architecture::from_str(&value.to_ascii_lowercase())
        .map(|arch| arch.as_str().to_string())
        .map_err(|_| {
            let valid: Vec<&str> = Architecture::ALL.iter().map(|arch| arch.as_str()).collect();
            format!(
                "invalid Ubuntu architecture '{value}'; valid architectures are: {}",
                valid.join(", ")
            )
        })
}

/// Launchpad build states, in the API documentation's spelling.
const BUILD_STATES: [&str; 12] = [
    "Needs building",
    "Successfully built",
    "Failed to build",
    "Dependency wait",
    "Chroot problem",
    "Build for superseded Source",
    "Currently building",
    "Failed to upload",
    "Uploading build",
    "Cancelling build",
    "Cancelled build",
    "Gathering build output",
];

/// Parse a build state argument for `package builds`.
///
/// Canonical Launchpad state names and common short forms are accepted
/// case-insensitively, with hyphens and underscores treated as spaces. The
/// result is Launchpad's canonical spelling for comparison with
/// `buildstate` values.
fn parse_build_state(s: &str) -> Result<String, String> {
    let normalized = s
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let canonical = match normalized.as_str() {
        "success" | "built" | "successful" | "successfully built" => "Successfully built",
        "failed" | "fail" | "failed to build" => "Failed to build",
        "building" | "currently building" => "Currently building",
        "needs build" | "needs building" | "pending" => "Needs building",
        "dependency wait" | "dep wait" | "waiting" => "Dependency wait",
        "chroot problem" => "Chroot problem",
        "superseded" | "build for superseded source" => "Build for superseded Source",
        "failed to upload" => "Failed to upload",
        "uploading" | "uploading build" => "Uploading build",
        "cancelling" | "cancelling build" => "Cancelling build",
        "cancelled" | "cancelled build" => "Cancelled build",
        "gathering" | "gathering build output" => "Gathering build output",
        _ => {
            return Err(format!(
                "invalid build state '{s}'; valid states are: {}",
                BUILD_STATES.map(str::to_ascii_lowercase).join(", ")
            ));
        }
    };
    Ok(canonical.to_string())
}

/// A Personal Package Archive reference accepted by `--ppa`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PpaRef {
    owner: String,
    name: String,
}

impl PpaRef {
    /// API path for the archive resource.
    fn api_path(&self) -> String {
        format!("/~{}/+archive/ubuntu/{}", self.owner, self.name)
    }

    /// Human-readable form used in output, e.g. `~my-username/my-ppa`.
    pub(crate) fn label(&self) -> String {
        format!("~{}/{}", self.owner, self.name)
    }
}

/// Resolve `--owner` and `--ppa` arguments, supporting both
/// `--owner <OWNER> --ppa <NAME>` and `--ppa <OWNER>/<NAME>` (when `--owner` is omitted).
pub(crate) fn resolve_ppa_args(
    owner: Option<&str>,
    ppa: Option<&str>,
) -> Result<Option<PpaRef>, String> {
    match (owner, ppa) {
        (None, None) => Ok(None),
        (Some(_), None) => Err("--owner requires --ppa".into()),
        (None, Some(ppa_val)) => {
            if ppa_val.contains('/') {
                parse_ppa_ref(ppa_val).map(Some)
            } else {
                Err(
                    "--ppa requires --owner, or pass --ppa OWNER/NAME (e.g. my-username/my-ppa)"
                        .into(),
                )
            }
        }
        (Some(owner_val), Some(ppa_val)) => {
            let owner_trimmed = owner_val.trim();
            let ppa_trimmed = ppa_val.trim();
            if owner_trimmed.is_empty() {
                return Err("--owner must not be empty".into());
            }
            if ppa_trimmed.is_empty() {
                return Err("--ppa must not be empty".into());
            }
            if ppa_trimmed.contains('/') {
                return Err(
                    "cannot combine --owner with --ppa OWNER/NAME; use either --owner OWNER --ppa NAME or --ppa OWNER/NAME"
                        .into(),
                );
            }
            parse_ppa_ref(&format!("{owner_trimmed}/{ppa_trimmed}")).map(Some)
        }
    }
}

/// Parse a `--ppa OWNER/NAME` argument for clap.
fn parse_ppa_ref(s: &str) -> Result<PpaRef, String> {
    let value = s.trim();
    let invalid = || format!("invalid PPA '{s}'; expected OWNER/NAME (e.g. my-username/my-ppa)");
    let Some((owner, name)) = value.split_once('/') else {
        return Err(invalid());
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(invalid());
    }
    if owner.chars().any(char::is_whitespace) || name.chars().any(char::is_whitespace) {
        return Err(format!(
            "invalid PPA '{s}'; owner and name must not contain whitespace"
        ));
    }
    if owner.starts_with('~') {
        return Err(format!(
            "invalid PPA '{s}'; omit the leading '~' (use 'owner/name')"
        ));
    }
    if !owner
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '+'))
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '+'))
    {
        return Err(format!(
            "invalid PPA '{s}'; owner and name may only contain letters, digits, '-', '.', '_' and '+'"
        ));
    }
    Ok(PpaRef {
        owner: owner.to_string(),
        name: name.to_string(),
    })
}

/// Build a helpful error message for an unknown architecture.
fn invalid_arch_error(value: &str) -> String {
    let mut valid: Vec<&str> = vec!["source", "all"];
    valid.extend(Architecture::ALL.iter().map(|arch| arch.as_str()));
    format!(
        "invalid Ubuntu architecture '{value}'; valid architectures are: {}",
        valid.join(", ")
    )
}

/// Look up an Ubuntu series by short code name, full code name, or version.
fn lookup_ubuntu_series(value: &str) -> Option<&'static SeriesInfo> {
    let value = value.trim();
    SeriesInfo::from_short_name(value)
        .or_else(|| SeriesInfo::from_code_name(value))
        .or_else(|| SeriesInfo::from_version(value))
}

/// Build a helpful error message for an unknown Ubuntu series.
fn invalid_series_error(value: &str) -> String {
    let valid: Vec<&str> = SeriesInfo::all()
        .map(|series| series.code_name_short)
        .collect();
    format!(
        "invalid Ubuntu series '{value}'; valid series are: {}",
        valid.join(", ")
    )
}

/// Validate an Ubuntu series argument.
fn validate_ubuntu_series(value: &str) -> Result<(), String> {
    lookup_ubuntu_series(value)
        .map(|_| ())
        .ok_or_else(|| invalid_series_error(value))
}

/// Validate a series argument when the target distribution is Ubuntu.
fn validate_series_for_target(target: &str, series: Option<&str>) -> Result<(), String> {
    if target.eq_ignore_ascii_case("ubuntu")
        && let Some(series) = series
    {
        validate_ubuntu_series(series)?;
    }
    Ok(())
}

/// Validate a comma-separated series list when the target distribution is Ubuntu.
fn validate_series_list_for_target(target: &str, series: Option<&str>) -> Result<(), String> {
    if target.eq_ignore_ascii_case("ubuntu")
        && let Some(series) = series
    {
        validate_ubuntu_series_list(series)?;
    }
    Ok(())
}

/// Validate a series filter for a bug task operation.
///
/// Bug tasks can target non-Ubuntu distributions, so series names are only
/// validated when the target is explicitly `ubuntu`.
fn validate_bug_series(
    target: &str,
    series: Option<&str>,
    many_series: Option<&str>,
) -> Result<(), String> {
    validate_series_for_target(target, series)?;
    validate_series_list_for_target(target, many_series)
}

/// Validate a series argument when the target distribution is Ubuntu.
///
/// Other Launchpad distributions use their own series names, so their series
/// arguments are left untouched.
fn validate_series_for_distro(distro: &str, series: &str) -> Result<(), String> {
    if distro.eq_ignore_ascii_case("ubuntu") {
        validate_ubuntu_series(series)?;
    }
    Ok(())
}

/// Validate an optional series argument when the target distribution is Ubuntu.
fn validate_optional_series_for_distro(distro: &str, series: Option<&str>) -> Result<(), String> {
    if let Some(series) = series {
        validate_series_for_distro(distro, series)?;
    }
    Ok(())
}

/// Canonicalise a series argument to its short code name for Ubuntu.
///
/// Non-Ubuntu distributions keep the original value because their series names
/// are not part of the Ubuntu series data.
fn canonical_series<'a>(distro: &str, series: &'a str) -> &'a str {
    if distro.eq_ignore_ascii_case("ubuntu")
        && let Some(info) = lookup_ubuntu_series(series)
    {
        return info.code_name_short;
    }
    series
}

/// Canonicalise a comma-separated series list to short code names for Ubuntu.
///
/// Non-Ubuntu distributions keep the original value.
fn canonical_series_list(distro: &str, series: &str) -> String {
    if distro.eq_ignore_ascii_case("ubuntu") {
        series
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| canonical_series(distro, part))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        series.to_string()
    }
}

/// Canonicalise bug-task series arguments for the selected target.
///
/// Only Ubuntu targets are canonicalised; other distributions keep their
/// series names unchanged.
fn canonical_bug_series(
    target: Option<&str>,
    series: Option<String>,
    many_series: Option<String>,
) -> (Option<String>, Option<String>) {
    let target = target.unwrap_or("");
    (
        series.map(|s| canonical_series(target, &s).to_string()),
        many_series.map(|m| canonical_series_list(target, &m)),
    )
}

// ---------------------------------------------------------------------------
// Top-level CLI structure
// ---------------------------------------------------------------------------

/// lpcli — A command-line client for Launchpad.net
#[derive(Clone, Debug, Parser)]
#[command(
    name = "lpcli",
    version,
    about = "Interact with Launchpad.net from the command line",
    long_about = None,
)]
struct Cli {
    /// Require authenticated reads; never downgrade when credentials are missing.
    #[arg(long, global = true, conflicts_with = "public")]
    authenticated: bool,

    /// Read public data only, without loading credentials or prompting for a key.
    #[arg(
        long,
        global = true,
        conflicts_with = "authenticated",
        long_help = "Read public data only. Never load stored credentials, touch a security key, or \
                     retry with authentication, even on HTTP 401/403/404. Private objects, fields and \
                     collection entries may be omitted. Complete private-inclusive results require \
                     authentication and may require a key touch. Not available for mutations or \
                     identity/security-management commands."
    )]
    public: bool,

    /// Output format: rich, plain, json, or tsv.
    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "FORMAT",
        long_help = "Output format. Overrides the LPCLI_FORMAT environment variable and \
                     display.format in config.toml. Without any of these, lpcli uses plain when \
                     display.accessible is true or TERM=dumb, and rich otherwise. json and tsv write \
                     data only to stdout. JSON is available for read commands, mutations, schema, \
                     and status; TSV is available for list commands."
    )]
    format: Option<OutputMode>,

    /// When to use color: auto, always, or never.
    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "WHEN",
        default_value_t = ColorChoice::Auto,
        long_help = "When to use color. auto follows CLICOLOR_FORCE, NO_COLOR, and TERM=dumb, and \
                     otherwise uses color only when the output stream is a terminal. Color is decided \
                     separately for stdout and stderr. plain, json, and tsv output is never colored."
    )]
    color: ColorChoice,

    /// How downloads report progress on stderr: auto, plain, rich, or none.
    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "MODE",
        default_value_t = ProgressChoice::Auto,
        long_help = "How downloads report progress on stderr. auto uses plain line-by-line \
                     progress in plain mode or when stderr is not a terminal, none in json and \
                     tsv modes, and animated progress bars in rich mode on a terminal. \
                     --progress none turns progress output off."
    )]
    progress: ProgressChoice,

    /// Select comma-separated Launchpad attributes in JSON entries.
    #[arg(long, global = true, value_name = "FIELDS", value_delimiter = ',', value_parser = parse_field_name)]
    fields: Option<Vec<String>>,

    /// Print only the resulting self_link for mutations.
    #[arg(long, global = true)]
    quiet: bool,

    /// Fail instead of prompting; implied when stdin is not a terminal.
    #[arg(long, global = true)]
    no_input: bool,

    /// Do not pipe output through a pager.
    #[arg(long, global = true)]
    no_pager: bool,

    #[command(subcommand)]
    command: Command,
}

fn parse_field_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("field names must be Launchpad attribute names".into());
    }
    Ok(value.to_string())
}

#[derive(Clone, Debug, Subcommand)]
enum Command {
    /// Check authentication status and verify the Launchpad server is reachable.
    Status,

    /// Print the JSON output contract version.
    Schema,

    /// Agent Skills (coding agents should read this first).
    #[command(visible_alias = "skill")]
    Skills,

    /// Authenticate with Launchpad (OAuth login).
    Login(LoginArgs),

    /// Log out from Launchpad and remove stored credentials.
    Logout(LogoutArgs),

    /// Manage FIDO2 security keys and hardware-bound API tokens.
    #[command(subcommand)]
    HardwareToken(HardwareTokenCommand),

    /// Query and manage Launchpad bugs.
    #[command(subcommand)]
    Bug(BugCommand),

    /// Query Launchpad people and teams.
    #[command(subcommand)]
    Person(PersonCommand),

    /// Query Ubuntu packages and distribution series.
    #[command(subcommand)]
    Package(PackageCommand),

    /// Query Launchpad projects.
    #[command(subcommand)]
    Project(ProjectCommand),

    /// Look up CVEs (Common Vulnerabilities and Exposures).
    #[command(subcommand)]
    Cve(CveCommand),

    /// Query and manage Launchpad Git repositories.
    #[command(subcommand)]
    Git(GitCommand),

    /// Query Launchpad specifications (blueprints).
    #[command(subcommand)]
    Spec(SpecCommand),

    /// Query Launchpad questions (answers/support).
    #[command(subcommand)]
    Question(QuestionCommand),

    /// Manage Launchpad webhooks.
    #[command(subcommand)]
    Webhook(WebhookCommand),

    /// Query Launchpad Translations.
    #[command(subcommand)]
    Translation(TranslationCommand),

    /// Query and manage Launchpad Snap recipes.
    #[command(subcommand)]
    Snap(SnapCommand),

    /// Query and manage Launchpad package upload queues.
    #[command(subcommand)]
    Queue(QueueCommand),

    /// Manage personal access tokens for projects and Git repositories.
    #[command(subcommand)]
    AccessToken(AccessTokenCommand),
}

// ---------------------------------------------------------------------------
// Bug sub-commands
// ---------------------------------------------------------------------------

/// Bug task statuses accepted on the command line.
///
/// CLI values are kebab-case (e.g. `in-progress`, `wont-fix`); the `Display`
/// implementation converts them to Launchpad's canonical form (e.g.
/// `In Progress`, `Won't Fix`) for API calls and user-facing output.
#[derive(Clone, Debug, Copy, ValueEnum)]
enum BugTaskStatus {
    New,
    Confirmed,
    Triaged,
    InProgress,
    FixCommitted,
    FixReleased,
    Invalid,
    WontFix,
    Opinion,
    Expired,
}

impl std::fmt::Display for BugTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::New => write!(f, "New"),
            Self::Confirmed => write!(f, "Confirmed"),
            Self::Triaged => write!(f, "Triaged"),
            Self::InProgress => write!(f, "In Progress"),
            Self::FixCommitted => write!(f, "Fix Committed"),
            Self::FixReleased => write!(f, "Fix Released"),
            Self::Invalid => write!(f, "Invalid"),
            Self::WontFix => write!(f, "Won't Fix"),
            Self::Opinion => write!(f, "Opinion"),
            Self::Expired => write!(f, "Expired"),
        }
    }
}

impl From<BugTaskStatus> for bugs::BugTaskStatus {
    fn from(status: BugTaskStatus) -> Self {
        match status {
            BugTaskStatus::New => Self::New,
            BugTaskStatus::Confirmed => Self::Confirmed,
            BugTaskStatus::Triaged => Self::Triaged,
            BugTaskStatus::InProgress => Self::InProgress,
            BugTaskStatus::FixCommitted => Self::FixCommitted,
            BugTaskStatus::FixReleased => Self::FixReleased,
            BugTaskStatus::Invalid => Self::Invalid,
            BugTaskStatus::WontFix => Self::WontFix,
            BugTaskStatus::Opinion => Self::Opinion,
            BugTaskStatus::Expired => Self::Expired,
        }
    }
}

#[derive(Clone, Debug, Copy, ValueEnum)]
enum BugTaskImportance {
    Undecided,
    Wishlist,
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for BugTaskImportance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undecided => write!(f, "Undecided"),
            Self::Wishlist => write!(f, "Wishlist"),
            Self::Low => write!(f, "Low"),
            Self::Medium => write!(f, "Medium"),
            Self::High => write!(f, "High"),
            Self::Critical => write!(f, "Critical"),
        }
    }
}

impl From<BugTaskImportance> for bugs::BugImportance {
    fn from(importance: BugTaskImportance) -> Self {
        match importance {
            BugTaskImportance::Undecided => Self::Undecided,
            BugTaskImportance::Wishlist => Self::Wishlist,
            BugTaskImportance::Low => Self::Low,
            BugTaskImportance::Medium => Self::Medium,
            BugTaskImportance::High => Self::High,
            BugTaskImportance::Critical => Self::Critical,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct TaskLinkSelector {
    target: Option<String>,
    package: Option<String>,
    series: Option<String>,
    task_url: Option<String>,
}

#[derive(Clone, Debug, Subcommand)]
enum BugCommand {
    /// Show details of a single bug.
    Show {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
    },
    /// List bug tasks for a bug.
    Tasks {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
    },
    /// Search bugs on a project or distribution.
    Search {
        /// Project or distribution name (e.g. "ubuntu", "launchpad").
        #[arg(short, long, default_value = "ubuntu")]
        target: String,
        /// Filter by status (e.g. "New", "Confirmed", "Fix Released").
        #[arg(short, long)]
        status: Option<String>,
        /// Filter by importance (e.g. "Critical", "High").
        #[arg(short, long)]
        importance: Option<String>,
        /// Filter by tag.
        #[arg(short = 'g', long)]
        tag: Option<String>,
        /// Restrict to a specific source package (e.g. "firefox").
        /// Only meaningful when the target is a distribution.
        #[arg(short, long)]
        package: Option<String>,
        /// Keyword search against bug titles and descriptions.
        #[arg(short, long)]
        keyword: Option<String>,
        /// Maximum results to return.
        #[arg(short, long, default_value = "25")]
        limit: u32,
    },
    /// Add a comment to a bug, optionally with a file attachment.
    AddComment {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Comment text.
        #[arg(short, long)]
        message: String,
        /// Path to a local file to attach to the comment.
        #[arg(short = 'f', long)]
        attachment_file: Option<String>,
        /// An external URL for the attachment.
        #[arg(short = 'u', long)]
        attachment_url: Option<String>,
        /// A short description of the attachment.
        #[arg(short = 'd', long)]
        attachment_description: Option<String>,
        /// Attachment type: "Patch" or "Unspecified".
        #[arg(short = 'a', long)]
        attachment_type: Option<String>,
    },
    /// List comments on a bug.
    Comments {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
    },
    /// Delete a comment that you previously added to a bug.
    ///
    /// The comment is identified by its index number as shown in
    /// `lpcli bug comments`. You must be the original author of the comment.
    DeleteComment {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// The comment index to delete, as shown by 'lpcli bug comments'.
        #[arg(short, long)]
        comment_index: u64,
    },
    /// Create a new bug on a project or distribution.
    Create {
        /// Project or distribution name (e.g. "ubuntu", "launchpad").
        #[arg(short, long, default_value = "ubuntu")]
        target: String,
        /// Source package name (only meaningful for distributions).
        #[arg(short, long)]
        package: Option<String>,
        /// Bug title.
        #[arg(short = 'T', long)]
        title: String,
        /// Bug description.
        #[arg(short, long)]
        description: String,
        /// Path to a local file to attach to the bug.
        #[arg(short = 'f', long)]
        attachment_file: Option<String>,
        /// An external URL for the attachment.
        #[arg(short = 'u', long)]
        attachment_url: Option<String>,
        /// A short description of the attachment.
        #[arg(long)]
        attachment_description: Option<String>,
        /// Attachment type: "Patch" or "Unspecified".
        #[arg(short = 'a', long)]
        attachment_type: Option<String>,
    },
    /// Change the description of a bug.
    #[command(
        group(ArgGroup::new("description_source")
            .required(true)
            .args(["description", "description_file"])),
    )]
    SetDescription {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// New description text. Mutually exclusive with --description-file.
        #[arg(short, long)]
        description: Option<String>,
        /// Path to a file containing the new description ('-' for standard
        /// input). Mutually exclusive with --description.
        #[arg(long)]
        description_file: Option<String>,
    },
    /// Mark a bug as a duplicate of another bug.
    MarkDuplicate {
        /// The Launchpad bug number to mark as a duplicate.
        #[arg(short, long)]
        bug_id: u64,
        /// The master bug number that this bug duplicates.
        #[arg(long)]
        duplicate_of: u64,
    },
    /// Remove a duplicate marker from a bug.
    UnmarkDuplicate {
        /// The Launchpad bug number to unmark as a duplicate.
        #[arg(short, long)]
        bug_id: u64,
    },
    /// Change the status of a bug task.
    #[command(
        group(ArgGroup::new("target_spec")
            .required(true)
            .args(["target", "many_targets", "all_targets"])),
        group(ArgGroup::new("series_spec")
            .required(true)
            .args(["series", "many_series", "all_series", "no_series"])),
    )]
    SetStatus {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Single target as shown by 'lpcli bug tasks' (e.g. "rust-alacritty").
        /// Mutually exclusive with --many-targets and --all-targets.
        #[arg(short, long)]
        target: Option<String>,
        /// Comma-separated list of targets (e.g. "rust-alacritty, rust-eza").
        /// Mutually exclusive with --target and --all-targets.
        #[arg(long)]
        many_targets: Option<String>,
        /// Apply to every target present in the bug's current tasks.
        /// Mutually exclusive with --target and --many-targets.
        #[arg(long)]
        all_targets: bool,
        /// Single Ubuntu series to update (e.g. "noble").
        /// Mutually exclusive with --many-series, --all-series, and --no-series.
        #[arg(long)]
        series: Option<String>,
        /// Comma-separated list of Ubuntu series (e.g. "noble, jammy").
        /// Mutually exclusive with --series, --all-series, and --no-series.
        #[arg(long)]
        many_series: Option<String>,
        /// Apply to every Ubuntu series present in the bug's current tasks,
        /// including series-less tasks for the matching targets.
        /// Mutually exclusive with --series, --many-series, and --no-series.
        #[arg(long)]
        all_series: bool,
        /// Only apply to series-less tasks for the matching targets
        /// (e.g. the distribution-level /ubuntu/+source/pkg task or an
        /// upstream project task). Mutually exclusive with --series,
        /// --many-series, and --all-series.
        #[arg(long)]
        no_series: bool,
        /// New status (e.g. "confirmed", "fix-released", "in-progress").
        #[arg(short, long)]
        status: BugTaskStatus,
    },
    /// Change the importance of a bug task.
    #[command(
        group(ArgGroup::new("target_spec")
            .required(true)
            .args(["target", "many_targets", "all_targets"])),
        group(ArgGroup::new("series_spec")
            .required(true)
            .args(["series", "many_series", "all_series", "no_series"])),
    )]
    SetImportance {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Single target as shown by 'lpcli bug tasks' (e.g. "rust-alacritty").
        /// Mutually exclusive with --many-targets and --all-targets.
        #[arg(short, long)]
        target: Option<String>,
        /// Comma-separated list of targets (e.g. "rust-alacritty, rust-eza").
        /// Mutually exclusive with --target and --all-targets.
        #[arg(long)]
        many_targets: Option<String>,
        /// Apply to every target present in the bug's current tasks.
        /// Mutually exclusive with --target and --many-targets.
        #[arg(long)]
        all_targets: bool,
        /// Single Ubuntu series to update (e.g. "noble").
        /// Mutually exclusive with --many-series, --all-series, and --no-series.
        #[arg(long)]
        series: Option<String>,
        /// Comma-separated list of Ubuntu series (e.g. "noble, jammy").
        /// Mutually exclusive with --series, --all-series, and --no-series.
        #[arg(long)]
        many_series: Option<String>,
        /// Apply to every Ubuntu series present in the bug's current tasks,
        /// including series-less tasks for the matching targets.
        /// Mutually exclusive with --series, --many-series, and --no-series.
        #[arg(long)]
        all_series: bool,
        /// Only apply to series-less tasks for the matching targets
        /// (e.g. the distribution-level /ubuntu/+source/pkg task or an
        /// upstream project task). Mutually exclusive with --series,
        /// --many-series, and --all-series.
        #[arg(long)]
        no_series: bool,
        /// New importance (e.g. "critical", "high", "medium", "low").
        #[arg(short, long)]
        importance: BugTaskImportance,
    },
    /// Assign a bug task to a Launchpad user.
    #[command(
        group(ArgGroup::new("target_spec")
            .required(true)
            .args(["target", "many_targets", "all_targets"])),
        group(ArgGroup::new("series_spec")
            .required(true)
            .args(["series", "many_series", "all_series", "no_series"])),
    )]
    SetAssignee {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Single target as shown by 'lpcli bug tasks' (e.g. "rust-alacritty").
        /// Mutually exclusive with --many-targets and --all-targets.
        #[arg(short, long)]
        target: Option<String>,
        /// Comma-separated list of targets (e.g. "rust-alacritty, rust-eza").
        /// Mutually exclusive with --target and --all-targets.
        #[arg(long)]
        many_targets: Option<String>,
        /// Apply to every target present in the bug's current tasks.
        /// Mutually exclusive with --target and --many-targets.
        #[arg(long)]
        all_targets: bool,
        /// Single Ubuntu series to update (e.g. "noble").
        /// Mutually exclusive with --many-series, --all-series, and --no-series.
        #[arg(long)]
        series: Option<String>,
        /// Comma-separated list of Ubuntu series (e.g. "noble, jammy").
        /// Mutually exclusive with --series, --all-series, and --no-series.
        #[arg(long)]
        many_series: Option<String>,
        /// Apply to every Ubuntu series present in the bug's current tasks,
        /// including series-less tasks for the matching targets.
        /// Mutually exclusive with --series, --many-series, and --no-series.
        #[arg(long)]
        all_series: bool,
        /// Only apply to series-less tasks for the matching targets
        /// (e.g. the distribution-level /ubuntu/+source/pkg task or an
        /// upstream project task). Mutually exclusive with --series,
        /// --many-series, and --all-series.
        #[arg(long)]
        no_series: bool,
        /// Launchpad username to assign (without ~).
        #[arg(short, long)]
        name: String,
    },
    /// Subscribe a person to a bug.
    Subscribe {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Launchpad person name to subscribe (without ~).
        #[arg(short, long)]
        name: String,
    },
    /// Unsubscribe a person from a bug.
    Unsubscribe {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Launchpad person name to unsubscribe (without ~).
        #[arg(short, long)]
        name: String,
    },
    /// List subscriptions for a bug.
    Subscriptions {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
    },
    /// List attachments for a bug.
    Attachments {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
    },
    /// Add a new bug task to a bug, optionally targeting a specific series.
    #[command(
        group(ArgGroup::new("series_spec")
            .args(["series", "many_series"])),
    )]
    AddTask {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Project or distribution name (e.g. "ubuntu", "launchpad").
        #[arg(short, long, default_value = "ubuntu")]
        target: String,
        /// Source package name (only meaningful for distributions).
        #[arg(short, long)]
        package: Option<String>,
        /// Single Ubuntu series (e.g. "noble").
        /// Mutually exclusive with --many-series.
        #[arg(long)]
        series: Option<String>,
        /// Comma-separated list of Ubuntu series (e.g. "noble, jammy").
        /// Mutually exclusive with --series.
        #[arg(long)]
        many_series: Option<String>,
        /// Task importance (e.g. "undecided", "critical", "high", "medium", "low").
        #[arg(long)]
        importance: Option<BugTaskImportance>,
        /// Task status (e.g. "new", "confirmed", "in-progress").
        #[arg(long)]
        status: Option<BugTaskStatus>,
        /// Launchpad username to assign (without ~).
        #[arg(long)]
        assignee: Option<String>,
        /// Remote bug tracker name from 'lpcli bug remote-bug-trackers'.
        #[arg(long, visible_alias = "tracker", requires = "remote_bug")]
        bug_tracker: Option<String>,
        /// Remote bug ID or number in the tracker (e.g. "1150409") to link this task to.
        #[arg(long, visible_alias = "remote", requires = "bug_tracker")]
        remote_bug: Option<String>,
    },
    /// Delete bug tasks from a bug matching the given target and optional series.
    #[command(
        group(ArgGroup::new("series_spec")
            .args(["series", "many_series"])),
    )]
    DeleteTask {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Project or distribution name (e.g. "ubuntu", "launchpad").
        #[arg(short, long, default_value = "ubuntu")]
        target: String,
        /// Source package name (only meaningful for distributions).
        #[arg(short, long)]
        package: Option<String>,
        /// Single Ubuntu series (e.g. "noble").
        /// Mutually exclusive with --many-series.
        #[arg(long)]
        series: Option<String>,
        /// Comma-separated list of Ubuntu series (e.g. "noble, jammy").
        /// Mutually exclusive with --series.
        #[arg(long)]
        many_series: Option<String>,
    },
    /// Add, remove, or clear tags on a bug.
    ///
    /// Remove operations run before add operations, so existing tags can be
    /// cleared and replaced in a single invocation.
    #[command(
        group(ArgGroup::new("add_spec")
            .args(["add_tag", "add_many_tags"])),
        group(ArgGroup::new("remove_spec")
            .args(["remove_tag", "remove_many_tags", "remove_all_tags"])),
    )]
    Tag {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Add a single tag to the bug.
        /// Mutually exclusive with --add-many-tags.
        #[arg(long)]
        add_tag: Option<String>,
        /// Add multiple tags to the bug. Values may be space-separated or
        /// comma-separated (or both). For example:
        ///   --add-many-tags homestar cheat strongbad
        ///   --add-many-tags "homestar, cheat, strongbad"
        /// Mutually exclusive with --add-tag.
        #[arg(long, num_args = 1..)]
        add_many_tags: Option<Vec<String>>,
        /// Remove a single tag from the bug.
        /// Mutually exclusive with --remove-many-tags and --remove-all-tags.
        #[arg(long)]
        remove_tag: Option<String>,
        /// Remove multiple tags from the bug. Values may be space-separated or
        /// comma-separated (or both). For example:
        ///   --remove-many-tags lettuce tomato
        ///   --remove-many-tags "lettuce, tomato"
        /// Mutually exclusive with --remove-tag and --remove-all-tags.
        #[arg(long, num_args = 1..)]
        remove_many_tags: Option<Vec<String>>,
        /// Remove all existing tags from the bug.
        /// Mutually exclusive with --remove-tag and --remove-many-tags.
        #[arg(long)]
        remove_all_tags: bool,
    },
    /// Link a bug task to a remote bug (e.g. in Debian or an upstream tracker).
    ///
    /// The remote bug is identified by a bug tracker (see
    /// `lpcli bug remote-bug-trackers`) plus the bug's ID in that tracker.
    /// Launchpad builds the watch URL from the tracker: watch URL =
    /// tracker base_url + remote bug ID, shaped by the tracker's type. For
    /// the "GitHub Issues" type the tracker is repo-specific, with base URL
    /// https://github.com/<owner>/<repo>/issues, so the final watch URL is
    /// https://github.com/<owner>/<repo>/issues/<N>; GitHub redirects that
    /// to /pull/<N> for pull requests, so the same form covers issues and
    /// PRs.
    ///
    /// Beware of mistyped trackers: a tracker whose type does not match its
    /// base URL produces junk watch URLs silently (e.g. the tracker named
    /// "github-issues" is registered as type "Google Code" and generates
    /// URLs like https://github.com/detail?id=...). The final watch URL is
    /// printed after linking; verify it and re-check `lpcli bug watches`.
    ///
    /// Either pass --bug-tracker and --remote-bug explicitly, or use
    /// --github "owner/repo#N", which finds or creates the
    /// auto-github-<owner>-<repo> tracker (type "GitHub Issues", base URL
    /// https://github.com/<owner>/<repo>/issues) and links to issue/PR N
    /// (issues and PRs share one numbering sequence). --github is mutually
    /// exclusive with --bug-tracker and --remote-bug.
    ///
    /// In both forms the task to link is selected the same way, via
    /// --target/--package/--series or a direct --task-url; the --github
    /// shorthand never guesses which task to modify.
    #[command(
        alias = "link-task",
        group(ArgGroup::new("remote_link_spec")
            .required(true)
            .args(["github", "bug_tracker"])),
    )]
    LinkRemote {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// GitHub issue or pull request to link, as "owner/repo#NUMBER"
        /// (e.g. "canonical/apport#658"). Ensures the
        /// auto-github-<owner>-<repo> tracker exists, links the selected
        /// task, and prints the final watch URL. Mutually exclusive with
        /// --bug-tracker and --remote-bug.
        #[arg(
            long,
            value_name = "OWNER/REPO#N",
            value_parser = parse_github_ref,
            conflicts_with_all = ["bug_tracker", "remote_bug"]
        )]
        github: Option<bugs::GitHubBugRef>,
        /// Remote bug tracker name from 'lpcli bug remote-bug-trackers'.
        /// Must be paired with --remote-bug; mutually exclusive with --github.
        #[arg(short = 'g', long, visible_alias = "tracker", requires = "remote_bug")]
        bug_tracker: Option<String>,
        /// Remote bug ID or number in the tracker (e.g. "1150409").
        /// Must be paired with --bug-tracker; mutually exclusive with --github.
        #[arg(short = 'r', long, visible_alias = "remote", requires = "bug_tracker")]
        remote_bug: Option<String>,
        /// Project or distribution name to match (e.g. "debian", "ubuntu").
        #[arg(short, long)]
        target: Option<String>,
        /// Source package name to match (e.g. "arctica-greeter").
        #[arg(short, long)]
        package: Option<String>,
        /// Ubuntu series to match (e.g. "noble").
        #[arg(short, long)]
        series: Option<String>,
        /// Direct API URL of the bug task to link (bypasses target/package matching).
        #[arg(short = 'u', long)]
        task_url: Option<String>,
    },
    /// Unlink a bug task from its remote bug watch.
    ///
    /// This only detaches the task from the watch (clears the task's
    /// bug_watch_link); the watch object itself remains on the bug. Bug
    /// watches cannot be deleted through the REST API: to fully remove a
    /// watch, use the Launchpad web UI page
    /// bugs.launchpad.net/bugs/<bug-id>/+watch/<watch-id>, which has a
    /// "Delete Bug Watch" button. Use `lpcli bug watches` to list watches,
    /// including orphaned ones left behind by unlinking.
    #[command(alias = "unlink-task", alias = "unlink-bug")]
    UnlinkRemote {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Project or distribution name to match (e.g. "debian", "ubuntu").
        #[arg(short, long)]
        target: Option<String>,
        /// Source package name to match (e.g. "arctica-greeter").
        #[arg(short, long)]
        package: Option<String>,
        /// Ubuntu series to match (e.g. "noble").
        #[arg(short, long)]
        series: Option<String>,
        /// Direct API URL of the bug task to unlink.
        #[arg(short = 'u', long)]
        task_url: Option<String>,
    },
    /// List bug watches (links to remote bugs) for a bug.
    ///
    /// The "Linked Tasks" column shows the bug tasks that point at each
    /// watch; a watch that no task links to is shown as "orphan". Orphan
    /// means no *visible* task links: the task list is fetched with the
    /// current authentication, so a watch whose only links are on tasks the
    /// signed-in user cannot see (e.g. tasks of private bugs) may still be
    /// reported as orphan. Bug watches cannot be deleted through the REST
    /// API: `bug unlink-remote` only detaches the task link. To fully
    /// remove a watch, use the Launchpad web UI page
    /// bugs.launchpad.net/bugs/<bug-id>/+watch/<watch-id>, which has a
    /// "Delete Bug Watch" button.
    Watches {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
    },
    /// List remote bug trackers registered in Launchpad.
    ///
    /// By default a compact table is printed. Use --format json for
    /// Launchpad's collection JSON ({"total_size", "start", "entries"}),
    /// with each tracker exactly as Launchpad returned it, or --format tsv
    /// for one tracker per line. --search filters locally, so total_size
    /// is omitted when --limit stops the search before every tracker was
    /// checked.
    RemoteBugTrackers {
        /// Maximum number of remote bug trackers to return.
        #[arg(short, long, value_parser = parse_positive_limit)]
        limit: Option<usize>,
        /// Case-insensitive substring to match against tracker metadata.
        #[arg(short, long)]
        search: Option<String>,
    },
    /// Manage remote bug trackers registered in Launchpad.
    Tracker {
        #[command(subcommand)]
        command: TrackerCommand,
    },
}

/// Sub-commands for managing remote bug trackers.
#[derive(Clone, Debug, Subcommand)]
enum TrackerCommand {
    /// Create a remote bug tracker, or idempotently ensure it exists.
    ///
    /// This wraps Launchpad's ensureBugTracker operation. If a tracker with
    /// the same base URL already exists, Launchpad returns that tracker and
    /// ignores the requested name and type; this command therefore prints
    /// the ACTUAL returned name, type and base URL, and whether the tracker
    /// was newly created or reused.
    ///
    /// With no --type, the type is inferred only for a canonical GitHub
    /// repository issues URL (https://github.com/<owner>/<repo>/issues); for
    /// that URL shape Launchpad also derives the
    /// auto-github-<owner>-<repo> tracker name when --name is omitted. Any
    /// other base URL requires an explicit --type, so no accidental
    /// mis-typed tracker can be registered.
    ///
    /// A tracker's watch URLs are built from its base URL, so a GitHub
    /// tracker's base URL must be https://github.com/<owner>/<repo>/issues
    /// for the resulting links (e.g. .../issues/658) to resolve. A
    /// github.com base URL combined with any other type is rejected: the
    /// tracker named "github-issues" is registered as "Google Code" and
    /// generates broken URLs like https://github.com/detail?id=...
    Create {
        /// Base URL of the remote tracker (e.g.
        /// "https://github.com/canonical/apport/issues" or
        /// "https://bugs.debian.org/"). Watch URLs are built by appending
        /// the remote bug ID to this URL, shaped by the tracker type.
        #[arg(long, value_name = "URL", value_parser = parse_tracker_base_url)]
        base_url: String,
        /// Tracker type. Optional only for a canonical GitHub repository
        /// issues URL (inferred as github); required for everything else.
        /// Values: github, gitlab, bugzilla, debbugs, trac, google-code,
        /// roundup, sourceforge, mantis, rt, savane, php-project, email.
        #[arg(long = "type", value_name = "TYPE", value_enum)]
        tracker_type: Option<TrackerType>,
        /// Tracker name. For GitHub base URLs this defaults to Launchpad's
        /// auto-github-<owner>-<repo> convention (lowercased). Note that if
        /// a tracker with the same base URL already exists, Launchpad keeps
        /// its existing name and ignores this value.
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
        /// Human-readable tracker title (e.g. "Debian Bug tracker").
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// Description of the tracker.
        #[arg(long, value_name = "SUMMARY")]
        summary: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// GitHub shorthand and bug-tracker create argument parsing
//
// Everything in this section is strict and pure: malformed GitHub references
// and tracker configurations are rejected at CLI parse/validation time,
// before authentication or any network access. GitHub reference name rules
// are delegated to the library ([`bugs::GitHubBugRef`]) so there is a single
// source of truth; this section only adds CLI-level strictness.
// ---------------------------------------------------------------------------

/// clap value parser for `bug link-remote --github`.
///
/// Rejects the reference at CLI parse time — before authentication or any
/// network access — so malformed input can never reach Launchpad. On top of
/// the library parser ([`bugs::parse_github_bug_ref`]) the CLI is stricter:
/// * whitespace anywhere is rejected (the library parser tolerates
///   surrounding whitespace);
/// * full URLs are rejected with a targeted message (the library parser
///   also accepts `github.com` issue/PR URLs for programmatic use, but the
///   CLI shorthand is `owner/repo#N` only).
fn parse_github_ref(value: &str) -> Result<bugs::GitHubBugRef, String> {
    if value.chars().any(char::is_whitespace) {
        return Err(format!(
            "invalid GitHub reference '{value}': must not contain whitespace; \
             expected 'owner/repo#NUMBER'"
        ));
    }
    if value.contains("://") {
        return Err(format!(
            "invalid GitHub reference '{value}': pass 'owner/repo#NUMBER', not a URL"
        ));
    }
    bugs::parse_github_bug_ref(value).map_err(|e| e.to_string())
}

/// Canonical GitHub issues URL check for `bug tracker create` type
/// inference: returns the lowercased `(owner, repo)` pair when `base_url` is
/// the strict canonical `https://github.com/<owner>/<repo>/issues` form
/// (https only, exact github.com host, no user information, no explicit
/// port, no query or fragment, no empty path segments).
/// Delegates entirely to [`bugs::parse_github_issues_base_url`].
fn parse_github_issues_base_url(base_url: &str) -> Option<(String, String)> {
    bugs::parse_github_issues_base_url(base_url).ok()
}

/// Remote bug tracker types accepted by `bug tracker create --type`.
///
/// Each value maps via [`TrackerType::launchpad_name`] to the literal,
/// human-readable `bug_tracker_type` string that Launchpad's ensureBugTracker
/// operation expects (e.g. "GitHub Issues", also available as
/// [`bugs::BUG_TRACKER_TYPE_GITHUB`]), and that `bug remote-bug-trackers`
/// prints in its Type column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TrackerType {
    /// GitHub Issues (github.com).
    #[value(alias = "gh")]
    Github,
    /// GitLab Issues (gitlab.com or self-hosted GitLab).
    #[value(alias = "gl")]
    Gitlab,
    /// Bugzilla instances.
    Bugzilla,
    /// Debian's Debbugs (e.g. bugs.debian.org).
    Debbugs,
    /// Trac instances.
    Trac,
    /// The legacy Google Code tracker type.
    #[value(alias = "googlecode")]
    GoogleCode,
    /// Roundup instances.
    Roundup,
    /// SourceForge or SourceForge derivative.
    Sourceforge,
    /// Mantis instances.
    Mantis,
    /// Request Tracker instances.
    #[value(alias = "request-tracker")]
    Rt,
    /// Savane instances.
    Savane,
    /// PHP Project Bugtracker instances.
    #[value(alias = "phpproject")]
    PhpProject,
    /// An email-address tracker.
    Email,
}

impl TrackerType {
    /// The literal `bug_tracker_type` string Launchpad expects for this type
    /// (GitHub shares the library constant as its single source of truth).
    fn launchpad_name(self) -> &'static str {
        match self {
            TrackerType::Github => bugs::BUG_TRACKER_TYPE_GITHUB,
            TrackerType::Gitlab => "GitLab Issues",
            TrackerType::Bugzilla => "Bugzilla",
            TrackerType::Debbugs => "Debbugs",
            TrackerType::Trac => "Trac",
            TrackerType::GoogleCode => "Google Code",
            TrackerType::Roundup => "Roundup",
            TrackerType::Sourceforge => "SourceForge or SourceForge derivative",
            TrackerType::Mantis => "Mantis",
            TrackerType::Rt => "Request Tracker (RT)",
            TrackerType::Savane => "Savane",
            TrackerType::PhpProject => "PHP Project Bugtracker",
            TrackerType::Email => "Email Address",
        }
    }
}

impl std::fmt::Display for TrackerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            TrackerType::Github => "github",
            TrackerType::Gitlab => "gitlab",
            TrackerType::Bugzilla => "bugzilla",
            TrackerType::Debbugs => "debbugs",
            TrackerType::Trac => "trac",
            TrackerType::GoogleCode => "google-code",
            TrackerType::Roundup => "roundup",
            TrackerType::Sourceforge => "sourceforge",
            TrackerType::Mantis => "mantis",
            TrackerType::Rt => "rt",
            TrackerType::Savane => "savane",
            TrackerType::PhpProject => "php-project",
            TrackerType::Email => "email",
        };
        f.write_str(name)
    }
}

/// clap value parser for `bug tracker create --base-url`.
///
/// Accepts absolute http and https URLs (self-hosted trackers may use http
/// and custom ports) without query strings, fragments or user information:
/// a watch URL is built by appending the remote bug ID to this URL, so a
/// query, fragment or embedded credentials would corrupt or leak into every
/// generated watch URL.
fn parse_tracker_base_url(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value).map_err(|e| format!("invalid base URL '{value}': {e}"))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "invalid base URL '{value}': scheme must be http(s), got '{other}'"
            ));
        }
    }
    if url.host_str().is_none() {
        return Err(format!("invalid base URL '{value}': missing host"));
    }
    // Any user information (a username, a password, or both) is rejected:
    // credentials must not be embedded in a registered tracker base URL,
    // from which Launchpad derives public watch URLs.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "invalid base URL '{value}': user information (username or password) \
             is not allowed in a tracker base URL"
        ));
    }
    if url.query().is_some() {
        return Err(format!(
            "invalid base URL '{value}': query strings are not allowed; watch URLs are \
             built by appending the remote bug ID to the base URL"
        ));
    }
    if url.fragment().is_some() {
        return Err(format!(
            "invalid base URL '{value}': URL fragments are not allowed"
        ));
    }
    Ok(value.to_string())
}

/// The `bug tracker create` arguments after CLI-side resolution (pure, no
/// network): the requested tracker type as Launchpad's literal
/// `bug_tracker_type` string, and the tracker name to request (explicit
/// `--name`; when omitted no name is sent and Launchpad derives one).
#[derive(Debug)]
struct ResolvedTrackerCreate {
    base_url: String,
    bug_tracker_type: &'static str,
    name: Option<String>,
}

/// Resolve and validate `bug tracker create` arguments.
///
/// Runs both in [`validate_cli`] (so malformed configurations fail before
/// authentication or any network access) and in the command handler (to
/// obtain the resolved values for dispatch).
///
/// Rules:
/// * No `--type`: the type is inferred only for a canonical GitHub
///   repository issues URL; anything else is an error.
/// * `--type github`: requires a canonical GitHub repository issues URL.
/// * A github.com base URL combined with any other type is rejected: that
///   combination recreates the broken `github-issues` (Google Code) trap.
/// * `--name` must not be empty or contain whitespace or '/'. When omitted,
///   no name is sent: Launchpad derives one from the base URL (for
///   `github.com` URLs, its auto-github-<owner>-<repo> convention) and owns
///   collision suffixing.
fn resolve_tracker_create(
    base_url: &str,
    tracker_type: Option<TrackerType>,
    name: Option<&str>,
) -> Result<ResolvedTrackerCreate, String> {
    let canonical_github = parse_github_issues_base_url(base_url);
    let is_github_host = url::Url::parse(base_url)
        .ok()
        .is_some_and(|u| u.host_str() == Some("github.com"));

    let bug_tracker_type = match tracker_type {
        Some(tracker_type) => {
            if is_github_host && tracker_type != TrackerType::Github {
                return Err(format!(
                    "base URL '{base_url}' is on github.com but --type {} was requested; \
                     GitHub repositories must be registered with --type github and the \
                     base URL https://github.com/<owner>/<repo>/issues so watch URLs \
                     resolve (the tracker named 'github-issues' is typed Google Code and \
                     generates broken URLs like https://github.com/detail?id=...)",
                    tracker_type
                ));
            }
            if tracker_type == TrackerType::Github && canonical_github.is_none() {
                return Err(format!(
                    "--type github requires a canonical GitHub repository issues base URL \
                     'https://github.com/<owner>/<repo>/issues', got '{base_url}'"
                ));
            }
            tracker_type.launchpad_name()
        }
        None => match &canonical_github {
            Some(_) => TrackerType::Github.launchpad_name(),
            None => {
                return Err(format!(
                    "cannot infer --type from base URL '{base_url}'; pass --type \
                     (e.g. bugzilla, trac, gitlab; run \
                     'lpcli bug tracker create --help' for all values)"
                ));
            }
        },
    };

    let name = match name {
        Some(name) => {
            let name = name.trim();
            if name.is_empty() {
                return Err("--name must not be empty".to_string());
            }
            if name.chars().any(char::is_whitespace) {
                return Err(format!(
                    "invalid tracker name '{name}': must not contain whitespace"
                ));
            }
            if name.contains('/') {
                return Err(format!(
                    "invalid tracker name '{name}': must not contain '/'"
                ));
            }
            Some(name.to_string())
        }
        // No default name is invented here: Launchpad derives the tracker
        // name from the base URL (auto-github-<owner>-<repo> for GitHub URLs)
        // and the command prints the actual returned name.
        None => None,
    };

    Ok(ResolvedTrackerCreate {
        base_url: base_url.to_string(),
        bug_tracker_type,
        name,
    })
}

// ---------------------------------------------------------------------------
// Person sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum PersonCommand {
    /// Show info about a Launchpad person or team.
    Show {
        /// Launchpad username (without leading ~).
        #[arg(short, long)]
        name: String,
    },
    /// Search for people by name.
    Search {
        /// Search query.
        #[arg(short, long)]
        query: String,
    },
    /// List members of a team.
    Members {
        /// Team name (without leading ~).
        #[arg(short, long)]
        team: String,
    },
    /// List bugs filed by or assigned to a person.
    Bugs {
        /// Launchpad username (without leading ~).
        #[arg(short, long)]
        name: String,
    },
    /// List PPAs owned by a person.
    Ppas {
        /// Launchpad username (without leading ~).
        #[arg(short, long)]
        name: String,
    },
    /// List teams owned by a person.
    OwnedTeams {
        /// Launchpad username (without leading ~).
        #[arg(short, long)]
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Package sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum PackageCommand {
    /// Show info about an Ubuntu distro series.
    Series {
        /// Series codename (e.g. "jammy", "noble").
        #[arg(short, long)]
        series: String,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
    },
    /// List all distro series.
    ListSeries {
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
    },
    /// Search published source packages in a distro series.
    Search {
        /// Series codename (e.g. "jammy").
        #[arg(short, long)]
        series: String,
        /// Source package name.
        #[arg(short, long)]
        name: Option<String>,
        /// Version filter.
        #[arg(short, long)]
        version: Option<String>,
        /// Pocket (e.g. "Release", "Updates", "Security").
        #[arg(short, long, value_parser = parse_pocket)]
        pocket: Option<Pocket>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Maximum number of results to return (default: 30).
        #[arg(short, long, default_value = "30")]
        limit: u32,
    },
    /// Show info about a PPA.
    Ppa {
        /// PPA owner (Launchpad name, without ~).
        #[arg(short, long)]
        owner: String,
        /// PPA name.
        #[arg(short, long)]
        ppa: String,
    },
    /// List source packages in a PPA.
    PpaSources {
        /// PPA owner.
        #[arg(short, long)]
        owner: String,
        /// PPA name.
        #[arg(short, long)]
        ppa: String,
        /// Source package name filter.
        #[arg(short, long)]
        name: Option<String>,
    },
    /// List build results for source packages in a PPA or the primary archive.
    #[command(after_help = "Examples:
  lpcli package builds --owner my-username --ppa my-ppa --name mutter
  lpcli package builds --ppa my-username/my-ppa --name mutter --state failed --exclude-state dependency-wait
  lpcli package builds --series resolute --name mutter --version 50.1-0ubuntu2.5")]
    Builds {
        /// PPA owner (Launchpad name, without ~). Requires --ppa.
        #[arg(short, long, requires = "ppa")]
        owner: Option<String>,
        /// PPA name (or OWNER/NAME when --owner is omitted).
        #[arg(short, long)]
        ppa: Option<String>,
        /// Source package name filter.
        #[arg(short, long)]
        name: Option<String>,
        /// Exact source package version filter.
        #[arg(short, long)]
        version: Option<String>,
        /// Ubuntu series codename. Required for the primary archive; optional PPA filter.
        #[arg(short, long)]
        series: Option<String>,
        /// Only show builds for this architecture (repeatable; e.g. "amd64").
        #[arg(short, long, value_parser = parse_build_arch)]
        arch: Vec<String>,
        /// Only show builds in this state (repeatable, case-insensitive; e.g.
        /// "failed", "success", "building", "needs-build", "dependency-wait").
        #[arg(long, value_parser = parse_build_state)]
        state: Vec<String>,
        /// Exclude builds in this state (repeatable, case-insensitive).
        #[arg(long, value_parser = parse_build_state)]
        exclude_state: Vec<String>,
        /// Filter by pocket (e.g. "Release", "Updates", "Security", "Proposed").
        #[arg(long, value_parser = parse_pocket)]
        pocket: Option<Pocket>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Maximum number of source publications to check (default: 30).
        #[arg(short, long, default_value = "30", value_parser = parse_positive_u32_limit)]
        limit: u32,
    },
    /// Inspect individual builds, download build logs, or retry failed builds.
    Build {
        #[command(subcommand)]
        command: BuildCommand,
    },
    /// Show info about a distribution.
    Distro {
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
    },
    /// Download source files for a published source package.
    Download {
        /// Source package name (required).
        #[arg(short, long)]
        name: String,
        /// Package version (optional; downloads the latest published version if omitted).
        #[arg(short, long)]
        version: Option<String>,
        /// Series codename (e.g. "jammy", "noble").
        #[arg(short, long)]
        series: String,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Output directory for downloaded files (default: current directory).
        #[arg(short, long, default_value = ".")]
        output: String,
    },
    /// Show the changelog for a source package publication.
    Changelog {
        /// Source package name (required).
        #[arg(short, long)]
        name: String,
        /// Series codename (e.g. "jammy", "noble"). Required unless --ppa is specified.
        #[arg(short, long, required_unless_present = "ppa")]
        series: Option<String>,
        /// Package version (optional; uses the latest published version if omitted).
        #[arg(short, long)]
        version: Option<String>,
        /// Pocket (e.g. "Release", "Updates", "Security").
        #[arg(short, long, value_parser = parse_pocket)]
        pocket: Option<Pocket>,
        /// PPA to query as OWNER/NAME (e.g. "my-username/my-ppa"). Ubuntu-only.
        #[arg(long, value_parser = parse_ppa_ref)]
        ppa: Option<PpaRef>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Maximum number of changelog entries to display (default: 5).
        #[arg(short, long, value_parser = parse_positive_limit, conflicts_with = "full")]
        limit: Option<usize>,
        /// Show the full changelog instead of only the latest 5 entries.
        #[arg(long, conflicts_with = "limit")]
        full: bool,
    },
}

/// Sub-commands for managing individual builds and build logs.
#[derive(Clone, Debug, Subcommand)]
enum BuildCommand {
    /// Show details for a single build record.
    #[command(after_help = "Examples:
  lpcli package build show --build-id 33824576
  lpcli package build show --build-id 33961943 --ppa deadsnakes/ppa
  lpcli package build show --build-id 33961943 --owner deadsnakes --ppa ppa
  lpcli package build show --build-url https://api.launchpad.net/devel/~my-username/+archive/ubuntu/my-ppa/+build/12345")]
    Show {
        /// Numeric Launchpad build ID. Conflicts with --build-url.
        #[arg(
            long,
            value_parser = parse_positive_u64,
            conflicts_with = "build_url",
            required_unless_present = "build_url"
        )]
        build_id: Option<u64>,
        /// Canonical Launchpad API self_link of the build record.
        #[arg(
            long,
            conflicts_with_all = ["build_id", "owner", "ppa"],
            required_unless_present = "build_id"
        )]
        build_url: Option<String>,
        /// PPA owner (Launchpad name, without ~). Requires --ppa.
        #[arg(short, long, requires = "ppa")]
        owner: Option<String>,
        /// PPA name (or OWNER/NAME when --owner is omitted).
        #[arg(short, long)]
        ppa: Option<String>,
        /// Distribution name for primary-archive builds (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
    },
    /// Download build logs for a single build or filtered builds.
    #[command(after_help = "Examples:
  lpcli package build logs --build-id 33824576 -O ./logs
  lpcli package build logs --ppa my-username/my-ppa --series noble --state failed -O ./logs
  lpcli package build logs --owner my-username --ppa my-ppa --arch amd64 --exclude-state dependency-wait")]
    Logs {
        /// Numeric Launchpad build ID.
        #[arg(
            long,
            value_parser = parse_positive_u64,
            conflicts_with_all = ["build_url", "name", "version", "series", "arch", "state", "exclude_state", "pocket"]
        )]
        build_id: Option<u64>,
        /// Canonical Launchpad API self_link of a single build record.
        #[arg(
            long,
            conflicts_with_all = ["build_id", "owner", "ppa", "name", "version", "series", "arch", "state", "exclude_state", "pocket"]
        )]
        build_url: Option<String>,
        /// PPA owner (Launchpad name, without ~). Requires --ppa.
        #[arg(short, long, requires = "ppa")]
        owner: Option<String>,
        /// PPA name (or OWNER/NAME when --owner is omitted).
        #[arg(short, long)]
        ppa: Option<String>,
        /// Source package name filter.
        #[arg(short, long)]
        name: Option<String>,
        /// Exact source package version filter.
        #[arg(short, long)]
        version: Option<String>,
        /// Ubuntu series codename. Required for primary-archive queries; optional PPA filter.
        #[arg(short, long)]
        series: Option<String>,
        /// Filter by architecture (repeatable; e.g. "amd64").
        #[arg(short, long, value_parser = parse_build_arch)]
        arch: Vec<String>,
        /// Filter by build state (repeatable, case-insensitive).
        #[arg(long, value_parser = parse_build_state)]
        state: Vec<String>,
        /// Exclude builds in this state (repeatable, case-insensitive).
        #[arg(long, value_parser = parse_build_state)]
        exclude_state: Vec<String>,
        /// Filter by pocket (e.g. "Release", "Updates", "Security", "Proposed").
        #[arg(long, value_parser = parse_pocket)]
        pocket: Option<Pocket>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Maximum number of builds/source publications to check (default: 30).
        #[arg(short, long, default_value = "30", value_parser = parse_positive_u32_limit)]
        limit: u32,
        /// Output directory for downloaded log files (default: current directory).
        #[arg(short = 'O', long, default_value = ".")]
        output: String,
        /// Also download failed upload logs (`upload_log_url`) when present.
        #[arg(long)]
        include_upload_logs: bool,
    },
    /// Retry an eligible build or a filtered set of builds.
    #[command(after_help = "Examples:
  lpcli package build retry --build-id 33824576 --ppa my-username/my-ppa
  lpcli package build retry --ppa my-username/my-ppa --series noble --arch amd64 --state failed --dry-run
  lpcli package build retry --ppa my-username/my-ppa --series noble --arch amd64 --state failed --save-logs ./logs --yes")]
    Retry {
        /// Numeric Launchpad build ID.
        #[arg(
            long,
            value_parser = parse_positive_u64,
            conflicts_with_all = ["build_url", "name", "version", "series", "arch", "state", "exclude_state", "pocket"]
        )]
        build_id: Option<u64>,
        /// Canonical Launchpad API self_link of a single build record.
        #[arg(
            long,
            conflicts_with_all = ["build_id", "owner", "ppa", "name", "version", "series", "arch", "state", "exclude_state", "pocket"]
        )]
        build_url: Option<String>,
        /// PPA owner (Launchpad name, without ~). Requires --ppa.
        #[arg(short, long, requires = "ppa")]
        owner: Option<String>,
        /// PPA name (or OWNER/NAME when --owner is omitted).
        #[arg(short, long)]
        ppa: Option<String>,
        /// Source package name filter.
        #[arg(short, long)]
        name: Option<String>,
        /// Exact source package version filter.
        #[arg(short, long)]
        version: Option<String>,
        /// Ubuntu series codename. Required for primary-archive queries; optional PPA filter.
        #[arg(short, long)]
        series: Option<String>,
        /// Filter by architecture (repeatable; e.g. "amd64").
        #[arg(short, long, value_parser = parse_build_arch)]
        arch: Vec<String>,
        /// Filter by build state (repeatable, case-insensitive; required for bulk retries).
        #[arg(long, value_parser = parse_build_state)]
        state: Vec<String>,
        /// Exclude builds in this state (repeatable, case-insensitive).
        #[arg(long, value_parser = parse_build_state)]
        exclude_state: Vec<String>,
        /// Filter by pocket (e.g. "Release", "Updates", "Security", "Proposed").
        #[arg(long, value_parser = parse_pocket)]
        pocket: Option<Pocket>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Maximum number of builds/source publications to check (default: 30).
        #[arg(short, long, default_value = "30", value_parser = parse_positive_u32_limit)]
        limit: u32,
        /// Preview matching eligible and skipped builds without retrying or saving logs.
        #[arg(long)]
        dry_run: bool,
        /// Skip the interactive confirmation prompt for bulk retries.
        #[arg(short = 'y', long)]
        yes: bool,
        /// Back up each eligible build's logs into DIR before requesting a retry.
        #[arg(long, value_name = "DIR")]
        save_logs: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Project sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum ProjectCommand {
    /// Show info about a Launchpad project.
    Show {
        /// Project name.
        #[arg(short, long)]
        name: String,
    },
    /// Search Launchpad projects.
    Search {
        /// Search query.
        #[arg(short, long)]
        query: String,
    },
    /// List milestones for a project.
    Milestones {
        /// Project name.
        #[arg(short, long)]
        project: String,
        /// Show only active milestones.
        #[arg(short, long)]
        active: bool,
    },
    /// Show a specific project milestone.
    ShowMilestone {
        /// Project name.
        #[arg(short, long)]
        project: String,
        /// Milestone name.
        #[arg(short, long)]
        name: String,
    },
    /// Show a project series (e.g. "trunk", "2.0").
    SeriesShow {
        /// Project name.
        #[arg(short, long)]
        project: String,
        /// Series name.
        #[arg(short, long)]
        series: String,
    },
    /// List all series for a project.
    ListSeries {
        /// Project name.
        #[arg(short, long)]
        project: String,
    },
    /// List releases in a project series.
    SeriesReleases {
        /// Project name.
        #[arg(short, long)]
        project: String,
        /// Series name.
        #[arg(short, long)]
        series: String,
    },
}

// ---------------------------------------------------------------------------
// CVE sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum CveCommand {
    /// Show details of a CVE.
    Show {
        /// CVE sequence, e.g. "2024-1234".
        #[arg(short, long)]
        sequence: String,
    },
    /// Search CVEs.
    Search {
        /// Restrict to CVEs affecting this distribution.
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Keyword search against CVE sequence numbers, titles, and descriptions.
        #[arg(short, long)]
        keyword: Option<String>,
        /// Maximum results to return.
        #[arg(short, long, default_value = "25")]
        limit: u32,
    },
    /// List CVEs linked to a bug.
    BugCves {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
    },
}

// ---------------------------------------------------------------------------
// Git sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum GitCommand {
    /// Show a Git repository by path (~owner/project/+git/name).
    Show {
        /// Repository path (e.g. "~user/project/+git/repo").
        #[arg(short, long)]
        path: String,
    },
    /// Show the default Git repository for a project or distribution source
    /// package.
    ///
    /// A target only has a default repository if one has been designated on
    /// Launchpad (for example, "launchpad" or "ubuntu/+source/unbound"). To
    /// list all repositories for a target instead, use `lpcli git list --target
    /// <TARGET>`.
    Default {
        /// Project name (e.g. "launchpad") or distribution source package slug
        /// (e.g. "ubuntu/+source/unbound"). Bare distribution names,
        /// distribution series, and ~person/~team names are not accepted.
        #[arg(short, long)]
        target: String,
    },
    /// List Git repositories owned by a person.
    ListPersonRepos {
        /// Launchpad person name (without ~).
        #[arg(short, long)]
        name: String,
    },
    /// List Git repositories for a target (includes personal forks).
    List {
        /// Project name (e.g. "launchpad"), distribution source package
        /// slug (e.g. "ubuntu/+source/apt"), or ~person/~team name (e.g.
        /// "~desktop-snappers"). Bare distribution names are not accepted
        /// by the API.
        #[arg(short, long)]
        target: String,
    },
    /// List refs (branches and tags) in a Git repository.
    Refs {
        /// Repository path (e.g. "~user/project/+git/repo").
        #[arg(short, long)]
        path: String,
    },
    /// List merge proposals originating from a Git repository.
    Proposals {
        /// Repository path.
        #[arg(short, long)]
        path: String,
        /// Filter by status (e.g. "Needs review", "Approved", "Merged").
        #[arg(short, long)]
        status: Option<String>,
    },
    /// List code review comments on a merge proposal.
    Comments {
        /// Repository path (e.g. "~user/project/+git/repo").
        #[arg(short, long)]
        path: String,
        /// Merge proposal ID (numeric, from the proposals listing).
        #[arg(short = 'i', long)]
        id: u64,
        /// Preview diff ID to use for inline comments (defaults to latest).
        #[arg(short, long)]
        diff: Option<u64>,
    },
    /// List all preview diffs for a merge proposal.
    Diffs {
        /// Repository path (e.g. "~user/project/+git/repo").
        #[arg(short, long)]
        path: String,
        /// Merge proposal ID (numeric, from the proposals listing).
        #[arg(short = 'i', long)]
        id: u64,
    },
}

// ---------------------------------------------------------------------------
// Spec sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum SpecCommand {
    /// Show a specification (blueprint).
    Show {
        /// Target project name.
        #[arg(short, long)]
        target: String,
        /// Specification name (slug).
        #[arg(short, long)]
        name: String,
    },
    /// List specifications for a project.
    List {
        /// Project name.
        #[arg(short, long)]
        target: String,
        /// Show all specs, not just currently valid ones.
        #[arg(short, long)]
        all: bool,
    },
}

// ---------------------------------------------------------------------------
// Question sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum QuestionCommand {
    /// Show a question by numeric ID.
    Show {
        /// Question ID number.
        #[arg(short, long)]
        question_id: u64,
    },
    /// Search questions on a target project or distribution.
    Search {
        /// Target project or distribution name.
        #[arg(short, long)]
        target: String,
        /// Text to search for.
        #[arg(short, long)]
        query: Option<String>,
        /// Filter by status (e.g. "Open", "Answered", "Solved").
        #[arg(short = 'x', long)]
        status: Option<String>,
    },
    /// Show messages on a question.
    Messages {
        /// Target project or distribution name.
        #[arg(short, long)]
        target: String,
        /// Question ID number.
        #[arg(short, long)]
        question_id: u64,
    },
}

// ---------------------------------------------------------------------------
// Webhook sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum WebhookCommand {
    /// List webhooks for a project, distribution, or Git repository.
    List {
        /// Target path (project name, distro name, or "~user/project/+git/repo").
        #[arg(short, long)]
        target: String,
    },
    /// Create a new webhook.
    Create {
        /// Target path (project name, distro name, or Git repository path).
        #[arg(short, long)]
        target: String,
        /// Delivery URL for webhook payloads.
        #[arg(short, long)]
        delivery_url: String,
        /// Comma-separated event types (e.g. "git:push:0.1,merge-proposal:0.1").
        #[arg(short, long)]
        event_types: String,
        /// Create the webhook as inactive (default is active).
        #[arg(long)]
        inactive: bool,
        /// Optional shared secret for payload verification.
        #[arg(short, long)]
        secret: Option<String>,
    },
    /// Delete a webhook.
    Delete {
        /// Webhook self_link URL.
        #[arg(short, long)]
        webhook_url: String,
    },
    /// Send a test ping delivery to a webhook.
    Ping {
        /// Webhook self_link URL.
        #[arg(short, long)]
        webhook_url: String,
    },
    /// List recent deliveries for a webhook.
    Deliveries {
        /// Webhook self_link URL.
        #[arg(short, long)]
        webhook_url: String,
    },
}

// ---------------------------------------------------------------------------
// Translation sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum TranslationCommand {
    /// List translation import queue entries for a distro series.
    Queue {
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Series codename (e.g. "jammy", "noble").
        #[arg(short, long)]
        series: String,
    },
    /// List translation templates for a distro series.
    Templates {
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Series codename (e.g. "jammy", "noble").
        #[arg(short, long)]
        series: String,
    },
}

// ---------------------------------------------------------------------------
// Snap sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum SnapCommand {
    /// Show a snap recipe.
    Show {
        /// Owner Launchpad name (without ~).
        #[arg(short, long)]
        owner: String,
        /// Snap recipe name.
        #[arg(short, long)]
        name: String,
    },
    /// Find snap recipes owned by a person.
    Find {
        /// Owner Launchpad name (without ~).
        #[arg(short, long)]
        owner: String,
    },
    /// List pending builds for a snap recipe.
    Builds {
        /// Owner Launchpad name (without ~).
        #[arg(short, long)]
        owner: String,
        /// Snap recipe name.
        #[arg(short, long)]
        name: String,
    },
    /// Request new builds for a snap recipe.
    RequestBuilds {
        /// Owner Launchpad name (without ~).
        #[arg(short, long)]
        owner: String,
        /// Snap recipe name.
        #[arg(short, long)]
        name: String,
        /// Pocket (default: "Release").
        #[arg(short, long, default_value = "Release", value_parser = parse_pocket)]
        pocket: Pocket,
        /// Distribution to build against (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
    },
}

// ---------------------------------------------------------------------------
// Queue sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum QueueCommand {
    /// Search packages in build/upload queues for a distro series.
    #[command(after_help = "Examples:
  lpcli queue search --status done --series tenacious --name mutter --ppa my-username/my-ppa
  lpcli queue search --status done --series tenacious --name mutter --archive ppa:my-username/my-ppa")]
    Search {
        /// Filter by pocket: Release, Security, Updates, Proposed, or Backports.
        #[arg(short, long, value_parser = parse_pocket)]
        pocket: Option<Pocket>,
        /// Filter by upload status: new, unapproved, accepted, done, or rejected
        /// (case-insensitive).
        #[arg(short, long, required = true, value_parser = parse_queue_status)]
        status: Option<String>,
        /// Keyword used to match the uploaded package name.
        #[arg(short, long)]
        name: Option<String>,
        /// Package version string used to match the uploaded package version.
        #[arg(short, long)]
        version: Option<String>,
        /// Require exact match for --name or --version (default: false).
        #[arg(short, long, default_value = "false")]
        exact_match: bool,
        /// Archive to query: "primary" (the default), "ubuntu/+archive/primary",
        /// "~owner/+archive/ubuntu/name", or "ppa:owner/name".
        #[arg(short, long, conflicts_with = "ppa")]
        archive: Option<String>,
        /// PPA to query as OWNER/NAME (e.g. "my-username/my-ppa").
        /// Resolves to ~OWNER/+archive/ubuntu/NAME; conflicts with --archive.
        /// PPA uploads are normally auto-accepted; search for done or accepted.
        /// The queried archive is shown in the results. PPAs are Ubuntu-only.
        #[arg(long, value_parser = parse_ppa_ref, conflicts_with = "archive")]
        ppa: Option<PpaRef>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Series codename (e.g. "oracular"). Defaults to the current development series.
        #[arg(long)]
        series: Option<String>,
        /// Maximum number of results to return (default: 30).
        #[arg(short, long, default_value = "30")]
        limit: u32,
    },
    /// Download files from a package in the upload queue.
    Download {
        /// Source package name (required).
        #[arg(short, long)]
        name: String,
        /// Exact package version (optional; downloads the first match if not given).
        #[arg(short, long)]
        version: Option<String>,
        /// Filter by upload status: new, unapproved, accepted, done, or rejected
        /// (case-insensitive).
        #[arg(short, long, value_parser = parse_queue_status)]
        status: String,
        /// Filter by architecture (e.g. "amd64", "arm64", "source").
        #[arg(short, long, value_parser = parse_queue_arch)]
        arch: QueueArch,
        /// Archive containing the upload (see `queue search --help`).
        /// Defaults to the distribution's primary archive.
        #[arg(long, conflicts_with = "ppa")]
        archive: Option<String>,
        /// PPA containing the upload as OWNER/NAME (e.g. "my-username/my-ppa").
        /// Conflicts with --archive; PPAs are Ubuntu-only. PPA uploads are
        /// normally auto-accepted; use the status shown by `queue search`.
        #[arg(long, value_parser = parse_ppa_ref, conflicts_with = "archive")]
        ppa: Option<PpaRef>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Series codename (e.g. "oracular"). Defaults to the current development series.
        #[arg(long)]
        series: Option<String>,
        /// Output directory for downloaded files (default: current directory).
        #[arg(short, long, default_value = ".")]
        output: String,
    },
    /// Display upload details, including binary .deb/.ddeb information, for a queue item.
    Info {
        /// Source package name (required).
        #[arg(short, long)]
        name: String,
        /// Exact package version (optional; shows the first match if not given).
        #[arg(short, long)]
        version: Option<String>,
        /// Filter by upload status: new, unapproved, accepted, done, or rejected
        /// (case-insensitive).
        #[arg(short, long, value_parser = parse_queue_status)]
        status: String,
        /// Filter by architecture (e.g. "amd64", "arm64", "source").
        #[arg(short, long, value_parser = parse_queue_arch)]
        arch: QueueArch,
        /// Archive containing the upload (see `queue search --help`).
        /// Defaults to the distribution's primary archive.
        #[arg(long, conflicts_with = "ppa")]
        archive: Option<String>,
        /// PPA containing the upload as OWNER/NAME (e.g. "my-username/my-ppa").
        /// Conflicts with --archive; PPAs are Ubuntu-only. PPA uploads are
        /// normally auto-accepted; use the status shown by `queue search`.
        #[arg(long, value_parser = parse_ppa_ref, conflicts_with = "archive")]
        ppa: Option<PpaRef>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Series codename (e.g. "oracular"). Defaults to the current development series.
        #[arg(long)]
        series: Option<String>,
    },
    /// Accept a package in the upload queue (requires queue-admin permissions).
    #[command(after_help = "Examples:
  lpcli queue accept --name mutter --version 51.0-1ubuntu4 --status unapproved --arch source --series tenacious --ppa my-username/my-ppa")]
    Accept {
        /// Source package name (required).
        #[arg(short, long)]
        name: String,
        /// Exact package version (optional; required when multiple uploads match).
        #[arg(short, long)]
        version: Option<String>,
        /// Filter by upload status, e.g. new or unapproved (case-insensitive).
        #[arg(short, long, default_value = "New", value_parser = parse_queue_status)]
        status: String,
        /// Filter by architecture (e.g. "amd64", "arm64", "source").
        #[arg(short, long, value_parser = parse_queue_arch)]
        arch: QueueArch,
        /// Archive containing the upload (see `queue search --help`).
        /// Defaults to the distribution's primary archive.
        #[arg(long, conflicts_with = "ppa")]
        archive: Option<String>,
        /// PPA containing the upload as OWNER/NAME (e.g. "my-username/my-ppa").
        /// Conflicts with --archive; PPAs are Ubuntu-only. PPA uploads are
        /// normally auto-accepted; use the status shown by `queue search`.
        #[arg(long, value_parser = parse_ppa_ref, conflicts_with = "archive")]
        ppa: Option<PpaRef>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
        /// Series codename (e.g. "oracular"). Defaults to the current development series.
        #[arg(long)]
        series: Option<String>,
    },
    /// Reject a package in the upload queue (requires queue-admin permissions).
    #[command(after_help = "Examples:
  lpcli queue reject --name mutter --version 51.0-1ubuntu4 --status unapproved --arch source --series tenacious --ppa my-username/my-ppa --comment \"Needs a bug fix.\"")]
    Reject {
        /// Source package name (required).
        #[arg(short, long)]
        name: String,
        /// Exact package version (optional; required when multiple uploads match).
        #[arg(short, long)]
        version: Option<String>,
        /// Filter by upload status, e.g. new or unapproved (case-insensitive).
        #[arg(short, long, default_value = "New", value_parser = parse_queue_status)]
        status: String,
        /// Filter by architecture (e.g. "amd64", "arm64", "source").
        #[arg(short, long, value_parser = parse_queue_arch)]
        arch: QueueArch,
        /// Archive containing the upload (see `queue search --help`).
        /// Defaults to the distribution's primary archive.
        #[arg(long, conflicts_with = "ppa")]
        archive: Option<String>,
        /// PPA containing the upload as OWNER/NAME (e.g. "my-username/my-ppa").
        /// Conflicts with --archive; PPAs are Ubuntu-only. PPA uploads are
        /// normally auto-accepted; use the status shown by `queue search`.
        #[arg(long, value_parser = parse_ppa_ref, conflicts_with = "archive")]
        ppa: Option<PpaRef>,
        /// Rejection comment explaining why the package is being rejected.
        #[arg(short, long)]
        comment: Option<String>,
        /// Distribution name (default: "ubuntu").
        #[arg(short = 'D', long, default_value = "ubuntu")]
        distro: String,
        /// Series codename (e.g. "oracular"). Defaults to the current development series.
        #[arg(long)]
        series: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Login, logout and hardware-token sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, clap::Args)]
struct LoginArgs {
    /// Log in with a hardware-bound grant, which needs a USB FIDO2 security
    /// key, instead of the browser OAuth flow.  Without --grant-token, the
    /// stored grant is used.
    #[arg(long)]
    hardware_token: bool,

    /// Import a newly issued grant by its oauth_token.  The secret is read
    /// from LPCLI_HARDWARE_GRANT_SECRET or prompted for.
    #[arg(long, value_name = "TOKEN", requires_all = ["hardware_token", "account"])]
    grant_token: Option<String>,

    /// The Launchpad account that owns the grant.
    #[arg(long, value_name = "NAME", requires = "hardware_token")]
    account: Option<String>,

    /// The OAuth consumer key the grant was issued for [default: lpcli-hardware].
    #[arg(long, value_name = "KEY", requires = "hardware_token")]
    consumer: Option<String>,

    /// Restrict the grant to one security key by its base64url WebAuthn
    /// credential ID.
    #[arg(long, value_name = "ID", requires = "hardware_token")]
    credential_id: Option<String>,
}

#[derive(Clone, Debug, clap::Args)]
struct LogoutArgs {
    /// Only remove the classic OAuth credentials (e.g. the bootstrap token
    /// used to register a key and issue a grant).
    #[arg(long, conflicts_with = "hardware_token")]
    classic: bool,

    /// Only remove the stored hardware-bound grant.  It remains valid on
    /// Launchpad; use `lpcli hardware-token revoke` to revoke it.
    #[arg(long)]
    hardware_token: bool,
}

/// OAuth permission levels for hardware-bound grants.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum PermissionArg {
    /// Read public data.
    ReadNonPrivate,
    /// Read and change public data.
    ChangeNonPrivate,
    /// Read all data, including private data.
    ReadAnything,
    /// Read and change all data, including private data.
    ChangeAnything,
}

impl From<PermissionArg> for OAuthPermission {
    fn from(arg: PermissionArg) -> Self {
        match arg {
            PermissionArg::ReadNonPrivate => Self::ReadNonPrivate,
            PermissionArg::ChangeNonPrivate => Self::ChangeNonPrivate,
            PermissionArg::ReadAnything => Self::ReadAnything,
            PermissionArg::ChangeAnything => Self::ChangeAnything,
        }
    }
}

/// Parse an expiry given as `YYYY-MM-DD` (midnight UTC) or RFC 3339.
fn parse_expiry(value: &str) -> Result<jiff::Timestamp, String> {
    if let Some(date) = lpcli::time::parse_rfc3339_timestamp(value) {
        return Ok(date);
    }
    jiff::civil::Date::strptime("%Y-%m-%d", value)
        .ok()
        .and_then(|date| date.to_zoned_datetime(jiff::tz::TimeZone::UTC).ok())
        .map(|date| date.timestamp())
        .ok_or_else(|| format!("invalid date '{value}'; use YYYY-MM-DD or RFC 3339"))
}

#[derive(Clone, Debug, Subcommand)]
enum HardwareTokenCommand {
    /// List the security keys registered with your account.
    Keys,

    /// Register a USB FIDO2 security key with your account (touch the key
    /// when prompted).
    Register {
        /// A name to help tell your registered keys apart.
        name: String,
        /// Authenticate API requests with the existing classic OAuth session
        /// from the system keyring or legacy credentials file, ignoring any
        /// stored hardware-bound grant. The new key is still needed to register.
        #[arg(long)]
        classic: bool,
    },

    /// Revoke a registered security key.  Its grants can no longer mint
    /// access tokens.
    RevokeKey {
        /// The key's ID or name (see `lpcli hardware-token keys`).
        key: String,
    },

    /// Issue a grant bound to a registered security key and log in with it.
    Issue {
        /// The security key's ID or name (see `lpcli hardware-token keys`).
        #[arg(short, long)]
        key: String,

        /// The grant's permission.  It cannot exceed the permission of the
        /// credentials used to issue it.
        #[arg(short, long, value_enum, default_value_t = PermissionArg::ChangeAnything)]
        permission: PermissionArg,

        /// Restrict the grant to a project or distribution, or to
        /// `distribution/sourcepackagename`.
        #[arg(long)]
        context: Option<String>,

        /// When the grant expires (YYYY-MM-DD or RFC 3339).  Launchpad
        /// defaults to, and limits grants to, 365 days.
        #[arg(long, value_parser = parse_expiry)]
        expires: Option<jiff::Timestamp>,

        /// The OAuth consumer key that will use the grant.
        #[arg(long, default_value = hardware_tokens::HARDWARE_CONSUMER_KEY)]
        consumer: String,

        /// Print the grant as JSON (as accepted by launchpadlib's
        /// login_with_hardware_token) instead of logging in with it.  The
        /// output contains the grant secret, which is shown only once.
        #[arg(long)]
        print: bool,
    },

    /// Show the stored hardware-bound grant's configuration (never its
    /// account, token key or secret).
    Show,

    /// Clear the local access-token cache without deleting the grant or revoking anything.
    ClearCache,

    /// Revoke a hardware-bound grant on Launchpad (by default the stored
    /// one) and remove the stored copy.
    Revoke {
        /// The token key of the grant to revoke; defaults to the stored grant.
        #[arg(long)]
        token: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// AccessToken sub-commands
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Subcommand)]
enum AccessTokenCommand {
    /// Issue a personal access token for a project.
    Issue {
        /// Project name.
        #[arg(short, long)]
        project: String,
        /// Token description.
        #[arg(short, long)]
        description: String,
        /// Comma-separated scopes (e.g. "repository:push,repository:build_status").
        #[arg(short, long)]
        scopes: String,
    },
    /// Issue a personal access token for a Git repository.
    IssueGit {
        /// Repository path (e.g. "~user/project/+git/repo").
        #[arg(short, long)]
        repo: String,
        /// Token description.
        #[arg(short, long)]
        description: String,
        /// Comma-separated scopes.
        #[arg(short, long)]
        scopes: String,
    },
    /// List personal access tokens for a project.
    List {
        /// Project name.
        #[arg(short, long)]
        project: String,
    },
    /// List personal access tokens for a Git repository.
    ListGit {
        /// Repository path (e.g. "~user/project/+git/repo").
        #[arg(short, long)]
        repo: String,
    },
    /// Revoke a personal access token.
    Revoke {
        /// Token self_link URL.
        #[arg(short, long)]
        token_url: String,
    },
}

// ---------------------------------------------------------------------------
// Skills command (embedded agent skill documentation)
// ---------------------------------------------------------------------------

/// The agent skill documentation (the repository-root `SKILL.md`), embedded in
/// the binary at compile time.
const SKILL_DOC: &str = include_str!("../../SKILL.md");

/// Print the embedded agent skill documentation to stdout, byte for byte.
///
/// No authentication, network access, or Launchpad API client is involved:
/// the documentation ships inside the binary itself.
fn handle_skills(output: &Output) -> lpcli::error::Result<()> {
    // Write the exact embedded bytes: no added newline, no terminal
    // formatting.  I/O failures (e.g. a closed pipe) propagate through the
    // standard error path instead of panicking.
    output.write_raw(SKILL_DOC)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let mut cli = Cli::parse();
    cli.no_input |= !std::io::stdin().is_terminal();
    set_prompts_disabled(cli.no_input);
    if let Err(e) = validate_cli(&cli) {
        e.exit();
    }
    let env = output::Environment::from_process();
    let terminals = output::terminals();
    let mut output = initial_error_output(&cli, &env, terminals);
    if let Err(e) = run_command(
        cli,
        config::load,
        handle_clear_cache,
        &mut output,
        &env,
        terminals,
    )
    .await
    {
        eprint!("{}", output.render_error(&e));
        std::process::exit(e.exit_code());
    }
}

/// Validate Ubuntu-specific command-line values that depend on the selected
/// distribution.
///
/// Series names are only validated when the target distribution is Ubuntu,
/// because other Launchpad distributions use their own series names.
fn validate_cli(cli: &Cli) -> Result<(), clap::Error> {
    if cli.public && !supports_public_reads(&cli.command) {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--public is only available for data reads, not mutations or identity/security management",
        ));
    }
    if let Some(format) = cli.format
        && !supports_output_mode(&cli.command, format)
    {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            format!(
                "--format {} is not available for this command; use --format rich or --format plain\n",
                format.name()
            ),
        ));
    }
    if cli.fields.is_some()
        && matches!(
            cli.format,
            Some(OutputMode::Rich | OutputMode::Plain | OutputMode::Tsv)
        )
    {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--fields requires JSON output\n",
        ));
    }
    if cli.quiet && !is_mutation(&cli.command) {
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            "--quiet is only available for mutations\n",
        ));
    }
    if cli.no_input && requires_prompt(&cli.command) {
        let message = if matches!(
            &cli.command,
            Command::Package(PackageCommand::Build {
                command: BuildCommand::Retry { .. }
            })
        ) {
            "bulk build retry needs interactive confirmation; rerun from a terminal, pass --yes, or use --dry-run to preview\n"
        } else {
            "this command needs interactive input; rerun from a terminal without --no-input\n"
        };
        return Err(clap::Error::raw(
            clap::error::ErrorKind::ArgumentConflict,
            message,
        ));
    }
    let result = match &cli.command {
        Command::Bug(cmd) => match cmd {
            BugCommand::AddTask {
                target,
                series,
                many_series,
                ..
            }
            | BugCommand::DeleteTask {
                target,
                series,
                many_series,
                ..
            } => validate_bug_series(target, series.as_deref(), many_series.as_deref()),
            BugCommand::MarkDuplicate {
                bug_id,
                duplicate_of,
            } if bug_id == duplicate_of => Err(format!(
                "a bug cannot be marked as a duplicate of itself (bug #{bug_id})"
            )),
            BugCommand::LinkRemote { target, series, .. }
            | BugCommand::UnlinkRemote { target, series, .. } => {
                validate_series_for_target(target.as_deref().unwrap_or(""), series.as_deref())
            }
            BugCommand::Tracker {
                command:
                    TrackerCommand::Create {
                        base_url,
                        tracker_type,
                        name,
                        ..
                    },
            } => resolve_tracker_create(base_url, *tracker_type, name.as_deref()).map(|_| ()),
            _ => Ok(()),
        },
        Command::Package(cmd) => match cmd {
            PackageCommand::Series { series, distro } => validate_series_for_distro(distro, series),
            PackageCommand::Search { series, distro, .. } => {
                validate_series_for_distro(distro, series)
            }
            PackageCommand::Download { series, distro, .. } => {
                validate_series_for_distro(distro, series)
            }
            PackageCommand::Changelog {
                series,
                distro,
                ppa,
                ..
            } => {
                if ppa.is_some() && !distro.eq_ignore_ascii_case("ubuntu") {
                    Err(format!(
                        "PPA archives are only available for Ubuntu, not '{distro}'"
                    ))
                } else {
                    validate_optional_series_for_distro(distro, series.as_deref())
                }
            }
            PackageCommand::Builds {
                owner,
                ppa,
                series,
                distro,
                ..
            } => validate_build_filter_args(
                owner.as_deref(),
                ppa.as_deref(),
                series.as_deref(),
                distro,
            ),
            PackageCommand::Build { command } => match command {
                BuildCommand::Show {
                    build_url,
                    owner,
                    ppa,
                    distro,
                    ..
                } => {
                    if let Some(url) = build_url {
                        validate_build_url(url)
                    } else {
                        validate_build_ppa_optional(owner.as_deref(), ppa.as_deref(), distro)
                    }
                }
                BuildCommand::Logs {
                    build_id,
                    build_url,
                    owner,
                    ppa,
                    series,
                    distro,
                    ..
                } => {
                    if let Some(url) = build_url {
                        validate_build_url(url)
                    } else if build_id.is_some() {
                        validate_build_ppa_optional(owner.as_deref(), ppa.as_deref(), distro)
                    } else {
                        validate_build_filter_args(
                            owner.as_deref(),
                            ppa.as_deref(),
                            series.as_deref(),
                            distro,
                        )
                    }
                }
                BuildCommand::Retry {
                    build_id,
                    build_url,
                    owner,
                    ppa,
                    series,
                    state,
                    distro,
                    save_logs,
                    ..
                } => {
                    if let Some(dir) = save_logs
                        && dir.trim().is_empty()
                    {
                        Err("--save-logs directory must not be empty".into())
                    } else if let Some(url) = build_url {
                        validate_build_url(url)
                    } else if build_id.is_some() {
                        validate_build_ppa_optional(owner.as_deref(), ppa.as_deref(), distro)
                    } else {
                        validate_build_filter_args(
                            owner.as_deref(),
                            ppa.as_deref(),
                            series.as_deref(),
                            distro,
                        )
                        .and_then(|()| {
                            if state.is_empty() {
                                Err("bulk build retry requires at least one --state filter (e.g. --state failed); use --build-id or --build-url to retry a single build".into())
                            } else {
                                Ok(())
                            }
                        })
                    }
                }
            },
            _ => Ok(()),
        },
        Command::Translation(cmd) => match cmd {
            TranslationCommand::Queue { distro, series }
            | TranslationCommand::Templates { distro, series } => {
                validate_series_for_distro(distro, series)
            }
        },
        Command::Queue(cmd) => match cmd {
            QueueCommand::Search { distro, series, .. }
            | QueueCommand::Download { distro, series, .. }
            | QueueCommand::Info { distro, series, .. }
            | QueueCommand::Accept { distro, series, .. }
            | QueueCommand::Reject { distro, series, .. } => {
                validate_optional_series_for_distro(distro, series.as_deref())
            }
        },
        _ => Ok(()),
    };

    result.map_err(|msg| clap::Error::raw(clap::error::ErrorKind::InvalidValue, msg))
}

fn resolve_command_output(
    cli: &Cli,
    settings: &config::AppConfig,
    env: &output::Environment,
    (stdout_terminal, stderr_terminal): (bool, bool),
) -> Result<Output, String> {
    Output::resolve(output::Settings {
        format: cli.format,
        color: cli.color,
        config_format: settings.display.format,
        accessible: settings.display.accessible,
        display: Display::new(settings.display.width),
        json_output: supports_output_mode(&cli.command, OutputMode::Json),
        tsv_output: supports_output_mode(&cli.command, OutputMode::Tsv),
        fields: cli.fields.clone(),
        env,
        stdout_terminal,
        stderr_terminal,
    })
    .map(|output| {
        output
            .with_progress(cli.progress, stderr_terminal)
            .with_pager(cli.no_pager, env, stdout_terminal)
    })
}

/// Error styling before configuration is available must not prevent dispatch
/// (especially clear-cache), even when LPCLI_FORMAT itself is invalid.
fn initial_error_output(cli: &Cli, env: &output::Environment, terminals: (bool, bool)) -> Output {
    let settings = config::AppConfig::default();
    resolve_command_output(cli, &settings, env, terminals).unwrap_or_else(|_| {
        let fallback_env = output::Environment {
            format: None,
            ..env.clone()
        };
        // `display.format` is not known yet, so field selection is checked
        // only after configuration loads. This fallback only styles errors.
        let mut error_cli = Cli {
            fields: None,
            ..cli.clone()
        };
        if error_cli
            .format
            .is_some_and(|format| !supports_output_mode(&error_cli.command, format))
        {
            error_cli.format = None;
        }
        resolve_command_output(&error_cli, &settings, &fallback_env, terminals)
            .expect("automatic output mode is valid")
    })
}

async fn run_command(
    cli: Cli,
    load_config: impl FnOnce() -> lpcli::error::Result<config::AppConfig>,
    clear_cache: impl FnOnce() -> lpcli::error::Result<()>,
    output: &mut Output,
    env: &output::Environment,
    terminals: (bool, bool),
) -> lpcli::error::Result<()> {
    if matches!(
        &cli.command,
        Command::HardwareToken(HardwareTokenCommand::ClearCache)
    ) {
        clear_cache()?;
        return output.print(&clear_cache_report());
    }
    let settings = load_config()?;
    *output = resolve_command_output(&cli, &settings, env, terminals)
        .map_err(lpcli::error::LpError::Usage)?;
    if let Some(warning) = output.warning() {
        output.warn(warning)?;
    }
    let cache_timeout = hardware_token_cache_timeout(&settings);
    let policy = read_auth_policy(&cli.command, cli.authenticated, cli.public);
    let (quiet, no_input) = (cli.quiet, cli.no_input);

    match cli.command {
        Command::Status => handle_status(output).await?,
        Command::Schema => output.print(&schema_report())?,

        Command::Login(args) => handle_login(args, output, no_input, cache_timeout).await?,
        Command::Logout(args) => handle_logout(args, output)?,
        Command::HardwareToken(cmd) => {
            handle_hardware_token(cmd, output, quiet, no_input, cache_timeout).await?
        }

        Command::Bug(cmd) => handle_bug(cmd, output, quiet, policy, cache_timeout).await?,
        Command::Person(cmd) => handle_person(cmd, output, policy, cache_timeout).await?,
        Command::Package(cmd) => {
            handle_package(cmd, output, quiet, no_input, policy, cache_timeout).await?
        }
        Command::Project(cmd) => handle_project(cmd, output, policy, cache_timeout).await?,
        Command::Cve(cmd) => handle_cve(cmd, output, policy, cache_timeout).await?,
        Command::Git(cmd) => handle_git(cmd, output, policy, cache_timeout).await?,
        Command::Spec(cmd) => handle_spec(cmd, output, policy, cache_timeout).await?,
        Command::Question(cmd) => handle_question(cmd, output, policy, cache_timeout).await?,
        Command::Webhook(cmd) => handle_webhook(cmd, output, cli.quiet, cache_timeout).await?,
        Command::Translation(cmd) => handle_translation(cmd, output, policy, cache_timeout).await?,
        Command::Snap(cmd) => handle_snap(cmd, output, cli.quiet, policy, cache_timeout).await?,
        Command::Queue(cmd) => handle_queue(cmd, output, cli.quiet, policy, cache_timeout).await?,
        Command::AccessToken(cmd) => {
            handle_access_token(cmd, output, cli.quiet, cache_timeout).await?
        }
        Command::Skills => handle_skills(output)?,
    }

    Ok(())
}

fn parse_positive_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be a positive number".to_string())?;
    if limit == 0 {
        return Err("limit must be a positive number".to_string());
    }
    Ok(limit)
}

fn parse_positive_u32_limit(value: &str) -> Result<u32, String> {
    let limit = parse_positive_limit(value)?;
    u32::try_from(limit).map_err(|_| format!("limit must be at most {}", u32::MAX))
}

fn parse_positive_u64(value: &str) -> Result<u64, String> {
    let id = value
        .trim()
        .parse::<u64>()
        .map_err(|_| "build ID must be a positive integer".to_string())?;
    if id == 0 {
        return Err("build ID must be a positive integer".to_string());
    }
    Ok(id)
}

fn validate_build_url(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("--build-url must not be empty".into());
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err("--build-url must not contain whitespace".into());
    }
    let parsed = url::Url::parse(trimmed).map_err(|e| {
        format!(
            "invalid --build-url '{}': {e}",
            lpcli::download::redact_url_for_display(trimmed)
        )
    })?;
    let safe = lpcli::download::redact_url_for_display(trimmed);
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "--build-url must use http or https scheme: '{safe}'"
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!(
            "--build-url must not contain credentials: '{safe}'"
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(format!(
            "--build-url must not contain a query string or fragment: '{safe}'"
        ));
    }
    lpcli::packages::validate_build_self_link(trimmed)
        .map_err(|e| format!("invalid --build-url: {e}"))?;
    Ok(())
}

fn validate_build_ppa_optional(
    owner: Option<&str>,
    ppa: Option<&str>,
    distro: &str,
) -> Result<(), String> {
    let ppa_ref = resolve_ppa_args(owner, ppa)?;
    if ppa_ref.is_some() && !distro.eq_ignore_ascii_case("ubuntu") {
        return Err(format!(
            "PPA archives are only available for Ubuntu, not '{distro}'"
        ));
    }
    Ok(())
}

fn validate_build_filter_args(
    owner: Option<&str>,
    ppa: Option<&str>,
    series: Option<&str>,
    distro: &str,
) -> Result<(), String> {
    let ppa_ref = resolve_ppa_args(owner, ppa)?;
    if ppa_ref.is_some() && !distro.eq_ignore_ascii_case("ubuntu") {
        return Err(format!(
            "PPA archives are only available for Ubuntu, not '{distro}'"
        ));
    }
    if ppa_ref.is_none() && series.is_none() {
        return Err(
            "specify --series for the primary archive, --ppa OWNER/NAME, or --owner and --ppa for a PPA"
                .into(),
        );
    }
    if let Some(series) = series {
        validate_series_for_distro(distro, series)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Status handler
// ---------------------------------------------------------------------------

/// Display authentication status and Launchpad server reachability.
///
/// Both checks run concurrently to minimise latency.
async fn handle_status(output: &Output) -> lpcli::error::Result<()> {
    let (server, auth) = tokio::join!(status::check_server(), status::check_auth());
    output.print(&status_report(&server, &auth))
}

/// Which kind of stored credentials `status` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusCredentials {
    HardwareToken,
    OAuth,
    None,
}

impl StatusCredentials {
    fn key(self) -> &'static str {
        match self {
            Self::HardwareToken => "hardware_token",
            Self::OAuth => "oauth",
            Self::None => "none",
        }
    }
}

/// Build the `status` report.
///
/// There is no Launchpad entry to pass through, so the JSON is an
/// lpcli-defined object under the `lpcli` member. It names the account but
/// never contains credentials, tokens, or grant secrets, which `AuthStatus`
/// does not carry.
fn status_report(server: &status::ServerStatus, auth: &status::AuthStatus) -> Report {
    // Launchpad names have no leading `~`; the display form adds one.
    let name = |account: &str| account.trim_start_matches('~').to_string();
    let (credentials, account, verified) = if let Some(account) = &auth.hardware_account {
        (StatusCredentials::HardwareToken, Some(name(account)), false)
    } else if auth.logged_in {
        let account = auth.username.as_deref().map(name);
        let verified = account.is_some();
        (StatusCredentials::OAuth, account, verified)
    } else {
        (StatusCredentials::None, None, false)
    };

    let mut blocks = vec![Block::Heading("Authentication".into())];
    match (credentials, &account) {
        (StatusCredentials::HardwareToken, Some(account)) => {
            blocks.push(Block::Status(
                Level::Ok,
                vec![
                    Span::Text("Hardware-bound grant stored for ".into()),
                    Span::Name(format!("~{account}")),
                ],
            ));
            blocks.push(Block::Note(vec![Span::Text(
                "Not verified with Launchpad, which would need a touch of your security key."
                    .into(),
            )]));
        }
        (StatusCredentials::OAuth, Some(account)) => blocks.push(Block::Status(
            Level::Ok,
            vec![
                Span::Text("Logged in as ".into()),
                Span::Name(format!("~{account}")),
            ],
        )),
        (StatusCredentials::OAuth, None) | (StatusCredentials::HardwareToken, None) => {
            blocks.push(Block::Status(
                Level::Warning,
                vec![Span::Text(
                    "Credentials found (could not verify with server)".into(),
                )],
            ));
        }
        (StatusCredentials::None, _) => blocks.push(Block::Status(
            Level::Error,
            vec![
                Span::Text("Not logged in. Run ".into()),
                Span::Command("lpcli login".into()),
                Span::Text(" to authenticate.".into()),
            ],
        )),
    }

    blocks.push(Block::Break);
    blocks.push(Block::Heading("Launchpad API Server".into()));
    if server.reachable {
        blocks.push(Block::Status(Level::Ok, vec![Span::Text("Online".into())]));
        if let Some(link) = &server.resource_type_link {
            blocks.push(Block::Field("Endpoint", link.clone()));
        }
    } else {
        blocks.push(Block::Status(
            Level::Error,
            vec![Span::Text("Unreachable".into())],
        ));
        if let Some(code) = server.http_status {
            blocks.push(Block::Field("HTTP status", code.to_string()));
        }
        if let Some(error) = &server.error {
            blocks.push(Block::Field("Reason", error.clone()));
        }
    }

    let json = serde_json::json!({
        "lpcli": {
            "authentication": {
                "credentials": credentials.key(),
                "account": account,
                "verified": verified,
            },
            "server": {
                "reachable": server.reachable,
                "http_status": server.http_status,
                "resource_type_link": server.resource_type_link,
                "error": server.error,
            },
        }
    });
    let column = |key| Column::new(key, key);
    let records = Records {
        columns: [
            "credentials",
            "account",
            "verified",
            "reachable",
            "http_status",
            "resource_type_link",
            "error",
        ]
        .into_iter()
        .map(column)
        .collect(),
        rows: vec![vec![
            Some(credentials.key().to_string()),
            account,
            Some(verified.to_string()),
            Some(server.reachable.to_string()),
            server.http_status.map(|code| code.to_string()),
            server.resource_type_link.clone(),
            server.error.clone(),
        ]],
    };
    Report {
        blocks,
        json,
        records,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// CLI policy only: never change the library's signing or token renewal semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadAuthPolicy {
    Anonymous,

    Authenticated,
}

fn supports_public_reads(command: &Command) -> bool {
    matches!(
        command,
        Command::Bug(
            BugCommand::Show { .. }
                | BugCommand::Tasks { .. }
                | BugCommand::Comments { .. }
                | BugCommand::Search { .. }
                | BugCommand::Subscriptions { .. }
                | BugCommand::Attachments { .. }
                | BugCommand::Watches { .. }
                | BugCommand::RemoteBugTrackers { .. }
        ) | Command::Person(_)
            | Command::Package(
                PackageCommand::Series { .. }
                    | PackageCommand::ListSeries { .. }
                    | PackageCommand::Search { .. }
                    | PackageCommand::Ppa { .. }
                    | PackageCommand::PpaSources { .. }
                    | PackageCommand::Builds { .. }
                    | PackageCommand::Build {
                        command: BuildCommand::Show { .. } | BuildCommand::Logs { .. },
                    }
                    | PackageCommand::Distro { .. }
                    | PackageCommand::Download { .. }
                    | PackageCommand::Changelog { .. }
            )
            | Command::Project(_)
            | Command::Cve(_)
            | Command::Git(_)
            | Command::Spec(_)
            | Command::Question(_)
            | Command::Translation(_)
            | Command::Snap(
                SnapCommand::Show { .. } | SnapCommand::Find { .. } | SnapCommand::Builds { .. }
            )
            | Command::Queue(
                QueueCommand::Search { .. }
                    | QueueCommand::Info { .. }
                    | QueueCommand::Download { .. }
            )
    )
}

fn is_ubuntu_primary_queue_archive(distro: &str, archive: Option<&str>) -> bool {
    if !distro.eq_ignore_ascii_case("ubuntu") {
        return false;
    }
    let Some(archive) = archive else {
        return true;
    };
    let trimmed = archive.trim_matches('/');
    if trimmed.starts_with("ppa:") {
        return false;
    }
    trimmed == "primary"
        || trimmed == "ubuntu/+archive/primary"
        || trimmed == "https://api.launchpad.net/devel/ubuntu/+archive/primary"
}

fn read_auth_policy(command: &Command, authenticated: bool, public: bool) -> ReadAuthPolicy {
    use ReadAuthPolicy::{Anonymous, Authenticated};
    if !supports_public_reads(command) || authenticated {
        return Authenticated;
    }
    if public {
        return Anonymous;
    }
    match command {
        // These collections are tied to public distribution data, not a person's
        // private archives, bugs, repositories, projects or recipes.
        Command::Package(
            PackageCommand::Series { .. }
            | PackageCommand::ListSeries { .. }
            | PackageCommand::Search { .. }
            | PackageCommand::Distro { .. }
            | PackageCommand::Download { .. }
            | PackageCommand::Changelog { ppa: None, .. }
            | PackageCommand::Builds {
                owner: None,
                ppa: None,
                ..
            }
            | PackageCommand::Build {
                command:
                    BuildCommand::Show {
                        owner: None,
                        ppa: None,
                        build_url: None,
                        ..
                    }
                    | BuildCommand::Logs {
                        owner: None,
                        ppa: None,
                        build_url: None,
                        ..
                    },
            },
        )
        | Command::Translation(_)
        | Command::Bug(BugCommand::RemoteBugTrackers { .. }) => Anonymous,

        Command::Queue(QueueCommand::Search {
            distro,
            archive,
            ppa,
            ..
        }) if ppa.is_none() && is_ubuntu_primary_queue_archive(distro, archive.as_deref()) => {
            Anonymous
        }
        Command::Queue(
            QueueCommand::Info {
                distro,
                archive,
                ppa,
                ..
            }
            | QueueCommand::Download {
                distro,
                archive,
                ppa,
                ..
            },
        ) if ppa.is_none() && is_ubuntu_primary_queue_archive(distro, archive.as_deref()) => {
            Anonymous
        }

        // Private-capable reads require auth even when no credentials are stored.
        // Only explicit --public opts into potentially incomplete public results.
        _ => Authenticated,
    }
}

fn read_client(
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<LaunchpadClient> {
    read_client_with_auth(policy, || authenticated_client(cache_timeout))
}

fn read_client_with_auth(
    policy: ReadAuthPolicy,
    authenticate: impl FnOnce() -> lpcli::error::Result<LaunchpadClient>,
) -> lpcli::error::Result<LaunchpadClient> {
    match policy {
        ReadAuthPolicy::Authenticated => authenticate(),
        ReadAuthPolicy::Anonymous => Ok(LaunchpadClient::new(None)),
    }
}

/// Metadata may be signed, but arbitrary file URLs and redirects must not be.
#[cfg(test)]
async fn download_public_files(
    urls: &[String],
    output_dir: &std::path::Path,
) -> lpcli::error::Result<Vec<download::DownloadOutcome>> {
    download_public_files_with_progress(urls, output_dir, download::ProgressMode::None).await
}

/// Download public files with an explicit stderr progress reporting mode.
async fn download_public_files_with_progress(
    urls: &[String],
    output_dir: &std::path::Path,
    progress: download::ProgressMode,
) -> lpcli::error::Result<Vec<download::DownloadOutcome>> {
    download::download_files_with_progress(&LaunchpadClient::new(None), urls, output_dir, progress)
        .await
}

fn hardware_token_cache_timeout(settings: &config::AppConfig) -> std::time::Duration {
    std::time::Duration::from_secs(settings.authentication.hardware_token_cache_seconds)
}

/// Build an authenticated client from stored credentials.
///
/// A stored hardware-bound grant takes precedence over classic OAuth
/// credentials. Requests reuse valid cached tokens according to the configured
/// timeout; minting a fresh token requires a touch of the security key.
///
/// Returns `LpError::NotAuthenticated` if no credentials are found, or the
/// underlying error (e.g. `LpError::Config`) if the file exists but cannot
/// be read or parsed.
fn authenticated_client(
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<LaunchpadClient> {
    let session = hardware_tokens::load_session_with_cache(
        &GrantStore::from_env(),
        usb_authenticator(),
        cache_timeout,
    )
    .map_err(|e| match e {
        lpcli::error::LpError::HardwareToken(message) => lpcli::error::LpError::HardwareToken(
            format!("{message} Run `lpcli logout --hardware-token` to remove the stored grant."),
        ),
        other => other,
    })?;
    if let Some(session) = session {
        return Ok(LaunchpadClient::with_hardware_token(Arc::new(session)));
    }
    classic_authenticated_client()
}

/// Load only the classic OAuth session from the keyring or legacy file.
/// This never reads, replaces or deletes a stored hardware-bound grant.
fn classic_authenticated_client() -> lpcli::error::Result<LaunchpadClient> {
    let creds = auth::load_credentials()?;
    Ok(LaunchpadClient::new(Some(creds)))
}

/// The USB security key, prompting on the terminal unless `--no-input` is in
/// effect. Touch notices are written to stderr either way.
fn usb_authenticator() -> Arc<dyn Authenticator> {
    Arc::new(UsbAuthenticator::new(security_key_interaction(
        prompts_disabled(),
    )))
}

/// Build a [`comfy_table::Table`] with headers and the resolved display width.
#[cfg(test)]
fn build_table(display: &Display, headers: Vec<&str>) -> display::DisplayTable {
    display.table(headers)
}

// ---------------------------------------------------------------------------
// Bug watch and remote bug tracker display helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Output-layer reports
// ---------------------------------------------------------------------------

/// Commands whose results support the requested structured output mode.
fn supports_output_mode(command: &Command, mode: OutputMode) -> bool {
    match mode {
        OutputMode::Rich | OutputMode::Plain => true,
        OutputMode::Json => supports_json_output(command),
        OutputMode::Tsv => supports_tsv_output(command),
    }
}

/// Commands with JSON output. Domain ports extend this list.
fn supports_json_output(command: &Command) -> bool {
    match command {
        Command::Status | Command::Schema => true,
        Command::Bug(command) => bug_supports_json(command),
        Command::Person(_)
        | Command::Package(_)
        | Command::Project(_)
        | Command::Cve(_)
        | Command::Spec(_)
        | Command::Question(_)
        | Command::Translation(_) => read_domain_supports_json(command),
        Command::Git(command) => git_supports_json(command),
        Command::Queue(command) => queue_supports_json(command),
        Command::Webhook(_) | Command::Snap(_) | Command::AccessToken(_) => {
            webhook_snap_token_supports_json(command)
        }
        Command::Login(_) | Command::Logout(_) | Command::HardwareToken(_) => {
            identity_supports_json(command)
        }
        Command::Skills => false,
    }
}

/// Commands with one-record-per-line TSV output. Domain ports extend this list.
fn supports_tsv_output(command: &Command) -> bool {
    match command {
        Command::Status => true,
        Command::Bug(command) => bug_supports_tsv(command),
        Command::Person(_)
        | Command::Package(_)
        | Command::Project(_)
        | Command::Cve(_)
        | Command::Spec(_)
        | Command::Question(_)
        | Command::Translation(_) => read_domain_supports_tsv(command),
        Command::Git(command) => git_supports_tsv(command),
        Command::Queue(command) => queue_supports_tsv(command),
        Command::Webhook(_) | Command::Snap(_) | Command::AccessToken(_) => {
            webhook_snap_token_supports_tsv(command)
        }
        Command::Login(_) | Command::Logout(_) | Command::HardwareToken(_) => {
            identity_supports_tsv(command)
        }
        Command::Schema | Command::Skills => false,
    }
}

fn is_mutation(command: &Command) -> bool {
    !supports_public_reads(command)
        && !matches!(
            command,
            Command::Status
                | Command::Schema
                | Command::Skills
                | Command::Login(_)
                | Command::Logout(_)
                | Command::HardwareToken(
                    HardwareTokenCommand::Keys
                        | HardwareTokenCommand::Show
                        | HardwareTokenCommand::ClearCache
                )
                | Command::Webhook(WebhookCommand::List { .. } | WebhookCommand::Deliveries { .. })
                | Command::AccessToken(
                    AccessTokenCommand::List { .. } | AccessTokenCommand::ListGit { .. }
                )
        )
}

fn requires_prompt(command: &Command) -> bool {
    matches!(
        command,
        Command::Login(LoginArgs {
            hardware_token: false,
            ..
        }) | Command::HardwareToken(HardwareTokenCommand::Register { .. })
            | Command::Package(PackageCommand::Build {
                command: BuildCommand::Retry {
                    build_id: None,
                    build_url: None,
                    dry_run: false,
                    yes: false,
                    ..
                },
            })
    )
}

const JSON_CONTRACT_VERSION: u32 = 1;

fn schema_report() -> Report {
    Report {
        blocks: vec![
            Block::Field("JSON contract version", JSON_CONTRACT_VERSION.to_string()),
            Block::Field(
                "Documentation",
                "https://github.com/canonical/lpcli/blob/main/docs/json-output.md".into(),
            ),
        ],
        json: serde_json::json!({"lpcli": {
            "json_contract_version": JSON_CONTRACT_VERSION,
            "documentation": "https://github.com/canonical/lpcli/blob/main/docs/json-output.md",
        }}),
        records: Records {
            columns: Vec::new(),
            rows: Vec::new(),
        },
    }
}

/// Build the `bug search` report. The JSON is the Launchpad result page,
/// unmodified, including `next_collection_link` when `--limit` cut the
/// results short and `total_size_link` when Launchpad sends one.
fn bug_search_report(page: &Raw<Collection<bugs::BugTask>>) -> Report {
    let rows = page
        .value
        .entries
        .iter()
        .map(|task| {
            vec![
                // The bug number is the last path segment of bug_link, e.g.
                // "https://api.launchpad.net/devel/bugs/12345" -> "12345".
                task.bug_link
                    .as_deref()
                    .and_then(|url| url.rsplit('/').next())
                    .filter(|id| !id.is_empty())
                    .map(str::to_string),
                task.title.clone(),
                task.status.clone(),
                task.bug_target_display_name.clone(),
            ]
        })
        .collect();
    Report {
        blocks: vec![Block::List {
            noun: Noun {
                singular: "bug",
                plural: "bugs",
            },
            empty: "No bugs found.".into(),
            hint: None,
        }],
        json: page.raw.clone(),
        records: Records {
            columns: vec![
                Column::new("ID", "id"),
                Column::new("Title", "title"),
                Column::new("Status", "status"),
                Column::new("Target", "bug_target_display_name"),
            ],
            rows,
        },
    }
}

/// Build the `bug remote-bug-trackers` report.
///
/// The JSON uses Launchpad's collection shape with every tracker exactly as
/// Launchpad returned it, in Launchpad's order. The search runs locally, so
/// no Launchpad link can continue the filtered list: `next_collection_link`
/// is never included, and `total_size` (the number of matches) only when
/// every page was scanned.
fn remote_bug_trackers_report(found: &bugs::BugTrackerSearch, searched: bool) -> Report {
    let entries: Vec<serde_json::Value> = found
        .trackers
        .iter()
        .map(|tracker| tracker.raw.clone())
        .collect();
    let mut json = serde_json::Map::new();
    if found.complete {
        json.insert("total_size".into(), entries.len().into());
    }
    json.insert("start".into(), 0.into());
    json.insert("entries".into(), entries.into());

    let rows = found
        .trackers
        .iter()
        .map(|tracker| {
            let tracker = &tracker.value;
            vec![
                tracker.name.clone(),
                tracker.title.clone(),
                tracker.bug_tracker_type.clone(),
                tracker.base_url.clone(),
            ]
        })
        .collect();
    let empty = if searched {
        "No remote bug trackers matched the search."
    } else {
        "No remote bug trackers found."
    };
    Report {
        blocks: vec![Block::List {
            noun: Noun {
                singular: "remote bug tracker",
                plural: "remote bug trackers",
            },
            empty: empty.into(),
            hint: Some("Use the Name value with --bug-tracker.".into()),
        }],
        json: json.into(),
        records: Records {
            columns: vec![
                Column::new("Name", "name"),
                Column::new("Title", "title"),
                Column::new("Type", "bug_tracker_type"),
                Column::new("Base URL", "base_url"),
            ],
            rows,
        },
    }
}

/// Best-effort display name for a bug task in watch-link listings.
fn bug_task_display_name(task: &bugs::BugTask) -> String {
    task.bug_target_display_name
        .clone()
        .or_else(|| task.target_link.clone())
        .or_else(|| task.self_link.clone())
        .unwrap_or_else(|| "unknown task".to_string())
}

/// Map each bug watch ID to the display names of the bug tasks linked to it.
///
/// Several tasks can share one watch (e.g. an Ubuntu series task and the
/// upstream project task), so each watch ID maps to a set of task names.
/// Tasks without a `bug_watch_link` are ignored. Matching is done on the
/// watch ID (the final path segment of the task's `bug_watch_link`), which
/// safely normalizes the API namespace, so links under any API version
/// (e.g. `/devel/`, `/beta/`) match the same watch.
fn collect_watch_task_links(tasks: &[bugs::BugTask]) -> BTreeMap<String, BTreeSet<String>> {
    let mut links: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for task in tasks {
        let Some(watch_id) = task.bug_watch_id() else {
            continue;
        };
        links
            .entry(watch_id.to_string())
            .or_default()
            .insert(bug_task_display_name(task));
    }
    links
}

/// Render the "Linked Tasks" cell for a row of `bug watches` output.
///
/// * A watch that tasks link to shows the display names of *all* linked
///   task targets, joined with ", ".
/// * A watch that no task links to shows "orphan".
/// * A watch without a usable ID shows "unknown", because its linkage
///   cannot be determined.
fn watch_linked_tasks_cell(
    watch_id: Option<&str>,
    links: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    let Some(id) = watch_id else {
        return "unknown".to_string();
    };
    links
        .get(id)
        .map(|names| {
            names
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "orphan".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "lpcli/tests.rs"]
mod tests;
