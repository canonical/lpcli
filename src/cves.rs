//! Launchpad CVE (Common Vulnerabilities and Exposures) operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_cve`] | Fetch a single CVE by sequence number |
//! | [`search_cves`] | Advanced CVE search with optional filters |
//! | [`get_bug_cves`] | List CVEs linked to a specific bug |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A CVE entry tracked in Launchpad.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cve {
    /// CVE sequence number in `YYYY-NNNNN` format (e.g. `"2024-12345"`).
    pub sequence: String,
    /// Current state: `"Candidate"`, `"Entry"`, `"Deprecated"`, or `"Rejected"`.
    pub status: Option<String>,
    /// Short human-readable description of the vulnerability.
    pub description: Option<String>,
    /// Concise title.
    pub title: Option<String>,
    /// Canonical URL for the CVE record.
    pub url: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// Launchpad API self-link.
    pub self_link: Option<String>,
    /// When the CVE was made public.
    pub date_made_public: Option<DateTime<Utc>>,
}

/// Filters used with [`search_cves`].
#[derive(Debug, Clone, Default)]
pub struct CveSearchParams<'a> {
    /// Restrict to CVEs linked to this distribution (e.g. `"ubuntu"`).
    pub in_distribution: Option<&'a str>,
    /// Exclude CVEs that are linked to this distribution.
    pub not_in_distribution: Option<&'a str>,
    /// An ISO 8601 timestamp string; only return CVEs modified after this.
    pub modified_since: Option<&'a str>,
    /// Maximum number of results to return.
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch a single CVE by its sequence number (e.g. `"2024-12345"`).
///
/// # Errors
/// Returns [`crate::error::LpError::NotFound`] when the sequence does not
/// correspond to a tracked CVE.
pub async fn get_cve(client: &LaunchpadClient, sequence: &str) -> Result<Cve> {
    client.get(&format!("/bugs/cve/{sequence}")).await
}

/// Search for CVEs with optional distribution and date filters.
pub async fn search_cves(
    client: &LaunchpadClient,
    params: &CveSearchParams<'_>,
) -> Result<Vec<Cve>> {
    let mut query = "/cves?ws.op=advancedSearch".to_string();

    if let Some(dist) = params.in_distribution {
        let dist_url = client.url(&format!("/{dist}"));
        query.push_str(&format!("&in_distribution={}", enc(&dist_url)));
    }
    if let Some(dist) = params.not_in_distribution {
        let dist_url = client.url(&format!("/{dist}"));
        query.push_str(&format!("&not_in_distribution={}", enc(&dist_url)));
    }
    if let Some(since) = params.modified_since {
        query.push_str(&format!("&modified_since={}", enc(since)));
    }
    if let Some(limit) = params.limit {
        query.push_str(&format!("&limit={limit}"));
    }

    let url = client.url(&query);
    Collection::fetch_all(client, &url).await
}

/// List all CVEs linked to a specific Launchpad bug.
pub async fn get_bug_cves(client: &LaunchpadClient, bug_id: u64) -> Result<Vec<Cve>> {
    let url = client.url(&format!("/bugs/{bug_id}/cves"));
    Collection::fetch_all(client, &url).await
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
    fn cve_deserialise_minimal() {
        let json = r#"{
            "sequence": "2024-00001",
            "status": "Entry",
            "description": "A test vulnerability.",
            "title": "Test CVE",
            "url": "https://cve.mitre.org/cgi-bin/cvename.cgi?name=CVE-2024-00001",
            "web_link": null,
            "self_link": null,
            "date_made_public": null
        }"#;
        let cve: Cve = serde_json::from_str(json).unwrap();
        assert_eq!(cve.sequence, "2024-00001");
        assert_eq!(cve.status.as_deref(), Some("Entry"));
    }

    #[test]
    fn cve_search_params_default() {
        let p = CveSearchParams::default();
        assert!(p.in_distribution.is_none());
        assert!(p.limit.is_none());
    }
}
