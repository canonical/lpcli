//! Launchpad bug tracking operations.
//!
//! This module provides types and functions for interacting with Launchpad bugs
//! via the REST API (`https://api.launchpad.net/devel/bugs/{id}`).
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_bug`] | Fetch a single bug by ID |
//! | [`get_bug_tasks`] | List all bug tasks (project assignments) for a bug |
//! | [`search_bugs`] | Search bugs on a project or source package |
//! | [`create_bug`] | File a new bug |
//! | [`add_bug_task`] | Add a new bug task to an existing bug |
//! | [`delete_bug_task`] | Delete a bug task by its API self-link |
//! | [`set_bug_status`] | Update the status of a bug task |
//! | [`set_bug_importance`] | Update the importance of a bug task |
//! | [`set_bug_assignee`] | Assign a bug task to a Launchpad user |
//! | [`add_bug_comment`] | Add a comment to a bug |
//! | [`edit_bug_comment`] | Edit a comment's text in place |
//! | [`get_bug_comments`] | List comments on a bug |
//! | [`delete_bug_comment`] | Delete a comment from a bug |
//! | [`set_bug_description`] | Replace the description of a bug |
//! | [`set_bug_tags`] | Replace the tag list on a bug |
//! | [`mark_bug_as_duplicate`] | Mark a bug as a duplicate of another bug |
//! | [`unmark_bug_as_duplicate`] | Remove a bug's duplicate marker |
//! | [`get_bug_tracker_by_name`] | Fetch a bug tracker by name |
//! | [`get_bug_trackers`] | List registered remote bug trackers |
//! | [`get_bug_tracker`] | Fetch a bug tracker by API self-link |
//! | [`add_bug_watch`] | Create a new bug watch on a bug |
//! | [`get_bug_watches`] | List all bug watches for a bug |
//! | [`get_bug_watch`] | Fetch a bug watch by API self-link |
//! | [`set_bug_watch`] | Link a bug task to a bug watch |
//! | [`unset_bug_watch`] | Unlink a bug watch from a bug task |
//! | [`link_bug_task_remote`] | Look up tracker, create/find watch, validate URL, and link bug task |
//! | [`find_bug_tracker_by_base_url`] | Find a tracker by base URL (`queryByBaseURL`) |
//! | [`ensure_bug_tracker`] | Register a tracker if missing (`ensureBugTracker`) |
//! | [`ensure_github_bug_tracker`] | Find/register the `auto-github-` tracker for a GitHub repo |
//! | [`parse_github_bug_ref`] | Parse `owner/repo#N` or a GitHub URL into a [`GitHubBugRef`] |
//! | [`validate_github_watch_url`] | Validate a watch URL shape for a [`GitHubBugRef`] |
//! | [`link_github_bug_task`] | Ensure tracker + watch, validate URL, and link the task |
//!
//! # Exact Launchpad JSON
//!
//! Reads and mutations that produce Launchpad entries have `_raw`
//! counterparts (for example [`get_bug_raw`], [`get_bug_tasks_raw`],
//! [`create_bug_raw`], [`set_bug_status_raw`] and
//! [`link_bug_task_remote_raw`]) that return [`Raw`] values: the typed value
//! together with the JSON exactly as Launchpad returned it. Lists use
//! Launchpad's collection shape with every page fetched.
//!
//! A raw mutation that Launchpad has already applied, but whose result cannot
//! be confirmed (a missing or untrusted `Location`, a failed read-back, an
//! unreadable response, or a failed later step), returns
//! [`LpError::Partial`] with recovery guidance, and invalid caller input is
//! reported as [`LpError::Usage`]. The typed counterparts keep their
//! historical error variants, except that [`delete_bug_comment`] reports a
//! partially applied deletion as [`LpError::Partial`] and a zero
//! [`search_bug_trackers`] limit is an [`LpError::Usage`].

use futures_util::future::try_join_all;
use jiff::Timestamp;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient, Raw};
use crate::error::{LpError, Result};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A Launchpad bug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bug {
    /// Numeric bug identifier.
    pub id: u64,
    /// Short one-line title.
    pub title: String,
    /// Full description of the bug.
    pub description: Option<String>,
    /// Tags attached to this bug.
    pub tags: Vec<String>,
    /// When the bug was filed.
    pub date_created: Option<Timestamp>,
    /// When the bug was last updated.
    pub date_last_updated: Option<Timestamp>,
    /// API self-link for this bug.
    pub self_link: Option<String>,
    /// URL of the bug in the Launchpad web UI.
    pub web_link: Option<String>,
    /// Display name of the person who filed the bug.
    pub owner_link: Option<String>,
    /// Number of users who are affected by this bug.
    pub users_affected_count: Option<u64>,
    /// API link to the master bug when this bug is marked as a duplicate.
    pub duplicate_of_link: Option<String>,
    /// Number of bugs marked as duplicates of this bug.
    pub number_of_duplicates: Option<u64>,
}

/// The status of a bug task (the per-project/package assignment of a bug).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BugTaskStatus {
    New,
    Incomplete,
    Opinion,
    Invalid,
    #[serde(rename = "Won't Fix")]
    WontFix,
    Expired,
    Confirmed,
    Triaged,
    #[serde(rename = "In Progress")]
    InProgress,
    #[serde(rename = "Fix Committed")]
    FixCommitted,
    #[serde(rename = "Fix Released")]
    FixReleased,
    Unknown,
}

impl std::fmt::Display for BugTaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::New => "New",
            Self::Incomplete => "Incomplete",
            Self::Opinion => "Opinion",
            Self::Invalid => "Invalid",
            Self::WontFix => "Won't Fix",
            Self::Expired => "Expired",
            Self::Confirmed => "Confirmed",
            Self::Triaged => "Triaged",
            Self::InProgress => "In Progress",
            Self::FixCommitted => "Fix Committed",
            Self::FixReleased => "Fix Released",
            Self::Unknown => "Unknown",
        };
        write!(f, "{s}")
    }
}

/// The importance of a bug task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BugImportance {
    Unknown,
    Undecided,
    Critical,
    High,
    Medium,
    Low,
    Wishlist,
}

impl std::fmt::Display for BugImportance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Unknown => "Unknown",
            Self::Undecided => "Undecided",
            Self::Critical => "Critical",
            Self::High => "High",
            Self::Medium => "Medium",
            Self::Low => "Low",
            Self::Wishlist => "Wishlist",
        };
        write!(f, "{s}")
    }
}

/// A bug task — a bug as it applies to a specific project or source package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BugTask {
    /// API self-link.
    pub self_link: Option<String>,
    /// Bug API link (e.g. `https://api.launchpad.net/devel/bugs/12345`).
    pub bug_link: Option<String>,
    /// The title of the bug related to this task.
    pub title: Option<String>,
    /// Status of this task.
    pub status: Option<String>,
    /// Importance of this task.
    pub importance: Option<String>,
    /// API link to the assignee, if any.
    pub assignee_link: Option<String>,
    /// Date this task was created.
    pub date_created: Option<Timestamp>,
    /// Target name (project or source package).
    pub bug_target_display_name: Option<String>,
    /// Target API link.
    pub target_link: Option<String>,
    /// API link to the associated bug watch, if any.
    pub bug_watch_link: Option<String>,
}

impl BugTask {
    /// Extract the bug watch ID (if any) from `bug_watch_link`.
    pub fn bug_watch_id(&self) -> Option<&str> {
        self.bug_watch_link
            .as_deref()
            .and_then(|url| url.rsplit('/').next().filter(|s| !s.is_empty()))
    }
}

/// A remote bug tracker registered in Launchpad (e.g. "debbugs").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BugTracker {
    /// API self-link (e.g. `https://api.launchpad.net/devel/bugs/bugtrackers/debbugs`).
    pub self_link: Option<String>,
    /// Unique identifier name of the bug tracker (e.g. `"debbugs"`).
    pub name: Option<String>,
    /// Title of the bug tracker (e.g. `"Debian Bug tracker"`).
    pub title: Option<String>,
    /// Summary description of the tracker.
    pub summary: Option<String>,
    /// Base URL of the remote tracker (e.g. `"https://bugs.debian.org/"`).
    pub base_url: Option<String>,
    /// Bug tracker type (e.g. `"Debbugs"`, `"Bugzilla"`, `"GitHub"`).
    pub bug_tracker_type: Option<String>,
    /// Web link on Launchpad.
    pub web_link: Option<String>,
}

/// A bug watch linking a Launchpad bug to a remote bug tracker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BugWatch {
    /// API self-link (e.g. `https://api.launchpad.net/devel/bugs/12345/+watch/67890`).
    pub self_link: Option<String>,
    /// Link to the parent Launchpad bug.
    pub bug_link: Option<String>,
    /// Link to the remote bug tracker.
    pub bug_tracker_link: Option<String>,
    /// The remote bug identifier/number (e.g. `"1150409"`).
    pub remote_bug: Option<String>,
    /// Title of the watch (e.g. `"Debian Bug tracker #1150409"`).
    pub title: Option<String>,
    /// Direct URL to the remote bug report (e.g. `"https://bugs.debian.org/cgi-bin/bugreport.cgi?bug=1150409"`).
    pub url: Option<String>,
    /// Web link to the bug watch on Launchpad.
    pub web_link: Option<String>,
    /// Status on the remote bug tracker.
    pub remote_status: Option<String>,
    /// Importance on the remote bug tracker.
    pub remote_importance: Option<String>,
    /// Date this watch was created.
    pub date_created: Option<Timestamp>,
    /// Date this watch was last checked by Launchpad.
    pub date_last_checked: Option<Timestamp>,
    /// Date this watch was last changed on the remote tracker.
    pub date_last_changed: Option<Timestamp>,
    /// API link to the owner who created this watch.
    pub owner_link: Option<String>,
    /// Last error encountered when checking the remote bug, if any.
    pub last_error_type: Option<String>,
}

impl BugWatch {
    /// Extract the bug watch numeric ID from its `self_link`.
    pub fn id(&self) -> Option<&str> {
        self.self_link
            .as_deref()
            .and_then(|url| url.rsplit('/').next().filter(|s| !s.is_empty()))
    }
}

/// A comment on a bug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BugComment {
    /// API self-link.
    pub self_link: Option<String>,
    /// Comment text.
    pub content: Option<String>,
    /// When the comment was posted.
    pub date_created: Option<Timestamp>,
    /// When the comment was deleted (if deleted).
    pub date_deleted: Option<Timestamp>,
    /// API link to the author.
    pub owner_link: Option<String>,
    /// Sequence number within the bug.
    pub index: Option<u64>,
}

/// A file attachment on a bug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BugAttachment {
    /// API self-link.
    pub self_link: Option<String>,
    /// Short descriptive title of the attachment.
    pub title: Option<String>,
    /// Attachment type (e.g. "Patch" or "Unspecified").
    #[serde(rename = "type")]
    pub attachment_type: Option<String>,
    /// API link to the bug this attachment belongs to.
    pub bug_link: Option<String>,
    /// API link to the file data (librarian-hosted content).
    pub data_link: Option<String>,
    /// API link to the message associated with this attachment.
    pub message_link: Option<String>,
    /// Web-accessible URL for the attachment.
    pub web_link: Option<String>,
}

/// Parameters for searching bugs.
#[derive(Debug, Clone, Default)]
pub struct BugSearchParams<'a> {
    /// Filter by status (e.g. "New", "Confirmed").
    pub status: Option<&'a str>,
    /// Filter by importance.
    pub importance: Option<&'a str>,
    /// Filter by tag.
    pub tag: Option<&'a str>,
    /// Filter by assignee Launchpad name.
    pub assignee: Option<&'a str>,
    /// Restrict to a specific source package (e.g. `"firefox"`). Only
    /// meaningful when the target is a distribution such as `"ubuntu"`.
    pub package_name: Option<&'a str>,
    /// Full-text keyword search (matches bug titles and descriptions).
    pub search_text: Option<&'a str>,
    /// Maximum number of results to return.
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch a single Launchpad bug by numeric ID.
///
/// # Errors
///
/// Returns [`crate::error::LpError::NotFound`] if the bug does not exist.
pub async fn get_bug(client: &LaunchpadClient, bug_id: u64) -> Result<Bug> {
    client.get(&format!("/bugs/{bug_id}")).await
}

/// Like [`get_bug()`], but keep the exact JSON Launchpad returned.
pub async fn get_bug_raw(client: &LaunchpadClient, bug_id: u64) -> Result<Raw<Bug>> {
    client.get_raw(&format!("/bugs/{bug_id}")).await
}

/// Fetch all bug tasks associated with a bug.
pub async fn get_bug_tasks(client: &LaunchpadClient, bug_id: u64) -> Result<Vec<BugTask>> {
    Collection::fetch_all(client, &bug_collection_url(client, bug_id, "bug_tasks")).await
}

/// Like [`get_bug_tasks()`], but return Launchpad's collection shape with
/// every entry unmodified.
pub async fn get_bug_tasks_raw(client: &LaunchpadClient, bug_id: u64) -> Result<Raw<Vec<BugTask>>> {
    Collection::fetch_all_raw(client, &bug_collection_url(client, bug_id, "bug_tasks")).await
}

/// API URL of a collection linked from a bug (e.g. `bug_tasks`).
fn bug_collection_url(client: &LaunchpadClient, bug_id: u64, collection: &str) -> String {
    client.url(&format!("/bugs/{bug_id}/{collection}"))
}

/// Search bug tasks on a Launchpad project, distribution, or source package.
///
/// `target` is the project or distribution name (e.g. `"ubuntu"`,
/// `"launchpad"`). Supply [`BugSearchParams::package_name`] to scope the
/// search to a specific source package within a distribution (e.g.
/// `"firefox"` within `"ubuntu"`), and [`BugSearchParams::search_text`] to
/// perform a keyword search against bug titles and descriptions.
///
/// Returns a list of [`BugTask`] entries. Each task carries the bug title,
/// status, importance, and the affected target.
pub async fn search_bugs(
    client: &LaunchpadClient,
    target: &str,
    params: &BugSearchParams<'_>,
) -> Result<Vec<BugTask>> {
    // ws.size sets the Launchpad page size, not a total cap.  fetch_page
    // requests exactly one page so the user-supplied limit is honoured.
    Collection::fetch_page(client, &search_tasks_url(client, target, params)).await
}

/// Like [`search_bugs()`], but return the whole result page together with
/// the exact JSON Launchpad returned for it.
///
/// The raw page keeps Launchpad's collection shape (`total_size` or
/// `total_size_link`, `start`, `entries`, and `next_collection_link` when
/// [`BugSearchParams::limit`] cut the results short), with every entry
/// unmodified.
pub async fn search_bugs_raw(
    client: &LaunchpadClient,
    target: &str,
    params: &BugSearchParams<'_>,
) -> Result<Raw<Collection<BugTask>>> {
    Collection::fetch_page_raw(client, &search_tasks_url(client, target, params)).await
}

/// Build the `searchTasks` URL shared by [`search_bugs()`] and
/// [`search_bugs_raw()`].
fn search_tasks_url(
    client: &LaunchpadClient,
    target: &str,
    params: &BugSearchParams<'_>,
) -> String {
    // Build the base path.  When a package name is provided we target the
    // distribution source package (`/{distro}/+source/{pkg}`); otherwise we
    // search the project / distribution directly.
    let base = if let Some(pkg) = params.package_name {
        format!(
            "/{}/+source/{}?ws.op=searchTasks",
            urlenc(target),
            urlenc(pkg)
        )
    } else {
        format!("/{}?ws.op=searchTasks", urlenc(target))
    };
    let mut query = base;
    if let Some(status) = params.status {
        query.push_str(&format!("&status={}", urlenc(status)));
    }
    if let Some(importance) = params.importance {
        query.push_str(&format!("&importance={}", urlenc(importance)));
    }
    if let Some(tag) = params.tag {
        query.push_str(&format!("&tags={}", urlenc(tag)));
    }
    if let Some(assignee) = params.assignee {
        let assignee_link = if assignee.starts_with("http://") || assignee.starts_with("https://") {
            assignee.to_string()
        } else {
            client.url(&format!("/~{}", urlenc(assignee.trim_start_matches('~'))))
        };
        query.push_str(&format!("&assignee={}", urlenc(&assignee_link)));
    }
    if let Some(text) = params.search_text {
        query.push_str(&format!("&search_text={}", urlenc(text)));
    }
    if let Some(limit) = params.limit {
        query.push_str(&format!("&ws.size={limit}"));
    }
    client.url(&query)
}

/// File a new bug on a Launchpad project.
pub async fn create_bug(
    client: &LaunchpadClient,
    target: &str,
    title: &str,
    description: &str,
) -> Result<Bug> {
    let location = create_bug_location(client, target, title, description).await?;
    client.get_url(&location).await
}

/// Like [`create_bug()`], but return the created bug's exact Launchpad JSON.
///
/// # Errors
///
/// If Launchpad accepts the bug but its location cannot be trusted or the
/// bug cannot be read back, returns [`LpError::Partial`]: the bug may
/// already exist, so check before retrying to avoid filing a duplicate.
pub async fn create_bug_raw(
    client: &LaunchpadClient,
    target: &str,
    title: &str,
    description: &str,
) -> Result<Raw<Bug>> {
    let guidance = format!(
        "A bug titled \"{title}\" may have been filed on '{target}'; check Launchpad \
         for it before retrying, to avoid filing a duplicate."
    );
    let location = create_bug_location(client, target, title, description)
        .await
        .map_err(|error| created_location_failure(error, &guidance))?;
    read_created_raw(client, &location, "Filing the bug", &guidance).await
}

