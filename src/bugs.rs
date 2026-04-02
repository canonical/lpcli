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
//! | [`set_bug_status`] | Update the status of a bug task |
//! | [`set_bug_importance`] | Update the importance of a bug task |
//! | [`add_bug_comment`] | Add a comment to a bug |
//! | [`get_bug_comments`] | List comments on a bug |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// API link to the author.
    pub owner_link: Option<String>,
    /// Sequence number within the bug.
    pub index: Option<u64>,
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
/// Returns [`LpError::NotFound`] if the bug does not exist.
pub async fn get_bug(client: &LaunchpadClient, bug_id: u64) -> Result<Bug> {
    client.get(&format!("/bugs/{bug_id}")).await
}

/// Fetch all bug tasks associated with a bug.
pub async fn get_bug_tasks(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Vec<BugTask>> {
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
        format!("/{}/+source/{}?ws.op=searchTasks", urlenc(target), urlenc(pkg))
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

/// Update the status of a bug task identified by its API self-link.
pub async fn set_bug_status(
    client: &LaunchpadClient,
    task_url: &str,
    status: &str,
) -> Result<BugTask> {
    use std::collections::HashMap;
    let mut params = HashMap::new();
    params.insert("status", status);
    client.patch_url(task_url, &params).await
}

/// Update the importance of a bug task identified by its API self-link.
pub async fn set_bug_importance(
    client: &LaunchpadClient,
    task_url: &str,
    importance: &str,
) -> Result<BugTask> {
    use std::collections::HashMap;
    let mut params = HashMap::new();
    params.insert("importance", importance);
    client.patch_url(task_url, &params).await
}

/// Add a comment to a bug.
///
/// The Launchpad `newMessage` operation returns `201 Created` with an empty
/// body (the new message URL is in the `Location` header), so this function
/// returns `()` rather than trying to deserialise a response object.
pub async fn add_bug_comment(
    client: &LaunchpadClient,
    bug_id: u64,
    comment: &str,
) -> Result<()> {
    use std::collections::HashMap;
    let mut params = HashMap::new();
    params.insert("ws.op", "newMessage");
    params.insert("content", comment);
    client.post_ok(&format!("/bugs/{bug_id}"), &params).await
}

/// Fetch comments for a bug.
pub async fn get_bug_comments(
    client: &LaunchpadClient,
    bug_id: u64,
) -> Result<Vec<BugComment>> {
    let url = client.url(&format!("/bugs/{bug_id}/messages"));
    Collection::fetch_all(client, &url).await
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn urlenc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
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
    fn bug_search_params_default() {
        let params = BugSearchParams::default();
        assert!(params.status.is_none());
        assert!(params.importance.is_none());
        assert!(params.limit.is_none());
    }
}
