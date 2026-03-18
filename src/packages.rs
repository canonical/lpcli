//! Launchpad source packages and Ubuntu distribution operations.
//!
//! This module exposes functions for querying:
//!
//! * Ubuntu distributions and their series (e.g. `jammy`, `noble`).
//! * Source packages and their publishing history within a distro series.
//! * Binary packages built from a source package.
//! * Copying or syncing packages between series (PPAs).
//!
//! # API roots
//!
//! | Resource | Path |
//! |----------|------|
//! | Ubuntu distribution | `/ubuntu` |
//! | Series | `/ubuntu/jammy` |
//! | Source publications | `/ubuntu/jammy?ws.op=getPublishedSources` |
//! | PPAs | `/~{person}/+archive/ubuntu/{ppa}` |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// An Ubuntu distribution series (e.g. Jammy Jellyfish / 22.04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistroSeries {
    /// Short name, e.g. `"jammy"`.
    pub name: String,
    /// Full display name, e.g. `"Ubuntu Jammy Jellyfish"`.
    pub display_name: Option<String>,
    /// Version string, e.g. `"22.04"`.
    pub version: Option<String>,
    /// Whether this series is currently active.
    pub active: Option<bool>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Web link.
    pub web_link: Option<String>,
    /// Current status of the series (e.g. "Active Development", "Supported").
    pub status: Option<String>,
}

/// A source package publication record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePackagePublishingHistory {
    /// API self-link.
    pub self_link: Option<String>,
    /// Display name of the source package.
    pub source_package_name: Option<String>,
    /// Version string.
    pub source_package_version: Option<String>,
    /// Component (e.g. `"main"`, `"universe"`).
    pub component_name: Option<String>,
    /// Section (e.g. `"devel"`, `"libs"`).
    pub section_name: Option<String>,
    /// Publishing status (e.g. "Published", "Superseded").
    pub status: Option<String>,
    /// When this record was created.
    pub date_published: Option<DateTime<Utc>>,
    /// When this record was superseded, if applicable.
    pub date_superseded: Option<DateTime<Utc>>,
    /// Pocket (e.g. `"Release"`, `"Updates"`, `"Security"`).
    pub pocket: Option<String>,
    /// Archive this was published in.
    pub archive_link: Option<String>,
    /// Series API link.
    pub distro_series_link: Option<String>,
}

/// A binary package publication record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryPackagePublishingHistory {
    /// API self-link.
    pub self_link: Option<String>,
    /// Binary package name.
    pub binary_package_name: Option<String>,
    /// Binary package version.
    pub binary_package_version: Option<String>,
    /// Architecture tag (e.g. `"amd64"`, `"arm64"`, `"all"`).
    pub architecture_specific: Option<bool>,
    /// Component (e.g. `"main"`).
    pub component_name: Option<String>,
    /// Status.
    pub status: Option<String>,
    /// When published.
    pub date_published: Option<DateTime<Utc>>,
}

/// A Launchpad Personal Package Archive (PPA).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archive {
    /// API self-link.
    pub self_link: Option<String>,
    /// Short archive name.
    pub name: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Whether the archive is enabled.
    pub enabled: Option<bool>,
    /// Number of source packages.
    pub num_pkgs: Option<u64>,
    /// Web link.
    pub web_link: Option<String>,
    /// Owner API link.
    pub owner_link: Option<String>,
}

/// Parameters for source package publication searches.
#[derive(Debug, Clone, Default)]
pub struct SourceSearchParams<'a> {
    /// Source package name filter.
    pub source_name: Option<&'a str>,
    /// Version filter.
    pub version: Option<&'a str>,
    /// Pocket filter (e.g. "Release", "Updates").
    pub pocket: Option<&'a str>,
    /// Status filter (e.g. "Published").
    pub status: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch metadata for an Ubuntu distro series.
///
/// `distro` is typically `"ubuntu"` and `series` is the codename, e.g. `"jammy"`.
pub async fn get_distro_series(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
) -> Result<DistroSeries> {
    client.get(&format!("/{distro}/{series}")).await
}

/// List all known distro series for a distribution.
pub async fn list_distro_series(
    client: &LaunchpadClient,
    distro: &str,
) -> Result<Vec<DistroSeries>> {
    let url = client.url(&format!("/{distro}/series"));
    Collection::fetch_all(client, &url).await
}

