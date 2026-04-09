//! lpcli — command-line interface for Launchpad.net
//!
//! Parse command-line arguments with `clap` and dispatch to the `lpcli` library.

use clap::{ArgGroup, Parser, Subcommand};
use colored::Colorize;
use comfy_table::{presets::UTF8_FULL_CONDENSED, Table};

use lpcli::{
    access_tokens,
    auth,
    bugs::{self, BugSearchParams},
    client::LaunchpadClient,
    cves::{self, CveSearchParams},
    git,
    packages::{self, SourceSearchParams},
    people,
    projects,
    questions::{self, QuestionSearchParams},
    snaps,
    specifications,
    status,
    translations,
    webhooks,
};

// ---------------------------------------------------------------------------
// Top-level CLI structure
// ---------------------------------------------------------------------------

/// lpcli — A command-line client for Launchpad.net
#[derive(Debug, Parser)]
#[command(
    name = "lpcli",
    version,
    about = "Interact with Launchpad.net from the command line",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check authentication status and verify the Launchpad server is reachable.
    Status,

    /// Authenticate with Launchpad (OAuth login).
    Login,

    /// Log out from Launchpad and remove stored credentials.
    Logout,

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

    /// Manage personal access tokens for projects and Git repositories.
    #[command(subcommand)]
    AccessToken(AccessTokenCommand),
}

// ---------------------------------------------------------------------------
// Bug sub-commands
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
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
    /// Add a comment to a bug.
    Comment {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Comment text.
        #[arg(short, long)]
        message: String,
    },
    /// List comments on a bug.
    Comments {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
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
    },
    /// Change the status of a bug task.
    #[command(
        group(ArgGroup::new("target_spec")
            .required(true)
            .args(["target", "many_targets", "all_targets"])),
        group(ArgGroup::new("series_spec")
            .required(true)
            .args(["series", "many_series", "all_series"])),
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
        /// Mutually exclusive with --many-series and --all-series.
        #[arg(long)]
        series: Option<String>,
        /// Comma-separated list of Ubuntu series (e.g. "noble, jammy").
        /// Mutually exclusive with --series and --all-series.
        #[arg(long)]
        many_series: Option<String>,
        /// Apply to every Ubuntu series present in the bug's current tasks.
        /// Mutually exclusive with --series and --many-series.
        #[arg(long)]
        all_series: bool,
        /// New status (e.g. "Confirmed", "Fix Released", "In Progress").
        #[arg(short, long)]
        status: String,
    },
    /// Change the importance of a bug task.
    #[command(
        group(ArgGroup::new("target_spec")
            .required(true)
            .args(["target", "many_targets", "all_targets"])),
        group(ArgGroup::new("series_spec")
            .required(true)
            .args(["series", "many_series", "all_series"])),
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
        /// Mutually exclusive with --many-series and --all-series.
        #[arg(long)]
        series: Option<String>,
        /// Comma-separated list of Ubuntu series (e.g. "noble, jammy").
        /// Mutually exclusive with --series and --all-series.
        #[arg(long)]
        many_series: Option<String>,
        /// Apply to every Ubuntu series present in the bug's current tasks.
        /// Mutually exclusive with --series and --many-series.
        #[arg(long)]
        all_series: bool,
        /// New importance (e.g. "Critical", "High", "Medium", "Low").
        #[arg(short, long)]
        importance: String,
    },
    /// Assign a bug task to a Launchpad user.
    #[command(
        group(ArgGroup::new("target_spec")
            .required(true)
            .args(["target", "many_targets", "all_targets"])),
        group(ArgGroup::new("series_spec")
            .required(true)
            .args(["series", "many_series", "all_series"])),
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
        /// Mutually exclusive with --many-series and --all-series.
        #[arg(long)]
        series: Option<String>,
        /// Comma-separated list of Ubuntu series (e.g. "noble, jammy").
        /// Mutually exclusive with --series and --all-series.
        #[arg(long)]
        many_series: Option<String>,
        /// Apply to every Ubuntu series present in the bug's current tasks.
        /// Mutually exclusive with --series and --many-series.
        #[arg(long)]
        all_series: bool,
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
        /// Task importance (e.g. "Undecided", "Critical", "High", "Medium", "Low").
        #[arg(long)]
        importance: Option<String>,
        /// Task status (e.g. "New", "Confirmed", "In Progress").
        #[arg(long)]
        status: Option<String>,
        /// Launchpad username to assign (without ~).
        #[arg(long)]
        assignee: Option<String>,
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
}

// ---------------------------------------------------------------------------
// Person sub-commands
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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
        #[arg(short, long)]
        pocket: Option<String>,
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
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
    /// Show info about a distribution.
    Distro {
        /// Distribution name (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
    },
}

// ---------------------------------------------------------------------------
// Project sub-commands
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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
        #[arg(short, long)]
        distro: Option<String>,
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

#[derive(Debug, Subcommand)]
enum GitCommand {
    /// Show a Git repository by path (~owner/project/+git/name).
    Show {
        /// Repository path (e.g. "~user/project/+git/repo").
        #[arg(short, long)]
        path: String,
    },
    /// Show the default Git repository for a target project or distribution.
    Default {
        /// Project or distribution name.
        #[arg(short, long)]
        target: String,
    },
    /// List Git repositories owned by a person.
    ListPersonRepos {
        /// Launchpad person name (without ~).
        #[arg(short, long)]
        name: String,
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
}

// ---------------------------------------------------------------------------
// Spec sub-commands
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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

#[derive(Debug, Subcommand)]
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
        #[arg(short, long, default_value = "Release")]
        pocket: String,
        /// Distribution to build against (default: "ubuntu").
        #[arg(short, long, default_value = "ubuntu")]
        distro: String,
    },
}

// ---------------------------------------------------------------------------
// AccessToken sub-commands
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
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
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{} {e}", "error:".red().bold());
        std::process::exit(1);
    }
}

async fn run() -> lpcli::error::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Status => handle_status().await?,

        Command::Login => {
            println!("{}", "Logging in to Launchpad...".bold());
            let creds = auth::login().await?;
            println!(
                "{} Stored credentials for token '{}'.",
                "✓".green().bold(),
                creds.access_token.token
            );
        }

        Command::Logout => {
            auth::logout()?;
        }

        Command::Bug(cmd) => handle_bug(cmd).await?,
        Command::Person(cmd) => handle_person(cmd).await?,
        Command::Package(cmd) => handle_package(cmd).await?,
        Command::Project(cmd) => handle_project(cmd).await?,
        Command::Cve(cmd) => handle_cve(cmd).await?,
        Command::Git(cmd) => handle_git(cmd).await?,
        Command::Spec(cmd) => handle_spec(cmd).await?,
        Command::Question(cmd) => handle_question(cmd).await?,
        Command::Webhook(cmd) => handle_webhook(cmd).await?,
        Command::Translation(cmd) => handle_translation(cmd).await?,
        Command::Snap(cmd) => handle_snap(cmd).await?,
        Command::AccessToken(cmd) => handle_access_token(cmd).await?,
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Bug handlers
// ---------------------------------------------------------------------------

