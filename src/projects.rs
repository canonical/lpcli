//! Launchpad project (product) operations.
//!
//! This module provides types and functions for querying Launchpad projects
//! and project groups via the REST API.
//!
//! # API roots
//!
//! | Resource | Path |
//! |----------|------|
//! | Project | `/{project_name}` |
//! | Project group | `/+projectgroups/{group}` |
//! | All milestones | `/{project}/all_milestones` |
//! | Active milestones | `/{project}/active_milestones` |
//! | Releases | `/{project}/{milestone_name}/release` |
//!
//! Each read function has a `_raw` counterpart that also returns the exact
//! JSON Launchpad sent: an entry, or a collection with `start`, `entries`, and
//! `total_size` or `total_size_link`.

use jiff::{Timestamp, civil::Date};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient, Raw};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A Launchpad project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Short project identifier (e.g. `"launchpad"`).
    pub name: String,
    /// Human-readable display name.
    pub display_name: Option<String>,
    /// One-line summary.
    pub summary: Option<String>,
    /// Full description.
    pub description: Option<String>,
    /// Homepage URL (external project website).
    pub homepage_url: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// Whether the project is active.
    pub active: Option<bool>,
    /// Date the project was registered.
    pub date_created: Option<Timestamp>,
    /// Owner API link.
    pub owner_link: Option<String>,
    /// Active series link.
    pub development_focus_link: Option<String>,
}

/// A project milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Milestone {
    /// Milestone name (e.g. `"2.0"`).
    pub name: String,
    /// Human-readable title.
    pub title: Option<String>,
    /// Milestone code name, if any.
    pub code_name: Option<String>,
    /// Whether this milestone is active.
    pub is_active: Option<bool>,
    /// Target date.
    pub date_targeted: Option<Date>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Web link.
    pub web_link: Option<String>,
    /// Project API link.
    pub target_link: Option<String>,
}

/// A project release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    /// Version string (e.g. `"2.0.0"`).
    pub version: Option<String>,
    /// Release date.
    pub date_released: Option<Timestamp>,
    /// Body of the release notes.
    pub release_notes: Option<String>,
    /// Body of the changelog.
    pub changelog: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Web link.
    pub web_link: Option<String>,
    /// Milestone API link.
    pub milestone_link: Option<String>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch a Launchpad project by its unique name.
///
/// # Errors
///
/// Returns [`crate::error::LpError::NotFound`] when no such project exists.
pub async fn get_project(client: &LaunchpadClient, name: &str) -> Result<Project> {
    Ok(get_project_raw(client, name).await?.value)
}

/// Like [`get_project()`], but also keep the exact JSON Launchpad returned.
pub async fn get_project_raw(client: &LaunchpadClient, name: &str) -> Result<Raw<Project>> {
    client.get_raw(&format!("/{}", urlenc(name))).await
}

/// Search Launchpad projects by keyword.
pub async fn search_projects(client: &LaunchpadClient, query: &str) -> Result<Vec<Project>> {
    Ok(search_projects_raw(client, query).await?.value)
}

/// Like [`search_projects()`], but also keep Launchpad's collection JSON.
pub async fn search_projects_raw(
    client: &LaunchpadClient,
    query: &str,
) -> Result<Raw<Vec<Project>>> {
    let url = client.url(&format!("/projects?ws.op=search&text={}", urlenc(query)));
    Collection::fetch_all_raw(client, &url).await
}

/// List all milestones for a project or distribution.
///
/// Uses the `all_milestones` collection link exposed by both `project` and
/// `distribution` resources in the Launchpad API.
pub async fn list_milestones(client: &LaunchpadClient, project: &str) -> Result<Vec<Milestone>> {
    Ok(list_milestones_raw(client, project).await?.value)
}

/// Like [`list_milestones()`], but also keep Launchpad's collection JSON.
pub async fn list_milestones_raw(
    client: &LaunchpadClient,
    project: &str,
) -> Result<Raw<Vec<Milestone>>> {
    let url = client.url(&format!("/{}/all_milestones", urlenc(project)));
    Collection::fetch_all_raw(client, &url).await
}

/// List only the active milestones for a project or distribution.
///
/// Uses the `active_milestones` collection link exposed by both `project` and
/// `distribution` resources in the Launchpad API.
pub async fn list_active_milestones(
    client: &LaunchpadClient,
    project: &str,
) -> Result<Vec<Milestone>> {
    Ok(list_active_milestones_raw(client, project).await?.value)
}

/// Like [`list_active_milestones()`], but also keep Launchpad's collection
/// JSON.
pub async fn list_active_milestones_raw(
    client: &LaunchpadClient,
    project: &str,
) -> Result<Raw<Vec<Milestone>>> {
    let url = client.url(&format!("/{}/active_milestones", urlenc(project)));
    Collection::fetch_all_raw(client, &url).await
}

/// Fetch a specific milestone by name.
pub async fn get_milestone(
    client: &LaunchpadClient,
    project: &str,
    milestone_name: &str,
) -> Result<Milestone> {
    Ok(get_milestone_raw(client, project, milestone_name)
        .await?
        .value)
}

