//! Launchpad Translations read-only operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_distro_series_import_queue`] | List translation import queue entries for a distro series |
//! | [`get_distro_series_templates`] | List translation templates for a distro series |
//!
//! Each read function has a `_raw` counterpart that also returns Launchpad's
//! collection JSON: `start`, exact `entries`, and `total_size` or
//! `total_size_link`.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient, Raw};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// An entry in the Launchpad translation import queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationImportQueueEntry {
    /// Path of the file being imported (e.g. `"po/fr.po"`).
    pub path: Option<String>,
    /// Import status: `"Approved"`, `"Imported"`, `"Deleted"`,
    /// `"Failed"`, `"Needs Review"`, `"Blocked"`, `"Needs Information"`.
    pub status: Option<String>,
    /// When this entry was created.
    pub date_created: Option<Timestamp>,
    /// API link to the person who uploaded the file.
    pub uploader_link: Option<String>,
    /// API link to the associated source package, if any.
    pub sourcepackage_link: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

/// A translation template (POT file) registered in Launchpad.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslationTemplate {
    /// Template name (e.g. `"firefox"`).
    pub name: Option<String>,
    /// Relative path to the template file.
    pub path: Option<String>,
    /// Explicit priority ordering — higher is more important.
    pub priority: Option<i64>,
    /// Whether this is an active current template.
    pub is_current: Option<bool>,
    /// Whether this template is obsolete.
    pub is_obsolete: Option<bool>,
    /// When the template was last updated.
    pub date_last_updated: Option<Timestamp>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// List translation import queue entries for an Ubuntu distro series.
///
/// Uses the `getTranslationImportQueueEntries` custom GET method on the
/// distro series resource.
pub async fn get_distro_series_import_queue(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
) -> Result<Vec<TranslationImportQueueEntry>> {
    Ok(get_distro_series_import_queue_raw(client, distro, series)
        .await?
        .value)
}

/// Like [`get_distro_series_import_queue()`], but also keep Launchpad's
/// collection JSON.
pub async fn get_distro_series_import_queue_raw(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
) -> Result<Raw<Vec<TranslationImportQueueEntry>>> {
    let url = client.url(&format!(
        "/{}/{}?ws.op=getTranslationImportQueueEntries",
        urlenc(distro),
        urlenc(series),
    ));
    Collection::fetch_all_raw(client, &url).await
}

/// List translation templates for an Ubuntu distro series.
///
/// Uses the `getTranslationTemplates` custom GET method on the distro series
/// resource.
pub async fn get_distro_series_templates(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
) -> Result<Vec<TranslationTemplate>> {
    Ok(get_distro_series_templates_raw(client, distro, series)
        .await?
        .value)
}

/// Like [`get_distro_series_templates()`], but also keep Launchpad's
/// collection JSON.
pub async fn get_distro_series_templates_raw(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
) -> Result<Raw<Vec<TranslationTemplate>>> {
    let url = client.url(&format!(
        "/{}/{}?ws.op=getTranslationTemplates",
        urlenc(distro),
        urlenc(series),
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
    fn translation_template_deserialise() {
        let json = r#"{
            "name": "firefox",
            "path": "po/firefox.pot",
            "priority": 100,
            "is_current": true,
            "is_obsolete": false,
            "date_last_updated": null,
            "self_link": null,
            "web_link": null
        }"#;
        let tmpl: TranslationTemplate = serde_json::from_str(json).unwrap();
        assert_eq!(tmpl.name.as_deref(), Some("firefox"));
        assert_eq!(tmpl.is_current, Some(true));
    }
}