async fn create_bug_location(
    client: &LaunchpadClient,
    target: &str,
    title: &str,
    description: &str,
) -> Result<String> {
    use std::collections::HashMap;
    let target_url = client.url(&format!("/{target}"));
    let mut params = HashMap::new();
    params.insert("ws.op", "createBug");
    params.insert("title", title);
    params.insert("description", description);
    params.insert("target", target_url.as_str());
    client.post_created_location("/bugs", &params).await
}

/// Add a new bug task to an existing bug.
///
/// `target_url` must be the full Launchpad API URL of the target
/// (e.g. `"https://api.launchpad.net/devel/ubuntu/noble/+source/linux"`).
///
/// Returns the newly created [`BugTask`].
pub async fn add_bug_task(
    client: &LaunchpadClient,
    bug_id: u64,
    target_url: &str,
) -> Result<BugTask> {
    let location = add_bug_task_location(client, bug_id, target_url).await?;
    client.get_url(&location).await
}

/// Like [`add_bug_task()`], but return the created task's exact Launchpad
/// JSON.
///
/// # Errors
///
/// If Launchpad accepts the task but it cannot be confirmed, returns
/// [`LpError::Partial`].
pub async fn add_bug_task_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    target_url: &str,
) -> Result<Raw<BugTask>> {
    let guidance = format!(
        "The task for '{target_url}' may have been added to bug #{bug_id}; check \
         `lpcli bug tasks -b {bug_id}` before retrying."
    );
    let location = add_bug_task_location(client, bug_id, target_url)
        .await
        .map_err(|error| created_location_failure(error, &guidance))?;
    read_created_raw(client, &location, "Adding the bug task", &guidance).await
}

async fn add_bug_task_location(
    client: &LaunchpadClient,
    bug_id: u64,
    target_url: &str,
) -> Result<String> {
    use std::collections::HashMap;
    let mut params = HashMap::new();
    params.insert("ws.op", "addTask");
    params.insert("target", target_url);
    client
        .post_created_location(&format!("/bugs/{bug_id}"), &params)
        .await
}

/// Delete a bug task identified by its API self-link.
///
/// The `task_url` must be the full Launchpad API URL returned in the
/// `self_link` field of a [`BugTask`] (e.g. the value from
/// [`get_bug_tasks`]).
pub async fn delete_bug_task(client: &LaunchpadClient, task_url: &str) -> Result<()> {
    client.delete_url_ok(task_url).await
}

/// Update the status of a bug task identified by its API self-link.
///
/// The status is converted to its Launchpad API form (e.g.
/// [`BugTaskStatus::WontFix`] is sent as `"Won't Fix"`).
pub async fn set_bug_status(
    client: &LaunchpadClient,
    task_url: &str,
    status: &BugTaskStatus,
) -> Result<BugTask> {
    use std::collections::HashMap;
    let status_str = status.to_string();
    let mut params = HashMap::new();
    params.insert("status", status_str.as_str());
    client.patch_url(task_url, &params).await
}

/// Like [`set_bug_status()`], but return the updated task's exact Launchpad
/// JSON.
///
/// # Errors
///
/// If Launchpad accepts the change but its response cannot be read,
/// returns [`LpError::Partial`].
pub async fn set_bug_status_raw(
    client: &LaunchpadClient,
    task_url: &str,
    status: &BugTaskStatus,
) -> Result<Raw<BugTask>> {
    let body = serde_json::json!({ "status": status.to_string() });
    patch_task_raw(client, task_url, &body, "Setting the bug task status").await
}

/// Update the importance of a bug task identified by its API self-link.
pub async fn set_bug_importance(
    client: &LaunchpadClient,
    task_url: &str,
    importance: &BugImportance,
) -> Result<BugTask> {
    use std::collections::HashMap;
    let importance_str = importance.to_string();
    let mut params = HashMap::new();
    params.insert("importance", importance_str.as_str());
    client.patch_url(task_url, &params).await
}

/// Like [`set_bug_importance()`], but return the updated task's exact
/// Launchpad JSON.
///
/// # Errors
///
/// If Launchpad accepts the change but its response cannot be read,
/// returns [`LpError::Partial`].
pub async fn set_bug_importance_raw(
    client: &LaunchpadClient,
    task_url: &str,
    importance: &BugImportance,
) -> Result<Raw<BugTask>> {
    let body = serde_json::json!({ "importance": importance.to_string() });
    patch_task_raw(client, task_url, &body, "Setting the bug task importance").await
}

/// Update the assignee of a bug task identified by its API self-link.
///
/// `assignee_url` must be the full Launchpad API URL for the person
/// (e.g. `"https://api.launchpad.net/devel/~jdoe"`). Build it from a
/// Launchpad username with [`LaunchpadClient::url`]:
/// ```text
/// client.url(&format!("/~{username}"))
/// ```
///
/// The updated [`BugTask`] is returned on success.
pub async fn set_bug_assignee(
    client: &LaunchpadClient,
    task_url: &str,
    assignee_url: &str,
) -> Result<BugTask> {
    use std::collections::HashMap;
    let mut params = HashMap::new();
    params.insert("assignee_link", assignee_url);
    client.patch_url(task_url, &params).await
}

/// Like [`set_bug_assignee()`], but return the updated task's exact
/// Launchpad JSON.
///
/// # Errors
///
/// If Launchpad accepts the change but its response cannot be read,
/// returns [`LpError::Partial`].
pub async fn set_bug_assignee_raw(
    client: &LaunchpadClient,
    task_url: &str,
    assignee_url: &str,
) -> Result<Raw<BugTask>> {
    let body = serde_json::json!({ "assignee_link": assignee_url });
    patch_task_raw(client, task_url, &body, "Setting the bug task assignee").await
}

/// PATCH a bug task and keep the exact entry Launchpad returned.
async fn patch_task_raw(
    client: &LaunchpadClient,
    task_url: &str,
    body: &serde_json::Value,
    description: &str,
) -> Result<Raw<BugTask>> {
    let guidance =
        format!("Check the task with `lpcli bug tasks` (API URL: {task_url}) before retrying.");
    patch_entry_raw(client, task_url, body, description, &guidance).await
}

/// Add a comment to a bug.
///
/// The Launchpad `newMessage` operation returns `201 Created` with an empty
/// body (the new message URL is in the `Location` header), so this function
/// returns `()` rather than trying to deserialise a response object.
pub async fn add_bug_comment(client: &LaunchpadClient, bug_id: u64, comment: &str) -> Result<()> {
    use std::collections::HashMap;
    let mut params = HashMap::new();
    params.insert("ws.op", "newMessage");
    params.insert("content", comment);
    client.post_ok(&format!("/bugs/{bug_id}"), &params).await
}

/// Like [`add_bug_comment()`], but follow the `Location` Launchpad reports
/// for the new message and return its exact Launchpad JSON.
///
/// # Errors
///
/// If Launchpad accepts the comment but it cannot be confirmed (no trusted
/// `Location`, or a failed read-back), returns [`LpError::Partial`]: the
/// comment may already be posted, so check before retrying.
pub async fn add_bug_comment_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    comment: &str,
) -> Result<Raw<BugComment>> {
    use std::collections::HashMap;
    let guidance = format!(
        "The comment may have been posted on bug #{bug_id}; check \
         `lpcli bug comments -b {bug_id}` before retrying, to avoid a duplicate."
    );
    let mut params = HashMap::new();
    params.insert("ws.op", "newMessage");
    params.insert("content", comment);
    let location = client
        .post_created_location(&format!("/bugs/{bug_id}"), &params)
        .await
        .map_err(|error| created_location_failure(error, &guidance))?;
    read_created_raw(client, &location, "Posting the comment", &guidance).await
}

/// Edit the text of a comment on a bug, keeping the old text as a revision.
///
/// The comment is identified by its `comment_index`, as shown by
/// [`get_bug_comments`]. Launchpad's `editContent` operation moves the current
/// content into the message's revisions and stores `content` as the new text.
/// The authenticated user must be the original author of the comment;
/// otherwise Launchpad returns a permissions error.
///
/// # Errors
///
/// Returns [`crate::error::LpError::LocalNotFound`] if the comment index does
/// not exist or the comment is deleted, [`crate::error::LpError::Unauthorized`]
/// if the user is not the author, and [`LpError::Partial`] if the edit was
/// applied but the updated entry could not be read back.
pub async fn edit_bug_comment(
    client: &LaunchpadClient,
    bug_id: u64,
    comment_index: u64,
    content: &str,
) -> Result<()> {
    edit_bug_comment_raw(client, bug_id, comment_index, content)
        .await
        .map(|_| ())
}

/// Like [`edit_bug_comment()`], but return the updated message entry.
///
/// # Errors
///
/// As for [`edit_bug_comment()`], except that a successful edit whose read-back
/// fails returns [`LpError::Partial`]: the text was already changed, so check
/// the comment before retrying.
pub async fn edit_bug_comment_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    comment_index: u64,
    content: &str,
) -> Result<Raw<BugComment>> {
    use std::collections::HashMap;

    let comments = get_bug_comments(client, bug_id).await?;
    let comment = find_bug_comment(&comments, bug_id, comment_index)?;
    if comment.date_deleted.is_some() {
        return Err(LpError::LocalNotFound(format!(
            "Comment #{comment_index} on bug #{bug_id} is deleted and cannot be edited."
        )));
    }
    let comment_url = comment.self_link.as_deref().ok_or_else(|| {
        LpError::Other(format!(
            "Comment #{comment_index} on bug #{bug_id} has no API self_link."
        ))
    })?;
    ensure_same_api_origin(client, comment_url, "Comment URL")?;

    let mut params = HashMap::new();
    params.insert("ws.op", "editContent");
    params.insert("new_content", content);
    client
        .post_url_ok(comment_url, &params)
        .await
        .map_err(|error| comment_permission_error(error, bug_id, comment_index))?;

    client.get_url_raw(comment_url).await.map_err(|error| {
        LpError::Partial(format!(
            "Editing comment #{comment_index} on bug #{bug_id} succeeded, but the \
             updated comment could not be read back: {error}. The new text was \
             applied; check `lpcli bug comments -b {bug_id}` before retrying."
        ))
    })
}

/// Find a comment by its index in a bug's message collection.
///
/// Launchpad does not send an `index` member for production message entries;
/// the collection position is the value that matches the `/comments/<index>`
/// suffix in the comment's `self_link` and the rich `#N` label. Deleted
/// comments stay in the collection, so positions remain stable. An `index`
/// member is preferred when Launchpad does send one.
fn find_bug_comment(
    comments: &[BugComment],
    bug_id: u64,
    comment_index: u64,
) -> Result<&BugComment> {
    comments
        .iter()
        .enumerate()
        .find(|(position, comment)| comment.index.unwrap_or(*position as u64) == comment_index)
        .map(|(_, comment)| comment)
        .ok_or_else(|| {
            LpError::LocalNotFound(format!(
                "Comment #{comment_index} does not exist on bug #{bug_id}."
            ))
        })
}

/// Explain an author-only comment edit when Launchpad denies permission.
fn comment_permission_error(error: LpError, bug_id: u64, comment_index: u64) -> LpError {
    match error {
        LpError::Api {
            status: 403,
            message,
        } => LpError::Unauthorized(format!(
            "Comment #{comment_index} on bug #{bug_id} can only be edited by its \
             original author. {message}"
        )),
        other => other,
    }
}

/// Fetch comments for a bug.
pub async fn get_bug_comments(client: &LaunchpadClient, bug_id: u64) -> Result<Vec<BugComment>> {
    Collection::fetch_all(client, &bug_collection_url(client, bug_id, "messages")).await
}

/// Like [`get_bug_comments()`], but return Launchpad's collection shape with
/// every entry unmodified.
pub async fn get_bug_comments_raw(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Raw<Vec<BugComment>>> {
    Collection::fetch_all_raw(client, &bug_collection_url(client, bug_id, "messages")).await
}

/// Delete the content of a comment from a bug, including any attachments.
///
/// The comment is identified by its `comment_index`, as shown by
/// [`get_bug_comments`]. This removes any bug attachments associated with the
/// comment (via the Launchpad `removeFromBug` operation) and then uses the
/// `deleteContent` operation on the comment message's canonical `self_link` to
/// remove the comment's text. The authenticated user must be the original
/// author of the comment; otherwise Launchpad will return a permissions error.
///
/// # Errors
///
/// Returns [`crate::error::LpError::NotFound`] if the comment index does not
/// exist, or [`crate::error::LpError::Unauthorized`] if the user is not
/// permitted to delete the comment. If some attachments were already removed
/// when a later step fails, returns [`LpError::Partial`] naming them.
pub async fn delete_bug_comment(
    client: &LaunchpadClient,
    bug_id: u64,
    comment_index: u64,
) -> Result<()> {
    use std::collections::HashMap;

    let (comments, attachments) = tokio::try_join!(
        get_bug_comments(client, bug_id),
        get_bug_attachments(client, bug_id),
    )?;

    let comment = find_bug_comment(&comments, bug_id, comment_index)?;
    let comment_url = comment.self_link.as_deref().ok_or_else(|| {
        crate::error::LpError::Other(format!(
            "Comment #{comment_index} on bug #{bug_id} has no API self_link."
        ))
    })?;

    // Remove any attachments associated with this comment.
    let comment_attachments: Vec<_> = attachments
        .iter()
        .filter(|a| a.message_link.as_deref() == Some(comment_url) && a.self_link.is_some())
        .collect();

    // Once an attachment is removed, a later failure leaves the comment
    // partially deleted, so report what was already applied.
    let mut removed: Vec<&str> = Vec::new();
    let partial = |removed: &[&str], error: LpError| {
        if removed.is_empty() {
            error
        } else {
            LpError::Partial(format!(
                "Removed attachment(s) {} from comment #{comment_index} on bug #{bug_id}, \
                 but a later step failed: {error}. The comment text was not deleted; \
                 check `lpcli bug comments -b {bug_id}` and \
                 `lpcli bug attachments -b {bug_id}` before retrying.",
                removed.join(", ")
            ))
        }
    };
    for attachment in &comment_attachments {
        let attachment_url = attachment.self_link.as_deref().unwrap();
        let mut params = HashMap::new();
        params.insert("ws.op", "removeFromBug");
        client
            .post_url_ok(attachment_url, &params)
            .await
            .map_err(|error| partial(&removed, error))?;
        removed.push(attachment_url);
    }

    // Delete the comment text.
    let mut params = HashMap::new();
    params.insert("ws.op", "deleteContent");
    client
        .post_url_ok(comment_url, &params)
        .await
        .map_err(|error| partial(&removed, error))
}

/// Parameters for adding a bug attachment.
#[derive(Debug, Clone, Default)]
pub struct AddAttachmentParams<'a> {
    /// Comment text associated with the attachment.
    pub comment: &'a str,
    /// Path to a local file to upload (mutually exclusive with `url`).
    pub file_path: Option<&'a std::path::Path>,
    /// An external URL for the attachment (mutually exclusive with `file_path`).
    pub url: Option<&'a str>,
    /// The filename to use for the upload. Derived from `file_path` when not
    /// explicitly given.
    pub filename: Option<&'a str>,
    /// A short description of the attachment.
    pub description: Option<&'a str>,
    /// Whether the attachment is a patch (`true`) or unspecified (`false`).
    pub is_patch: bool,
}

/// Add a file attachment to a bug.
///
/// Either `params.file_path` or `params.url` must be provided.
/// Returns the API URL (`Location` header) of the newly created attachment.
pub async fn add_bug_attachment(
    client: &LaunchpadClient,
    bug_id: u64,
    params: &AddAttachmentParams<'_>,
) -> Result<String> {
    let form = attachment_form(params).await?;
    client
        .post_multipart_created_location(&format!("/bugs/{bug_id}"), form)
        .await
}

/// Like [`add_bug_attachment()`], but follow the reported location and
/// return the created attachment's exact Launchpad JSON.
///
/// # Errors
///
/// Returns [`LpError::Usage`] before any request unless exactly one of
/// `params.file_path` and `params.url` is given. If Launchpad accepts the
/// attachment but it cannot be confirmed, returns [`LpError::Partial`].
pub async fn add_bug_attachment_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    params: &AddAttachmentParams<'_>,
) -> Result<Raw<BugAttachment>> {
    if params.file_path.is_some() == params.url.is_some() {
        return Err(LpError::Usage(
            "Give exactly one of an attachment file or an attachment URL.".into(),
        ));
    }
    let guidance = format!(
        "The attachment may have been added to bug #{bug_id}; check \
         `lpcli bug attachments -b {bug_id}` before retrying, to avoid a duplicate."
    );
    let form = attachment_form(params).await?;
    let location = client
        .post_multipart_created_location(&format!("/bugs/{bug_id}"), form)
        .await
        .map_err(|error| created_location_failure(error, &guidance))?;
    read_created_raw(client, &location, "Adding the attachment", &guidance).await
}

/// Build the `addAttachment` multipart form, reading any local file.
async fn attachment_form(params: &AddAttachmentParams<'_>) -> Result<reqwest::multipart::Form> {
    use reqwest::multipart::{Form, Part};

    let mut form = Form::new()
        .text("ws.op", "addAttachment")
        .text("comment", params.comment.to_string());

    if params.is_patch {
        form = form.text("is_patch", "true");
    }

    if let Some(desc) = params.description {
        form = form.text("description", desc.to_string());
    }

    if let Some(url) = params.url {
        form = form.text("url", url.to_string());
    }

    if let Some(path) = params.file_path {
        let filename = params
            .filename
            .map(|s| s.to_string())
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "attachment".to_string());

        let data = tokio::fs::read(path).await.map_err(|e| {
            crate::error::LpError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read attachment file '{}': {e}", path.display()),
            ))
        })?;

        let content_type = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();

        let part = Part::bytes(data)
            .file_name(filename.clone())
            .mime_str(&content_type)
            .unwrap_or_else(|_| Part::bytes(vec![]).file_name(filename));
        form = form.part("data", part);

        // Launchpad also wants a "filename" text field.
        if let Some(fname) = params.filename {
            form = form.text("filename", fname.to_string());
        } else if let Some(name) = path.file_name() {
            form = form.text("filename", name.to_string_lossy().into_owned());
        }
    }
    Ok(form)
}