async fn handle_bug(cmd: BugCommand) -> lpcli::error::Result<()> {
    let client = authenticated_client()?;

    match cmd {
        BugCommand::Show { bug_id } => {
            let bug = bugs::get_bug(&client, bug_id).await?;
            println!("{}", format!("Bug #{}", bug.id).bold());
            println!("{}", "─".repeat(60));
            println!("{}", bug.title.bold());
            if let Some(desc) = &bug.description {
                println!("\n{desc}");
            }
            println!();
            if !bug.tags.is_empty() {
                println!("Tags:    {}", bug.tags.join(", ").cyan());
            }
            if let Some(count) = bug.users_affected_count {
                println!("Affects: {count} user(s)");
            }
            if let Some(link) = &bug.web_link {
                println!("URL:     {}", link.underline());
            }
        }

        BugCommand::Tasks { bug_id } => {
            let tasks = bugs::get_bug_tasks(&client, bug_id).await?;

            // Parse every task into (target_name, series, task) and collect
            // into a BTreeMap keyed by target_name so tasks are grouped by
            // target and ordered deterministically.
            struct ParsedTask<'a> {
                series: Option<String>,
                task: &'a bugs::BugTask,
            }
            let mut by_target: std::collections::BTreeMap<String, Vec<ParsedTask>> =
                std::collections::BTreeMap::new();
            for t in &tasks {
                let (target_name, series) =
                    bugs::parse_target_link(t.target_link.as_deref().unwrap_or(""));
                let key = if !target_name.is_empty() {
                    target_name
                } else {
                    t.bug_target_display_name.as_deref().unwrap_or("—").to_string()
                };
                by_target
                    .entry(key)
                    .or_default()
                    .push(ParsedTask { series, task: t });
            }

            // Within each target group: the series-less task first, then
            // series tasks ordered alphabetically.
            for tasks_for_target in by_target.values_mut() {
                tasks_for_target.sort_by(|a, b| match (&a.series, &b.series) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (Some(sa), Some(sb)) => sa.cmp(sb),
                });
            }

            let mut table = build_table(vec!["Target", "Series", "Status", "Importance", "Assignee"]);
            for (target_name, tasks_for_target) in &by_target {
                for pt in tasks_for_target {
                    // Tasks with a series are indented; base tasks are not.
                    let display_target = match &pt.series {
                        None => target_name.clone(),
                        Some(_) => format!("    {target_name}"),
                    };
                    let series_str = pt.series.as_deref().unwrap_or("").to_string();

                    // Show only the Launchpad username rather than the full API URL.
                    let assignee_str = pt
                        .task
                        .assignee_link
                        .as_deref()
                        .and_then(|url| {
                            url.rsplit('/').next().map(|seg| seg.trim_start_matches('~').to_string())
                        })
                        .unwrap_or_else(|| "—".to_string());

                    table.add_row(vec![
                        display_target,
                        series_str,
                        pt.task.status.as_deref().unwrap_or("—").to_string(),
                        pt.task.importance.as_deref().unwrap_or("—").to_string(),
                        assignee_str,
                    ]);
                }
            }
            println!("{table}");
        }

        BugCommand::Search {
            target,
            status,
            importance,
            tag,
            package,
            keyword,
            limit,
        } => {
            let params = BugSearchParams {
                status: status.as_deref(),
                importance: importance.as_deref(),
                tag: tag.as_deref(),
                package_name: package.as_deref(),
                search_text: keyword.as_deref(),
                limit: Some(limit),
                ..Default::default()
            };
            let tasks = bugs::search_bugs(&client, &target, &params).await?;
            let mut table = build_table(vec!["ID", "Title", "Status", "Target"]);
            for task in &tasks {
                // The bug number is the last path segment of bug_link, e.g.
                // "https://api.launchpad.net/devel/bugs/12345" → "12345".
                let bug_id = task
                    .bug_link
                    .as_deref()
                    .and_then(|url| url.rsplit('/').next())
                    .unwrap_or("—")
                    .to_string();
                table.add_row(vec![
                    bug_id,
                    truncate(task.title.as_deref().unwrap_or("—"), 55),
                    task.status.as_deref().unwrap_or("—").to_string(),
                    task.bug_target_display_name
                        .as_deref()
                        .unwrap_or("—")
                        .to_string(),
                ]);
            }
            println!("{table}");
            println!("{} bug(s) found.", tasks.len());
        }

        BugCommand::Comment { bug_id, message } => {
            bugs::add_bug_comment(&client, bug_id, &message).await?;
            println!("{} Comment added to bug #{bug_id}.", "✓".green().bold());
        }

        BugCommand::Comments { bug_id } => {
            let comments = bugs::get_bug_comments(&client, bug_id).await?;
            for (i, c) in comments.iter().enumerate() {
                let idx = c.index.unwrap_or(i as u64);
                let author = c.owner_link.as_deref().unwrap_or("unknown");
                let date = c
                    .date_created
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                println!("{}", format!("Comment #{idx} — {author} — {date}").bold());
                println!("{}", c.content.as_deref().unwrap_or(""));
                println!("{}", "─".repeat(60));
            }
        }

        BugCommand::Create {
            target,
            package,
            title,
            description,
        } => {
            let effective_target = match &package {
                Some(pkg) => format!("{target}/+source/{pkg}"),
                None => target.clone(),
            };
            let bug =
                bugs::create_bug(&client, &effective_target, &title, &description).await?;
            println!("{} Bug #{} created.", "✓".green().bold(), bug.id);
            if let Some(link) = &bug.web_link {
                println!("URL: {}", link.underline());
            }
        }

        BugCommand::SetStatus {
            bug_id,
            target,
            many_targets,
            all_targets,
            series,
            many_series,
            all_series,
            status,
        } => {
            let target_filter = TargetFilter::from_args(target, many_targets, all_targets);
            let series_filter = SeriesFilter::from_args(series, many_series, all_series);
            let tasks = bugs::get_bug_tasks(&client, bug_id).await?;
            let matched =
                collect_matching_tasks(&tasks, bug_id, &target_filter, &series_filter)?;
            for task in &matched {
                let task_url = task.self_link.as_deref().ok_or_else(|| {
                    lpcli::error::LpError::Other("Bug task has no self_link".into())
                })?;
                bugs::set_bug_status(&client, task_url, &status).await?;
            }
            println!(
                "{} Status updated to '{}' for {} task(s) on bug #{bug_id}.",
                "✓".green().bold(),
                status,
                matched.len(),
            );
        }

        BugCommand::SetImportance {
            bug_id,
            target,
            many_targets,
            all_targets,
            series,
            many_series,
            all_series,
            importance,
        } => {
            let target_filter = TargetFilter::from_args(target, many_targets, all_targets);
            let series_filter = SeriesFilter::from_args(series, many_series, all_series);
            let tasks = bugs::get_bug_tasks(&client, bug_id).await?;
            let matched =
                collect_matching_tasks(&tasks, bug_id, &target_filter, &series_filter)?;
            for task in &matched {
                let task_url = task.self_link.as_deref().ok_or_else(|| {
                    lpcli::error::LpError::Other("Bug task has no self_link".into())
                })?;
                bugs::set_bug_importance(&client, task_url, &importance).await?;
            }
            println!(
                "{} Importance updated to '{}' for {} task(s) on bug #{bug_id}.",
                "✓".green().bold(),
                importance,
                matched.len(),
            );
        }

        BugCommand::SetAssignee {
            bug_id,
            target,
            many_targets,
            all_targets,
            series,
            many_series,
            all_series,
            name,
        } => {
            let target_filter = TargetFilter::from_args(target, many_targets, all_targets);
            let series_filter = SeriesFilter::from_args(series, many_series, all_series);
            let assignee_url = client.url(&format!("/~{name}"));
            let tasks = bugs::get_bug_tasks(&client, bug_id).await?;
            let matched =
                collect_matching_tasks(&tasks, bug_id, &target_filter, &series_filter)?;
            for task in &matched {
                let task_url = task.self_link.as_deref().ok_or_else(|| {
                    lpcli::error::LpError::Other("Bug task has no self_link".into())
                })?;
                bugs::set_bug_assignee(&client, task_url, &assignee_url).await?;
            }
            println!(
                "{} Assigned {} task(s) on bug #{bug_id} to ~{name}.",
                "✓".green().bold(),
                matched.len(),
            );
        }

        BugCommand::Subscribe { bug_id, name } => {
            let person_url = client.url(&format!("/~{name}"));
            let sub = bugs::subscribe_to_bug(&client, bug_id, &person_url).await?;
            println!(
                "{} Subscribed ~{name} to bug #{bug_id}.",
                "✓".green().bold()
            );
            if let Some(link) = &sub.self_link {
                println!("Subscription: {link}");
            }
        }

        BugCommand::Unsubscribe { bug_id, name } => {
            let person_url = client.url(&format!("/~{name}"));
            bugs::unsubscribe_from_bug(&client, bug_id, &person_url).await?;
            println!(
                "{} Unsubscribed ~{name} from bug #{bug_id}.",
                "✓".green().bold()
            );
        }

        BugCommand::Subscriptions { bug_id } => {
            let subs = bugs::get_bug_subscriptions(&client, bug_id).await?;
            let mut table = build_table(vec!["Person", "Subscribed By", "Date"]);
            for s in &subs {
                let person = s
                    .person_link
                    .as_deref()
                    .and_then(|u| u.rsplit('/').next())
                    .unwrap_or("—");
                let by = s
                    .subscribed_by_link
                    .as_deref()
                    .and_then(|u| u.rsplit('/').next())
                    .unwrap_or("—");
                let date = s
                    .date_created
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                table.add_row(vec![person, by, &date]);
            }
            println!("{table}");
        }

        BugCommand::AddTask {
            bug_id,
            target,
            package,
            series,
            many_series,
            importance,
            status,
            assignee,
        } => {
            // Build the list of target URLs to add tasks for.  When no series
            // is supplied we add a single task directly against the base target
            // (i.e. the distribution or project without a series component).
            enum TaskTarget {
                Base,
                Series(Vec<String>),
            }
            let task_target = match (series, many_series) {
                (Some(s), _) => TaskTarget::Series(vec![s]),
                (None, Some(many)) => TaskTarget::Series(parse_comma_separated(&many)),
                (None, None) => TaskTarget::Base,
            };

            // Resolve to a list of (label, url) pairs.
            let targets: Vec<(String, String)> = match task_target {
                TaskTarget::Base => {
                    let path = if let Some(pkg) = &package {
                        format!("/{target}/+source/{pkg}")
                    } else {
                        format!("/{target}")
                    };
                    vec![(target.clone(), client.url(&path))]
                }
                TaskTarget::Series(series_list) => series_list
                    .into_iter()
                    .map(|s| {
                        let path = if let Some(pkg) = &package {
                            format!("/{target}/{s}/+source/{pkg}")
                        } else {
                            format!("/{target}/{s}")
                        };
                        let url = client.url(&path);
                        (s, url)
                    })
                    .collect(),
            };

            let mut created = 0usize;
            for (series_name, target_url) in &targets {
                let task = match bugs::add_bug_task(&client, bug_id, target_url).await {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!(
                            "{} Failed to add task for series '{}' on bug #{bug_id}: {e}",
                            "✗".red().bold(),
                            series_name,
                        );
                        continue;
                    }
                };

                let task_url = match task.self_link.as_deref() {
                    Some(url) => url.to_string(),
                    None => {
                        eprintln!(
                            "{} Task for series '{}' has no self_link; skipping attribute updates.",
                            "!".yellow().bold(),
                            series_name,
                        );
                        created += 1;
                        continue;
                    }
                };

                if let Some(imp) = &importance {
                    if let Err(e) = bugs::set_bug_importance(&client, &task_url, imp).await {
                        eprintln!(
                            "{} Failed to set importance on task for '{}': {e}",
                            "!".yellow().bold(),
                            series_name,
                        );
                    }
                }
                if let Some(st) = &status {
                    if let Err(e) = bugs::set_bug_status(&client, &task_url, st).await {
                        eprintln!(
                            "{} Failed to set status on task for '{}': {e}",
                            "!".yellow().bold(),
                            series_name,
                        );
                    }
                }
                if let Some(assignee_name) = &assignee {
                    let assignee_api_url = client.url(&format!("/~{assignee_name}"));
                    if let Err(e) =
                        bugs::set_bug_assignee(&client, &task_url, &assignee_api_url).await
                    {
                        eprintln!(
                            "{} Failed to set assignee on task for '{}': {e}",
                            "!".yellow().bold(),
                            series_name,
                        );
                    }
                }

                let display_name = task
                    .bug_target_display_name
                    .as_deref()
                    .unwrap_or(series_name.as_str());
                println!(
                    "{} Bug task added: {}",
                    "✓".green().bold(),
                    display_name,
                );
                created += 1;
            }

            if created == 0 {
                return Err(lpcli::error::LpError::Other(format!(
                    "No bug tasks were created on bug #{bug_id}."
                )));
            }
        }

        BugCommand::DeleteTask {
            bug_id,
            target,
            package,
            series,
            many_series,
        } => {
            // The effective target name is the package name when a package is
            // provided, otherwise the distribution/project name — matching how
            // parse_target_link returns the package name (not the distro) for
            // source-package tasks.
            let effective_target = match &package {
                Some(pkg) => pkg.clone(),
                None => target.clone(),
            };
            let tasks = bugs::get_bug_tasks(&client, bug_id).await?;

            // When no series is given, delete every task that matches the
            // target regardless of series.  Otherwise apply the series filter.
            let matched: Vec<&bugs::BugTask> = if series.is_none() && many_series.is_none() {
                let parsed: Vec<(&bugs::BugTask, String, Option<String>)> = tasks
                    .iter()
                    .map(|t| {
                        let (tgt, ser) =
                            bugs::parse_target_link(t.target_link.as_deref().unwrap_or(""));
                        (t, tgt, ser)
                    })
                    .collect();
                let result: Vec<&bugs::BugTask> = parsed
                    .iter()
                    .filter(|(_, tgt, _)| tgt.eq_ignore_ascii_case(&effective_target))
                    .map(|(t, _, _)| *t)
                    .collect();
                if result.is_empty() {
                    return Err(lpcli::error::LpError::NotFound(format!(
                        "No tasks found for target '{}' on bug #{bug_id}.",
                        effective_target,
                    )));
                }
                result
            } else {
                let series_filter = match (series, many_series) {
                    (Some(s), _) => SeriesFilter::One(s),
                    (None, Some(many)) => SeriesFilter::Many(parse_comma_separated(&many)),
                    (None, None) => unreachable!(),
                };
                let target_filter = TargetFilter::One(effective_target);
                collect_matching_tasks(&tasks, bug_id, &target_filter, &series_filter)?
            };

            let mut deleted = 0usize;
            let mut errors = 0usize;
            for task in &matched {
                let task_url = task.self_link.as_deref().ok_or_else(|| {
                    lpcli::error::LpError::Other("Bug task has no self_link".into())
                })?;
                let display_name = task
                    .bug_target_display_name
                    .as_deref()
                    .unwrap_or("(unknown)");
                match bugs::delete_bug_task(&client, task_url).await {
                    Ok(()) => {
                        println!(
                            "{} Deleted bug task: {}",
                            "✓".green().bold(),
                            display_name,
                        );
                        deleted += 1;
                    }
                    Err(e) => {
                        eprintln!(
                            "{} Failed to delete task '{}': {e}",
                            "✗".red().bold(),
                            display_name,
                        );
                        errors += 1;
                    }
                }
            }

            println!(
                "\n{} Deleted {deleted} task(s) on bug #{bug_id}{}.",
                "✓".green().bold(),
                if errors > 0 {
                    format!(", {} error(s)", errors)
                } else {
                    String::new()
                },
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Person handlers
// ---------------------------------------------------------------------------

async fn handle_person(cmd: PersonCommand) -> lpcli::error::Result<()> {
    let client = authenticated_client()?;

    match cmd {
        PersonCommand::Show { name } => {
            let person = people::get_person(&client, &name).await?;
            println!("{}", person.display_name.bold());
            println!("Name:    ~{}", person.name);
            if let Some(kind) = &person.account_type {
                println!("Type:    {kind}");
            }
            if let Some(karma) = person.karma {
                println!("Karma:   {karma}");
            }
            if let Some(desc) = &person.description {
                println!("\n{desc}");
            }
            if let Some(link) = &person.web_link {
                println!("\nURL:     {}", link.underline());
            }
        }

        PersonCommand::Search { query } => {
            let results = people::search_people(&client, &query).await?;
            let mut table = build_table(vec!["Name", "Display Name", "Type"]);
            for p in &results {
                table.add_row(vec![
                    format!("~{}", p.name),
                    p.display_name.clone(),
                    p.account_type
                        .as_ref()
                        .map(|t| t.to_string())
                        .unwrap_or_default(),
                ]);
            }
            println!("{table}");
        }

        PersonCommand::Members { team } => {
            let members = people::get_team_members(&client, &team).await?;
            let mut table = build_table(vec!["Member", "Status", "Since"]);
            for m in &members {
                // Extract "~username" from the full API URL (e.g. ".../~jdoe" → "~jdoe").
                let member_name = m
                    .member_link
                    .as_deref()
                    .and_then(|url| url.rsplit('/').next())
                    .unwrap_or("—");
                let date = m
                    .date_joined
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                table.add_row(vec![
                    member_name,
                    m.status.as_deref().unwrap_or("—"),
                    &date,
                ]);
            }
            println!("{table}");
        }

        PersonCommand::Bugs { name } => {
            let bugs_list = people::get_person_bugs(&client, &name).await?;
            let mut table = build_table(vec!["Bug", "Title", "Status", "Importance"]);
            for b in &bugs_list {
                let bug_id = b
                    .bug_link
                    .as_deref()
                    .and_then(|url| url.rsplit('/').next())
                    .unwrap_or("—");
                table.add_row(vec![
                    bug_id.to_string(),
                    truncate(b.title.as_deref().unwrap_or("—"), 55),
                    b.status.as_deref().unwrap_or("—").to_string(),
                    b.importance.as_deref().unwrap_or("—").to_string(),
                ]);
            }
            println!("{table}");
            println!("{} bug task(s) found.", bugs_list.len());
        }

        PersonCommand::Ppas { name } => {
            let ppas = people::list_person_ppas(&client, &name).await?;
            let mut table = build_table(vec!["Name", "Description", "Enabled"]);
            for p in &ppas {
                table.add_row(vec![
                    p.name.as_deref().unwrap_or("—"),
                    p.description.as_deref().unwrap_or(""),
                    &p.enabled.map(|e| e.to_string()).unwrap_or_default(),
                ]);
            }
            println!("{table}");
        }

        PersonCommand::OwnedTeams { name } => {
            let teams = people::get_person_owned_teams(&client, &name).await?;
            let mut table = build_table(vec!["Name", "Display Name"]);
            for t in &teams {
                table.add_row(vec![
                    format!("~{}", t.name),
                    t.display_name.clone(),
                ]);
            }
            println!("{table}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Package handlers
// ---------------------------------------------------------------------------

async fn handle_package(cmd: PackageCommand) -> lpcli::error::Result<()> {
    let client = LaunchpadClient::new(None); // packages can be queried anonymously

    match cmd {
        PackageCommand::Series { series, distro } => {
            let s = packages::get_distro_series(&client, &distro, &series).await?;
            println!("{}", s.display_name.as_deref().unwrap_or(&s.name).bold());
            if let Some(v) = &s.version {
                println!("Version: {v}");
            }
            if let Some(status) = &s.status {
                println!("Status:  {status}");
            }
            if let Some(active) = s.active {
                println!("Active:  {active}");
            }
        }

        PackageCommand::ListSeries { distro } => {
            let series = packages::list_distro_series(&client, &distro).await?;
            let mut table = build_table(vec!["Name", "Version", "Status", "Active"]);
            for s in &series {
                table.add_row(vec![
                    s.name.as_str(),
                    s.version.as_deref().unwrap_or("—"),
                    s.status.as_deref().unwrap_or("—"),
                    &s.active.map(|a| a.to_string()).unwrap_or_default(),
                ]);
            }
            println!("{table}");
        }

        PackageCommand::Search {
            series,
            name,
            version,
            pocket,
            distro,
        } => {
            let params = SourceSearchParams {
                source_name: name.as_deref(),
                version: version.as_deref(),
                pocket: pocket.as_deref(),
                status: Some("Published"),
            };
            let pubs =
                packages::search_published_sources(&client, &distro, &series, &params).await?;
            let mut table = build_table(vec!["Package", "Version", "Component", "Pocket"]);
            for p in &pubs {
                table.add_row(vec![
                    p.source_package_name.as_deref().unwrap_or("—"),
                    p.source_package_version.as_deref().unwrap_or("—"),
                    p.component_name.as_deref().unwrap_or("—"),
                    p.pocket.as_deref().unwrap_or("—"),
                ]);
            }
            println!("{table}");
        }

        PackageCommand::Ppa { owner, ppa } => {
            let archive = packages::get_ppa(&client, &owner, &ppa).await?;
            println!(
                "{}",
                archive
                    .name
                    .as_deref()
                    .unwrap_or("PPA")
                    .bold()
            );
            if let Some(desc) = &archive.description {
                println!("{desc}");
            }
            if let Some(enabled) = archive.enabled {
                println!("Enabled: {enabled}");
            }
            if let Some(link) = &archive.web_link {
                println!("URL:     {}", link.underline());
            }
        }

        PackageCommand::PpaSources { owner, ppa, name } => {
            let params = SourceSearchParams {
                source_name: name.as_deref(),
                ..Default::default()
            };
            let pubs = packages::list_ppa_sources(&client, &owner, &ppa, &params).await?;
            let mut table = build_table(vec!["Package", "Version", "Status"]);
            for p in &pubs {
                table.add_row(vec![
                    p.source_package_name.as_deref().unwrap_or("—"),
                    p.source_package_version.as_deref().unwrap_or("—"),
                    p.status.as_deref().unwrap_or("—"),
                ]);
            }
            println!("{table}");
        }

        PackageCommand::Distro { distro } => {
            let d = packages::get_distro(&client, &distro).await?;
            println!("{}", d.display_name.as_deref().unwrap_or(&d.name).bold());
            if let Some(title) = &d.title {
                println!("Title:    {title}");
            }
            if let Some(summary) = &d.summary {
                println!("{summary}");
            }
            if let Some(official) = d.official_packages {
                println!("Official: {official}");
            }
            if let Some(link) = &d.web_link {
                println!("URL:      {}", link.underline());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Project handlers
// ---------------------------------------------------------------------------

async fn handle_project(cmd: ProjectCommand) -> lpcli::error::Result<()> {
    let client = LaunchpadClient::new(None);

    match cmd {
        ProjectCommand::Show { name } => {
            let project = projects::get_project(&client, &name).await?;
            println!(
                "{}",
                project.display_name.as_deref().unwrap_or(&project.name).bold()
            );
            if let Some(summary) = &project.summary {
                println!("{summary}");
            }
            if let Some(url) = &project.homepage_url {
                println!("Website: {}", url.underline());
            }
            if let Some(link) = &project.web_link {
                println!("Launchpad: {}", link.underline());
            }
        }

        ProjectCommand::Search { query } => {
            let results = projects::search_projects(&client, &query).await?;
            let mut table = build_table(vec!["Name", "Display Name", "Summary"]);
            for p in &results {
                table.add_row(vec![
                    p.name.as_str(),
                    p.display_name.as_deref().unwrap_or("—"),
                    &truncate(p.summary.as_deref().unwrap_or(""), 50),
                ]);
            }
            println!("{table}");
        }

        ProjectCommand::Milestones { project, active } => {
            let milestones = if active {
                projects::list_active_milestones(&client, &project).await?
            } else {
                projects::list_milestones(&client, &project).await?
            };
            let mut table = build_table(vec!["Name", "Title", "Active", "Target Date"]);
            for m in &milestones {
                table.add_row(vec![
                    m.name.as_str(),
                    m.title.as_deref().unwrap_or("—"),
                    &m.is_active.map(|a| a.to_string()).unwrap_or_default(),
                    &m.date_targeted
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| "—".to_string()),
                ]);
            }
            println!("{table}");
        }

        ProjectCommand::ShowMilestone { project, name } => {
            let m = projects::get_milestone(&client, &project, &name).await?;
            println!("{}", m.name.bold());
            if let Some(title) = &m.title {
                println!("Title:   {title}");
            }
            if let Some(active) = m.is_active {
                println!("Active:  {active}");
            }
            if let Some(date) = m.date_targeted {
                println!("Target:  {date}");
            }
            if let Some(link) = &m.web_link {
                println!("URL:     {}", link.underline());
            }
        }

        ProjectCommand::SeriesShow { project, series } => {
            let s = projects::get_project_series(&client, &project, &series).await?;
            println!("{}", s.name.bold());
            if let Some(title) = &s.title {
                println!("Title:   {title}");
            }
            if let Some(status) = &s.status {
                println!("Status:  {status}");
            }
            if let Some(summary) = &s.summary {
                println!("\n{summary}");
            }
            if let Some(link) = &s.web_link {
                println!("URL:     {}", link.underline());
            }
        }

        ProjectCommand::ListSeries { project } => {
            let series = projects::list_project_series(&client, &project).await?;
            let mut table = build_table(vec!["Name", "Title", "Status"]);
            for s in &series {
                table.add_row(vec![
                    s.name.as_str(),
                    s.title.as_deref().unwrap_or("—"),
                    s.status.as_deref().unwrap_or("—"),
                ]);
            }
            println!("{table}");
        }

        ProjectCommand::SeriesReleases { project, series } => {
            let releases = projects::list_series_releases(&client, &project, &series).await?;
            let mut table = build_table(vec!["Version", "Release Date", "Notes"]);
            for r in &releases {
                let date = r
                    .date_released
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "—".to_string());
                let notes = truncate(r.release_notes.as_deref().unwrap_or(""), 40);
                table.add_row(vec![
                    r.version.as_deref().unwrap_or("—"),
                    &date,
                    &notes,
                ]);
            }
            println!("{table}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// CVE handlers
// ---------------------------------------------------------------------------

async fn handle_cve(cmd: CveCommand) -> lpcli::error::Result<()> {
    let client = LaunchpadClient::new(None);

    match cmd {
        CveCommand::Show { sequence } => {
            let cve = cves::get_cve(&client, &sequence).await?;
            println!("{}", cve.sequence.bold());
            if let Some(title) = &cve.title {
                println!("{title}");
            }
            if let Some(status) = &cve.status {
                println!("Status:  {status}");
            }
            if let Some(desc) = &cve.description {
                println!("\n{desc}");
            }
            if let Some(link) = &cve.web_link {
                println!("\nURL:     {}", link.underline());
            }
        }

        CveCommand::Search { distro, limit } => {
            let params = CveSearchParams {
                in_distribution: distro.as_deref(),
                limit: Some(limit),
                ..Default::default()
            };
            let results = cves::search_cves(&client, &params).await?;
            let mut table = build_table(vec!["CVE", "Status", "Title"]);
            for c in &results {
                table.add_row(vec![
                    c.sequence.as_str(),
                    c.status.as_deref().unwrap_or("—"),
                    &truncate(c.title.as_deref().unwrap_or(""), 50),
                ]);
            }
            println!("{table}");
            println!("{} CVE(s) found.", results.len());
        }

        CveCommand::BugCves { bug_id } => {
            let results = cves::get_bug_cves(&client, bug_id).await?;
            let mut table = build_table(vec!["CVE", "Status", "Title"]);
            for c in &results {
                table.add_row(vec![
                    c.sequence.as_str(),
                    c.status.as_deref().unwrap_or("—"),
                    &truncate(c.title.as_deref().unwrap_or(""), 50),
                ]);
            }
            println!("{table}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Git handlers
// ---------------------------------------------------------------------------

async fn handle_git(cmd: GitCommand) -> lpcli::error::Result<()> {
    let client = LaunchpadClient::new(None);

    match cmd {
        GitCommand::Show { path } => {
            let repo = git::get_git_repository(&client, &path).await?;
            println!(
                "{}",
                repo.unique_name
                    .as_deref()
                    .or(repo.name.as_deref())
                    .unwrap_or("—")
                    .bold()
            );
            if let Some(desc) = &repo.description {
                println!("{desc}");
            }
            if let Some(kind) = &repo.repository_type {
                println!("Type:     {kind}");
            }
            if let Some(info) = &repo.information_type {
                println!("Access:   {info}");
            }
            if let Some(modified) = repo.date_last_modified {
                println!("Modified: {}", modified.format("%Y-%m-%d"));
            }
            if let Some(link) = &repo.web_link {
                println!("URL:      {}", link.underline());
            }
        }

        GitCommand::Default { target } => {
            let repo = git::get_default_git_repository(&client, &target).await?;
            println!(
                "{}",
                repo.unique_name
                    .as_deref()
                    .or(repo.name.as_deref())
                    .unwrap_or("—")
                    .bold()
            );
            if let Some(link) = &repo.web_link {
                println!("URL: {}", link.underline());
            }
        }

        GitCommand::ListPersonRepos { name } => {
            let repos = git::list_person_git_repositories(&client, &name).await?;
            let mut table = build_table(vec!["Repository", "Type", "Modified"]);
            for r in &repos {
                let modified = r
                    .date_last_modified
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                table.add_row(vec![
                    r.unique_name.as_deref().or(r.name.as_deref()).unwrap_or("—"),
                    r.repository_type.as_deref().unwrap_or("—"),
                    &modified,
                ]);
            }
            println!("{table}");
        }

        GitCommand::Refs { path } => {
            let refs = git::list_git_refs(&client, &path).await?;
            let mut table = build_table(vec!["Ref", "Display Name", "SHA1"]);
            for r in &refs {
                table.add_row(vec![
                    r.path.as_deref().unwrap_or("—"),
                    r.display_name.as_deref().unwrap_or("—"),
                    r.commit_sha1
                        .as_deref()
                        .map(|s| &s[..12.min(s.len())])
                        .unwrap_or("—"),
                ]);
            }
            println!("{table}");
        }

        GitCommand::Proposals { path, status } => {
            let proposals =
                git::list_merge_proposals(&client, &path, status.as_deref()).await?;
            let mut table =
                build_table(vec!["Status", "Source Branch", "Target Branch", "Date"]);
            for p in &proposals {
                let date = p
                    .date_created
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                table.add_row(vec![
                    p.queue_status.as_deref().unwrap_or("—"),
                    p.source_git_path.as_deref().unwrap_or("—"),
                    p.target_git_path.as_deref().unwrap_or("—"),
                    &date,
                ]);
            }
            println!("{table}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Spec handlers
// ---------------------------------------------------------------------------

async fn handle_spec(cmd: SpecCommand) -> lpcli::error::Result<()> {
    let client = LaunchpadClient::new(None);

    match cmd {
        SpecCommand::Show { target, name } => {
            let spec = specifications::get_specification(&client, &target, &name).await?;
            println!("{}", spec.name.bold());
            if let Some(title) = &spec.title {
                println!("{title}");
            }
            if let Some(priority) = &spec.priority {
                println!("Priority:   {priority}");
            }
            if let Some(lifecycle) = &spec.lifecycle_status {
                println!("Lifecycle:  {lifecycle}");
            }
            if let Some(impl_status) = &spec.implementation_status {
                println!("Impl:       {impl_status}");
            }
            if let Some(def_status) = &spec.definition_status {
                println!("Definition: {def_status}");
            }
            if let Some(summary) = &spec.summary {
                println!("\n{summary}");
            }
            if let Some(link) = &spec.web_link {
                println!("\nURL: {}", link.underline());
            }
        }

        SpecCommand::List { target, all } => {
            let specs = if all {
                specifications::list_project_specifications(&client, &target).await?
            } else {
                specifications::list_valid_project_specifications(&client, &target).await?
            };
            let mut table =
                build_table(vec!["Name", "Title", "Priority", "Lifecycle", "Impl"]);
            for s in &specs {
                table.add_row(vec![
                    s.name.as_str(),
                    &truncate(s.title.as_deref().unwrap_or(""), 40),
                    s.priority.as_deref().unwrap_or("—"),
                    s.lifecycle_status.as_deref().unwrap_or("—"),
                    s.implementation_status.as_deref().unwrap_or("—"),
                ]);
            }
            println!("{table}");
            println!("{} spec(s) found.", specs.len());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Question handlers
// ---------------------------------------------------------------------------

async fn handle_question(cmd: QuestionCommand) -> lpcli::error::Result<()> {
    let client = LaunchpadClient::new(None);

    match cmd {
        QuestionCommand::Show { question_id } => {
            let q = questions::get_question_by_id(&client, question_id).await?;
            println!("{}", format!("Question #{question_id}").bold());
            println!("{}", "─".repeat(60));
            if let Some(title) = &q.title {
                println!("{}", title.bold());
            }
            if let Some(status) = &q.status {
                println!("Status:  {status}");
            }
            if let Some(desc) = &q.description {
                println!("\n{desc}");
            }
            if let Some(link) = &q.web_link {
                println!("\nURL: {}", link.underline());
            }
        }

        QuestionCommand::Search {
            target,
            query,
            status,
        } => {
            let params = QuestionSearchParams {
                search_text: query.as_deref(),
                status: status.as_deref(),
            };
            let results = questions::search_questions(&client, &target, &params).await?;
            let mut table = build_table(vec!["ID", "Title", "Status"]);
            for q in &results {
                let id = q
                    .self_link
                    .as_deref()
                    .and_then(|u| u.rsplit('/').next())
                    .unwrap_or("—");
                table.add_row(vec![
                    id,
                    &truncate(q.title.as_deref().unwrap_or(""), 55),
                    q.status.as_deref().unwrap_or("—"),
                ]);
            }
            println!("{table}");
            println!("{} question(s) found.", results.len());
        }

        QuestionCommand::Messages { target, question_id } => {
            let msgs =
                questions::get_question_messages(&client, &target, question_id).await?;
            for m in &msgs {
                let idx = m
                    .index
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let author = m
                    .owner_link
                    .as_deref()
                    .and_then(|u| u.rsplit('/').next())
                    .unwrap_or("unknown");
                let date = m
                    .date_created
                    .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                println!(
                    "{}",
                    format!("Message #{idx} — {author} — {date}").bold()
                );
                if let Some(action) = &m.action {
                    println!("[{action}]");
                }
                println!("{}", m.content.as_deref().unwrap_or(""));
                println!("{}", "─".repeat(60));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Webhook handlers
// ---------------------------------------------------------------------------

async fn handle_webhook(cmd: WebhookCommand) -> lpcli::error::Result<()> {
    let client = authenticated_client()?;

    match cmd {
        WebhookCommand::List { target } => {
            let hooks = webhooks::list_target_webhooks(&client, &target).await?;
            let mut table = build_table(vec!["Delivery URL", "Active", "Events", "Created"]);
            for h in &hooks {
                let events = h
                    .event_types
                    .as_ref()
                    .map(|v| v.join(", "))
                    .unwrap_or_default();
                let date = h
                    .date_created
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                table.add_row(vec![
                    h.delivery_url.as_deref().unwrap_or("—"),
                    &h.active.map(|a| a.to_string()).unwrap_or_default(),
                    &truncate(&events, 40),
                    &date,
                ]);
            }
            println!("{table}");
        }

        WebhookCommand::Create {
            target,
            delivery_url,
            event_types,
            inactive,
            secret,
        } => {
            let types: Vec<&str> = event_types.split(',').map(str::trim).collect();
            let hook = webhooks::create_webhook(
                &client,
                &target,
                &delivery_url,
                &types,
                !inactive,
                secret.as_deref(),
            )
            .await?;
            println!("{} Webhook created.", "✓".green().bold());
            if let Some(link) = &hook.self_link {
                println!("Webhook URL: {link}");
            }
        }

        WebhookCommand::Delete { webhook_url } => {
            webhooks::delete_webhook(&client, &webhook_url).await?;
            println!("{} Webhook deleted.", "✓".green().bold());
        }

        WebhookCommand::Ping { webhook_url } => {
            let delivery = webhooks::ping_webhook(&client, &webhook_url).await?;
            println!("{} Test delivery sent.", "✓".green().bold());
            if let Some(status) = delivery.response_status_code {
                println!("Response status: {status}");
            }
            if let Some(success) = delivery.successful {
                println!("Successful: {success}");
            }
        }

        WebhookCommand::Deliveries { webhook_url } => {
            let deliveries = webhooks::list_deliveries(&client, &webhook_url).await?;
            let mut table =
                build_table(vec!["Sent", "Successful", "Status Code", "Pending"]);
            for d in &deliveries {
                let date = d
                    .date_sent
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                table.add_row(vec![
                    &date,
                    &d.successful.map(|s| s.to_string()).unwrap_or_default(),
                    &d.response_status_code
                        .map(|c| c.to_string())
                        .unwrap_or_default(),
                    &d.pending.map(|p| p.to_string()).unwrap_or_default(),
                ]);
            }
            println!("{table}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Translation handlers
// ---------------------------------------------------------------------------

async fn handle_translation(cmd: TranslationCommand) -> lpcli::error::Result<()> {
    let client = LaunchpadClient::new(None);

    match cmd {
        TranslationCommand::Queue { distro, series } => {
            let entries =
                translations::get_distro_series_import_queue(&client, &distro, &series)
                    .await?;
            let mut table = build_table(vec!["Path", "Status", "Date"]);
            for e in &entries {
                let date = e
                    .date_created
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                table.add_row(vec![
                    e.path.as_deref().unwrap_or("—"),
                    e.status.as_deref().unwrap_or("—"),
                    &date,
                ]);
            }
            println!("{table}");
            println!("{} entry/entries in queue.", entries.len());
        }

        TranslationCommand::Templates { distro, series } => {
            let templates =
                translations::get_distro_series_templates(&client, &distro, &series)
                    .await?;
            let mut table = build_table(vec!["Name", "Path", "Priority", "Current"]);
            for t in &templates {
                table.add_row(vec![
                    t.name.as_deref().unwrap_or("—"),
                    t.path.as_deref().unwrap_or("—"),
                    &t.priority.map(|p| p.to_string()).unwrap_or_default(),
                    &t.is_current.map(|c| c.to_string()).unwrap_or_default(),
                ]);
            }
            println!("{table}");
            println!("{} template(s) found.", templates.len());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Snap handlers
// ---------------------------------------------------------------------------

async fn handle_snap(cmd: SnapCommand) -> lpcli::error::Result<()> {
    let client = authenticated_client()?;

    match cmd {
        SnapCommand::Show { owner, name } => {
            let snap = snaps::get_snap(&client, &owner, &name).await?;
            println!(
                "{}",
                snap.name.as_deref().unwrap_or(&name).bold()
            );
            if let Some(desc) = &snap.description {
                println!("{desc}");
            }
            if let Some(store) = &snap.store_name {
                println!("Store name:   {store}");
            }
            if let Some(private) = snap.private {
                println!("Private:      {private}");
            }
            if let Some(upload) = snap.store_upload {
                println!("Auto-upload:  {upload}");
            }
            if let Some(git_url) = &snap.git_repository_url {
                println!("Git URL:      {git_url}");
            }
            if let Some(git_path) = &snap.git_path {
                println!("Git path:     {git_path}");
            }
            if let Some(link) = &snap.web_link {
                println!("URL:          {}", link.underline());
            }
        }

        SnapCommand::Find { owner } => {
            let snap_list = snaps::find_snaps_by_owner(&client, &owner).await?;
            let mut table = build_table(vec!["Name", "Store Name", "Private"]);
            for s in &snap_list {
                table.add_row(vec![
                    s.name.as_deref().unwrap_or("—"),
                    s.store_name.as_deref().unwrap_or("—"),
                    &s.private.map(|p| p.to_string()).unwrap_or_default(),
                ]);
            }
            println!("{table}");
        }

        SnapCommand::Builds { owner, name } => {
            let builds = snaps::get_snap_pending_builds(&client, &owner, &name).await?;
            let mut table = build_table(vec!["Title", "Pocket", "Score", "Upload Status"]);
            for b in &builds {
                table.add_row(vec![
                    b.title.as_deref().unwrap_or("—"),
                    b.pocket.as_deref().unwrap_or("—"),
                    &b.score.map(|s| s.to_string()).unwrap_or_default(),
                    b.store_upload_status.as_deref().unwrap_or("—"),
                ]);
            }
            println!("{table}");
        }

        SnapCommand::RequestBuilds {
            owner,
            name,
            pocket,
            distro,
        } => {
            let archive_url = client.url(&format!("/{distro}/+archive/primary"));
            let req =
                snaps::request_snap_builds(&client, &owner, &name, &archive_url, &pocket)
                    .await?;
            println!("{} Build request submitted.", "✓".green().bold());
            if let Some(status) = &req.status {
                println!("Status: {status}");
            }
            if let Some(link) = &req.self_link {
                println!("Request URL: {link}");
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// AccessToken handlers
// ---------------------------------------------------------------------------

async fn handle_access_token(cmd: AccessTokenCommand) -> lpcli::error::Result<()> {
    let client = authenticated_client()?;

    match cmd {
        AccessTokenCommand::Issue {
            project,
            description,
            scopes,
        } => {
            let scope_list: Vec<&str> = scopes.split(',').map(str::trim).collect();
            let secret =
                access_tokens::issue_project_access_token(
                    &client,
                    &project,
                    &description,
                    &scope_list,
                )
                .await?;
            println!("{} Access token issued.", "✓".green().bold());
            println!(
                "{} Save this secret — it will not be shown again:",
                "!".yellow().bold()
            );
            println!("{}", secret.bold());
        }

        AccessTokenCommand::IssueGit {
            repo,
            description,
            scopes,
        } => {
            let scope_list: Vec<&str> = scopes.split(',').map(str::trim).collect();
            let secret =
                access_tokens::issue_git_access_token(
                    &client,
                    &repo,
                    &description,
                    &scope_list,
                )
                .await?;
            println!("{} Access token issued.", "✓".green().bold());
            println!(
                "{} Save this secret — it will not be shown again:",
                "!".yellow().bold()
            );
            println!("{}", secret.bold());
        }

        AccessTokenCommand::List { project } => {
            let tokens =
                access_tokens::list_project_access_tokens(&client, &project).await?;
            let mut table =
                build_table(vec!["Description", "Scopes", "Created", "Last Used"]);
            for t in &tokens {
                let scopes = t
                    .scopes
                    .as_ref()
                    .map(|v| v.join(", "))
                    .unwrap_or_default();
                let created = t
                    .date_created
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                let last_used = t
                    .date_last_used
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "never".to_string());
                table.add_row(vec![
                    t.description.as_deref().unwrap_or("—"),
                    &scopes,
                    &created,
                    &last_used,
                ]);
            }
            println!("{table}");
        }

        AccessTokenCommand::ListGit { repo } => {
            let tokens =
                access_tokens::list_git_access_tokens(&client, &repo).await?;
            let mut table =
                build_table(vec!["Description", "Scopes", "Created", "Last Used"]);
            for t in &tokens {
                let scopes = t
                    .scopes
                    .as_ref()
                    .map(|v| v.join(", "))
                    .unwrap_or_default();
                let created = t
                    .date_created
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                let last_used = t
                    .date_last_used
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "never".to_string());
                table.add_row(vec![
                    t.description.as_deref().unwrap_or("—"),
                    &scopes,
                    &created,
                    &last_used,
                ]);
            }
            println!("{table}");
        }

        AccessTokenCommand::Revoke { token_url } => {
            access_tokens::revoke_access_token(&client, &token_url).await?;
            println!("{} Access token revoked.", "✓".green().bold());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Status handler
// ---------------------------------------------------------------------------

/// Display authentication status and Launchpad server reachability.
///
/// Both checks run concurrently to minimise latency.
async fn handle_status() -> lpcli::error::Result<()> {
    // Run both checks in parallel.
    let (server, auth) = tokio::join!(status::check_server(), status::check_auth());

    // Authentication section.
    println!("{}", "Authentication".bold());
    println!("{}", "─".repeat(40));
    if auth.logged_in {
        match &auth.username {
            Some(name) => println!(
                "{} Logged in as {}",
                "✓".green().bold(),
                name.cyan().bold()
            ),
            None => println!(
                "{} Credentials found (could not verify with server)",
                "~".yellow().bold()
            ),
        }
    } else {
        println!(
            "{} Not logged in.  Run {} to authenticate.",
            "✗".red().bold(),
            "`lpcli login`".bold()
        );
    }

    println!();

    // Launchpad server section.
    println!("{}", "Launchpad API Server".bold());
    println!("{}", "─".repeat(40));
    if server.reachable {
        println!("{} Online", "✓".green().bold());
        if let Some(ref rtl) = server.resource_type_link {
            println!("  Endpoint: {rtl}");
        }
    } else {
        println!("{} Unreachable", "✗".red().bold());
        if let Some(status_code) = server.http_status {
            println!("  HTTP status: {status_code}");
        }
        if let Some(ref err) = server.error {
            println!("  Error: {err}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an authenticated client from stored credentials.
///
/// Returns `LpError::NotAuthenticated` if no credentials are found, or the
/// underlying error (e.g. `LpError::Config`) if the file exists but cannot
/// be read or parsed.
fn authenticated_client() -> lpcli::error::Result<LaunchpadClient> {
    let creds = auth::load_credentials()?;
    Ok(LaunchpadClient::new(Some(creds)))
}

/// Build a [`comfy_table::Table`] with headers.
fn build_table(headers: Vec<&str>) -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL_CONDENSED);
    table.set_header(headers);
    table
}

// ---------------------------------------------------------------------------
// Target / series filter types and task collection helper
// ---------------------------------------------------------------------------

/// Split a comma-separated string into a trimmed, non-empty list of strings.
fn parse_comma_separated(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Which targets to apply a bug-task operation to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TargetFilter {
    /// Exactly one named target.
    One(String),
    /// An explicit set of named targets (OR logic within the set).
    Many(Vec<String>),
    /// Every target that already has a task on this bug.
    All,
}

impl TargetFilter {
    fn from_args(
        target: Option<String>,
        many_targets: Option<String>,
        all_targets: bool,
    ) -> Self {
        match (all_targets, many_targets, target) {
            (true, _, _) => TargetFilter::All,
            (false, Some(many), _) => TargetFilter::Many(parse_comma_separated(&many)),
            (false, None, Some(t)) => TargetFilter::One(t),
            // Clap's required ArgGroup prevents this branch; defensive fallback.
            (false, None, None) => TargetFilter::All,
        }
    }
}

/// Which Ubuntu series to apply a bug-task operation to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SeriesFilter {
    /// Exactly one named series.
    One(String),
    /// An explicit set of named series (OR logic within the set).
    Many(Vec<String>),
    /// Every series that already has a task on this bug.
    All,
}

impl SeriesFilter {
    fn from_args(
        series: Option<String>,
        many_series: Option<String>,
        all_series: bool,
    ) -> Self {
        match (all_series, many_series, series) {
            (true, _, _) => SeriesFilter::All,
            (false, Some(many), _) => SeriesFilter::Many(parse_comma_separated(&many)),
            (false, None, Some(s)) => SeriesFilter::One(s),
            // Clap's required ArgGroup prevents this branch; defensive fallback.
            (false, None, None) => SeriesFilter::All,
        }
    }
}

/// Return all bug tasks that match `target_filter` AND `series_filter`.
///
/// Every explicitly named target and series is validated against what is
/// actually present on the bug before filtering, so callers receive an
/// actionable error message instead of a silent empty result.
///
/// The series filter only matches tasks that have a series component in their
/// `target_link` (e.g. `/ubuntu/noble/+source/pkg`).  Series-less tasks
/// (e.g. the distribution-level `/ubuntu/+source/pkg` row or an upstream
/// project task) are excluded when `SeriesFilter::All` is used.
fn collect_matching_tasks<'a>(
    tasks: &'a [bugs::BugTask],
    bug_id: u64,
    target_filter: &TargetFilter,
    series_filter: &SeriesFilter,
) -> std::result::Result<Vec<&'a bugs::BugTask>, lpcli::error::LpError> {
    // Parse every task's target link once.
    let parsed: Vec<(&bugs::BugTask, String, Option<String>)> = tasks
        .iter()
        .map(|t| {
            let (tgt, ser) =
                bugs::parse_target_link(t.target_link.as_deref().unwrap_or(""));
            (t, tgt, ser)
        })
        .collect();

    // Sorted, de-duplicated lists of targets and series present on this bug —
    // used to produce helpful "did you mean…?" error messages.
    let available_targets: Vec<String> = parsed
        .iter()
        .filter(|(_, t, _)| !t.is_empty())
        .map(|(_, t, _)| t.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    let available_series: Vec<String> = parsed
        .iter()
        .filter_map(|(_, _, ser)| ser.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    // Validate every explicitly named target.
    match target_filter {
        TargetFilter::One(t) => {
            if !available_targets.iter().any(|a| a.eq_ignore_ascii_case(t)) {
                return Err(lpcli::error::LpError::NotFound(format!(
                    "Target '{t}' not found on bug #{bug_id}. \
                     Available targets: {}.",
                    available_targets.join(", ")
                )));
            }
        }
        TargetFilter::Many(ts) => {
            let missing: Vec<&str> = ts
                .iter()
                .filter(|t| !available_targets.iter().any(|a| a.eq_ignore_ascii_case(t)))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                return Err(lpcli::error::LpError::NotFound(format!(
                    "Targets not found on bug #{bug_id}: {}. \
                     Available targets: {}.",
                    missing.join(", "),
                    available_targets.join(", ")
                )));
            }
        }
        TargetFilter::All => {}
    }

    // Validate every explicitly named series.
    match series_filter {
        SeriesFilter::One(s) => {
            if !available_series.iter().any(|a| a.eq_ignore_ascii_case(s)) {
                return Err(lpcli::error::LpError::NotFound(format!(
                    "Series '{s}' not found on bug #{bug_id}. \
                     Available series: {}.",
                    available_series.join(", ")
                )));
            }
        }
        SeriesFilter::Many(ss) => {
            let missing: Vec<&str> = ss
                .iter()
                .filter(|s| !available_series.iter().any(|a| a.eq_ignore_ascii_case(s)))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                return Err(lpcli::error::LpError::NotFound(format!(
                    "Series not found on bug #{bug_id}: {}. \
                     Available series: {}.",
                    missing.join(", "),
                    available_series.join(", ")
                )));
            }
        }
        SeriesFilter::All => {}
    }

    // Apply the combined filter (target AND series must both match).
    let matched: Vec<&bugs::BugTask> = parsed
        .iter()
        .filter(|(_, task_tgt, task_ser)| {
            let target_ok = match target_filter {
                TargetFilter::One(t) => task_tgt.eq_ignore_ascii_case(t),
                TargetFilter::Many(ts) => {
                    ts.iter().any(|t| task_tgt.eq_ignore_ascii_case(t))
                }
                TargetFilter::All => true,
            };
            let series_ok = match series_filter {
                SeriesFilter::One(s) => task_ser
                    .as_deref()
                    .map(|ts| ts.eq_ignore_ascii_case(s))
                    .unwrap_or(false),
                SeriesFilter::Many(ss) => task_ser
                    .as_deref()
                    .map(|ts| ss.iter().any(|s| ts.eq_ignore_ascii_case(s)))
                    .unwrap_or(false),
                SeriesFilter::All => task_ser.is_some(),
            };
            target_ok && series_ok
        })
        .map(|(t, _, _)| *t)
        .collect();

    if matched.is_empty() {
        return Err(lpcli::error::LpError::NotFound(format!(
            "No tasks found matching the given target and series filters on bug #{bug_id}."
        )));
    }

    Ok(matched)
}

/// Truncate a string to at most `max` characters, appending `…` if truncated.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Helpers shared by multiple tests
    // -----------------------------------------------------------------------

    /// Build a minimal `BugTask` whose only meaningful field is `target_link`.
    fn make_task(target_link: &str) -> bugs::BugTask {
        bugs::BugTask {
            self_link: Some(format!(
                "https://api.launchpad.net/devel{target_link}/+bugtask"
            )),
            bug_link: Some("https://api.launchpad.net/devel/bugs/1".to_string()),
            title: Some("Test bug".to_string()),
            status: Some("New".to_string()),
            importance: Some("Undecided".to_string()),
            assignee_link: None,
            date_created: None,
            bug_target_display_name: None,
            target_link: Some(format!("https://api.launchpad.net/devel{target_link}")),
        }
    }

    /// A representative set of tasks covering several targets, series, and
    /// also series-less rows (distribution-level and upstream project tasks).
    fn sample_tasks() -> Vec<bugs::BugTask> {
        vec![
            // rust-alacritty: noble and jammy only
            make_task("/ubuntu/noble/+source/rust-alacritty"),
            make_task("/ubuntu/jammy/+source/rust-alacritty"),
            // rust-eza: noble, jammy, and focal
            make_task("/ubuntu/noble/+source/rust-eza"),
            make_task("/ubuntu/jammy/+source/rust-eza"),
            make_task("/ubuntu/focal/+source/rust-eza"),
            // Series-less rows — should NOT appear in series-filtered results
            make_task("/ubuntu/+source/rust-alacritty"),
            make_task("/rust-alacritty"),
        ]
    }

    /// Extract the target_link strings from a slice of task references so that
    /// test assertions stay readable.
    fn target_links<'a>(tasks: &'a [&'a bugs::BugTask]) -> Vec<&'a str> {
        tasks
            .iter()
            .filter_map(|t| t.target_link.as_deref())
            .collect()
    }

    // -----------------------------------------------------------------------
    // parse_comma_separated
    // -----------------------------------------------------------------------

    #[test]
    fn parse_comma_separated_single() {
        assert_eq!(parse_comma_separated("noble"), vec!["noble"]);
    }

    #[test]
    fn parse_comma_separated_two() {
        assert_eq!(
            parse_comma_separated("noble, jammy"),
            vec!["noble", "jammy"]
        );
    }

    #[test]
    fn parse_comma_separated_trims_whitespace() {
        assert_eq!(
            parse_comma_separated("  noble  ,  jammy  "),
            vec!["noble", "jammy"]
        );
    }

    #[test]
    fn parse_comma_separated_ignores_empty_segments() {
        assert_eq!(parse_comma_separated(",noble,,jammy,"), vec!["noble", "jammy"]);
    }

    // -----------------------------------------------------------------------
    // TargetFilter::from_args
    // -----------------------------------------------------------------------

    #[test]
    fn target_filter_from_all_targets_flag() {
        let f = TargetFilter::from_args(None, None, true);
        assert_eq!(f, TargetFilter::All);
    }

    #[test]
    fn target_filter_from_many_targets() {
        let f = TargetFilter::from_args(None, Some("rust-alacritty, rust-eza".into()), false);
        assert_eq!(f, TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]));
    }

    #[test]
    fn target_filter_from_single_target() {
        let f = TargetFilter::from_args(Some("rust-alacritty".into()), None, false);
        assert_eq!(f, TargetFilter::One("rust-alacritty".into()));
    }

    // -----------------------------------------------------------------------
    // SeriesFilter::from_args
    // -----------------------------------------------------------------------

    #[test]
    fn series_filter_from_all_series_flag() {
        let f = SeriesFilter::from_args(None, None, true);
        assert_eq!(f, SeriesFilter::All);
    }

    #[test]
    fn series_filter_from_many_series() {
        let f = SeriesFilter::from_args(None, Some("noble, jammy".into()), false);
        assert_eq!(f, SeriesFilter::Many(vec!["noble".into(), "jammy".into()]));
    }

    #[test]
    fn series_filter_from_single_series() {
        let f = SeriesFilter::from_args(Some("noble".into()), None, false);
        assert_eq!(f, SeriesFilter::One("noble".into()));
    }

    // -----------------------------------------------------------------------
    // collect_matching_tasks — successful combinations
    // -----------------------------------------------------------------------

    #[test]
    fn collect_one_target_one_series() {
        let tasks = sample_tasks();
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::One("rust-alacritty".into()),
            &SeriesFilter::One("noble".into()),
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert!(target_links(&result)[0].contains("noble/+source/rust-alacritty"));
    }

    #[test]
    fn collect_one_target_one_series_case_insensitive() {
        let tasks = sample_tasks();
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::One("Rust-Alacritty".into()),
            &SeriesFilter::One("Noble".into()),
        )
        .unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn collect_one_target_many_series() {
        let tasks = sample_tasks();
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::One("rust-alacritty".into()),
            &SeriesFilter::Many(vec!["noble".into(), "jammy".into()]),
        )
        .unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn collect_many_targets_one_series() {
        let tasks = sample_tasks();
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]),
            &SeriesFilter::One("noble".into()),
        )
        .unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn collect_many_targets_many_series() {
        let tasks = sample_tasks();
        // rust-alacritty × {noble, jammy} + rust-eza × {noble, jammy} = 4
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]),
            &SeriesFilter::Many(vec!["noble".into(), "jammy".into()]),
        )
        .unwrap();
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn collect_all_targets_one_series() {
        let tasks = sample_tasks();
        // noble has rust-alacritty and rust-eza → 2 tasks
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::All,
            &SeriesFilter::One("noble".into()),
        )
        .unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn collect_one_target_all_series() {
        let tasks = sample_tasks();
        // rust-alacritty has tasks in noble and jammy (not focal); series-less rows excluded
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::One("rust-alacritty".into()),
            &SeriesFilter::All,
        )
        .unwrap();
        assert_eq!(result.len(), 2);
        for link in target_links(&result) {
            assert!(link.contains("noble") || link.contains("jammy"));
        }
    }

    #[test]
    fn collect_all_targets_many_series() {
        let tasks = sample_tasks();
        // noble + jammy: rust-alacritty × 2 + rust-eza × 2 = 4
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::All,
            &SeriesFilter::Many(vec!["noble".into(), "jammy".into()]),
        )
        .unwrap();
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn collect_all_targets_all_series() {
        let tasks = sample_tasks();
        // All series-specific tasks: noble/alacritty, jammy/alacritty,
        //   noble/eza, jammy/eza, focal/eza = 5
        // Series-less rows (ubuntu/+source/rust-alacritty, /rust-alacritty) excluded.
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::All,
            &SeriesFilter::All,
        )
        .unwrap();
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn collect_many_targets_all_series() {
        let tasks = sample_tasks();
        // rust-eza has noble, jammy, focal → 3; rust-alacritty has noble, jammy → 2
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]),
            &SeriesFilter::All,
        )
        .unwrap();
        assert_eq!(result.len(), 5);
    }

    // -----------------------------------------------------------------------
    // collect_matching_tasks — error cases
    // -----------------------------------------------------------------------

    #[test]
    fn collect_invalid_single_target_returns_error() {
        let tasks = sample_tasks();
        let err = collect_matching_tasks(
            &tasks,
            42,
            &TargetFilter::One("nonexistent-pkg".into()),
            &SeriesFilter::One("noble".into()),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nonexistent-pkg"), "msg: {msg}");
        assert!(msg.contains("42"), "msg: {msg}");
        assert!(msg.contains("rust-alacritty") || msg.contains("rust-eza"), "msg: {msg}");
    }

    #[test]
    fn collect_invalid_one_of_many_targets_returns_error() {
        let tasks = sample_tasks();
        let err = collect_matching_tasks(
            &tasks,
            42,
            &TargetFilter::Many(vec!["rust-alacritty".into(), "ghost-pkg".into()]),
            &SeriesFilter::One("noble".into()),
        )
        .unwrap_err();
        let msg = err.to_string();
        // The missing target must be named in the error.
        assert!(msg.contains("ghost-pkg"), "msg: {msg}");
        // The valid targets must appear in the 'Available targets' list.
        assert!(msg.contains("rust-alacritty"), "msg: {msg}");
    }

    #[test]
    fn collect_all_of_many_targets_invalid_returns_error() {
        let tasks = sample_tasks();
        let err = collect_matching_tasks(
            &tasks,
            42,
            &TargetFilter::Many(vec!["ghost-a".into(), "ghost-b".into()]),
            &SeriesFilter::One("noble".into()),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ghost-a") && msg.contains("ghost-b"), "msg: {msg}");
    }

    #[test]
    fn collect_invalid_single_series_returns_error() {
        let tasks = sample_tasks();
        let err = collect_matching_tasks(
            &tasks,
            42,
            &TargetFilter::One("rust-alacritty".into()),
            &SeriesFilter::One("groovy".into()),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("groovy"), "msg: {msg}");
        assert!(msg.contains("noble") || msg.contains("jammy"), "msg: {msg}");
    }

    #[test]
    fn collect_invalid_one_of_many_series_returns_error() {
        let tasks = sample_tasks();
        let err = collect_matching_tasks(
            &tasks,
            42,
            &TargetFilter::One("rust-alacritty".into()),
            &SeriesFilter::Many(vec!["noble".into(), "eoan".into()]),
        )
        .unwrap_err();
        let msg = err.to_string();
        // The missing series must be named in the error.
        assert!(msg.contains("eoan"), "msg: {msg}");
        // The valid series must appear in the 'Available series' list.
        assert!(msg.contains("noble"), "msg: {msg}");
    }

    #[test]
    fn collect_valid_target_and_series_but_no_intersection_returns_error() {
        // rust-alacritty has no focal task; rust-eza has focal.
        let tasks = sample_tasks();
        let err = collect_matching_tasks(
            &tasks,
            42,
            &TargetFilter::One("rust-alacritty".into()),
            &SeriesFilter::One("focal".into()),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("No tasks found"), "msg: {msg}");
    }

    #[test]
    fn collect_series_less_tasks_excluded_by_all_series() {
        // The series-less rows (/ubuntu/+source/rust-alacritty, /rust-alacritty)
        // must not appear when SeriesFilter::All is used.
        let tasks = vec![
            make_task("/ubuntu/+source/rust-alacritty"),
            make_task("/rust-alacritty"),
            make_task("/ubuntu/noble/+source/rust-alacritty"),
        ];
        let result = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::All,
            &SeriesFilter::All,
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert!(target_links(&result)[0].contains("noble"));
    }

    #[test]
    fn collect_empty_tasks_list_returns_error() {
        let tasks: Vec<bugs::BugTask> = vec![];
        let err = collect_matching_tasks(
            &tasks,
            1,
            &TargetFilter::One("rust-alacritty".into()),
            &SeriesFilter::One("noble".into()),
        )
        .unwrap_err();
        // With no tasks the target validation fires first.
        assert!(err.to_string().contains("rust-alacritty"));
    }

    // -----------------------------------------------------------------------
    // truncate / build_table (pre-existing)
    // -----------------------------------------------------------------------

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let s = truncate("abcdefghij", 5);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 5);
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn build_table_smoke() {
        let table = build_table(vec!["Col A", "Col B"]);
        let rendered = table.to_string();
        assert!(rendered.contains("Col A"));
        assert!(rendered.contains("Col B"));
    }
}
