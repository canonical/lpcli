//! lpcli — command-line interface for Launchpad.net
//!
//! Parse command-line arguments with `clap` and dispatch to the `lpcli` library.

use clap::{Parser, Subcommand};
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
        #[arg(short, long)]
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
    SetStatus {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Target project or distribution to find the bug task for.
        #[arg(short, long)]
        target: String,
        /// New status (e.g. "Confirmed", "Fix Released", "In Progress").
        #[arg(short, long)]
        status: String,
    },
    /// Change the importance of a bug task.
    SetImportance {
        /// The Launchpad bug number.
        #[arg(short, long)]
        bug_id: u64,
        /// Target project or distribution to find the bug task for.
        #[arg(short, long)]
        target: String,
        /// New importance (e.g. "Critical", "High", "Medium", "Low").
        #[arg(short, long)]
        importance: String,
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
            let mut table = build_table(vec!["Target", "Status", "Importance", "Assignee"]);
            for t in &tasks {
                table.add_row(vec![
                    t.bug_target_display_name.as_deref().unwrap_or("—"),
                    t.status.as_deref().unwrap_or("—"),
                    t.importance.as_deref().unwrap_or("—"),
                    t.assignee_link.as_deref().unwrap_or("—"),
                ]);
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
            status,
        } => {
            let tasks = bugs::get_bug_tasks(&client, bug_id).await?;
            let task = tasks
                .iter()
                .find(|t| {
                    t.bug_target_display_name
                        .as_deref()
                        .map(|n| n.eq_ignore_ascii_case(&target))
                        .unwrap_or(false)
                        || t.target_link
                            .as_deref()
                            .and_then(|u| u.rsplit('/').next())
                            .map(|n| n.eq_ignore_ascii_case(&target))
                            .unwrap_or(false)
                })
                .ok_or_else(|| {
                    lpcli::error::LpError::NotFound(format!(
                        "No task for target '{target}' on bug #{bug_id}"
                    ))
                })?;
            let task_url = task
                .self_link
                .as_deref()
                .ok_or(lpcli::error::LpError::Other(
                    "Bug task has no self_link".into(),
                ))?;
            let updated = bugs::set_bug_status(&client, task_url, &status).await?;
            println!(
                "{} Status updated to '{}'.",
                "✓".green().bold(),
                updated.status.as_deref().unwrap_or(&status)
            );
        }

        BugCommand::SetImportance {
            bug_id,
            target,
            importance,
        } => {
            let tasks = bugs::get_bug_tasks(&client, bug_id).await?;
            let task = tasks
                .iter()
                .find(|t| {
                    t.bug_target_display_name
                        .as_deref()
                        .map(|n| n.eq_ignore_ascii_case(&target))
                        .unwrap_or(false)
                        || t.target_link
                            .as_deref()
                            .and_then(|u| u.rsplit('/').next())
                            .map(|n| n.eq_ignore_ascii_case(&target))
                            .unwrap_or(false)
                })
                .ok_or_else(|| {
                    lpcli::error::LpError::NotFound(format!(
                        "No task for target '{target}' on bug #{bug_id}"
                    ))
                })?;
            let task_url = task
                .self_link
                .as_deref()
                .ok_or(lpcli::error::LpError::Other(
                    "Bug task has no self_link".into(),
                ))?;
            let updated = bugs::set_bug_importance(&client, task_url, &importance).await?;
            println!(
                "{} Importance updated to '{}'.",
                "✓".green().bold(),
                updated.importance.as_deref().unwrap_or(&importance)
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
