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
//! | [`get_bug_comments`] | List comments on a bug |
//! | [`delete_bug_comment`] | Delete a comment from a bug |
//! | [`set_bug_tags`] | Replace the tag list on a bug |
//! | [`get_bug_tracker_by_name`] | Fetch a bug tracker by name |
//! | [`get_bug_trackers`] | List registered remote bug trackers |
//! | [`get_bug_tracker`] | Fetch a bug tracker by API self-link |
//! | [`add_bug_watch`] | Create a new bug watch on a bug |
//! | [`get_bug_watches`] | List all bug watches for a bug |
//! | [`get_bug_watch`] | Fetch a bug watch by API self-link |
//! | [`set_bug_watch`] | Link a bug task to a bug watch |
//! | [`unset_bug_watch`] | Unlink a bug watch from a bug task |
//! | [`link_bug_task_remote`] | Look up tracker, create/find watch, and link bug task |

use chrono::{DateTime, Utc};
use futures_util::future::try_join_all;
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
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
    pub date_created: Option<DateTime<Utc>>,
    /// When the bug was last updated.
    pub date_last_updated: Option<DateTime<Utc>>,
    /// API self-link for this bug.
    pub self_link: Option<String>,
    /// URL of the bug in the Launchpad web UI.
    pub web_link: Option<String>,
    /// Display name of the person who filed the bug.
    pub owner_link: Option<String>,
    /// Number of users who are affected by this bug.
    pub users_affected_count: Option<u64>,
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
    pub date_created: Option<DateTime<Utc>>,
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
    pub date_created: Option<DateTime<Utc>>,
    /// Date this watch was last checked by Launchpad.
    pub date_last_checked: Option<DateTime<Utc>>,
    /// Date this watch was last changed on the remote tracker.
    pub date_last_changed: Option<DateTime<Utc>>,
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
    pub date_created: Option<DateTime<Utc>>,
    /// When the comment was deleted (if deleted).
    pub date_deleted: Option<DateTime<Utc>>,
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

/// Fetch all bug tasks associated with a bug.
pub async fn get_bug_tasks(client: &LaunchpadClient, bug_id: u64) -> Result<Vec<BugTask>> {
    let url = client.url(&format!("/bugs/{bug_id}/bug_tasks"));
    Collection::fetch_all(client, &url).await
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
    let url = client.url(&query);
    // ws.size sets the Launchpad page size, not a total cap.  fetch_page
    // requests exactly one page so the user-supplied limit is honoured.
    Collection::fetch_page(client, &url).await
}

