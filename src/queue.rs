//! Launchpad package upload queue operations.
//!
//! This module exposes functions for querying the package upload queue
//! (a.k.a. "unapproved queue" / "NEW queue") using the `getPackageUploads`
//! custom GET method on `distro_series`.
//!
//! # API endpoint
//!
//! | Resource | Path |
//! |----------|------|
//! | Package uploads | `/{distro}/{series}?ws.op=getPackageUploads` |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A package upload record from the Launchpad queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageUpload {
    /// Unique numeric identifier.
    pub id: Option<u64>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Name of the uploaded source package.
    pub package_name: Option<String>,
    /// Source package version.
    pub package_version: Option<String>,
    /// Component name (e.g. "main", "universe").
    pub component_name: Option<String>,
    /// Section name (e.g. "devel", "libs").
    pub section_name: Option<String>,
    /// Generic display name for the queue item.
    pub display_name: Option<String>,
    /// Displayable source package version.
    pub display_version: Option<String>,
    /// Architectures related to this item.
    pub display_arches: Option<String>,
    /// The pocket targeted by this upload (e.g. "Release", "Proposed").
    pub pocket: Option<String>,
    /// Queue status (e.g. "New", "Accepted", "Done", "Rejected", "Unapproved").
    pub status: Option<String>,
    /// The date this package upload was done.
    pub date_created: Option<DateTime<Utc>>,
    /// Whether this upload contains sources.
    pub contains_source: Option<bool>,
    /// Whether this upload contains binaries.
    pub contains_build: Option<bool>,
    /// Whether this upload is a copy from another series.
    pub contains_copy: Option<bool>,
    /// The distroseries targeted by this upload.
    pub distroseries_link: Option<String>,
    /// The archive for this upload.
    pub archive_link: Option<String>,
    /// Changes file URL.
    pub changes_file_url: Option<String>,
}

/// Parameters for querying package uploads.
#[derive(Debug, Clone, Default)]
pub struct QueueSearchParams<'a> {
    /// Filter by pocket (e.g. "Release", "Security", "Updates", "Proposed", "Backports").
    pub pocket: Option<&'a str>,
    /// Filter by upload status (e.g. "New", "Unapproved", "Accepted", "Done", "Rejected").
    pub status: Option<&'a str>,
    /// Filter by package or file name.
    pub name: Option<&'a str>,
    /// Filter by package version.
    pub version: Option<&'a str>,
    /// Whether to filter name and version by exact matching.
    pub exact_match: bool,
    /// Archive link to filter by (full API URL of the archive).
    pub archive: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Get package upload records for a distro series using the `getPackageUploads`
/// custom GET method.
///
/// `distro` is typically `"ubuntu"` and `series` is the codename (e.g. `"oracular"`).
///
/// # Example
///
/// ```no_run
/// # use lpcli::client::LaunchpadClient;
/// # use lpcli::queue::{get_package_uploads, QueueSearchParams};
/// # tokio_test::block_on(async {
/// let client = LaunchpadClient::new(None);
/// let params = QueueSearchParams {
///     pocket: Some("Proposed"),
///     status: Some("New"),
///     ..Default::default()
/// };
/// let uploads = get_package_uploads(&client, "ubuntu", "oracular", &params).await.unwrap();
/// # });
/// ```
pub async fn get_package_uploads(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
    params: &QueueSearchParams<'_>,
) -> Result<Vec<PackageUpload>> {
    let series_url = client.url(&format!("/{}/{}", enc(distro), enc(series)));
    let mut query = format!("{}?ws.op=getPackageUploads", series_url);

    if let Some(pocket) = params.pocket {
        query.push_str(&format!("&pocket={}", enc(pocket)));
    }
    if let Some(status) = params.status {
        query.push_str(&format!("&status={}", enc(status)));
    }
    if let Some(name) = params.name {
        query.push_str(&format!("&name={}", enc(name)));
    }
    if let Some(version) = params.version {
        query.push_str(&format!("&version={}", enc(version)));
    }
    if params.exact_match {
        query.push_str("&exact_match=true");
    }
    if let Some(archive) = params.archive {
        query.push_str(&format!("&archive={}", enc(archive)));
    }

    Collection::fetch_all(client, &query).await
}

/// Resolve the current Ubuntu development series name.
///
/// Queries the Ubuntu distribution and returns the codename of the series
/// marked as the current development focus.
pub async fn get_devel_series_name(client: &LaunchpadClient, distro: &str) -> Result<String> {
    // The distribution object has a `current_series_link` that points to the
    // devel series resource. We can GET the distribution and extract it.
    #[derive(Deserialize)]
    struct DistroInfo {
        current_series_link: Option<String>,
    }

    let info: DistroInfo = client.get(&format!("/{}", enc(distro))).await?;
    if let Some(link) = info.current_series_link {
        // The link looks like "https://api.launchpad.net/devel/ubuntu/oracular"
        // We want just the last path segment.
        if let Some(name) = link.rsplit('/').next() {
            return Ok(name.to_string());
        }
    }

    // Fallback: if no current_series_link found, return an error.
    Err(crate::error::LpError::Api {
        status: 404,
        message: format!("Could not determine the current development series for '{distro}'."),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
