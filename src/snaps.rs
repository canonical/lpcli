//! Launchpad Snap recipe operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_snap`] | Fetch a snap recipe by owner and name |
//! | [`find_snaps_by_owner`] | List snaps owned by a Launchpad person |
//! | [`find_snaps_by_store_name`] | Find snaps by registered store name |
//! | [`get_snap_pending_builds`] | List pending builds for a snap |
//! | [`request_snap_builds`] | Request builds for a snap recipe |
//!
//! Reads and entry-creating mutations also have `*_raw` counterparts that
//! keep the exact JSON Launchpad returned (see [`Raw`]).

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::client::{Collection, LaunchpadClient, Raw};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A Launchpad snap recipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snap {
    /// Recipe name.
    pub name: Option<String>,
    /// API link to the owner.
    pub owner_link: Option<String>,
    /// Registered store package name, if any.
    pub store_name: Option<String>,
    /// Whether builds are automatically uploaded to the store.
    pub store_upload: Option<bool>,
    /// Whether the recipe is private.
    pub private: Option<bool>,
    /// Description of the snap.
    pub description: Option<String>,
    /// The Git repository URL this snap builds from.
    pub git_repository_url: Option<String>,
    /// The Git repository API link.
    pub git_repository_link: Option<String>,
    /// The Git branch path.
    pub git_path: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// Date the recipe was created.
    pub date_created: Option<Timestamp>,
}

/// A snap build record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapBuild {
    /// Human-readable title.
    pub title: Option<String>,
    /// The pocket built against.
    pub pocket: Option<String>,
    /// Build farm job state.
    pub buildstate: Option<String>,
    /// API link to the person who requested the build.
    pub requester_link: Option<String>,
    /// API link to the target distribution architecture series.
    pub distro_arch_series_link: Option<String>,
    /// When the build started running.
    pub date_started: Option<Timestamp>,
    /// Build farm queue score.
    pub score: Option<i64>,
    /// Store upload status.
    pub store_upload_status: Option<String>,
    /// URL of the upload failure log, if any.
    pub upload_log_url: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

/// A request to build a snap for multiple architectures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapBuildRequest {
    /// Request status: `"Pending"`, `"Failed"`, or `"Completed"`.
    pub status: Option<String>,
    /// Error message, if the request failed.
    pub error_message: Option<String>,
    /// When the request was created.
    pub date_requested: Option<Timestamp>,
    /// When the request finished.
    pub date_finished: Option<Timestamp>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch a snap recipe by owner and name.
///
/// `owner` is the Launchpad name without `~`; `name` is the recipe name.
pub async fn get_snap(client: &LaunchpadClient, owner: &str, name: &str) -> Result<Snap> {
    client.get(&snap_path(owner, name)).await
}

/// Like [`get_snap()`], but also keep the exact Launchpad JSON.
pub async fn get_snap_raw(client: &LaunchpadClient, owner: &str, name: &str) -> Result<Raw<Snap>> {
    client.get_raw(&snap_path(owner, name)).await
}

fn snap_path(owner: &str, name: &str) -> String {
    format!("/~{}/+snap/{}", urlenc(owner), urlenc(name))
}

/// List snap recipes owned by a Launchpad person or team.
pub async fn find_snaps_by_owner(client: &LaunchpadClient, owner: &str) -> Result<Vec<Snap>> {
    Collection::fetch_all(client, &snaps_by_owner_url(client, owner)).await
}

/// Like [`find_snaps_by_owner()`], but return Launchpad's collection shape
/// with every page's entries unmodified.
pub async fn find_snaps_by_owner_raw(
    client: &LaunchpadClient,
    owner: &str,
) -> Result<Raw<Vec<Snap>>> {
    Collection::fetch_all_raw(client, &snaps_by_owner_url(client, owner)).await
}

fn snaps_by_owner_url(client: &LaunchpadClient, owner: &str) -> String {
    let owner_url = client.url(&format!("/~{}", urlenc(owner)));
    let enc: String = url::form_urlencoded::byte_serialize(owner_url.as_bytes()).collect();
    client.url(&format!("/+snaps?ws.op=findByOwner&owner={enc}"))
}

/// Find snap recipes by their registered store package name.
pub async fn find_snaps_by_store_name(
    client: &LaunchpadClient,
    store_name: &str,
) -> Result<Vec<Snap>> {
    Collection::fetch_all(client, &snaps_by_store_name_url(client, store_name)).await
}

