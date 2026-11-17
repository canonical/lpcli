//! Launchpad CVE (Common Vulnerabilities and Exposures) operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_cve`] | Fetch a single CVE by sequence number |
//! | [`search_cves`] | Advanced CVE search with optional filters |
//! | [`get_bug_cves`] | List CVEs linked to a specific bug |
//!
//! Each read function has a `_raw` counterpart that also keeps the exact
//! JSON Launchpad sent. [`search_cves_raw`] filters locally, so it returns a
//! [`CveSearch`] rather than a Launchpad collection.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient, Raw};
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
    pub date_made_public: Option<Timestamp>,
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
    /// Maximum number of CVEs Launchpad returns, before
    /// [`Self::search_text`] is applied.
    pub limit: Option<u32>,
    /// Keyword filter applied client-side against the CVE sequence number,
    /// title, and description (case-insensitive substring match).
    pub search_text: Option<&'a str>,
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
    Ok(get_cve_raw(client, sequence).await?.value)
}

/// Like [`get_cve()`], but also keep the exact JSON Launchpad returned.
pub async fn get_cve_raw(client: &LaunchpadClient, sequence: &str) -> Result<Raw<Cve>> {
    client
        .get_raw(&format!("/bugs/cve/{}", enc(sequence)))
        .await
}

/// CVEs found by [`search_cves_raw()`].
#[derive(Debug, Clone, PartialEq)]
pub struct CveSearch {
    /// Matching CVEs, in the order Launchpad returned them, each with the
    /// exact JSON Launchpad returned for it.
    pub cves: Vec<Raw<Cve>>,
    /// `true` when every CVE matching the server-side filters was checked,
    /// so `cves` holds every match. `false` when Launchpad's result limit may
    /// have left further CVEs unchecked.
    pub complete: bool,
}

/// Search for CVEs with optional distribution and date filters.
///
/// The [`CveSearchParams::search_text`] field performs a client-side
/// case-insensitive substring match against each CVE's sequence number,
/// title, and description. The Launchpad CVE `advancedSearch` API does not
/// expose a server-side full-text search parameter, so filtering happens
/// after results are fetched.
pub async fn search_cves(
    client: &LaunchpadClient,
    params: &CveSearchParams<'_>,
) -> Result<Vec<Cve>> {
    let found = search_cves_raw(client, params).await?;
    Ok(found.cves.into_iter().map(|cve| cve.value).collect())
}

/// Like [`search_cves()`], but keep the exact JSON of each CVE and report
/// whether the result is complete.
///
/// Launchpad applies [`CveSearchParams::limit`] before the local
/// [`CveSearchParams::search_text`] filter, so no Launchpad link or count can
/// describe the filtered list. [`CveSearch::complete`] is `true` only when
/// Launchpad returned fewer CVEs than `limit`. Without a limit, Launchpad
/// applies its own default batch size, so the result is never reported as
/// complete.
pub async fn search_cves_raw(
    client: &LaunchpadClient,
    params: &CveSearchParams<'_>,
) -> Result<CveSearch> {
    let mut query = "/bugs/cve?ws.op=advancedSearch".to_string();

    if let Some(dist) = params.in_distribution {
        let dist_url = client.url(&format!("/{}", enc(dist)));
        query.push_str(&format!("&in_distribution={}", enc(&dist_url)));
    }
    if let Some(dist) = params.not_in_distribution {
        let dist_url = client.url(&format!("/{}", enc(dist)));
        query.push_str(&format!("&not_in_distribution={}", enc(&dist_url)));
    }
    if let Some(since) = params.modified_since {
        query.push_str(&format!("&modified_since={}", enc(since)));
    }
    if let Some(limit) = params.limit {
        query.push_str(&format!("&limit={limit}"));
    }

    let mut page_url = client.url(&query);
    let needle = params.search_text.map(str::to_lowercase);
    let mut fetched: u64 = 0;
    let mut cves = Vec::new();
    loop {
        let page: Collection<serde_json::Value> = client.get_url(&page_url).await?;
        for entry in page.entries {
            fetched += 1;
            let cve = Raw::<Cve>::from_value(entry)?;
            if needle
                .as_deref()
                .is_none_or(|needle| cve_matches(&cve.value, needle))
            {
                cves.push(cve);
            }
        }
        match page.next_collection_link {
            Some(next) => page_url = next,
            None => break,
        }
    }

    // Without an explicit limit, Launchpad's unknown default limit applies.
    let complete = params.limit.is_some_and(|limit| fetched < u64::from(limit));
    Ok(CveSearch { cves, complete })
}

/// Case-insensitive substring match; `needle` must already be lower case.
fn cve_matches(cve: &Cve, needle: &str) -> bool {
    [
        Some(cve.sequence.as_str()),
        cve.title.as_deref(),
        cve.description.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|text| text.to_lowercase().contains(needle))
}

/// List all CVEs linked to a specific Launchpad bug.
pub async fn get_bug_cves(client: &LaunchpadClient, bug_id: u64) -> Result<Vec<Cve>> {
    Ok(get_bug_cves_raw(client, bug_id).await?.value)
}

/// Like [`get_bug_cves()`], but also keep Launchpad's collection JSON.
pub async fn get_bug_cves_raw(client: &LaunchpadClient, bug_id: u64) -> Result<Raw<Vec<Cve>>> {
    let url = client.url(&format!("/bugs/{bug_id}/cves"));
    Collection::fetch_all_raw(client, &url).await
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