/// Search for source package publications in a distro series.
///
/// `distro` is typically `"ubuntu"`, `series` is the codename.
pub async fn search_published_sources(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
    params: &SourceSearchParams<'_>,
) -> Result<Vec<SourcePackagePublishingHistory>> {
    let series_url = client.url(&format!("/{distro}/{series}"));
    let mut query = format!("{series_url}?ws.op=getPublishedSources");
    if let Some(name) = params.source_name {
        query.push_str(&format!("&source_name={}", enc(name)));
    }
    if let Some(version) = params.version {
        query.push_str(&format!("&version={}", enc(version)));
    }
    if let Some(pocket) = params.pocket {
        query.push_str(&format!("&pocket={}", enc(pocket)));
    }
    if let Some(status) = params.status {
        query.push_str(&format!("&status={}", enc(status)));
    }
    Collection::fetch_all(client, &query).await
}

/// Fetch details of a PPA by owner and archive name.
///
/// # Example
///
/// ```no_run
/// # use lpcli::client::LaunchpadClient;
/// # use lpcli::packages::get_ppa;
/// # tokio_test::block_on(async {
/// let client = LaunchpadClient::new(None);
/// let ppa = get_ppa(&client, "canonical-kernel-team", "ppa").await.unwrap();
/// # });
/// ```
pub async fn get_ppa(
    client: &LaunchpadClient,
    owner: &str,
    ppa_name: &str,
) -> Result<Archive> {
    client
        .get(&format!("/~{owner}/+archive/ubuntu/{ppa_name}"))
        .await
}

/// List source package publications in a PPA.
pub async fn list_ppa_sources(
    client: &LaunchpadClient,
    owner: &str,
    ppa_name: &str,
    params: &SourceSearchParams<'_>,
) -> Result<Vec<SourcePackagePublishingHistory>> {
    let archive_url = client.url(&format!("/~{owner}/+archive/ubuntu/{ppa_name}"));
    let mut query = format!("{archive_url}?ws.op=getPublishedSources");
    if let Some(name) = params.source_name {
        query.push_str(&format!("&source_name={}", enc(name)));
    }
    if let Some(version) = params.version {
        query.push_str(&format!("&version={}", enc(version)));
    }
    if let Some(status) = params.status {
        query.push_str(&format!("&status={}", enc(status)));
    }
    Collection::fetch_all(client, &query).await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distro_series_deserialise() {
        let json = r#"{
            "name": "jammy",
            "display_name": "Ubuntu Jammy Jellyfish",
            "version": "22.04",
            "active": true,
            "self_link": "https://api.launchpad.net/devel/ubuntu/jammy",
            "web_link": "https://launchpad.net/ubuntu/jammy",
            "status": "Current Stable Release"
        }"#;
        let series: DistroSeries = serde_json::from_str(json).unwrap();
        assert_eq!(series.name, "jammy");
        assert_eq!(series.version.as_deref(), Some("22.04"));
        assert_eq!(series.active, Some(true));
    }

    #[test]
    fn source_publication_deserialise_minimal() {
        let json = r#"{
            "self_link": null,
            "source_package_name": "linux",
            "source_package_version": "5.15.0-1.1",
            "component_name": "main",
            "section_name": "devel",
            "status": "Published",
            "date_published": null,
            "date_superseded": null,
            "pocket": "Release",
            "archive_link": null,
            "distro_series_link": null
        }"#;
        let pub_history: SourcePackagePublishingHistory =
            serde_json::from_str(json).unwrap();
        assert_eq!(pub_history.source_package_name.as_deref(), Some("linux"));
        assert_eq!(pub_history.pocket.as_deref(), Some("Release"));
    }

    #[test]
    fn archive_deserialise() {
        let json = r#"{
            "self_link": "https://api.launchpad.net/devel/~canonical-kernel-team/+archive/ubuntu/ppa",
            "name": "ppa",
            "description": "Canonical Kernel Team PPA",
            "enabled": true,
            "num_pkgs": 42,
            "web_link": "https://launchpad.net/~canonical-kernel-team/+archive/ubuntu/ppa",
            "owner_link": "https://api.launchpad.net/devel/~canonical-kernel-team"
        }"#;
        let archive: Archive = serde_json::from_str(json).unwrap();
        assert_eq!(archive.name.as_deref(), Some("ppa"));
        assert_eq!(archive.enabled, Some(true));
    }

    #[test]
    fn source_search_params_default() {
        let p = SourceSearchParams::default();
        assert!(p.source_name.is_none());
        assert!(p.pocket.is_none());
    }

    #[test]
    fn enc_encodes_spaces() {
        let out = enc("Fix Released");
        assert!(!out.contains(' '));
    }
}