/// Fetch all attachments for a bug.
pub async fn get_bug_attachments(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Vec<BugAttachment>> {
    Collection::fetch_all(client, &bug_collection_url(client, bug_id, "attachments")).await
}

/// Like [`get_bug_attachments()`], but return Launchpad's collection shape
/// with every entry unmodified.
pub async fn get_bug_attachments_raw(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Raw<Vec<BugAttachment>>> {
    Collection::fetch_all_raw(client, &bug_collection_url(client, bug_id, "attachments")).await
}

/// Replace the description of a bug.
///
/// The Launchpad bug entry's `description` field is writable via a PATCH to
/// the bug's API entry (the same mechanism [`set_bug_tags`] uses for tags).
///
/// Launchpad's `launchpad.Edit` permission on the bug governs who may edit
/// the description: on public bugs any legitimate user (including the bug
/// reporter) may edit it, while bugs locked to comment-only can only be
/// edited by users with a target role (e.g. bug supervisor) or
/// administrators.
///
/// # Errors
///
/// Returns [`crate::error::LpError::Api`] with `status` 401 if the
/// authenticated user lacks `launchpad.Edit` on the bug (e.g. the bug is
/// locked to comment-only), and 404 if the bug does not exist.
///
/// Returns the updated [`Bug`] on success.
pub async fn set_bug_description(
    client: &LaunchpadClient,
    bug_id: u64,
    description: &str,
) -> Result<Bug> {
    let url = client.url(&format!("/bugs/{bug_id}"));
    let body = serde_json::json!({ "description": description });
    client.patch_url_with_value(&url, &body).await
}

/// Like [`set_bug_description()`], but return the updated bug's exact
/// Launchpad JSON.
///
/// # Errors
///
/// If Launchpad accepts the change but its response cannot be read,
/// returns [`LpError::Partial`].
pub async fn set_bug_description_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    description: &str,
) -> Result<Raw<Bug>> {
    let body = serde_json::json!({ "description": description });
    patch_bug_raw(client, bug_id, &body, "Setting the bug description").await
}

/// Replace the complete tag list on a bug.
///
/// The supplied `tags` slice becomes the new and only set of tags on the bug.
/// To add or remove individual tags, first call [`get_bug`] to read the
/// current tags, modify the list, then call this function.
///
/// Returns the updated [`Bug`] on success.
pub async fn set_bug_tags(client: &LaunchpadClient, bug_id: u64, tags: &[String]) -> Result<Bug> {
    let url = client.url(&format!("/bugs/{bug_id}"));
    let body = serde_json::json!({ "tags": tags });
    client.patch_url_with_value(&url, &body).await
}

/// Like [`set_bug_tags()`], but return the updated bug's exact Launchpad
/// JSON.
///
/// # Errors
///
/// If Launchpad accepts the change but its response cannot be read,
/// returns [`LpError::Partial`].
pub async fn set_bug_tags_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    tags: &[String],
) -> Result<Raw<Bug>> {
    let body = serde_json::json!({ "tags": tags });
    patch_bug_raw(client, bug_id, &body, "Setting the bug tags").await
}

/// Mark a bug as a duplicate of another bug.
///
/// Launchpad exposes duplicate marking as the writable `duplicate_of_link`
/// field on the bug resource, not as a named `markAsDuplicate` operation.
/// Marking a bug that already has duplicates also re-targets those duplicates
/// to the new master bug.
///
/// Returns the updated [`Bug`] on success.
///
/// # Errors
///
/// Returns [`LpError::Usage`] if `bug_id` and `duplicate_of` are the same, and
/// [`LpError::Api`] if Launchpad rejects the request (for example `status` 400
/// on an invalid target bug or information-type conflict, `status` 401/403 if
/// the authenticated user lacks `launchpad.Edit` on the bug, or `status` 404
/// if the bug does not exist).
pub async fn mark_bug_as_duplicate(
    client: &LaunchpadClient,
    bug_id: u64,
    duplicate_of: u64,
) -> Result<Bug> {
    let url = duplicate_of_url(client, bug_id, duplicate_of)?;
    let body = serde_json::json!({ "duplicate_of_link": url });
    let bug_url = client.url(&format!("/bugs/{bug_id}"));
    client.patch_url_with_value(&bug_url, &body).await
}

/// Like [`mark_bug_as_duplicate()`], but return the updated bug's exact
/// Launchpad JSON.
///
/// # Errors
///
/// Returns [`LpError::Usage`] if `bug_id` and `duplicate_of` are the same,
/// [`LpError::Api`] if Launchpad rejects the request, and [`LpError::Partial`]
/// if Launchpad accepts the change but its response cannot be read.
pub async fn mark_bug_as_duplicate_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    duplicate_of: u64,
) -> Result<Raw<Bug>> {
    let url = duplicate_of_url(client, bug_id, duplicate_of)?;
    let body = serde_json::json!({ "duplicate_of_link": url });
    patch_bug_raw(client, bug_id, &body, "Marking the bug as a duplicate").await
}

/// Remove a bug's duplicate marker, making it a normal bug again.
///
/// Returns the updated [`Bug`] on success.
///
/// # Errors
///
/// Returns [`LpError::Api`] if Launchpad rejects the request (for example
/// `status` 401/403 if the authenticated user lacks `launchpad.Edit` on the
/// bug, or `status` 404 if the bug does not exist).
pub async fn unmark_bug_as_duplicate(client: &LaunchpadClient, bug_id: u64) -> Result<Bug> {
    let url = client.url(&format!("/bugs/{bug_id}"));
    let body = serde_json::json!({ "duplicate_of_link": null });
    client.patch_url_with_value(&url, &body).await
}

/// Like [`unmark_bug_as_duplicate()`], but return the updated bug's exact
/// Launchpad JSON.
///
/// # Errors
///
/// Returns [`LpError::Api`] if Launchpad rejects the request, and
/// [`LpError::Partial`] if Launchpad accepts the change but its response
/// cannot be read.
pub async fn unmark_bug_as_duplicate_raw(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Raw<Bug>> {
    let body = serde_json::json!({ "duplicate_of_link": null });
    patch_bug_raw(client, bug_id, &body, "Removing the duplicate marker").await
}

/// Validate a duplicate target and return its API link.
fn duplicate_of_url(client: &LaunchpadClient, bug_id: u64, duplicate_of: u64) -> Result<String> {
    if bug_id == duplicate_of {
        return Err(LpError::Usage(format!(
            "Bug #{bug_id} cannot be marked as a duplicate of itself."
        )));
    }
    Ok(client.url(&format!("/bugs/{duplicate_of}")))
}

/// PATCH a bug and keep the exact entry Launchpad returned.
async fn patch_bug_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    body: &serde_json::Value,
    description: &str,
) -> Result<Raw<Bug>> {
    let url = client.url(&format!("/bugs/{bug_id}"));
    let guidance = format!("Check `lpcli bug show -b {bug_id}` before retrying.");
    patch_entry_raw(client, &url, body, description, &guidance).await
}

// ---------------------------------------------------------------------------
// Bug subscriptions
// ---------------------------------------------------------------------------

/// A subscription connecting a person to a bug.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BugSubscription {
    /// API self-link.
    pub self_link: Option<String>,
    /// API link to the subscribed person.
    pub person_link: Option<String>,
    /// API link to the bug.
    pub bug_link: Option<String>,
    /// When the subscription was created.
    pub date_created: Option<Timestamp>,
    /// API link to the person who created the subscription.
    pub subscribed_by_link: Option<String>,
}

/// Subscribe a person to a bug.
///
/// `person_url` must be the full Launchpad API URL for the person
/// (e.g. `"https://api.launchpad.net/devel/~jdoe"`).
///
/// Returns the new [`BugSubscription`] record.
pub async fn subscribe_to_bug(
    client: &LaunchpadClient,
    bug_id: u64,
    person_url: &str,
) -> Result<BugSubscription> {
    use std::collections::HashMap;
    let path = format!("/bugs/{bug_id}");
    let url = client.url(&path);
    let mut params: HashMap<&str, &str> = HashMap::new();
    params.insert("ws.op", "subscribe");
    params.insert("person", person_url);
    client.post_url(&url, &params).await
}

/// Like [`subscribe_to_bug()`], but return the subscription's exact
/// Launchpad JSON.
///
/// # Errors
///
/// If Launchpad accepts the subscription but its response cannot be read,
/// returns [`LpError::Partial`].
pub async fn subscribe_to_bug_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    person_url: &str,
) -> Result<Raw<BugSubscription>> {
    use std::collections::HashMap;
    let url = client.url(&format!("/bugs/{bug_id}"));
    let mut params: HashMap<&str, &str> = HashMap::new();
    params.insert("ws.op", "subscribe");
    params.insert("person", person_url);
    let guidance = format!(
        "'{person_url}' may be subscribed to bug #{bug_id}; check \
         `lpcli bug subscriptions -b {bug_id}`."
    );
    let response = client.post_url(&url, &params).await;
    applied_response(response, "Subscribing to the bug", &guidance)
}

/// Unsubscribe a person from a bug.
///
/// `person_url` must be the full Launchpad API URL for the person.
pub async fn unsubscribe_from_bug(
    client: &LaunchpadClient,
    bug_id: u64,
    person_url: &str,
) -> Result<()> {
    use std::collections::HashMap;
    let mut params: HashMap<&str, &str> = HashMap::new();
    params.insert("ws.op", "unsubscribe");
    params.insert("person", person_url);
    client.post_ok(&format!("/bugs/{bug_id}"), &params).await
}