/// Like [`find_snaps_by_store_name()`], but return Launchpad's collection
/// shape with every page's entries unmodified.
pub async fn find_snaps_by_store_name_raw(
    client: &LaunchpadClient,
    store_name: &str,
) -> Result<Raw<Vec<Snap>>> {
    Collection::fetch_all_raw(client, &snaps_by_store_name_url(client, store_name)).await
}

fn snaps_by_store_name_url(client: &LaunchpadClient, store_name: &str) -> String {
    let enc: String = url::form_urlencoded::byte_serialize(store_name.as_bytes()).collect();
    client.url(&format!("/+snaps?ws.op=findByStoreName&store_name={enc}"))
}

/// List pending builds for a snap recipe.
pub async fn get_snap_pending_builds(
    client: &LaunchpadClient,
    owner: &str,
    name: &str,
) -> Result<Vec<SnapBuild>> {
    let url = client.url(&format!("{}/pending_builds", snap_path(owner, name)));
    Collection::fetch_all(client, &url).await
}

/// Like [`get_snap_pending_builds()`], but return Launchpad's collection
/// shape with every page's entries unmodified.
pub async fn get_snap_pending_builds_raw(
    client: &LaunchpadClient,
    owner: &str,
    name: &str,
) -> Result<Raw<Vec<SnapBuild>>> {
    let url = client.url(&format!("{}/pending_builds", snap_path(owner, name)));
    Collection::fetch_all_raw(client, &url).await
}

/// Request builds for a snap recipe.
///
/// `archive_url` should be the full Launchpad API URL of the archive, e.g.
/// `"https://api.launchpad.net/devel/ubuntu/+archive/primary"`.
///
/// `pocket` is one of: `"Release"`, `"Security"`, `"Updates"`,
/// `"Proposed"`, `"Backports"`.
pub async fn request_snap_builds(
    client: &LaunchpadClient,
    owner: &str,
    name: &str,
    archive_url: &str,
    pocket: &str,
) -> Result<SnapBuildRequest> {
    let location = request_snap_builds_location(client, owner, name, archive_url, pocket).await?;
    client.get_url(&location).await
}

/// Like [`request_snap_builds()`], but return the created build request's
/// exact Launchpad JSON.
///
/// # Errors
///
/// If Launchpad accepts the request but reading it back from the reported
/// location fails, returns [`LpError::Partial`](crate::error::LpError::Partial).
pub async fn request_snap_builds_raw(
    client: &LaunchpadClient,
    owner: &str,
    name: &str,
    archive_url: &str,
    pocket: &str,
) -> Result<Raw<SnapBuildRequest>> {
    let location = request_snap_builds_location(client, owner, name, archive_url, pocket).await?;
    client
        .get_created_raw(&location, "Requesting snap builds")
        .await
}

async fn request_snap_builds_location(
    client: &LaunchpadClient,
    owner: &str,
    name: &str,
    archive_url: &str,
    pocket: &str,
) -> Result<String> {
    let mut params = HashMap::new();
    params.insert("ws.op", "requestBuilds");
    params.insert("archive", archive_url);
    params.insert("pocket", pocket);
    client
        .post_created_location(&snap_path(owner, name), &params)
        .await
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
    fn snap_deserialise_minimal() {
        let json = r#"{
            "name": "my-snap",
            "owner_link": "https://api.launchpad.net/devel/~jdoe",
            "store_name": "my-snap",
            "store_upload": true,
            "private": false,
            "description": null,
            "git_repository_url": null,
            "git_repository_link": null,
            "git_path": "refs/heads/main",
            "self_link": null,
            "web_link": null,
            "date_created": null
        }"#;
        let snap: Snap = serde_json::from_str(json).unwrap();
        assert_eq!(snap.name.as_deref(), Some("my-snap"));
        assert_eq!(snap.store_upload, Some(true));
    }

    #[test]
    fn snap_build_request_deserialise() {
        let json = r#"{
            "status": "Pending",
            "error_message": null,
            "date_requested": null,
            "date_finished": null,
            "self_link": "https://api.launchpad.net/devel/~jdoe/+snap/my-snap/+build-request/1",
            "web_link": null
        }"#;
        let req: SnapBuildRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.status.as_deref(), Some("Pending"));
    }
}
