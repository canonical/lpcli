//! Launchpad Translations read-only operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_distro_series_import_queue`] | List translation import queue entries for a distro series |
//! | [`get_distro_series_templates`] | List translation templates for a distro series |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
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
    pub date_created: Option<DateTime<Utc>>,
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
    pub date_last_updated: Option<DateTime<Utc>>,
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
    let url = client.url(&format!(
        "/{distro}/{series}?ws.op=getTranslationImportQueueEntries"
    ));
    Collection::fetch_all(client, &url).await
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
    let url = client.url(&format!(
        "/{distro}/{series}?ws.op=getTranslationTemplates"
    ));
    Collection::fetch_all(client, &url).await
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