/// List all subscriptions for a bug.
pub async fn get_bug_subscriptions(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Vec<BugSubscription>> {
    Collection::fetch_all(client, &bug_collection_url(client, bug_id, "subscriptions")).await
}

/// Like [`get_bug_subscriptions()`], but return Launchpad's collection shape
/// with every entry unmodified.
pub async fn get_bug_subscriptions_raw(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Raw<Vec<BugSubscription>>> {
    Collection::fetch_all_raw(client, &bug_collection_url(client, bug_id, "subscriptions")).await
}

/// Fetch a bug tracker by its unique name (e.g. `"debbugs"`).
///
/// Calls `bug_trackers.getByName(name)`.
///
/// # Errors
///
/// Returns [`crate::error::LpError::NotFound`] for an HTTP 404, or
/// [`crate::error::LpError::LocalNotFound`] when Launchpad reports no tracker
/// with a successful `null` response.
pub async fn get_bug_tracker_by_name(client: &LaunchpadClient, name: &str) -> Result<BugTracker> {
    get_bug_tracker_by_name_raw(client, name)
        .await
        .map(|tracker| tracker.value)
}

/// Like [`get_bug_tracker_by_name()`], but keep the exact JSON Launchpad
/// returned.
pub async fn get_bug_tracker_by_name_raw(
    client: &LaunchpadClient,
    name: &str,
) -> Result<Raw<BugTracker>> {
    let url = client.url(&format!(
        "/bugs/bugtrackers?ws.op=getByName&name={}",
        urlenc(name.trim())
    ));
    get_optional_raw(client, &url)
        .await?
        .ok_or_else(|| LpError::LocalNotFound(format!("Bug tracker '{name}' not found.")))
}

/// GET a named-operation result that Launchpad answers with an entry or a
/// JSON `null`.
async fn get_optional_raw<T: DeserializeOwned>(
    client: &LaunchpadClient,
    url: &str,
) -> Result<Option<Raw<T>>> {
    match client.get_url::<serde_json::Value>(url).await? {
        serde_json::Value::Null => Ok(None),
        value => Raw::from_value(value).map(Some),
    }
}

/// List all remote bug trackers registered in Launchpad.
///
/// The `name` field on each returned tracker is the value accepted by
/// [`get_bug_tracker_by_name`] and the `--bug-tracker` CLI option.
pub async fn get_bug_trackers(client: &LaunchpadClient) -> Result<Vec<BugTracker>> {
    const PAGE_SIZE: usize = 100;
    const PAGES_PER_BATCH: usize = 8;

    let first_url = client.url(&format!("/bugs/bugtrackers?ws.size={PAGE_SIZE}"));
    let first_page: Collection<BugTracker> = client.get_url(&first_url).await?;
    let Some(total_size) = first_page.total_size else {
        let mut trackers = first_page.entries;
        let mut next_url = first_page.next_collection_link;
        while let Some(url) = next_url {
            let page: Collection<BugTracker> = client.get_url(&url).await?;
            trackers.extend(page.entries);
            next_url = page.next_collection_link;
        }
        return Ok(trackers);
    };

    let total_size = usize::try_from(total_size)
        .map_err(|_| LpError::Other("Bug tracker collection is too large.".to_string()))?;
    let mut trackers = first_page.entries;
    let page_starts: Vec<usize> = (PAGE_SIZE..total_size).step_by(PAGE_SIZE).collect();

    for batch in page_starts.chunks(PAGES_PER_BATCH) {
        let pages = try_join_all(batch.iter().map(|start| {
            let url = client.url(&format!(
                "/bugs/bugtrackers?ws.size={PAGE_SIZE}&memo={start}&ws.start={start}"
            ));
            async move { Collection::<BugTracker>::fetch_page(client, &url).await }
        }))
        .await?;
        trackers.extend(pages.into_iter().flatten());
    }

    Ok(trackers)
}

/// Search registered remote bug trackers and optionally stop after `limit`
/// matches.
///
/// Search is a case-insensitive substring match over tracker metadata.
/// Pages are fetched sequentially so a limit can stop requests as soon as
/// enough matching entries have been collected.
pub async fn search_bug_trackers(
    client: &LaunchpadClient,
    search: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<BugTracker>> {
    let found = search_bug_trackers_raw(client, search, limit).await?;
    Ok(found
        .trackers
        .into_iter()
        .map(|tracker| tracker.value)
        .collect())
}

/// Remote bug trackers found by [`search_bug_trackers_raw()`].
#[derive(Debug, Clone, PartialEq)]
pub struct BugTrackerSearch {
    /// Matching trackers, in the order Launchpad returned them, each with the
    /// exact JSON Launchpad returned for it.
    pub trackers: Vec<Raw<BugTracker>>,
    /// `true` when every page was scanned, so `trackers` holds every match.
    /// `false` when the limit stopped the scan while more trackers might
    /// still match.
    pub complete: bool,
}

/// Like [`search_bug_trackers()`], but keep the exact JSON of each tracker
/// and report whether every page was scanned.
///
/// The search runs locally over Launchpad's pages, so no Launchpad link can
/// continue a filtered list; [`BugTrackerSearch::complete`] tells callers
/// whether the number of matches is the true total.
///
/// # Errors
///
/// A zero limit is rejected before making any HTTP request.
pub async fn search_bug_trackers_raw(
    client: &LaunchpadClient,
    search: Option<&str>,
    limit: Option<usize>,
) -> Result<BugTrackerSearch> {
    const PAGE_SIZE: usize = 100;

    if limit == Some(0) {
        return Err(LpError::Usage(
            "Bug tracker search limit must be positive.".into(),
        ));
    }

    let mut page_url = client.url(&format!("/bugs/bugtrackers?ws.size={PAGE_SIZE}"));
    let search = search.map(str::to_lowercase);
    let mut matches = Vec::new();

    loop {
        let page: Collection<serde_json::Value> = client.get_url(&page_url).await?;
        let mut entries = page.entries.into_iter().peekable();
        while let Some(entry) = entries.next() {
            let tracker = Raw::<BugTracker>::from_value(entry)?;
            if tracker_matches(&tracker.value, search.as_deref()) {
                matches.push(tracker);
                if limit.is_some_and(|limit| matches.len() >= limit) {
                    // The total is only known if nothing is left to scan.
                    let complete = entries.peek().is_none() && page.next_collection_link.is_none();
                    return Ok(BugTrackerSearch {
                        trackers: matches,
                        complete,
                    });
                }
            }
        }

        let Some(next_url) = page.next_collection_link else {
            return Ok(BugTrackerSearch {
                trackers: matches,
                complete: true,
            });
        };
        page_url = next_url;
    }
}

/// Case-insensitive substring match over tracker metadata; `search` must
/// already be lower case.
fn tracker_matches(tracker: &BugTracker, search: Option<&str>) -> bool {
    search.is_none_or(|search| {
        [
            tracker.name.as_deref(),
            tracker.title.as_deref(),
            tracker.summary.as_deref(),
            tracker.bug_tracker_type.as_deref(),
            tracker.base_url.as_deref(),
            tracker.self_link.as_deref(),
            tracker.web_link.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|field| field.to_lowercase().contains(search))
    })
}

/// Fetch a bug tracker by its API self-link URL.
pub async fn get_bug_tracker(client: &LaunchpadClient, tracker_url: &str) -> Result<BugTracker> {
    client.get_url(tracker_url).await
}

/// Like [`get_bug_tracker()`], but keep the exact JSON Launchpad returned.
pub async fn get_bug_tracker_raw(
    client: &LaunchpadClient,
    tracker_url: &str,
) -> Result<Raw<BugTracker>> {
    client.get_url_raw(tracker_url).await
}

/// Create a new bug watch on a bug for the given remote bug tracker and remote bug ID.
///
/// `bug_tracker_url` must be the full Launchpad API URL of the bug tracker
/// (e.g. `"https://api.launchpad.net/devel/bugs/bugtrackers/debbugs"`), typically
/// obtained via [`get_bug_tracker_by_name`].
///
/// `remote_bug` is the bug number or identifier in the remote tracker (e.g. `"1150409"`).
///
/// Returns the newly created [`BugWatch`].
///
/// # Errors
///
/// API failures of the creation request propagate unchanged. Creation is a
/// POST followed by a GET of the reported location, and bug watches cannot
/// be deleted through the REST API, so failures after Launchpad accepted the
/// POST are reported as [`LpError::Other`] with recovery guidance: an
/// untrusted or missing `Location`, or a failed or malformed read-back (which
/// also names the watch and its web-UI page). The original error text is
/// preserved in the message.
pub async fn add_bug_watch(
    client: &LaunchpadClient,
    bug_id: u64,
    bug_tracker_url: &str,
    remote_bug: &str,
) -> Result<BugWatch> {
    add_bug_watch_inner(
        client,
        bug_id,
        bug_tracker_url,
        remote_bug,
        ErrorStyle::Legacy,
    )
    .await
    .map(|watch| watch.value)
}

/// Like [`add_bug_watch()`], but return the created watch's exact Launchpad
/// JSON.
///
/// # Errors
///
/// Failures after Launchpad accepted the POST (an untrusted or missing
/// `Location`, or a failed or malformed read-back) are reported as
/// [`LpError::Partial`] with the same recovery guidance as
/// [`add_bug_watch()`].
pub async fn add_bug_watch_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    bug_tracker_url: &str,
    remote_bug: &str,
) -> Result<Raw<BugWatch>> {
    add_bug_watch_inner(
        client,
        bug_id,
        bug_tracker_url,
        remote_bug,
        ErrorStyle::Partial,
    )
    .await
}

async fn add_bug_watch_inner(
    client: &LaunchpadClient,
    bug_id: u64,
    bug_tracker_url: &str,
    remote_bug: &str,
    style: ErrorStyle,
) -> Result<Raw<BugWatch>> {
    use std::collections::HashMap;
    let clean_remote = remote_bug.trim().trim_start_matches('#');
    let mut params = HashMap::new();
    params.insert("ws.op", "addWatch");
    params.insert("bug_tracker", bug_tracker_url);
    params.insert("remote_bug", clean_remote);
    let location = client
        .post_created_location(&format!("/bugs/{bug_id}"), &params)
        .await
        .map_err(|error| match error {
            // `LpError::Other` is only produced here after the POST succeeded
            // (a missing or untrusted `Location`), so the watch may exist.
            LpError::Other(message) | LpError::Partial(message) => style.applied(format!(
                "{message} The bug watch may have been created on bug #{bug_id}; check \
                 `lpcli bug watches -b {bug_id}` before retrying."
            )),
            other => other,
        })?;
    client
        .get_url_raw(&location)
        .await
        .map_err(|error| created_watch_read_failure(bug_id, &location, error, style))
}

/// Build the error for a bug watch that Launchpad created but that could not
/// be read back: the watch exists and can only be deleted in the web UI, so
/// the error names it while preserving the original failure.
fn created_watch_read_failure(
    bug_id: u64,
    location: &str,
    error: LpError,
    style: ErrorStyle,
) -> LpError {
    let watch = BugWatch {
        self_link: Some(location.to_string()),
        ..Default::default()
    };
    style.applied(format!(
        "Launchpad created a bug watch at '{location}', but it could not be read \
         back: {error}. {}",
        watch_cleanup_guidance(bug_id, &watch)
    ))
}

/// Fetch all bug watches associated with a bug.
pub async fn get_bug_watches(client: &LaunchpadClient, bug_id: u64) -> Result<Vec<BugWatch>> {
    Collection::fetch_all(client, &bug_collection_url(client, bug_id, "bug_watches")).await
}

/// Like [`get_bug_watches()`], but return Launchpad's collection shape with
/// every entry unmodified.
pub async fn get_bug_watches_raw(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Raw<Vec<BugWatch>>> {
    Collection::fetch_all_raw(client, &bug_collection_url(client, bug_id, "bug_watches")).await
}

/// Fetch a single bug watch by its API URL.
pub async fn get_bug_watch(client: &LaunchpadClient, watch_url: &str) -> Result<BugWatch> {
    client.get_url(watch_url).await
}

/// Like [`get_bug_watch()`], but keep the exact JSON Launchpad returned.
pub async fn get_bug_watch_raw(client: &LaunchpadClient, watch_url: &str) -> Result<Raw<BugWatch>> {
    client.get_url_raw(watch_url).await
}

/// Link a bug task to a bug watch identified by its API self-link.
///
/// `task_url` must be the API self-link of the bug task.
/// `watch_url` must be the API self-link of the bug watch (e.g.
/// `"https://api.launchpad.net/devel/bugs/12345/+watch/67890"`).
///
/// Setting the bug watch makes the task follow the remote bug's status.
/// Returns the updated [`BugTask`].
pub async fn set_bug_watch(
    client: &LaunchpadClient,
    task_url: &str,
    watch_url: &str,
) -> Result<BugTask> {
    use std::collections::HashMap;
    let mut params = HashMap::new();
    params.insert("bug_watch_link", watch_url);
    client.patch_url(task_url, &params).await
}

/// Like [`set_bug_watch()`], but return the updated task's exact Launchpad
/// JSON.
///
/// # Errors
///
/// If Launchpad accepts the change but its response cannot be read,
/// returns [`LpError::Partial`].
pub async fn set_bug_watch_raw(
    client: &LaunchpadClient,
    task_url: &str,
    watch_url: &str,
) -> Result<Raw<BugTask>> {
    let body = serde_json::json!({ "bug_watch_link": watch_url });
    patch_task_raw(client, task_url, &body, "Linking the bug task").await
}

/// Unlink a bug watch from a bug task.
///
/// `task_url` must be the API self-link of the bug task.
///
/// Returns the updated [`BugTask`].
pub async fn unset_bug_watch(client: &LaunchpadClient, task_url: &str) -> Result<BugTask> {
    let body = serde_json::json!({ "bug_watch_link": null });
    client.patch_url_with_value(task_url, &body).await
}

/// Like [`unset_bug_watch()`], but return the updated task's exact Launchpad
/// JSON.
///
/// # Errors
///
/// If Launchpad accepts the change but its response cannot be read,
/// returns [`LpError::Partial`].
pub async fn unset_bug_watch_raw(client: &LaunchpadClient, task_url: &str) -> Result<Raw<BugTask>> {
    let body = serde_json::json!({ "bug_watch_link": null });
    patch_task_raw(client, task_url, &body, "Unlinking the bug task").await
}

/// Link a bug task to a remote bug on an external bug tracker.
///
/// This performs the steps required by Launchpad:
/// 1. Preflight the bug task (`task_url`) — it must exist and belong to
///    `bug_id` — before any watch is created, because bug watches cannot be
///    deleted through the Launchpad REST API.
/// 2. Look up the remote bug tracker by name (or by API self-link URL when
///    `bug_tracker` is an `http(s)://` URL) using
///    [`get_bug_tracker_by_name`] / [`get_bug_tracker`].
/// 3. Find an existing watch for that tracker and remote bug or create one
///    via [`add_bug_watch`].
/// 4. Validate the tracker configuration and the actual watch URL before
///    the task is linked, then link the bug task to the watch via
///    [`set_bug_watch`].
///
/// # Watch URL safety
///
/// The GitHub validation guard triggers when the tracker is typed
/// [`BUG_TRACKER_TYPE_GITHUB`] **or** when its `base_url` is on `github.com`.
/// In both cases the tracker must have a `https://github.com/{owner}/{repo}/issues`
/// base URL (exact host — lookalike hosts such as `github.com.evil.com` are
/// rejected), the `GitHub Issues` type, and a numeric remote bug; a tracker
/// typed `GitHub Issues` without a base URL is also rejected. Otherwise
/// Launchpad derives unusable or misleadingly github-shaped watch URLs (the
/// registered `github-issues` tracker is typed `Google Code` and yields URLs
/// such as `https://github.com/detail?id=...`). Such links are rejected
/// before any watch is created. Trackers of other types with other base URLs
/// are unaffected (generic compatibility). The final watch URL — created or
/// reused — is validated before the task is patched; if a watch had to be
/// created and its URL turns out to be invalid, the error names the watch and
/// its web-UI deletion URL.
///
/// Returns the updated [`BugTask`] and the associated [`BugWatch`].
///
/// # Errors
///
/// Returns [`LpError::NotFound`] when the tracker or task does not exist and
/// [`LpError::Other`] for configuration or URL mismatches — including a task
/// or explicit tracker URL that is not on the configured API origin
/// (rejected before any request is issued). The task
/// preflight and tracker validation run before any mutation: no watch is
/// created and no task is modified when they fail. If a watch is created and
/// its derived URL then fails validation — or the task cannot be linked
/// afterwards — the watch may remain on the bug (the REST API cannot delete
/// watches); the error names the watch and its web-UI page, and points at
/// `lpcli bug watches` for verification before retrying or deleting it.
pub async fn link_bug_task_remote(
    client: &LaunchpadClient,
    bug_id: u64,
    task_url: &str,
    bug_tracker: &str,
    remote_bug: &str,
) -> Result<(BugTask, BugWatch)> {
    let (task, watch) = link_bug_task_remote_inner(
        client,
        bug_id,
        task_url,
        bug_tracker,
        remote_bug,
        ErrorStyle::Legacy,
    )
    .await?;
    Ok((task.value, watch.value))
}

/// Like [`link_bug_task_remote()`], but return the updated task and the
/// watch with their exact Launchpad JSON.
///
/// # Errors
///
/// Invalid caller input detected before any mutation — a task or tracker
/// URL that is not on the configured API origin, a task that belongs to a
/// different bug, or a non-numeric GitHub issue number — is reported as
/// [`LpError::Usage`]. When a watch was created and a later step fails, or
/// the linked task cannot be read back, returns [`LpError::Partial`] with the
/// same cleanup guidance as [`link_bug_task_remote()`].
pub async fn link_bug_task_remote_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    task_url: &str,
    bug_tracker: &str,
    remote_bug: &str,
) -> Result<(Raw<BugTask>, Raw<BugWatch>)> {
    link_bug_task_remote_inner(
        client,
        bug_id,
        task_url,
        bug_tracker,
        remote_bug,
        ErrorStyle::Partial,
    )
    .await
}

async fn link_bug_task_remote_inner(
    client: &LaunchpadClient,
    bug_id: u64,
    task_url: &str,
    bug_tracker: &str,
    remote_bug: &str,
    style: ErrorStyle,
) -> Result<(Raw<BugTask>, Raw<BugWatch>)> {
    let clean_remote_bug = remote_bug.trim().trim_start_matches('#');

    // Preflight: fail before creating anything if the task is unusable.
    preflight_bug_task(client, bug_id, task_url, style).await?;

    let tracker = if bug_tracker.starts_with("http://") || bug_tracker.starts_with("https://") {
        // An explicit tracker URL is fetched directly, so it must be on the
        // configured API origin too (tracker names go through the API as
        // query parameters and need no check).
        ensure_same_api_origin(client, bug_tracker, "Bug tracker URL")
            .map_err(|error| style.usage_error(error))?;
        get_bug_tracker(client, bug_tracker).await?
    } else {
        get_bug_tracker_by_name(client, bug_tracker).await?
    };

    // A tracker typed `GitHub Issues` or with a github.com base URL predicts
    // a GitHub-derived watch URL: a wrong type, a lookalike/wrong host, or a
    // missing base URL makes Launchpad derive unusable (or misleadingly
    // github-shaped) URLs, so validate it before any mutation. Trackers of
    // other types with other base URLs are left to the generic checks below.
    let github_typed = tracker.bug_tracker_type.as_deref() == Some(BUG_TRACKER_TYPE_GITHUB);
    let github_host = tracker
        .base_url
        .as_deref()
        .is_some_and(|base_url| base_url_host_is(base_url, "github.com"));
    let github_ref = if github_typed || github_host {
        Some(github_ref_from_tracker(&tracker, clean_remote_bug, style)?)
    } else {
        None
    };

    // GitHub remote bug IDs are stored canonically (leading zeros normalised
    // away), so the derived watch URL is canonical too.
    let remote_bug_id = match &github_ref {
        Some(reference) => reference.number().to_string(),
        None => clean_remote_bug.to_string(),
    };

    link_task_to_watch(
        client,
        bug_id,
        task_url,
        &tracker,
        &remote_bug_id,
        github_ref.as_ref(),
        style,
    )
    .await
}

// ---------------------------------------------------------------------------
// Bug tracker registration (ensureBugTracker) and GitHub shorthand
// ---------------------------------------------------------------------------

/// Launchpad `bug_tracker_type` for GitHub repositories.
///
/// This is the exact human-readable enum value Launchpad's `ensureBugTracker`
/// operation expects for GitHub (see the service WADL at
/// <https://api.launchpad.net/devel/>). Using any other value — for example
/// registering a `github.com` base URL with the legacy "Google Code" type —
/// makes Launchpad build unusable watch URLs.
pub const BUG_TRACKER_TYPE_GITHUB: &str = "GitHub Issues";

/// A validated reference to a GitHub issue or pull request.
///
/// GitHub issues and pull requests share one numbering sequence, so
/// `owner/repo#658` may refer to either. The reference is canonicalised:
/// owner and repo are lowercased (to avoid registering case-duplicate bug
/// trackers on Launchpad) and the number is a strictly positive decimal
/// integer (leading zeros are normalised away).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitHubBugRef {
    owner: String,
    repo: String,
    number: u64,
}

impl GitHubBugRef {
    /// Build a reference from its parts.
    ///
    /// `owner` must be a valid GitHub user/organisation name (1-39 ASCII
    /// alphanumeric or hyphen characters, no leading/trailing hyphen) and
    /// `repo` a valid repository name (1-100 ASCII alphanumeric, `-`, `_` or
    /// `.` characters, without `..`). Both are canonicalised to lower case.
    ///
    /// # Errors
    ///
    /// Returns [`LpError::Other`] if a name is malformed or `number` is zero.
    pub fn new(owner: &str, repo: &str, number: u64) -> Result<Self> {
        validate_github_owner(owner)?;
        validate_github_repo(repo)?;
        if number == 0 {
            return Err(LpError::Other(
                "GitHub issue/PR number must be positive (0 is not a valid issue).".to_string(),
            ));
        }
        Ok(Self {
            owner: owner.to_ascii_lowercase(),
            repo: repo.to_ascii_lowercase(),
            number,
        })
    }

    /// The canonical lowercased repository owner.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The canonical lowercased repository name.
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// The issue/PR number (GitHub issues and PRs share one sequence).
    pub fn number(&self) -> u64 {
        self.number
    }

    /// The tracker base URL Launchpad derives watch URLs from
    /// (`https://github.com/{owner}/{repo}/issues`).
    pub fn base_url(&self) -> String {
        format!("https://github.com/{}/{}/issues", self.owner, self.repo)
    }

    /// The remote watch URL Launchpad derives for this reference
    /// (`https://github.com/{owner}/{repo}/issues/{number}`; GitHub redirects
    /// `/issues/N` to `/pull/N` for pull requests).
    pub fn watch_url(&self) -> String {
        format!(
            "https://github.com/{}/{}/issues/{}",
            self.owner, self.repo, self.number
        )
    }
}

impl std::fmt::Display for GitHubBugRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

/// Parse a GitHub issue/PR reference from a shorthand or a URL.
///
/// Accepted forms:
///
/// * `owner/repo#N` (the CLI shorthand)
/// * `https://github.com/owner/repo/issues/N`
/// * `https://github.com/owner/repo/pull/N`
///
/// URLs must be strict https GitHub web URLs (no userinfo, no explicit
/// port, no query or fragment, no empty path segments; a single trailing
/// slash is accepted). Owner/repo are canonicalised to lower case;
/// `N` must be a strictly positive decimal number (`+1` and `-1` are
/// rejected, `#007` is normalised to `7`).
///
/// # Errors
///
/// Returns [`LpError::Other`] for anything else, including URLs on other
/// hosts, missing or non-positive issue numbers, and malformed owner/repo
/// names.
pub fn parse_github_bug_ref(input: &str) -> Result<GitHubBugRef> {
    let malformed = || {
        LpError::Other(format!(
            "'{}' is not a GitHub issue/PR reference; accepted forms are \
             'owner/repo#N', 'https://github.com/owner/repo/issues/N' and \
             'https://github.com/owner/repo/pull/N'.",
            input.trim()
        ))
    };

    let trimmed = input.trim();

    // Shorthand form: owner/repo#N.
    if let Some((owner_repo, number)) = trimmed.split_once('#') {
        let (owner, repo) = split_owner_repo(owner_repo)?;
        let number = parse_github_number(number)?;
        return GitHubBugRef::new(owner, repo, number);
    }

    // URL form: https://github.com/{owner}/{repo}/{issues|pull}/{N}.
    let url = match url::Url::parse(trimmed) {
        Ok(url) => url,
        Err(_) => return Err(malformed()),
    };
    if !strict_github_url(trimmed, &url) {
        return Err(malformed());
    }
    let segments = match url_path_segments(url.path()) {
        Some(segments) => segments,
        None => return Err(malformed()),
    };
    if segments.len() == 4 && matches!(segments[2], "issues" | "pull") {
        let owner_repo = format!("{}/{}", segments[0], segments[1]);
        let (owner, repo) = split_owner_repo(&owner_repo)?;
        let number = parse_github_number(segments[3])?;
        return GitHubBugRef::new(owner, repo, number);
    }
    Err(malformed())
}

/// Validate that a bug watch's derived URL has the expected GitHub shape.
///
/// This is a pure shape check: it parses the URL and compares it against
/// `https://github.com/{owner}/{repo}/(issues|pull)/{number}`, applying the
/// same strictness as `parse_github_issues_base_url` (https only, no
/// userinfo, no explicit port, no query or fragment, no empty path
/// segments; a single trailing slash is accepted). It never performs a
/// network request — and remote tracker URLs must never be fetched with an
/// authenticated [`LaunchpadClient`], because stored-credential clients
/// attach Launchpad OAuth credentials to *every* request, including those to
/// third-party hosts. GitHub's `/issues/N` → `/pull/N` redirect is therefore
/// accepted by shape (a `pull` path is valid) rather than by following it.
///
/// Returns the validated watch URL.
///
/// # Errors
///
/// Returns [`LpError::Other`] when the watch has no URL, or when the URL
/// violates the strict GitHub shape or does not match the `{owner}/{repo}`
/// pair and issue number.
pub fn validate_github_watch_url(watch: &BugWatch, github_ref: &GitHubBugRef) -> Result<String> {
    let url_text = watch.url.as_deref().ok_or_else(|| {
        LpError::Other(format!(
            "Bug watch has no derived URL; expected '{}'.",
            github_ref.watch_url()
        ))
    })?;
    let mismatch = || {
        LpError::Other(format!(
            "Bug watch URL '{url_text}' does not have the expected GitHub shape \
             '{}'. Launchpad derives this URL from the tracker type and base \
             URL, so a mismatch means the tracker is mis-configured.",
            github_ref.watch_url()
        ))
    };
    let url = url::Url::parse(url_text).map_err(|e| {
        LpError::Other(format!(
            "Bug watch URL '{url_text}' is not a valid URL: {e}"
        ))
    })?;
    if !strict_github_url(url_text.trim(), &url) {
        return Err(mismatch());
    }
    let segments = url_path_segments(url.path()).ok_or_else(mismatch)?;
    let matches_shape = segments.len() == 4
        && matches!(segments[2], "issues" | "pull")
        && segments[0].eq_ignore_ascii_case(github_ref.owner())
        && segments[1].eq_ignore_ascii_case(github_ref.repo())
        // The number must be in canonical decimal form (no leading zeros),
        // so a watch whose stored remote bug id is not canonical is surfaced
        // instead of silently accepted.
        && segments[3] == github_ref.number().to_string();
    if !matches_shape {
        return Err(mismatch());
    }
    Ok(url_text.to_string())
}

/// Look up a bug tracker by its base URL (`queryByBaseURL`).
///
/// Launchpad matches scheme (`http`/`https`) and trailing-slash permutations
/// of the base URL, so `https://github.com/o/r/issues` also finds a tracker
/// registered as `https://github.com/o/r/issues/`.
///
/// Returns `Ok(None)` when no tracker is registered for the base URL.
///
/// # Errors
///
/// Returns transport/API errors; "no tracker found" is reported as
/// `Ok(None)`.
pub async fn find_bug_tracker_by_base_url(
    client: &LaunchpadClient,
    base_url: &str,
) -> Result<Option<BugTracker>> {
    find_bug_tracker_by_base_url_raw(client, base_url)
        .await
        .map(|tracker| tracker.map(|tracker| tracker.value))
}

/// Like [`find_bug_tracker_by_base_url()`], but keep the exact JSON
/// Launchpad returned.
pub async fn find_bug_tracker_by_base_url_raw(
    client: &LaunchpadClient,
    base_url: &str,
) -> Result<Option<Raw<BugTracker>>> {
    let url = client.url(&format!(
        "/bugs/bugtrackers?ws.op=queryByBaseURL&base_url={}",
        urlenc(base_url.trim())
    ));
    match get_optional_raw(client, &url).await {
        Ok(tracker) => Ok(tracker),
        // Launchpad answers "no tracker for this base URL" with a JSON null
        // body; tolerate a 404-style answer too.
        Err(LpError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Parameters for [`ensure_bug_tracker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureBugTrackerParams<'a> {
    /// Base URL of the tracker (e.g. `https://github.com/owner/repo/issues`).
    /// Launchpad keys trackers by this value, matching scheme
    /// (`http`/`https`) and trailing-slash permutations.
    pub base_url: &'a str,
    /// Exact human-readable tracker type — one of the `BUG_TRACKER_TYPE_*`
    /// constants (e.g. [`BUG_TRACKER_TYPE_GITHUB`], `"GitHub Issues"`).
    pub bug_tracker_type: &'a str,
    /// Tracker name. When `None`, Launchpad derives one from the base URL
    /// (for `github.com` URLs: `auto-github-{owner}-{repo}`, lowercased and
    /// sanitised) and auto-suffixes it on collision. When `Some`, a name that
    /// is already used by a tracker with a different base URL is rejected
    /// before any request is made.
    pub name: Option<&'a str>,
    /// Display title. When `None`, Launchpad derives one from the base URL.
    pub title: Option<&'a str>,
    /// Summary text.
    pub summary: Option<&'a str>,
}

/// The tracker found or created by [`ensure_bug_tracker`].
///
/// `tracker` is authoritative: `ensureBugTracker` keys on `base_url` and
/// returns an existing tracker unchanged (ignoring any requested name and
/// type), so always use the returned [`BugTracker`] fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureBugTrackerOutcome {
    /// The tracker registered for the base URL.
    pub tracker: BugTracker,
    /// Whether this call issued the creation request (`false` when an
    /// existing tracker was found). Best-effort; only the returned tracker is
    /// authoritative.
    pub created: bool,
}

/// The tracker found or created by [`ensure_bug_tracker_raw`], with its
/// exact Launchpad JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct EnsureBugTrackerRawOutcome {
    /// The tracker registered for the base URL, as Launchpad returned it.
    pub tracker: Raw<BugTracker>,
    /// Whether this call issued the creation request; see
    /// [`EnsureBugTrackerOutcome::created`].
    pub created: bool,
}

/// Register a remote bug tracker if it does not exist yet, wrapping
/// Launchpad's idempotent `ensureBugTracker` operation.
///
/// `POST /bugs/bugtrackers` with `ws.op=ensureBugTracker` answers `201
/// Created` with a `Location` header pointing at the tracker (the same
/// contract as `addWatch`), which is followed to fetch the tracker.
///
/// Existing trackers are never modified: the tracker is looked up by
/// `base_url` first, and Launchpad itself returns an existing tracker
/// unchanged. The returned tracker — reused or created — is validated once
/// against the request before the outcome is returned: the type must match
/// exactly, and the base URL must match under Launchpad's `queryByBaseURL`
/// permutation semantics (`http`↔`https` and trailing-slash insignificant,
/// host lowercased, path case-sensitive). For
/// [`BUG_TRACKER_TYPE_GITHUB`] requests both base URLs must additionally be
/// strict canonical GitHub issues URLs for the same lowercased owner/repo
/// pair, so a mis-configured existing tracker (e.g. the registered
/// `github-issues` tracker, typed `Google Code`) is rejected instead of
/// silently repurposed.
///
/// # Errors
///
/// Returns [`LpError::Other`] for an empty `base_url`, for an explicit
/// `name` that is already used by a tracker with a different base URL, or
/// when the returned tracker does not match the request (the tracker is
/// never modified in that case). API failures of the lookups and of the
/// registration request (including permission errors — tracker registration
/// requires an authenticated Launchpad account) are propagated unchanged.
/// Failures after Launchpad accepted the registration (an untrusted or
/// missing `Location`, or a failed or malformed read-back) are reported as
/// [`LpError::Other`], preserving the original error text, because the
/// tracker may already exist.
pub async fn ensure_bug_tracker(
    client: &LaunchpadClient,
    params: &EnsureBugTrackerParams<'_>,
) -> Result<EnsureBugTrackerOutcome> {
    let outcome = ensure_bug_tracker_inner(client, params, ErrorStyle::Legacy).await?;
    Ok(EnsureBugTrackerOutcome {
        tracker: outcome.tracker.value,
        created: outcome.created,
    })
}

/// Like [`ensure_bug_tracker()`], but keep the tracker's exact Launchpad
/// JSON.
///
/// # Errors
///
/// An empty `base_url`, a non-canonical GitHub base URL, or an explicit
/// `name` already used by another tracker is reported as [`LpError::Usage`]
/// before any mutation. Failures after Launchpad accepted the registration
/// (an untrusted or missing `Location`, a failed read-back, or a created
/// tracker that does not match the request) are reported as
/// [`LpError::Partial`] with the same guidance as [`ensure_bug_tracker()`].
pub async fn ensure_bug_tracker_raw(
    client: &LaunchpadClient,
    params: &EnsureBugTrackerParams<'_>,
) -> Result<EnsureBugTrackerRawOutcome> {
    ensure_bug_tracker_inner(client, params, ErrorStyle::Partial).await
}

async fn ensure_bug_tracker_inner(
    client: &LaunchpadClient,
    params: &EnsureBugTrackerParams<'_>,
    style: ErrorStyle,
) -> Result<EnsureBugTrackerRawOutcome> {
    let base_url = params.base_url.trim();
    if base_url.is_empty() {
        return Err(style.usage_error(LpError::Other(
            "Bug tracker base_url must not be empty.".to_string(),
        )));
    }
    // A GitHub request must carry the strict canonical issues base URL so
    // the library never registers a junk tracker of its own (the CLI value
    // parser enforces the same invariant earlier).
    if params.bug_tracker_type == BUG_TRACKER_TYPE_GITHUB {
        parse_github_issues_base_url(base_url).map_err(|error| style.usage_error(error))?;
    }

    // Preflight: if a tracker for this base URL exists, `ensureBugTracker`
    // would return it unchanged, so answer without a mutation-capable POST.
    if let Some(existing) = find_bug_tracker_by_base_url_raw(client, base_url).await? {
        validate_ensure_tracker_result(&existing.value, params)?;
        return Ok(EnsureBugTrackerRawOutcome {
            tracker: existing,
            created: false,
        });
    }

    // An explicitly requested name that is taken by a tracker with a
    // different base URL would fail server-side with an opaque integrity
    // error; fail cleanly instead.
    if let Some(name) = params.name.map(str::trim).filter(|n| !n.is_empty()) {
        match get_bug_tracker_by_name_raw(client, name).await {
            Ok(existing) => {
                let existing_base = existing.value.base_url.as_deref();
                if base_urls_equivalent(existing_base.unwrap_or(""), base_url) {
                    // Same tracker under a different lookup path.
                    validate_ensure_tracker_result(&existing.value, params)?;
                    return Ok(EnsureBugTrackerRawOutcome {
                        tracker: existing,
                        created: false,
                    });
                }
                return Err(style.usage_error(LpError::Other(format!(
                    "Bug tracker name '{name}' is already used by a tracker with \
                     base_url '{}'; pick a different name. No tracker was created \
                     or modified.",
                    existing_base.unwrap_or("<unknown>")
                ))));
            }
            Err(LpError::NotFound(_) | LpError::LocalNotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }

    let url = client.url("/bugs/bugtrackers");
    let mut form: Vec<(&str, &str)> = Vec::with_capacity(5);
    form.push(("ws.op", "ensureBugTracker"));
    form.push(("base_url", base_url));
    form.push(("bug_tracker_type", params.bug_tracker_type));
    if let Some(name) = params.name.map(str::trim).filter(|n| !n.is_empty()) {
        form.push(("name", name));
    }
    if let Some(title) = params.title.map(str::trim).filter(|t| !t.is_empty()) {
        form.push(("title", title));
    }
    if let Some(summary) = params.summary.map(str::trim).filter(|s| !s.is_empty()) {
        form.push(("summary", summary));
    }

    let location = client
        .post_pairs_url_created_location(&url, &form)
        .await
        .map_err(|error| match error {
            // `LpError::Other` is only produced here after the POST succeeded
            // (a missing or untrusted `Location`), so the tracker may exist.
            LpError::Other(message) | LpError::Partial(message) => style.applied(format!(
                "{message} {}",
                unconfirmed_tracker_guidance(base_url)
            )),
            other => other,
        })?;
    let tracker: Raw<BugTracker> = client.get_url_raw(&location).await.map_err(|error| {
        style.applied(format!(
            "Launchpad accepted the bug tracker registration and reported it at \
             '{location}', but it could not be read back: {error}. {}",
            unconfirmed_tracker_guidance(base_url)
        ))
    })?;
    // The registration was accepted, so a mismatching result is a change
    // Launchpad already made.
    validate_ensure_tracker_result(&tracker.value, params).map_err(|error| match error {
        LpError::Other(message) => style.applied(message),
        other => other,
    })?;
    Ok(EnsureBugTrackerRawOutcome {
        tracker,
        created: true,
    })
}

/// Guidance for a tracker registration that Launchpad accepted but whose
/// result could not be confirmed. Registration is keyed by base URL, so a
/// retry finds the tracker rather than creating a duplicate.
fn unconfirmed_tracker_guidance(base_url: &str) -> String {
    format!(
        "The bug tracker for base_url '{base_url}' may have been created; check \
         `lpcli bug remote-bug-trackers --search \"{base_url}\"` before retrying \
         (a retry reuses a tracker registered for the same base URL)."
    )
}

/// Validate a tracker returned by `ensureBugTracker` against the requested
/// configuration: exact `bug_tracker_type`, and base URL equivalence per
/// [`base_urls_equivalent`] (http↔https and trailing-slash insignificant,
/// host lowercased, path case-sensitive). For [`BUG_TRACKER_TYPE_GITHUB`]
/// requests both base URLs must additionally be strict canonical GitHub
/// issues URLs for the same lowercased owner/repo pair. The tracker is never
/// modified on mismatch.
fn validate_ensure_tracker_result(
    tracker: &BugTracker,
    params: &EnsureBugTrackerParams<'_>,
) -> Result<()> {
    let name = tracker.name.as_deref().unwrap_or("<unnamed>");
    if tracker.bug_tracker_type.as_deref() != Some(params.bug_tracker_type) {
        return Err(LpError::Other(format!(
            "Bug tracker '{name}' for base_url '{}' has bug tracker type '{}', not \
             the requested '{}'. Launchpad builds watch URLs from the tracker type, \
             so linking against this tracker produces unusable URLs. The tracker \
             was NOT modified.",
            params.base_url,
            tracker.bug_tracker_type.as_deref().unwrap_or("<unset>"),
            params.bug_tracker_type
        )));
    }
    let base_matches = tracker.base_url.as_deref().is_some_and(|returned| {
        if params.bug_tracker_type == BUG_TRACKER_TYPE_GITHUB {
            match (
                parse_github_issues_base_url(params.base_url),
                parse_github_issues_base_url(returned),
            ) {
                (Ok(requested), Ok(found)) => requested == found,
                // Unparsable bases are never treated as equivalent: fail safe.
                _ => false,
            }
        } else {
            base_urls_equivalent(returned, params.base_url)
        }
    });
    if !base_matches {
        return Err(LpError::Other(format!(
            "Bug tracker '{name}' has base_url '{}', which does not match the \
             requested '{}'. The tracker was NOT modified.",
            tracker.base_url.as_deref().unwrap_or("<unset>"),
            params.base_url
        )));
    }
    Ok(())
}

/// Find or register the correctly configured GitHub bug tracker for a
/// repository: type [`BUG_TRACKER_TYPE_GITHUB`], base URL
/// `https://github.com/{owner}/{repo}/issues`, summary describing the
/// repository.
///
/// The tracker name is omitted, so Launchpad derives the `auto-github-`
/// convention name from the base URL and owns collision suffixing; the
/// tracker returned by [`ensure_bug_tracker`] is authoritative and is
/// validated there (exact type, strict GitHub base URL pair) before this
/// returns.
///
/// # Errors
///
/// Returns API failures unchanged and [`LpError::Other`] when the returned
/// tracker is mis-configured (for example the registered `github-issues`
/// tracker, which is typed `Google Code`). No tracker is modified on
/// validation failure.
pub async fn ensure_github_bug_tracker(
    client: &LaunchpadClient,
    github_ref: &GitHubBugRef,
) -> Result<EnsureBugTrackerOutcome> {
    let outcome = ensure_github_bug_tracker_inner(client, github_ref, ErrorStyle::Legacy).await?;
    Ok(EnsureBugTrackerOutcome {
        tracker: outcome.tracker.value,
        created: outcome.created,
    })
}

/// Like [`ensure_github_bug_tracker()`], but keep the tracker's exact
/// Launchpad JSON. Errors are classified as for [`ensure_bug_tracker_raw()`].
pub async fn ensure_github_bug_tracker_raw(
    client: &LaunchpadClient,
    github_ref: &GitHubBugRef,
) -> Result<EnsureBugTrackerRawOutcome> {
    ensure_github_bug_tracker_inner(client, github_ref, ErrorStyle::Partial).await
}

async fn ensure_github_bug_tracker_inner(
    client: &LaunchpadClient,
    github_ref: &GitHubBugRef,
    style: ErrorStyle,
) -> Result<EnsureBugTrackerRawOutcome> {
    let base_url = github_ref.base_url();
    let summary = format!(
        "Bug tracker for the {}/{} GitHub repository issues/PRs.",
        github_ref.owner(),
        github_ref.repo()
    );
    let params = EnsureBugTrackerParams {
        base_url: &base_url,
        bug_tracker_type: BUG_TRACKER_TYPE_GITHUB,
        name: None,
        title: None,
        summary: Some(&summary),
    };
    ensure_bug_tracker_inner(client, &params, style).await
}

/// Link a Launchpad bug task to a GitHub issue/PR in one library call (the
/// library half of `link-remote --github owner/repo#N`).
///
/// Order of operations (reads before any mutation):
/// 1. Preflight the bug task: `task_url` must exist and belong to `bug_id`
///    (watches cannot be deleted via the REST API, so an ineligible task
///    must abort before a watch is created for it).
/// 2. Find or register the correctly configured tracker for the repository
///    ([`ensure_github_bug_tracker`]).
/// 3. Reuse an existing watch for that tracker and issue number, or create
///    one, validate the actual watch URL shape against the original
///    `github_ref` — for created *and* reused watches — and link the task
///    ([`set_bug_watch`]). A mismatch means Launchpad derived an unusable
///    URL from a mis-configured tracker; the final watch URL always matches
///    the intended repository before the task is patched.
///
/// This makes only Launchpad-API requests plus local URL parsing: remote
/// `github.com` URLs are never fetched (and must not be, because a
/// stored-credential client would attach Launchpad OAuth credentials to
/// third-party requests).
///
/// # Errors
///
/// Returns [`LpError::Other`] when the tracker or an existing watch is
/// mis-configured; when a just-created watch turns out to have an invalid
/// URL, the error includes the watch ID and its web-UI page. A `task_url`
/// that is not on the configured API origin is rejected before any request.
/// The task
/// preflight and tracker validation run before any mutation: no watch
/// is created and no task is modified when they fail. If a watch is created
/// and the task cannot be linked afterwards, the watch may remain on the
/// bug (the REST API cannot delete watches); the error names it and points
/// at `lpcli bug watches` for verification before retrying or deleting it.
pub async fn link_github_bug_task(
    client: &LaunchpadClient,
    bug_id: u64,
    task_url: &str,
    github_ref: &GitHubBugRef,
) -> Result<(BugTask, BugWatch)> {
    let (task, watch) =
        link_github_bug_task_inner(client, bug_id, task_url, github_ref, ErrorStyle::Legacy)
            .await?;
    Ok((task.value, watch.value))
}

/// Like [`link_github_bug_task()`], but return the updated task and the
/// watch with their exact Launchpad JSON.
///
/// # Errors
///
/// Invalid caller input detected before any mutation (see
/// [`link_bug_task_remote_raw()`]) is reported as [`LpError::Usage`]. When
/// this call registered a tracker or created a watch and a later step fails,
/// or the linked task cannot be read back, returns [`LpError::Partial`] with
/// recovery guidance.
pub async fn link_github_bug_task_raw(
    client: &LaunchpadClient,
    bug_id: u64,
    task_url: &str,
    github_ref: &GitHubBugRef,
) -> Result<(Raw<BugTask>, Raw<BugWatch>)> {
    link_github_bug_task_inner(client, bug_id, task_url, github_ref, ErrorStyle::Partial).await
}

async fn link_github_bug_task_inner(
    client: &LaunchpadClient,
    bug_id: u64,
    task_url: &str,
    github_ref: &GitHubBugRef,
    style: ErrorStyle,
) -> Result<(Raw<BugTask>, Raw<BugWatch>)> {
    // 1. Task eligibility before any mutation, including tracker registration.
    preflight_bug_task(client, bug_id, task_url, style).await?;

    // 2. Tracker (find-or-register, validated against the reference).
    let outcome = ensure_github_bug_tracker_inner(client, github_ref, style).await?;

    // 3. Reuse or create the watch, validate its URL, and link the task —
    //    passing the original reference so the final watch URL is validated
    //    against the intended repository before the task is patched.
    let linked = link_task_to_watch(
        client,
        bug_id,
        task_url,
        &outcome.tracker.value,
        &github_ref.number().to_string(),
        Some(github_ref),
        style,
    )
    .await;
    match linked {
        // A tracker registered by this call stays registered; a retry
        // reuses it, but the failure is still a partial change.
        Err(error) if style == ErrorStyle::Partial && outcome.created => match error {
            LpError::Partial(_) => Err(error),
            error => Err(LpError::Partial(format!(
                "Registered bug tracker for '{}', but linking the bug task failed: \
                 {error}. A retry reuses the registered tracker.",
                github_ref.base_url()
            ))),
        },
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn urlenc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// How multi-step operations classify their failures.
///
/// The typed functions keep their historical [`LpError::Other`] messages;
/// the `_raw` functions report already-applied changes as
/// [`LpError::Partial`] and invalid caller input as [`LpError::Usage`],
/// with the same message text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorStyle {
    Legacy,
    Partial,
}

impl ErrorStyle {
    /// An error for a change Launchpad has (or may have) already applied.
    fn applied(self, message: String) -> LpError {
        match self {
            Self::Legacy => LpError::Other(message),
            Self::Partial => LpError::Partial(message),
        }
    }

    /// Reclassify a validation failure of caller-supplied input.
    fn usage_error(self, error: LpError) -> LpError {
        match (self, error) {
            (Self::Partial, LpError::Other(message)) => LpError::Usage(message),
            (_, error) => error,
        }
    }
}

/// Classify a failure of a `Location`-returning POST for a `_raw` function.
///
/// `post_*_created_location` returns [`LpError::Partial`] after Launchpad
/// accepted the request but a missing or untrusted `Location` prevents read-back, so
/// the change may exist: that becomes [`LpError::Partial`]. Other errors
/// mean the request failed and propagate unchanged.
fn created_location_failure(error: LpError, guidance: &str) -> LpError {
    match error {
        LpError::Other(message) | LpError::Partial(message) => {
            LpError::Partial(format!("{message} {guidance}"))
        }
        other => other,
    }
}

/// Read back an entry created by a successful POST, appending `guidance` to
/// the [`LpError::Partial`] reported when that fails.
async fn read_created_raw<T: DeserializeOwned>(
    client: &LaunchpadClient,
    location: &str,
    description: &str,
    guidance: &str,
) -> Result<Raw<T>> {
    client
        .get_created_raw(location, description)
        .await
        .map_err(|error| match error {
            LpError::Partial(message) => LpError::Partial(format!("{message}. {guidance}")),
            other => other,
        })
}

/// PATCH an entry and keep the exact entry Launchpad returned.
async fn patch_entry_raw<T: DeserializeOwned>(
    client: &LaunchpadClient,
    url: &str,
    body: &serde_json::Value,
    description: &str,
    guidance: &str,
) -> Result<Raw<T>> {
    let response = client.patch_url_with_value(url, body).await;
    applied_response(response, description, guidance)
}

/// Decode the JSON returned by a successful mutation.
///
/// A JSON error means Launchpad answered with a success status, so the change
/// was applied even though its result cannot be shown: that is reported as
/// [`LpError::Partial`]. HTTP and API errors propagate unchanged.
fn applied_response<T: DeserializeOwned>(
    response: Result<serde_json::Value>,
    description: &str,
    guidance: &str,
) -> Result<Raw<T>> {
    response
        .and_then(Raw::from_value)
        .map_err(|error| match error {
            LpError::Json(error) => LpError::Partial(format!(
                "{description} succeeded, but Launchpad's response could not be read: \
             {error}. {guidance}"
            )),
            other => other,
        })
}

// ---------------------------------------------------------------------------
// GitHub / watch-link helpers
// ---------------------------------------------------------------------------

/// Reject remote-linking URLs that are not on the configured client's API
/// origin, before any authenticated request is issued.
///
/// [`LaunchpadClient`] attaches the stored Launchpad credentials to *every*
/// request it sends, so a user-supplied URL (a bug task URL, or an explicit
/// bug tracker URL handed to [`link_bug_task_remote`]) must be on the
/// configured API origin before it is fetched. Origins are compared with
/// `url::Url` semantics — exact scheme, hostname and effective port — never
/// by string prefix or host suffix; different API-version paths on the same
/// origin are allowed. Malformed URLs and origins that do not match the
/// configured API are rejected.
fn ensure_same_api_origin(client: &LaunchpadClient, url_text: &str, what: &str) -> Result<()> {
    let api_base = client.url("/");
    let api = url::Url::parse(&api_base).map_err(|_| {
        LpError::Other(format!(
            "The configured Launchpad API base URL '{api_base}' is not a valid URL."
        ))
    })?;
    let url = url::Url::parse(url_text.trim())
        .map_err(|_| LpError::Other(format!("{what} '{url_text}' is not a valid URL.")))?;
    if url.origin() != api.origin() {
        return Err(LpError::Other(format!(
            "{what} '{url_text}' is not on the configured Launchpad API origin \
             '{}'; a request to it would be sent with Launchpad credentials. \
             Pass a URL on the configured API server.",
            api.origin().ascii_serialization()
        )));
    }
    Ok(())
}

/// Fetch `task_url` and verify it is eligible to receive a remote bug watch
/// link *before* any mutation-capable request is issued.
///
/// The URL must be on the configured client's API origin
/// ([`ensure_same_api_origin`]): the client attaches the stored Launchpad
/// credentials to every request it sends, so a foreign URL is rejected
/// before the first request is issued.
///
/// Bug watches cannot be deleted through the Launchpad REST API, so a task
/// that does not exist, belongs to a different bug, or cannot prove its bug
/// membership at all must be rejected first — otherwise a watch would be
/// created (and stranded on the wrong bug) with no API-side way to remove
/// it. A missing `bug_link` fails closed, and a response without a
/// `target_link` (e.g. a bug-watch URL passed as `--task-url`, which
/// deserializes into the same all-Option entry shape) is rejected as well:
/// only real tasks carry a target.
///
/// With [`ErrorStyle::Partial`], these rejections of the caller's task URL
/// are reported as [`LpError::Usage`].
async fn preflight_bug_task(
    client: &LaunchpadClient,
    bug_id: u64,
    task_url: &str,
    style: ErrorStyle,
) -> Result<()> {
    ensure_same_api_origin(client, task_url, "Bug task URL")
        .map_err(|error| style.usage_error(error))?;
    let task: BugTask = client.get_url(task_url).await?;
    validate_preflight_task(&task, bug_id, task_url).map_err(|error| style.usage_error(error))
}

fn validate_preflight_task(task: &BugTask, bug_id: u64, task_url: &str) -> Result<()> {
    // Fail closed: without a bug_link the task's ownership cannot be
    // verified, and an entry-shaped response that deserialises to all-None
    // fields must not be treated as linkable.
    let bug_link = task.bug_link.as_deref().ok_or_else(|| {
        LpError::Other(format!(
            "Bug task {task_url} has no bug_link; refusing to link a remote bug \
             because task ownership cannot be verified and a watch created for \
             the wrong bug could only be removed through the web UI."
        ))
    })?;
    let expected = format!("/bugs/{bug_id}");
    if !bug_link.trim_end_matches('/').ends_with(&expected) {
        return Err(LpError::Other(format!(
            "Bug task {task_url} belongs to bug {bug_link}, not bug #{bug_id}; \
             refusing to link a remote bug because a watch created for the \
             wrong bug could only be removed through the web UI."
        )));
    }
    // A bug-watch JSON also carries a bug_link and deserializes into
    // all-Option fields, so a mistyped --task-url can pose as a task. A real
    // task always has a target_link; a bug watch never does.
    let has_target = task
        .target_link
        .as_deref()
        .is_some_and(|target| !target.trim().is_empty());
    if !has_target {
        return Err(LpError::Other(format!(
            "URL {task_url} did not identify a bug task (no target_link; is it a \
             bug watch URL?). The task was NOT modified."
        )));
    }
    Ok(())
}

/// Shared tail of [`link_bug_task_remote`] and [`link_github_bug_task`]:
/// reuse an existing watch for `tracker` + `remote_bug` or create one, then
/// validate the actual watch URL and link `task_url` to the watch.
///
/// Tracker links are compared by canonical resource identity
/// ([`api_resource_identity`]) so that watches registered via another API
/// version (`/1.0/` vs `/devel/`) are still recognised. `github_ref` selects
/// the strict GitHub watch-URL shape check and the numeric remote-bug
/// comparison; when `None` (a non-GitHub tracker), the remote bug is
/// compared and stored as given. The final watch URL — created or reused —
/// is validated before the task is patched; if a watch had to be created and
/// its URL turns out to be invalid, the error names the watch and its
/// web-UI deletion URL.
///
/// With [`ErrorStyle::Partial`], failures after this call created a watch,
/// and a successful link whose response cannot be read, are reported as
/// [`LpError::Partial`]; failures involving only a reused watch keep their
/// original classification because nothing was changed.
async fn link_task_to_watch(
    client: &LaunchpadClient,
    bug_id: u64,
    task_url: &str,
    tracker: &BugTracker,
    remote_bug: &str,
    github_ref: Option<&GitHubBugRef>,
    style: ErrorStyle,
) -> Result<(Raw<BugTask>, Raw<BugWatch>)> {
    let tracker_url = tracker.self_link.as_deref().ok_or_else(|| {
        LpError::LocalNotFound(format!(
            "Bug tracker '{}' has no self_link.",
            tracker.name.as_deref().unwrap_or("<unknown>")
        ))
    })?;
    let tracker_identity = api_resource_identity(tracker_url);
    let existing_watches = Collection::<serde_json::Value>::fetch_all(
        client,
        &bug_collection_url(client, bug_id, "bug_watches"),
    )
    .await?
    .into_iter()
    .map(Raw::<BugWatch>::from_value)
    .collect::<Result<Vec<_>>>()?;
    let (raw_watch, watch_created) = match existing_watches.into_iter().find(|raw| {
        let w = &raw.value;
        let matches_tracker = w
            .bug_tracker_link
            .as_deref()
            .and_then(api_resource_identity)
            .is_some_and(|identity| Some(identity) == tracker_identity);
        let matches_bug = match github_ref {
            // GitHub remote bug IDs are numeric; compare numerically so that
            // e.g. "0658" still reuses the watch for issue 658.
            Some(reference) => remote_bug_is(w.remote_bug.as_deref(), reference.number()),
            None => w
                .remote_bug
                .as_deref()
                .map(|b| b.trim_start_matches('#') == remote_bug)
                .unwrap_or(false),
        };
        matches_tracker && matches_bug
    }) {
        Some(found) => (found, false),
        None => (
            add_bug_watch_inner(client, bug_id, tracker_url, remote_bug, style).await?,
            true,
        ),
    };
    let watch = &raw_watch.value;
    // Only a watch created by this call is a change made by this call.
    let created_style = if watch_created {
        style
    } else {
        ErrorStyle::Legacy
    };

    // Validate the actual watch URL before patching the task. The URL is
    // derived by Launchpad from the tracker type and base URL, so this is the
    // point where a mis-configured tracker becomes observable.
    match github_ref {
        Some(reference) => {
            validate_github_watch_url(watch, reference)
                .map_err(|error| watch_link_failure(bug_id, watch, error, created_style))?;
        }
        None => {
            if let Some(url) = watch.url.as_deref()
                && let Err(e) = url::Url::parse(url)
            {
                return Err(watch_link_failure(
                    bug_id,
                    watch,
                    LpError::Other(format!("Bug watch URL '{url}' is not a valid URL: {e}")),
                    created_style,
                ));
            }
        }
    }

    let watch_api_url = watch.self_link.as_deref().ok_or_else(|| {
        if watch_created {
            style.applied(format!(
                "Bug watch response has no self_link; the bug task was NOT linked. {}",
                watch_cleanup_guidance(bug_id, watch)
            ))
        } else {
            LpError::LocalNotFound("Bug watch has no self_link.".to_string())
        }
    })?;

    let body = serde_json::json!({ "bug_watch_link": watch_api_url });
    let response = client
        .patch_url_with_value::<serde_json::Value>(task_url, &body)
        .await
        .and_then(Raw::<BugTask>::from_value);
    let updated_task = response.map_err(|error| match (style, error) {
        // A success status with an unreadable body: the task is linked.
        (ErrorStyle::Partial, LpError::Json(error)) => LpError::Partial(format!(
            "Linking the bug task succeeded, but Launchpad's response could not be \
             read: {error}. Check `lpcli bug watches -b {bug_id}` and the task \
             ({task_url}) before retrying."
        )),
        (_, error) => task_link_failure(bug_id, watch, watch_created, error, style),
    })?;
    Ok((updated_task, raw_watch))
}

/// Whether `base_url` is an absolute URL with the given host (`url::Url`
/// lowercases hosts, so the comparison is effectively case-insensitive).
fn base_url_host_is(base_url: &str, host: &str) -> bool {
    url::Url::parse(base_url.trim())
        .ok()
        .is_some_and(|url| url.host_str() == Some(host))
}

/// Compare two tracker base URLs the way Launchpad's `queryByBaseURL`
/// permutations do: the `http`/`https` scheme and a single trailing slash
/// are insignificant, while host and path are compared case-sensitively (a
/// case-distinct path is a different tracker key, not a duplicate of the
/// same tracker).
fn base_urls_equivalent(a: &str, b: &str) -> bool {
    let (a, b) = match (url::Url::parse(a.trim()), url::Url::parse(b.trim())) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return false,
    };
    let scheme_equivalent = match (a.scheme(), b.scheme()) {
        ("http", "https") | ("https", "http") => true,
        (a_scheme, b_scheme) => a_scheme == b_scheme,
    };
    scheme_equivalent
        && a.host_str() == b.host_str()
        && a.port() == b.port()
        && a.path().trim_end_matches('/') == b.path().trim_end_matches('/')
        && a.query() == b.query()
}

/// Canonical identity of a Launchpad API resource URL.
///
/// Launchpad API links embed the API version in the path (`/devel/`,
/// `/beta/`, `/1.0/`, date-stamped versions). Two links to the same resource
/// via different API versions must compare equal, so the version segment is
/// stripped and the identity is `<host>/<path-after-version>` (hosts are
/// lowercased by `url::Url`; trailing slashes, queries and fragments are
/// ignored). Hosts are compared exactly, never by suffix.
fn api_resource_identity(url_text: &str) -> Option<String> {
    let url = url::Url::parse(url_text.trim()).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let mut segments: Vec<&str> = url.path().split('/').filter(|s| !s.is_empty()).collect();
    if !segments.is_empty() && is_api_version_segment(segments[0]) {
        segments.remove(0);
    }
    Some(format!("{host}/{}", segments.join("/")))
}

/// Whether a path segment names a Launchpad API version (`devel`, `beta`,
/// `1.0`, `2011-03-25`, ...).
fn is_api_version_segment(segment: &str) -> bool {
    segment == "devel"
        || segment == "beta"
        || (segment
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
            && segment.bytes().any(|b| b.is_ascii_digit()))
}

/// Shared strictness for GitHub web URLs: `https`, `github.com`, no explicit
/// port, no userinfo, no query or fragment.
///
/// `url::Url` normalises an explicit default port (`:443`) away and accepts
/// some non-canonical spellings, so the raw `text` must also spell the
/// authority exactly as `https://github.com` (host case-insensitive). This
/// keeps non-canonical base URLs from being registered or accepted.
fn strict_github_url(text: &str, url: &url::Url) -> bool {
    let canonical_authority = text
        .get(.."https://".len())
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
        && text["https://".len()..]
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|authority| authority.eq_ignore_ascii_case("github.com"));
    canonical_authority
        && url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

/// Split a URL path into segments, allowing at most one trailing empty
/// segment (a single trailing slash).
///
/// Returns `None` when the path contains empty internal segments (e.g.
/// `/o//r/issues`). Adjacent equal segments are valid: GitHub repositories
/// may share their owner's name (e.g. `/babel/babel/issues`) or be named
/// `issues` (e.g. `/owner/issues/issues`); the exact segment count and
/// keywords are enforced by the callers.
fn url_path_segments(path: &str) -> Option<Vec<&str>> {
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    let mut parts: Vec<&str> = trimmed.split('/').collect();
    if parts.last() == Some(&"") {
        parts.pop();
    }
    if parts.iter().any(|s| s.is_empty()) {
        return None;
    }
    Some(parts)
}

/// Parse a GitHub repository issues base URL into its `(owner, repo)` pair.
///
/// Only accepts the strict canonical shape
/// `https://github.com/{owner}/{repo}/issues` (a single trailing slash is
/// tolerated): https only, exact `github.com` host, no user information, no
/// explicit port, no query or fragment, no empty path segments. Owner and
/// repo are validated and lowercased.
///
/// # Errors
///
/// Returns [`LpError::Other`] for URLs that violate the strict shape above.
pub fn parse_github_issues_base_url(base_url: &str) -> Result<(String, String)> {
    let malformed = || {
        LpError::Other(format!(
            "Bug tracker base URL '{base_url}' does not look like a GitHub \
             repository issues URL (expected 'https://github.com/{{owner}}/{{repo}}/issues')."
        ))
    };
    let url = url::Url::parse(base_url.trim()).map_err(|e| {
        LpError::Other(format!(
            "Bug tracker base URL '{base_url}' is not a valid URL: {e}"
        ))
    })?;
    if !strict_github_url(base_url.trim(), &url) {
        return Err(malformed());
    }
    let segments = url_path_segments(url.path()).ok_or_else(malformed)?;
    if segments.len() != 3 || segments[2] != "issues" {
        return Err(malformed());
    }
    split_owner_repo(&format!("{}/{}", segments[0], segments[1]))
        .map(|(owner, repo)| (owner.to_ascii_lowercase(), repo.to_ascii_lowercase()))
}

/// Validate a tracker involved in a GitHub link — triggered when the tracker
/// is typed [`BUG_TRACKER_TYPE_GITHUB`] **or** when its base URL points at
/// `github.com` — and derive the [`GitHubBugRef`] implied by its base URL and
/// remote bug number.
///
/// Both guard directions are enforced before any mutation:
///
/// * A tracker with a github.com base URL but a different type is rejected:
///   Launchpad builds watch URLs from the type, so a mis-typed tracker
///   produces unusable URLs such as `https://github.com/detail?id=...` (the
///   registered `github-issues` trap).
/// * A tracker typed `GitHub Issues` without a base URL is rejected with an
///   actionable error: Launchpad cannot derive watch URLs at all.
/// * A tracker typed `GitHub Issues` with a base URL on another host —
///   including lookalike hosts such as `github.com.evil.com` — is rejected
///   by the strict `https://github.com/{owner}/{repo}/issues` shape check
///   (hosts are compared exactly, never by suffix).
///
/// With [`ErrorStyle::Partial`], a malformed `remote_bug` (caller input) is
/// reported as [`LpError::Usage`]; tracker configuration problems are not.
fn github_ref_from_tracker(
    tracker: &BugTracker,
    remote_bug: &str,
    style: ErrorStyle,
) -> Result<GitHubBugRef> {
    let name = tracker.name.as_deref().unwrap_or("<unnamed>");
    if tracker.bug_tracker_type.as_deref() != Some(BUG_TRACKER_TYPE_GITHUB) {
        return Err(LpError::Other(format!(
            "Bug tracker '{name}' has bug tracker type '{}', not \
             '{BUG_TRACKER_TYPE_GITHUB}'. Launchpad builds watch URLs from the \
             tracker type, so linking against this tracker produces unusable URLs \
             such as 'https://github.com/detail?id=...'. Use a tracker typed \
             '{BUG_TRACKER_TYPE_GITHUB}' with a 'https://github.com/{{owner}}/{{repo}}/issues' \
             base URL instead (see ensure_github_bug_tracker). The tracker was \
             NOT modified.",
            tracker.bug_tracker_type.as_deref().unwrap_or("<unset>")
        )));
    }
    let Some(base_url) = tracker.base_url.as_deref() else {
        return Err(LpError::Other(format!(
            "Bug tracker '{name}' is typed '{BUG_TRACKER_TYPE_GITHUB}' but has no \
             base_url, so Launchpad cannot derive a watch URL. Set the tracker's \
             base URL to 'https://github.com/{{owner}}/{{repo}}/issues' (or link \
             against a correctly configured tracker). It was NOT modified."
        )));
    };
    let (owner, repo) = parse_github_issues_base_url(base_url)?;
    let number = parse_github_number(remote_bug).map_err(|error| style.usage_error(error))?;
    GitHubBugRef::new(&owner, &repo, number)
}

/// Parse a GitHub issue/PR number: strictly decimal digits, positive, with
/// leading zeros normalised away (`007` becomes `7`; `+1`, `-1` and `1a` are
/// rejected).
fn parse_github_number(number: &str) -> Result<u64> {
    let trimmed = number.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return Err(LpError::Other(format!(
            "GitHub issue/PR number '{number}' must be a plain positive decimal \
             number (digits only)."
        )));
    }
    let parsed = trimmed
        .parse::<u64>()
        .map_err(|_| LpError::Other(format!("GitHub issue/PR number '{number}' is too large.")))?;
    if parsed == 0 {
        return Err(LpError::Other(
            "GitHub issue/PR number must be positive (0 is not a valid issue).".to_string(),
        ));
    }
    Ok(parsed)
}

/// Split and validate an `owner/repo` pair.
fn split_owner_repo(owner_repo: &str) -> Result<(&str, &str)> {
    let (owner, repo) = owner_repo.split_once('/').ok_or_else(|| {
        LpError::Other(format!(
            "'{owner_repo}' is not an 'owner/repo' GitHub repository path."
        ))
    })?;
    validate_github_owner(owner)?;
    validate_github_repo(repo)?;
    Ok((owner, repo))
}

/// Validate a GitHub owner (user or organisation) name.
fn validate_github_owner(owner: &str) -> Result<()> {
    let valid = (1..=39).contains(&owner.len())
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !owner.starts_with('-')
        && !owner.ends_with('-');
    if valid {
        Ok(())
    } else {
        Err(LpError::Other(format!(
            "'{owner}' is not a valid GitHub owner name (1-39 ASCII letters, \
             digits or single inner hyphens)."
        )))
    }
}

/// Validate a GitHub repository name.
fn validate_github_repo(repo: &str) -> Result<()> {
    let valid = (1..=100).contains(&repo.len())
        && repo
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !repo.contains("..");
    if valid {
        Ok(())
    } else {
        Err(LpError::Other(format!(
            "'{repo}' is not a valid GitHub repository name (1-100 ASCII letters, \
             digits, '-', '_' or '.', without '..')."
        )))
    }
}

/// Whether a watch's `remote_bug` identifier equals `number` numerically
/// (tolerating a leading `#` and leading zeros).
fn remote_bug_is(remote_bug: Option<&str>, number: u64) -> bool {
    remote_bug
        .and_then(|b| b.trim().trim_start_matches('#').parse::<u64>().ok())
        .is_some_and(|parsed| parsed == number)
}

/// Web-UI URL for deleting a bug watch (the Launchpad REST API cannot delete
/// watches).
fn watch_web_url(bug_id: u64, watch: &BugWatch) -> String {
    watch.web_link.clone().unwrap_or_else(|| {
        format!(
            "https://bugs.launchpad.net/bugs/{bug_id}/+watch/{}",
            watch.id().unwrap_or("<unknown>")
        )
    })
}

/// Guidance appended to errors that can leave a watch behind on the bug: a
/// PATCH failure may have committed server-side (timeout, malformed
/// response), and a factory 201 can return an existing watch after a race,
/// so one truthful message is used regardless of whether the watch was
/// created or reused by this call.
fn watch_cleanup_guidance(bug_id: u64, watch: &BugWatch) -> String {
    let watch_id = watch.id().unwrap_or("<unknown>");
    let web_url = watch_web_url(bug_id, watch);
    format!(
        "Watch #{watch_id} may remain on bug #{bug_id}; check \
         `lpcli bug watches -b {bug_id}` before retrying or deleting it in \
         the web UI: {web_url}"
    )
}

/// Build the error for a watch whose derived URL failed validation. The
/// error names the watch, its derived URL, and its web-UI page, and points
/// at `bug watches` for verification before retrying or deleting it.
fn watch_link_failure(
    bug_id: u64,
    watch: &BugWatch,
    detail: LpError,
    style: ErrorStyle,
) -> LpError {
    let url_text = watch.url.as_deref().unwrap_or("<none>");
    style.applied(format!(
        "{detail} (watch URL: {url_text}): {}",
        watch_cleanup_guidance(bug_id, watch)
    ))
}

/// Enrich a task-link (PATCH) failure that happened after a watch was just
/// created with the original error text plus the watch-cleanup guidance.
/// Failures for reused watches propagate unchanged (nothing irreversible
/// was created by this call).
fn task_link_failure(
    bug_id: u64,
    watch: &BugWatch,
    watch_created: bool,
    error: LpError,
    style: ErrorStyle,
) -> LpError {
    if watch_created {
        style.applied(format!(
            "{error}: {}",
            watch_cleanup_guidance(bug_id, watch)
        ))
    } else {
        error
    }
}

/// Parse a Launchpad `target_link` URL and return `(target_name, series)`.
///
/// Launchpad source-package tasks have two URL forms:
///
/// | URL path (relative to API root)        | target_name | series        |
/// |----------------------------------------|-------------|---------------|
/// | `/ubuntu/+source/{pkg}`                | `pkg`       | `None`        |
/// | `/ubuntu/{series}/+source/{pkg}`       | `pkg`       | `Some(series)`|
/// | `/{project}`                           | `project`   | `None`        |
///
/// The returned `target_name` is the short package or project name that matches
/// the value accepted by `--target` on the `set-status`, `set-importance`, and
/// `set-assignee` sub-commands.
pub fn parse_target_link(url: &str) -> (String, Option<String>) {
    // Strip common API base URL prefixes and leading slashes.
    let stripped = url
        .trim_start_matches("https://api.launchpad.net/devel")
        .trim_start_matches("https://api.staging.launchpad.net/devel")
        .trim_start_matches('/');

    let parts: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();

    if let Some(src_idx) = parts.iter().position(|&p| p == "+source") {
        // Source-package task: name is the segment after "+source".
        let pkg = parts.get(src_idx + 1).unwrap_or(&"").to_string();
        // Series is the segment between the distro (index 0) and "+source" when
        // src_idx > 1 (i.e. the path is /distro/series/+source/pkg).
        let series = if src_idx > 1 {
            parts.get(1).map(|s| s.to_string())
        } else {
            None
        };
        (pkg, series)
    } else {
        // Project task: the name is the first path segment.
        let name = parts.first().unwrap_or(&"").to_string();
        (name, None)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_task_status_display() {
        assert_eq!(BugTaskStatus::New.to_string(), "New");
        assert_eq!(BugTaskStatus::FixReleased.to_string(), "Fix Released");
        assert_eq!(BugTaskStatus::WontFix.to_string(), "Won't Fix");
        assert_eq!(BugTaskStatus::InProgress.to_string(), "In Progress");
    }

    #[test]
    fn bug_importance_display() {
        assert_eq!(BugImportance::Critical.to_string(), "Critical");
        assert_eq!(BugImportance::Wishlist.to_string(), "Wishlist");
        assert_eq!(BugImportance::Undecided.to_string(), "Undecided");
    }

    #[test]
    fn bug_deserialise_minimal() {
        let json = r#"{
            "id": 12345,
            "title": "App crashes on startup",
            "description": null,
            "tags": [],
            "date_created": null,
            "date_last_updated": null,
            "self_link": null,
            "web_link": null,
            "owner_link": null,
            "users_affected_count": null
        }"#;
        let bug: Bug = serde_json::from_str(json).unwrap();
        assert_eq!(bug.id, 12345);
        assert_eq!(bug.title, "App crashes on startup");
        assert!(bug.tags.is_empty());
    }

    #[test]
    fn bug_deserialise_with_tags() {
        let json = r#"{
            "id": 99,
            "title": "Bug with tags",
            "description": "Some description",
            "tags": ["regression", "focal"],
            "date_created": null,
            "date_last_updated": null,
            "self_link": "https://api.launchpad.net/devel/bugs/99",
            "web_link": "https://bugs.launchpad.net/bugs/99",
            "owner_link": "https://api.launchpad.net/devel/~user",
            "users_affected_count": 5
        }"#;
        let bug: Bug = serde_json::from_str(json).unwrap();
        assert_eq!(bug.tags, vec!["regression", "focal"]);
        assert_eq!(bug.users_affected_count, Some(5));
    }

    #[test]
    fn bug_deserialise_with_duplicate_fields() {
        let json = r#"{
            "id": 101,
            "title": "Duplicate bug",
            "description": null,
            "tags": [],
            "date_created": null,
            "date_last_updated": null,
            "self_link": "https://api.launchpad.net/devel/bugs/101",
            "web_link": "https://bugs.launchpad.net/bugs/101",
            "owner_link": null,
            "users_affected_count": null,
            "duplicate_of_link": "https://api.launchpad.net/devel/bugs/100",
            "number_of_duplicates": 3
        }"#;
        let bug: Bug = serde_json::from_str(json).unwrap();
        assert_eq!(
            bug.duplicate_of_link.as_deref(),
            Some("https://api.launchpad.net/devel/bugs/100")
        );
        assert_eq!(bug.number_of_duplicates, Some(3));

        let cleared: Bug = serde_json::from_str(
            r#"{"id": 102, "title": "Clean bug", "tags": [], "duplicate_of_link": null}"#,
        )
        .unwrap();
        assert_eq!(cleared.duplicate_of_link, None);
        assert_eq!(cleared.number_of_duplicates, None);
    }

    #[test]
    fn urlenc_encodes_spaces_and_special_chars() {
        assert!(!urlenc("Fix Released").contains(' '));
        assert!(!urlenc("Won't Fix").contains('\''));
    }

    #[test]
    fn parse_target_link_project() {
        let (name, series) = parse_target_link("https://api.launchpad.net/devel/rust-alacritty");
        assert_eq!(name, "rust-alacritty");
        assert_eq!(series, None);
    }

    #[test]
    fn parse_target_link_ubuntu_source_no_series() {
        let (name, series) =
            parse_target_link("https://api.launchpad.net/devel/ubuntu/+source/rust-alacritty");
        assert_eq!(name, "rust-alacritty");
        assert_eq!(series, None);
    }

    #[test]
    fn parse_target_link_ubuntu_source_with_series() {
        let (name, series) = parse_target_link(
            "https://api.launchpad.net/devel/ubuntu/noble/+source/rust-alacritty",
        );
        assert_eq!(name, "rust-alacritty");
        assert_eq!(series, Some("noble".to_string()));
    }

    #[test]
    fn bug_search_params_default() {
        let params = BugSearchParams::default();
        assert!(params.status.is_none());
        assert!(params.importance.is_none());
        assert!(params.limit.is_none());
    }

    #[test]
    fn bug_tracker_deserialise() {
        let json = r#"{
            "self_link": "https://api.launchpad.net/devel/bugs/bugtrackers/debbugs",
            "name": "debbugs",
            "title": "Debian Bug tracker",
            "summary": "Bug tracker for debian project.",
            "base_url": "https://bugs.debian.org/",
            "bug_tracker_type": "Debbugs",
            "web_link": "https://bugs.launchpad.net/bugs/bugtrackers/debbugs"
        }"#;
        let tracker: BugTracker = serde_json::from_str(json).unwrap();
        assert_eq!(tracker.name.as_deref(), Some("debbugs"));
        assert_eq!(tracker.title.as_deref(), Some("Debian Bug tracker"));
        assert_eq!(tracker.bug_tracker_type.as_deref(), Some("Debbugs"));
        assert_eq!(
            tracker.base_url.as_deref(),
            Some("https://bugs.debian.org/")
        );
    }

    #[test]
    fn bug_watch_deserialise() {
        let json = r#"{
            "self_link": "https://api.launchpad.net/devel/bugs/2120577/+watch/166955",
            "bug_link": "https://api.launchpad.net/devel/bugs/2120577",
            "bug_tracker_link": "https://api.launchpad.net/devel/bugs/bugtrackers/debbugs",
            "remote_bug": "1150409",
            "title": "Debian Bug tracker #1150409",
            "url": "https://bugs.debian.org/cgi-bin/bugreport.cgi?bug=1150409",
            "web_link": "https://bugs.launchpad.net/bugs/2120577/+watch/166955",
            "remote_status": "open wishlist patch",
            "remote_importance": "UNKNOWN"
        }"#;
        let watch: BugWatch = serde_json::from_str(json).unwrap();
        assert_eq!(watch.remote_bug.as_deref(), Some("1150409"));
        assert_eq!(watch.id(), Some("166955"));
        assert_eq!(watch.remote_status.as_deref(), Some("open wishlist patch"));
    }

    #[test]
    fn bug_task_deserialise_with_watch() {
        let json = r#"{
            "self_link": "https://api.launchpad.net/devel/debian/+source/arctica-greeter/+bug/2120577",
            "bug_link": "https://api.launchpad.net/devel/bugs/2120577",
            "title": "Bug #2120577 in arctica-greeter (Debian)",
            "status": "New",
            "importance": "Unknown",
            "bug_target_display_name": "arctica-greeter (Debian)",
            "target_link": "https://api.launchpad.net/devel/debian/+source/arctica-greeter",
            "bug_watch_link": "https://api.launchpad.net/devel/bugs/2120577/+watch/166955"
        }"#;
        let task: BugTask = serde_json::from_str(json).unwrap();
        assert_eq!(
            task.bug_watch_link.as_deref(),
            Some("https://api.launchpad.net/devel/bugs/2120577/+watch/166955")
        );
        assert_eq!(task.bug_watch_id(), Some("166955"));
    }

    // -----------------------------------------------------------------------
    // GitHub reference parsing and validation
    // -----------------------------------------------------------------------

    #[test]
    fn parse_github_bug_ref_accepts_shorthand_and_urls() {
        let shorthand = parse_github_bug_ref("canonical/apport#658").unwrap();
        assert_eq!(shorthand.owner(), "canonical");
        assert_eq!(shorthand.repo(), "apport");
        assert_eq!(shorthand.number(), 658);

        let pull = parse_github_bug_ref("https://github.com/canonical/apport/pull/9").unwrap();
        assert_eq!(pull.number(), 9);
        assert_eq!(pull.repo(), "apport");

        let issues = parse_github_bug_ref(" https://github.com/Owner/Repo/issues/12/ ").unwrap();
        assert_eq!(issues.owner(), "owner");
        assert_eq!(issues.repo(), "repo");
        assert_eq!(issues.number(), 12);

        // Case is canonicalised to lower case.
        assert_eq!(parse_github_bug_ref("O/R#1").unwrap().to_string(), "o/r#1");

        // Equal owner and repo names, and a repository named `issues`
        // (path `issues/issues`), are valid GitHub repositories.
        let same = parse_github_bug_ref("babel/babel#658").unwrap();
        assert_eq!(
            (same.owner(), same.repo(), same.number()),
            ("babel", "babel", 658)
        );
        let issues_url = parse_github_bug_ref("https://github.com/babel/babel/issues/658").unwrap();
        assert_eq!(
            (issues_url.owner(), issues_url.repo(), issues_url.number()),
            ("babel", "babel", 658)
        );
        let pull_url = parse_github_bug_ref("https://github.com/babel/babel/pull/9").unwrap();
        assert_eq!(
            (pull_url.owner(), pull_url.repo(), pull_url.number()),
            ("babel", "babel", 9)
        );
        let issues_repo =
            parse_github_bug_ref("https://github.com/babel/issues/issues/658").unwrap();
        assert_eq!(
            (
                issues_repo.owner(),
                issues_repo.repo(),
                issues_repo.number()
            ),
            ("babel", "issues", 658)
        );
    }

    #[test]
    fn parse_github_bug_ref_normalises_leading_zeros() {
        let r = parse_github_bug_ref("canonical/apport#007").unwrap();
        assert_eq!(r.number(), 7);
        assert_eq!(
            r.watch_url(),
            "https://github.com/canonical/apport/issues/7"
        );
    }

    #[test]
    fn parse_github_bug_ref_rejects_malformed_input() {
        for bad in [
            "",
            "owner/repo",
            "owner#5",
            "owner/repo#",
            "owner/repo#0",
            "owner/repo#+1",
            "owner/repo#-1",
            "owner/repo#1a",
            "owner/repo#1 2",
            "-owner/repo#5",
            "owner//repo#5",
            "github.com/owner/repo#5",
            "http://github.com/owner/repo/issues/5",
            "git://github.com/owner/repo/issues/5",
            "https://user:pass@github.com/owner/repo/issues/5",
            "https://github.com:8443/owner/repo/issues/5",
            "https://github.com:443/owner/repo/issues/5",
            "https://github.com:/owner/repo/issues/5",
            "https:github.com/owner/repo/issues/5",
            "https://@github.com/owner/repo/issues/5",
            "https://github.com//owner/repo/issues/5",
            "https://github.com/owner//repo/issues/5",
            "https://gitlab.com/owner/repo/issues/5",
            "https://github.com/owner/issues/5",
            "https://github.com/owner/repo/pulls/5",
            "https://github.com/owner/repo/issues/abc",
            "https://github.com/owner/repo/issues/5?x=1",
            "https://github.com/owner/repo/issues/5#frag",
        ] {
            assert!(
                parse_github_bug_ref(bad).is_err(),
                "expected '{bad}' to be rejected"
            );
        }
    }

    #[test]
    fn github_ref_derives_launchpad_tracker_fields() {
        let r = GitHubBugRef::new("Canonical", "apport", 658).unwrap();
        assert_eq!(r.base_url(), "https://github.com/canonical/apport/issues");
        assert_eq!(
            r.watch_url(),
            "https://github.com/canonical/apport/issues/658"
        );
    }

    #[test]
    fn github_ref_new_rejects_invalid_parts() {
        assert!(GitHubBugRef::new("", "repo", 1).is_err());
        assert!(GitHubBugRef::new("-owner", "repo", 1).is_err());
        assert!(GitHubBugRef::new("owner-", "repo", 1).is_err());
        assert!(GitHubBugRef::new("owner", "", 1).is_err());
        assert!(GitHubBugRef::new("owner", "..", 1).is_err());
        assert!(GitHubBugRef::new("owner", "repo", 0).is_err());
    }

    #[test]
    fn base_urls_equivalent_follows_launchpad_permutations() {
        // Trailing slash and http/https scheme are insignificant.
        assert!(base_urls_equivalent(
            "https://github.com/o/r/issues/",
            "https://github.com/o/r/issues"
        ));
        assert!(base_urls_equivalent(
            "http://example.com/bugs",
            "https://example.com/bugs/"
        ));
        // Host and path are case-sensitive: a case-distinct path is a
        // different tracker key, not a duplicate of the same tracker.
        assert!(!base_urls_equivalent(
            "https://example.com/Bugs",
            "https://example.com/bugs"
        ));
        assert!(!base_urls_equivalent(
            "https://github.com/o/r/issues",
            "https://github.com/o/r"
        ));
        // Ports and query strings must match too.
        assert!(!base_urls_equivalent(
            "https://example.com:8443/bugs",
            "https://example.com/bugs"
        ));
        assert!(!base_urls_equivalent(
            "https://example.com/bugs?a=1",
            "https://example.com/bugs"
        ));
        assert!(!base_urls_equivalent(
            "not a url",
            "https://example.com/bugs"
        ));
    }

    #[test]
    fn api_resource_identity_ignores_api_version() {
        let devel = "https://api.launchpad.net/devel/bugs/bugtrackers/debbugs";
        let v1 = "https://api.launchpad.net/1.0/bugs/bugtrackers/debbugs";
        assert_eq!(api_resource_identity(devel), api_resource_identity(v1));
        // A different resource has a different identity; invalid URLs have
        // none.
        assert_ne!(
            api_resource_identity(devel),
            api_resource_identity("https://api.launchpad.net/devel/bugs/bugtrackers/other")
        );
        assert_eq!(api_resource_identity("not a url"), None);
        // Beta and date-stamped versions are recognised too.
        assert_eq!(
            api_resource_identity("https://api.launchpad.net/beta/bugs/bugtrackers/debbugs"),
            api_resource_identity("https://api.launchpad.net/2011-03-25/bugs/bugtrackers/debbugs")
        );
    }

    #[test]
    fn validate_github_watch_url_accepts_issues_and_pull() {
        let r = GitHubBugRef::new("canonical", "apport", 658).unwrap();
        let watch = |url: &str| BugWatch {
            url: Some(url.to_string()),
            ..Default::default()
        };
        assert_eq!(
            validate_github_watch_url(&watch("https://github.com/canonical/apport/issues/658"), &r)
                .unwrap(),
            "https://github.com/canonical/apport/issues/658"
        );
        validate_github_watch_url(&watch("https://github.com/canonical/apport/pull/658"), &r)
            .unwrap();
        // Case differences and a trailing slash are accepted.
        validate_github_watch_url(
            &watch("https://github.com/Canonical/Apport/issues/658/"),
            &r,
        )
        .unwrap();
        // Equal owner/repo names and a repository named `issues` are valid:
        // the derived URL then contains equal adjacent segments.
        let same = GitHubBugRef::new("babel", "babel", 658).unwrap();
        validate_github_watch_url(&watch("https://github.com/babel/babel/issues/658"), &same)
            .unwrap();
        validate_github_watch_url(&watch("https://github.com/babel/babel/pull/658"), &same)
            .unwrap();
        let issues_repo = GitHubBugRef::new("babel", "issues", 658).unwrap();
        validate_github_watch_url(
            &watch("https://github.com/babel/issues/issues/658"),
            &issues_repo,
        )
        .unwrap();
    }

    #[test]
    fn validate_github_watch_url_rejects_mismatched_shapes() {
        let r = GitHubBugRef::new("canonical", "apport", 658).unwrap();
        let watch = |url: &str| BugWatch {
            url: Some(url.to_string()),
            ..Default::default()
        };
        // The 'github-issues' Google Code trap shape.
        assert!(validate_github_watch_url(&watch("https://github.com/detail?id=658"), &r).is_err());
        // `url::Url` normalises these spellings away, so the raw text is
        // checked too: explicit ports and non-canonical authorities fail.
        for non_canonical in [
            "https://github.com:443/canonical/apport/issues/658",
            "https://github.com:/canonical/apport/issues/658",
            "https:github.com/canonical/apport/issues/658",
            "https:\\\\github.com\\canonical\\apport\\issues\\658",
            "https://@github.com/canonical/apport/issues/658",
        ] {
            assert!(
                validate_github_watch_url(&watch(non_canonical), &r).is_err(),
                "'{non_canonical}' must be rejected"
            );
        }
        assert!(
            validate_github_watch_url(&watch("https://github.com/canonical/apport/issues/657"), &r)
                .is_err()
        );
        assert!(
            validate_github_watch_url(&watch("https://github.com/other/apport/issues/658"), &r)
                .is_err()
        );
        assert!(
            validate_github_watch_url(&watch("https://gitlab.com/canonical/apport/issues/658"), &r)
                .is_err()
        );
        // Leading zeros are not canonical.
        assert!(
            validate_github_watch_url(
                &watch("https://github.com/canonical/apport/issues/0658"),
                &r
            )
            .is_err()
        );
        assert!(
            validate_github_watch_url(
                &watch("https://github.com/canonical/apport/issues/658?x=1"),
                &r
            )
            .is_err()
        );
        // Strict https / no userinfo / no non-default port / no empty or
        // repeated path segments.
        assert!(
            validate_github_watch_url(&watch("http://github.com/canonical/apport/issues/658"), &r)
                .is_err()
        );
        assert!(
            validate_github_watch_url(
                &watch("https://user:pass@github.com/canonical/apport/issues/658"),
                &r
            )
            .is_err()
        );
        assert!(
            validate_github_watch_url(
                &watch("https://github.com:8443/canonical/apport/issues/658"),
                &r
            )
            .is_err()
        );
        assert!(
            validate_github_watch_url(
                &watch("https://github.com/canonical//apport/issues/658"),
                &r
            )
            .is_err()
        );
        assert!(
            validate_github_watch_url(
                &watch("https://github.com/canonical/canonical/apport/issues/658"),
                &r
            )
            .is_err()
        );
        assert!(validate_github_watch_url(&BugWatch::default(), &r).is_err());
    }

    #[test]
    fn parse_github_issues_base_url_accepts_only_issues_urls() {
        let (owner, repo) =
            parse_github_issues_base_url("https://github.com/Canonical/Apport/issues/").unwrap();
        assert_eq!((owner.as_str(), repo.as_str()), ("canonical", "apport"));
        // Equal owner/repo names and a repository named `issues` (path
        // `issues/issues`) are valid; only empty segments are rejected.
        let (owner, repo) =
            parse_github_issues_base_url("https://github.com/babel/babel/issues").unwrap();
        assert_eq!((owner.as_str(), repo.as_str()), ("babel", "babel"));
        let (owner, repo) =
            parse_github_issues_base_url("https://github.com/babel/issues/issues").unwrap();
        assert_eq!((owner.as_str(), repo.as_str()), ("babel", "issues"));
        assert!(parse_github_issues_base_url("https://github.com/").is_err());
        assert!(parse_github_issues_base_url("https://github.com/o/r").is_err());
        assert!(parse_github_issues_base_url("https://github.com/o/r/pulls").is_err());
        assert!(parse_github_issues_base_url("https://gitlab.com/o/r/issues").is_err());
        // Strict https, no userinfo, no non-default port, no empty or
        // repeated internal path segments; a single trailing slash is fine.
        assert!(parse_github_issues_base_url("git://github.com/o/r/issues").is_err());
        assert!(parse_github_issues_base_url("http://github.com/o/r/issues").is_err());
        assert!(parse_github_issues_base_url("https://user:pass@github.com/o/r/issues").is_err());
        assert!(parse_github_issues_base_url("https://github.com:8443/o/r/issues").is_err());
        assert!(parse_github_issues_base_url("https://github.com:443/o/r/issues").is_err());
        assert!(parse_github_issues_base_url("https:github.com/o/r/issues").is_err());
        // The host is case-insensitive.
        assert!(parse_github_issues_base_url("HTTPS://GitHub.com/o/r/issues").is_ok());
        assert!(parse_github_issues_base_url("https://github.com/o//r/issues").is_err());
        assert!(parse_github_issues_base_url("https://github.com/o/o/r/issues").is_err());
        assert!(parse_github_issues_base_url("https://github.com/o/r/issues//").is_err());
        assert!(parse_github_issues_base_url("https://github.com/o/r/issues?x=1").is_err());
        assert!(parse_github_issues_base_url("https://github.com/o/r/issues#frag").is_err());
    }

    #[test]
    fn bug_tracker_type_constants_match_launchpad_values() {
        // Value verified against the Launchpad service WADL; the other
        // tracker types are spelled out as literals by the CLI.
        assert_eq!(BUG_TRACKER_TYPE_GITHUB, "GitHub Issues");
    }

    #[test]
    fn remote_bug_is_compares_numerically() {
        assert!(remote_bug_is(Some("#658"), 658));
        assert!(remote_bug_is(Some("0658"), 658));
        assert!(!remote_bug_is(Some("659"), 658));
        assert!(!remote_bug_is(Some("not-a-number"), 658));
        assert!(!remote_bug_is(None, 658));
    }

    // -----------------------------------------------------------------------
    // Remote-linking API origin guard
    // -----------------------------------------------------------------------

    #[test]
    fn ensure_same_api_origin_pure_cases() {
        let client = LaunchpadClient::new(None).with_base_url("https://api.launchpad.net/devel");
        // Same origin: a different API-version path and an explicit default
        // port (same effective port) are both accepted.
        ensure_same_api_origin(
            &client,
            "https://api.launchpad.net/1.0/bugs/42",
            "Bug task URL",
        )
        .unwrap();
        ensure_same_api_origin(
            &client,
            "https://api.launchpad.net:443/devel/bugs/42",
            "Bug task URL",
        )
        .unwrap();
        // Exact scheme/hostname/effective-port comparison: deceptive host
        // suffixes, scheme mismatches, port mismatches, non-http(s) schemes
        // and malformed URLs are all rejected.
        for foreign in [
            "https://api.launchpad.net.evil.com/devel/bugs/42",
            "http://api.launchpad.net/devel/bugs/42",
            "https://api.launchpad.net:8443/devel/bugs/42",
            "ftp://api.launchpad.net/devel/bugs/42",
            "/devel/bugs/42",
            "not a url",
        ] {
            assert!(
                ensure_same_api_origin(&client, foreign, "Bug task URL").is_err(),
                "'{foreign}' must be rejected by the API origin guard"
            );
        }
    }
}
