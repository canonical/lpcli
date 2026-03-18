//! lpcli — command-line interface for Launchpad.net
//!
//! Parse command-line arguments with `clap` and dispatch to the `lpcli` library.

use clap::{Parser, Subcommand};
use colored::Colorize;
use comfy_table::{presets::UTF8_FULL_CONDENSED, Table};

use lpcli::{
    auth,
    bugs::{self, BugSearchParams},
    client::LaunchpadClient,
    error::LpError,
    packages::{self, SourceSearchParams},
    people,
    projects,
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
}

// ---------------------------------------------------------------------------
// Bug sub-commands
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
enum BugCommand {
    /// Show details of a single bug.
    Show {
        /// The Launchpad bug number.
        bug_id: u64,
    },
    /// List bug tasks for a bug.
    Tasks {
        /// The Launchpad bug number.
        bug_id: u64,
    },
    /// Search bugs on a project or distribution.
    Search {
        /// Project or distribution name (e.g. "ubuntu", "launchpad").
        target: String,
        /// Filter by status (e.g. "New", "Confirmed", "Fix Released").
        #[arg(short, long)]
        status: Option<String>,
        /// Filter by importance (e.g. "Critical", "High").
        #[arg(short, long)]
        importance: Option<String>,
        /// Filter by tag.
        #[arg(short, long)]
        tag: Option<String>,
        /// Maximum results to return.
        #[arg(short, long, default_value = "25")]
        limit: u32,
    },
    /// Add a comment to a bug.
    Comment {
        /// The Launchpad bug number.
        bug_id: u64,
        /// Comment text.
        message: String,
    },
    /// List comments on a bug.
    Comments {
        /// The Launchpad bug number.
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
        name: String,
    },
    /// Search for people by name.
    Search {
        /// Search query.
        query: String,
    },
    /// List members of a team.
    Members {
        /// Team name (without leading ~).
        team: String,
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
        owner: String,
        /// PPA name.
        ppa: String,
    },
    /// List source packages in a PPA.
    PpaSources {
        /// PPA owner.
        owner: String,
        /// PPA name.
        ppa: String,
        /// Source package name filter.
        #[arg(short, long)]
        name: Option<String>,
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
        name: String,
    },
    /// Search Launchpad projects.
    Search {
        /// Search query.
        query: String,
    },
    /// List milestones for a project.
    Milestones {
        /// Project name.
        project: String,
        /// Show only active milestones.
        #[arg(short, long)]
        active: bool,
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
            limit,
        } => {
            let params = BugSearchParams {
                status: status.as_deref(),
                importance: importance.as_deref(),
                tag: tag.as_deref(),
                limit: Some(limit),
                ..Default::default()
            };
            let task_list = bugs::search_bugs(&client, &target, &params).await?;
            let mut table = build_table(vec!["ID", "Title", "Status"]);
            for bug in &task_list {
                table.add_row(vec![
                    bug.id.to_string(),
                    truncate(&bug.title, 60),
                    String::new(),
                ]);
            }
            println!("{table}");
            println!("{} bug(s) found.", task_list.len());
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
                let date = m
                    .date_created
                    .map(|d| d.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                table.add_row(vec![
                    m.member_link.as_deref().unwrap_or("—"),
                    m.status.as_deref().unwrap_or("—"),
                    &date,
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
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an authenticated client from stored credentials.
///
/// Returns `LpError::NotAuthenticated` if no credentials are found.
fn authenticated_client() -> lpcli::error::Result<LaunchpadClient> {
    let creds = auth::load_credentials().map_err(|_| LpError::NotAuthenticated)?;
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