/// File a new bug on a Launchpad project.
pub async fn create_bug(
    client: &LaunchpadClient,
    target: &str,
    title: &str,
    description: &str,
) -> Result<Bug> {
    use std::collections::HashMap;
    let target_url = client.url(&format!("/{target}"));
    let mut params = HashMap::new();
    params.insert("ws.op", "createBug");
    params.insert("title", title);
    params.insert("description", description);
    params.insert("target", target_url.as_str());
    let location = client.post_created_location("/bugs", &params).await?;
    client.get_url(&location).await
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
    use std::collections::HashMap;
    let mut params = HashMap::new();
    params.insert("ws.op", "addTask");
    params.insert("target", target_url);
    let location = client
        .post_created_location(&format!("/bugs/{bug_id}"), &params)
        .await?;
    client.get_url(&location).await
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

/// Fetch comments for a bug.
pub async fn get_bug_comments(client: &LaunchpadClient, bug_id: u64) -> Result<Vec<BugComment>> {
    let url = client.url(&format!("/bugs/{bug_id}/messages"));
    Collection::fetch_all(client, &url).await
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
/// permitted to delete the comment.
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

    let comment = comments
        .iter()
        .enumerate()
        .find(|(i, comment)| comment.index.unwrap_or(*i as u64) == comment_index)
        .map(|(_, comment)| comment)
        .ok_or_else(|| {
            crate::error::LpError::NotFound(format!(
                "Comment #{comment_index} does not exist on bug #{bug_id}."
            ))
        })?;
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

    for attachment in &comment_attachments {
        let attachment_url = attachment.self_link.as_deref().unwrap();
        let mut params = HashMap::new();
        params.insert("ws.op", "removeFromBug");
        client.post_url_ok(attachment_url, &params).await?;
    }

    // Delete the comment text.
    let mut params = HashMap::new();
    params.insert("ws.op", "deleteContent");
    client.post_url_ok(comment_url, &params).await
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

    client
        .post_multipart_created_location(&format!("/bugs/{bug_id}"), form)
        .await
}

/// Fetch all attachments for a bug.
pub async fn get_bug_attachments(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Vec<BugAttachment>> {
    let url = client.url(&format!("/bugs/{bug_id}/attachments"));
    Collection::fetch_all(client, &url).await
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
    pub date_created: Option<DateTime<Utc>>,
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
    let url = client.url(&format!("/bugs/{bug_id}/subscriptions"));
    Collection::fetch_all(client, &url).await
}

/// Fetch a bug tracker by its unique name (e.g. `"debbugs"`).
///
/// Calls `bug_trackers.getByName(name)`.
///
/// # Errors
///
/// Returns [`crate::error::LpError::NotFound`] if the tracker does not exist.
pub async fn get_bug_tracker_by_name(client: &LaunchpadClient, name: &str) -> Result<BugTracker> {
    let url = client.url(&format!(
        "/bugs/bugtrackers?ws.op=getByName&name={}",
        urlenc(name.trim())
    ));
    let tracker: Option<BugTracker> = client.get_url(&url).await?;
    tracker.ok_or_else(|| LpError::NotFound(format!("Bug tracker '{name}' not found.")))
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
    const PAGE_SIZE: usize = 100;

    let mut page_url = client.url(&format!("/bugs/bugtrackers?ws.size={PAGE_SIZE}"));
    let search = search.map(str::to_lowercase);
    let mut matches = Vec::new();

    loop {
        let page: Collection<BugTracker> = client.get_url(&page_url).await?;
        for tracker in page.entries {
            let matches_search = search.as_deref().is_none_or(|search| {
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
            });

            if matches_search {
                matches.push(tracker);
                if limit.is_some_and(|limit| matches.len() >= limit) {
                    return Ok(matches);
                }
            }
        }

        let Some(next_url) = page.next_collection_link else {
            return Ok(matches);
        };
        page_url = next_url;
    }
}

/// Fetch a bug tracker by its API self-link URL.
pub async fn get_bug_tracker(client: &LaunchpadClient, tracker_url: &str) -> Result<BugTracker> {
    client.get_url(tracker_url).await
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
pub async fn add_bug_watch(
    client: &LaunchpadClient,
    bug_id: u64,
    bug_tracker_url: &str,
    remote_bug: &str,
) -> Result<BugWatch> {
    use std::collections::HashMap;
    let clean_remote = remote_bug.trim().trim_start_matches('#');
    let mut params = HashMap::new();
    params.insert("ws.op", "addWatch");
    params.insert("bug_tracker", bug_tracker_url);
    params.insert("remote_bug", clean_remote);
    let location = client
        .post_created_location(&format!("/bugs/{bug_id}"), &params)
        .await?;
    client.get_url(&location).await
}

/// Fetch all bug watches associated with a bug.
pub async fn get_bug_watches(client: &LaunchpadClient, bug_id: u64) -> Result<Vec<BugWatch>> {
    let url = client.url(&format!("/bugs/{bug_id}/bug_watches"));
    Collection::fetch_all(client, &url).await
}

/// Fetch a single bug watch by its API URL.
pub async fn get_bug_watch(client: &LaunchpadClient, watch_url: &str) -> Result<BugWatch> {
    client.get_url(watch_url).await
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

/// Unlink a bug watch from a bug task.
///
/// `task_url` must be the API self-link of the bug task.
///
/// Returns the updated [`BugTask`].
pub async fn unset_bug_watch(client: &LaunchpadClient, task_url: &str) -> Result<BugTask> {
    let body = serde_json::json!({ "bug_watch_link": null });
    client.patch_url_with_value(task_url, &body).await
}

/// Link a bug task to a remote bug on an external bug tracker.
///
/// This performs the three steps required by Launchpad:
/// 1. Look up the remote bug tracker by name using [`get_bug_tracker_by_name`].
/// 2. Find an existing watch for that tracker and remote bug or create one via [`add_bug_watch`].
/// 3. Link the bug task to the bug watch via [`set_bug_watch`].
///
/// Returns the updated [`BugTask`] and the associated [`BugWatch`].
pub async fn link_bug_task_remote(
    client: &LaunchpadClient,
    bug_id: u64,
    task_url: &str,
    bug_tracker: &str,
    remote_bug: &str,
) -> Result<(BugTask, BugWatch)> {
    let clean_remote_bug = remote_bug.trim().trim_start_matches('#');
    let tracker = if bug_tracker.starts_with("http://") || bug_tracker.starts_with("https://") {
        get_bug_tracker(client, bug_tracker).await?
    } else {
        get_bug_tracker_by_name(client, bug_tracker).await?
    };

    let tracker_url = tracker.self_link.as_deref().ok_or_else(|| {
        LpError::NotFound(format!("Bug tracker '{bug_tracker}' has no self_link."))
    })?;

    // Check if an existing watch on this bug already matches the tracker and remote bug.
    let existing_watches = get_bug_watches(client, bug_id).await?;
    let watch = if let Some(found) = existing_watches.into_iter().find(|w| {
        let matches_tracker = w
            .bug_tracker_link
            .as_deref()
            .map(|link| link.trim_end_matches('/') == tracker_url.trim_end_matches('/'))
            .unwrap_or(false);
        let matches_bug = w
            .remote_bug
            .as_deref()
            .map(|b| b.trim_start_matches('#') == clean_remote_bug)
            .unwrap_or(false);
        matches_tracker && matches_bug
    }) {
        found
    } else {
        add_bug_watch(client, bug_id, tracker_url, clean_remote_bug).await?
    };

    let watch_url = watch
        .self_link
        .as_deref()
        .ok_or_else(|| LpError::NotFound("Bug watch has no self_link.".to_string()))?;

    let updated_task = set_bug_watch(client, task_url, watch_url).await?;
    Ok((updated_task, watch))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn urlenc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
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
}
