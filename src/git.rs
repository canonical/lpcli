//! Launchpad Git repository operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_git_repository`] | Fetch a Git repository by its Launchpad path |
//! | [`get_git_repository_by_unique_name`] | Fetch a repo by its unique name |
//! | [`get_default_git_repository`] | Fetch the default repo for a target |
//! | [`list_person_git_repositories`] | List repos owned by a person |
//! | [`list_git_refs`] | List branches and tags in a repository |
//! | [`list_merge_proposals`] | List merge proposals for a repository |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A Launchpad-hosted Git repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepository {
    /// Repository name (the last segment of the unique name).
    pub name: Option<String>,
    /// Unique name, e.g. `"~person/project/+git/repo"`.
    pub unique_name: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Whether this is the default repo for the owner+target combination.
    pub owner_default: Option<bool>,
    /// Whether this is the target's globally default repo.
    pub target_default: Option<bool>,
    /// Repository type: `"Hosted"`, `"Imported"`, or `"Remote"`.
    pub repository_type: Option<String>,
    /// Information type: `"Public"`, `"Private"`, etc.
    pub information_type: Option<String>,
    /// Whether the repository is private.
    pub private: Option<bool>,
    /// API link to the owner.
    pub owner_link: Option<String>,
    /// API link to the target project, distribution, or source package.
    pub target_link: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// Date created.
    pub date_created: Option<DateTime<Utc>>,
    /// Date last modified.
    pub date_last_modified: Option<DateTime<Utc>>,
    /// Number of loose objects (indicates whether a repack would help).
    pub loose_object_count: Option<u64>,
    /// Number of pack files.
    pub pack_count: Option<u64>,
}

/// A reference (branch or tag) within a Git repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRef {
    /// The full ref path (e.g. `"refs/heads/main"`).
    pub path: Option<String>,
    /// A human-readable display name (usually the branch/tag name).
    pub display_name: Option<String>,
    /// The commit SHA1 at this ref.
    pub commit_sha1: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// API link to the containing repository.
    pub repository_link: Option<String>,
}

/// A merge proposal for merging one Git branch into another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeProposal {
    /// Status: `"Work in progress"`, `"Needs review"`, `"Approved"`, etc.
    pub queue_status: Option<String>,
    /// Proposed commit message.
    pub commit_message: Option<String>,
    /// Description of the change.
    pub description: Option<String>,
    /// API link to the source repository.
    pub source_git_repository_link: Option<String>,
    /// Source branch path.
    pub source_git_path: Option<String>,
    /// API link to the target repository.
    pub target_git_repository_link: Option<String>,
    /// Target branch path.
    pub target_git_path: Option<String>,
    /// API link to the person who registered the proposal.
    pub registrant_link: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// Date the proposal was created.
    pub date_created: Option<DateTime<Utc>>,
    /// Date last updated.
    pub date_last_modified: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch a Git repository by its Launchpad API path.
///
/// `path` is the repository slug, with or without a leading `/`,
/// e.g. `"~person/project/+git/name"` or `"/~person/+git/name"`.
pub async fn get_git_repository(
    client: &LaunchpadClient,
    path: &str,
) -> Result<GitRepository> {
    let clean = path.trim_start_matches('/');
    client.get(&format!("/{clean}")).await
}

/// Look up a Git repository by its unique Launchpad name.
///
/// `unique_name` is in `~person/project/+git/name` format.
pub async fn get_git_repository_by_unique_name(
    client: &LaunchpadClient,
    unique_name: &str,
) -> Result<GitRepository> {
    let enc: String =
        url::form_urlencoded::byte_serialize(unique_name.as_bytes()).collect();
    let url = client.url(&format!("/+git?ws.op=getByUniqueName&unique_name={enc}"));
    client.get_url(&url).await
}

/// Return the default Git repository for a project, distribution, or
/// distribution source package.
///
/// `target` is a project or distribution name (e.g. `"launchpad"`, `"ubuntu"`).
pub async fn get_default_git_repository(
    client: &LaunchpadClient,
    target: &str,
) -> Result<GitRepository> {
    let target_url = client.url(&format!("/{target}"));
    let enc: String =
        url::form_urlencoded::byte_serialize(target_url.as_bytes()).collect();
    let url = client.url(&format!("/+git?ws.op=getDefaultRepository&target={enc}"));
    client.get_url(&url).await
}

/// List Git repositories owned by a Launchpad person or team.
pub async fn list_person_git_repositories(
    client: &LaunchpadClient,
    person_name: &str,
) -> Result<Vec<GitRepository>> {
    let url = client.url(&format!("/~{person_name}/+git"));
    Collection::fetch_all(client, &url).await
}

/// List references (branches and tags) in a Git repository.
///
/// `repo_path` is the repository slug, e.g. `"~person/project/+git/name"`.
pub async fn list_git_refs(
    client: &LaunchpadClient,
    repo_path: &str,
) -> Result<Vec<GitRef>> {
    let clean = repo_path.trim_start_matches('/');
    let url = client.url(&format!("/{clean}/refs"));
    Collection::fetch_all(client, &url).await
}

/// List merge proposals for a Git repository, optionally filtered by status.
///
/// `status` values include `"Work in progress"`, `"Needs review"`,
/// `"Approved"`, `"Rejected"`, `"Merged"`.
pub async fn list_merge_proposals(
    client: &LaunchpadClient,
    repo_path: &str,
    status: Option<&str>,
) -> Result<Vec<MergeProposal>> {
    let clean = repo_path.trim_start_matches('/');
    let mut url = client.url(&format!("/{clean}?ws.op=getMergeProposals"));
    if let Some(s) = status {
        let enc: String = url::form_urlencoded::byte_serialize(s.as_bytes()).collect();
        url.push_str(&format!("&status={enc}"));
    }
    Collection::fetch_all(client, &url).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_repository_deserialise_minimal() {
        let json = r#"{
            "name": "lpcli",
            "unique_name": "~jdoe/lpcli/+git/lpcli",
            "description": "lpcli git repo",
            "owner_default": true,
            "target_default": true,
            "repository_type": "Hosted",
            "information_type": "Public",
            "private": false,
            "owner_link": null,
            "target_link": null,
            "self_link": null,
            "web_link": null,
            "date_created": null,
            "date_last_modified": null,
            "loose_object_count": 0,
            "pack_count": 1
        }"#;
        let repo: GitRepository = serde_json::from_str(json).unwrap();
        assert_eq!(repo.name.as_deref(), Some("lpcli"));
        assert_eq!(repo.private, Some(false));
    }

    #[test]
    fn git_ref_deserialise() {
        let json = r#"{
            "path": "refs/heads/main",
            "display_name": "main",
            "commit_sha1": "abc123",
            "self_link": null,
            "web_link": null,
            "repository_link": null
        }"#;
        let r: GitRef = serde_json::from_str(json).unwrap();
        assert_eq!(r.path.as_deref(), Some("refs/heads/main"));
        assert_eq!(r.display_name.as_deref(), Some("main"));
    }
}