/// Like [`get_milestone()`], but also keep the exact JSON Launchpad returned.
pub async fn get_milestone_raw(
    client: &LaunchpadClient,
    project: &str,
    milestone_name: &str,
) -> Result<Raw<Milestone>> {
    client
        .get_raw(&format!(
            "/{}/+milestone/{}",
            urlenc(project),
            urlenc(milestone_name)
        ))
        .await
}

/// Fetch the release associated with a milestone.
pub async fn get_release(
    client: &LaunchpadClient,
    project: &str,
    milestone_name: &str,
) -> Result<Release> {
    Ok(get_release_raw(client, project, milestone_name)
        .await?
        .value)
}

/// Like [`get_release()`], but also keep the exact JSON Launchpad returned.
pub async fn get_release_raw(
    client: &LaunchpadClient,
    project: &str,
    milestone_name: &str,
) -> Result<Raw<Release>> {
    client
        .get_raw(&format!(
            "/{}/+milestone/{}/release",
            urlenc(project),
            urlenc(milestone_name)
        ))
        .await
}

// ---------------------------------------------------------------------------
// Project series
// ---------------------------------------------------------------------------

/// A series of releases within a Launchpad project
/// (e.g. the `"trunk"` or `"2.0"` series of `"launchpad"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSeries {
    /// Short identifier used in URLs (e.g. `"trunk"`, `"2.0"`).
    pub name: String,
    /// Human-readable title.
    pub title: Option<String>,
    /// Summary of the series.
    pub summary: Option<String>,
    /// Lifecycle status (e.g. `"Active Development"`, `"Supported"`, `"Obsolete"`).
    pub status: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// API link to the parent project.
    pub project_link: Option<String>,
    /// API link to the series owner.
    pub owner_link: Option<String>,
    /// Date the series was created.
    pub date_created: Option<Timestamp>,
}

/// Fetch a single project series by project name and series name.
pub async fn get_project_series(
    client: &LaunchpadClient,
    project: &str,
    series_name: &str,
) -> Result<ProjectSeries> {
    Ok(get_project_series_raw(client, project, series_name)
        .await?
        .value)
}

/// Like [`get_project_series()`], but also keep the exact JSON Launchpad
/// returned.
pub async fn get_project_series_raw(
    client: &LaunchpadClient,
    project: &str,
    series_name: &str,
) -> Result<Raw<ProjectSeries>> {
    client
        .get_raw(&format!("/{}/{}", urlenc(project), urlenc(series_name)))
        .await
}

/// List all series for a project.
pub async fn list_project_series(
    client: &LaunchpadClient,
    project: &str,
) -> Result<Vec<ProjectSeries>> {
    Ok(list_project_series_raw(client, project).await?.value)
}

/// Like [`list_project_series()`], but also keep Launchpad's collection JSON.
pub async fn list_project_series_raw(
    client: &LaunchpadClient,
    project: &str,
) -> Result<Raw<Vec<ProjectSeries>>> {
    let url = client.url(&format!("/{}/series", urlenc(project)));
    Collection::fetch_all_raw(client, &url).await
}

/// List all releases in a project series.
pub async fn list_series_releases(
    client: &LaunchpadClient,
    project: &str,
    series_name: &str,
) -> Result<Vec<Release>> {
    Ok(list_series_releases_raw(client, project, series_name)
        .await?
        .value)
}

/// Like [`list_series_releases()`], but also keep Launchpad's collection JSON.
pub async fn list_series_releases_raw(
    client: &LaunchpadClient,
    project: &str,
    series_name: &str,
) -> Result<Raw<Vec<Release>>> {
    let url = client.url(&format!(
        "/{}/{}/releases",
        urlenc(project),
        urlenc(series_name)
    ));
    Collection::fetch_all_raw(client, &url).await
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
    fn project_deserialise_minimal() {
        let json = r#"{
            "name": "launchpad",
            "display_name": "Launchpad",
            "summary": "Collaborative software development",
            "description": null,
            "homepage_url": "https://launchpad.net",
            "self_link": "https://api.launchpad.net/devel/launchpad",
            "web_link": "https://launchpad.net/launchpad",
            "active": true,
            "date_created": null,
            "owner_link": null,
            "development_focus_link": null
        }"#;
        let project: Project = serde_json::from_str(json).unwrap();
        assert_eq!(project.name, "launchpad");
        assert_eq!(project.active, Some(true));
    }

    #[test]
    fn milestone_deserialise() {
        let json = r#"{
            "name": "2.0",
            "title": "Launchpad 2.0",
            "code_name": "Awesome",
            "is_active": true,
            "date_targeted": "2025-06-01",
            "self_link": null,
            "web_link": null,
            "target_link": null
        }"#;
        let milestone: Milestone = serde_json::from_str(json).unwrap();
        assert_eq!(milestone.name, "2.0");
        assert_eq!(milestone.is_active, Some(true));
        assert!(milestone.date_targeted.is_some());
    }

    #[test]
    fn release_deserialise_minimal() {
        let json = r#"{
            "version": "2.0.0",
            "date_released": null,
            "release_notes": "First stable release.",
            "changelog": null,
            "self_link": null,
            "web_link": null,
            "milestone_link": null
        }"#;
        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(release.version.as_deref(), Some("2.0.0"));
        assert_eq!(
            release.release_notes.as_deref(),
            Some("First stable release.")
        );
    }
}
