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
    /// Collection link for upload log entries (status change history).
    pub logs_collection_link: Option<String>,
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
    /// Maximum number of results to return.
    pub limit: Option<u32>,
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
    if let Some(limit) = params.limit {
        query.push_str(&format!("&ws.size={limit}"));
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

/// Get the source file URLs for a package upload.
///
/// Calls the `sourceFileUrls` custom GET method on a `package_upload` entry
/// to retrieve download URLs for the source files associated with the upload.
///
/// `upload_self_link` is the API self-link of the `PackageUpload` entry.
pub async fn get_source_file_urls(
    client: &LaunchpadClient,
    upload_self_link: &str,
) -> Result<Vec<String>> {
    let url = format!("{upload_self_link}?ws.op=sourceFileUrls");
    client.get_url_string_list(&url).await
}

/// Get the binary file URLs for a package upload.
///
/// Calls the `binaryFileUrls` custom GET method on a `package_upload` entry.
///
/// `upload_self_link` is the API self-link of the `PackageUpload` entry.
pub async fn get_binary_file_urls(
    client: &LaunchpadClient,
    upload_self_link: &str,
) -> Result<Vec<String>> {
    let url = format!("{upload_self_link}?ws.op=binaryFileUrls");
    client.get_url_string_list(&url).await
}

/// Properties of a single binary package in a queue upload.
///
/// Returned by the `getBinaryProperties` custom GET method on a
/// `package_upload` entry.  Each dictionary in the response describes one
/// `.deb` or `.ddeb` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryProperties {
    /// Binary package name (e.g. `"libfoo1"`).
    pub name: Option<String>,
    /// Binary package version.
    pub version: Option<String>,
    /// Architecture (e.g. `"amd64"`, `"all"`).
    pub architecture: Option<String>,
    /// Component (e.g. `"main"`, `"universe"`).
    pub component: Option<String>,
    /// Section (e.g. `"libs"`, `"devel"`).
    pub section: Option<String>,
    /// Priority (e.g. `"optional"`, `"extra"`).
    pub priority: Option<String>,
    /// Whether this is a new package not previously seen in the archive.
    pub is_new: Option<bool>,
}

impl BinaryProperties {
    /// Returns `true` if this binary is a debug-symbol package (`.ddeb`).
    ///
    /// Launchpad's `getBinaryProperties` does not return an explicit
    /// `is_debug` field, so we detect debug packages by the conventional
    /// `-dbgsym` or `-dbg` name suffix.
    pub fn is_debug_package(&self) -> bool {
        self.name
            .as_deref()
            .map(|n| n.ends_with("-dbgsym") || n.ends_with("-dbg"))
            .unwrap_or(false)
    }
}

/// Get the binary properties for a package upload.
///
/// Calls the `getBinaryProperties` custom GET method on a `package_upload`
/// entry to retrieve metadata (name, version, architecture, component, etc.)
/// for the `.deb` and `.ddeb` files associated with the upload.
///
/// `upload_self_link` is the API self-link of the `PackageUpload` entry.
pub async fn get_binary_properties(
    client: &LaunchpadClient,
    upload_self_link: &str,
) -> Result<Vec<BinaryProperties>> {
    let url = format!("{upload_self_link}?ws.op=getBinaryProperties");
    client.get_url(&url).await
}

// ---------------------------------------------------------------------------
// Queue actions
// ---------------------------------------------------------------------------

/// Accept a package upload from the queue.
///
/// Calls the `acceptFromQueue` custom POST method on a `package_upload`
/// entry.  The authenticated user must have queue-admin permissions for
/// the target archive and component.
///
/// `upload_self_link` is the API self-link of the `PackageUpload` entry.
pub async fn accept_from_queue(client: &LaunchpadClient, upload_self_link: &str) -> Result<()> {
    let params = [("ws.op", "acceptFromQueue")];
    client.post_pairs_url_ok(upload_self_link, &params).await
}

/// Reject a package upload from the queue.
///
/// Calls the `rejectFromQueue` custom POST method on a `package_upload`
/// entry with an optional rejection comment.  The authenticated user must
/// have queue-admin permissions for the target archive and component.
///
/// `upload_self_link` is the API self-link of the `PackageUpload` entry.
pub async fn reject_from_queue(
    client: &LaunchpadClient,
    upload_self_link: &str,
    comment: Option<&str>,
) -> Result<()> {
    let mut params = vec![("ws.op", "rejectFromQueue")];
    if let Some(c) = comment {
        params.push(("comment", c));
    }
    client.post_pairs_url_ok(upload_self_link, &params).await
}

// ---------------------------------------------------------------------------
// Upload logs
// ---------------------------------------------------------------------------

/// A log entry recording a status change for a package upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageUploadLog {
    /// The reviewer (person who performed the action).
    pub reviewer_link: Option<String>,
    /// Previous queue status.
    pub old_status: Option<String>,
    /// New queue status after the action.
    pub new_status: Option<String>,
    /// Comment left by the reviewer (e.g. rejection reason).
    pub comment: Option<String>,
    /// When this action happened.
    pub date_created: Option<DateTime<Utc>>,
}

/// Fetch upload log entries for a package upload.
///
/// Returns the log entries associated with the upload, which record
/// status transitions (accept, reject, etc.) along with reviewer comments.
///
/// `logs_collection_url` is the `logs_collection_link` from a `PackageUpload`.
pub async fn get_upload_logs(
    client: &LaunchpadClient,
    logs_collection_url: &str,
) -> Result<Vec<PackageUploadLog>> {
    Collection::fetch_all(client, logs_collection_url).await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
