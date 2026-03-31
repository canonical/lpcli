//! Launchpad specification (blueprint) operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_specification`] | Fetch a single specification by target and slug |
//! | [`list_project_specifications`] | List all specs for a project |
//! | [`list_valid_project_specifications`] | List non-obsolete specs for a project |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A Launchpad specification (blueprint).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Specification {
    /// Short URL-friendly slug (e.g. `"my-feature"`).
    pub name: String,
    /// Human-readable title.
    pub title: Option<String>,
    /// One-paragraph summary.
    pub summary: Option<String>,
    /// Priority: `"Essential"`, `"High"`, `"Medium"`, `"Low"`, `"Undefined"`.
    pub priority: Option<String>,
    /// Lifecycle status: `"Not started"`, `"Started"`, `"Complete"`.
    pub lifecycle_status: Option<String>,
    /// Implementation status (e.g. `"Unknown"`, `"Good progress"`, etc.).
    pub implementation_status: Option<String>,
    /// Approval status (e.g. `"Approved"`, `"Pending Approval"`, etc.).
    pub definition_status: Option<String>,
    /// URL of the full specification document (usually a wiki page).
    pub specification_url: Option<String>,
    /// API link to the owner.
    pub owner_link: Option<String>,
    /// API link to the assignee.
    pub assignee_link: Option<String>,
    /// API link to the drafter.
    pub drafter_link: Option<String>,
    /// API link to the target project or distribution.
    pub target_link: Option<String>,
    /// API link to the target milestone, if any.
    pub milestone_link: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// Date created.
    pub date_created: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch a single specification by target and slug.
///
/// `target` is a project or distribution name; `name` is the spec slug
/// (e.g. `"my-feature"`).
pub async fn get_specification(
    client: &LaunchpadClient,
    target: &str,
    name: &str,
) -> Result<Specification> {
    client.get(&format!("/{target}/+spec/{name}")).await
}

/// List all specifications for a project, including obsolete ones.
pub async fn list_project_specifications(
    client: &LaunchpadClient,
    project: &str,
) -> Result<Vec<Specification>> {
    let url = client.url(&format!("/{project}/all_specifications"));
    Collection::fetch_all(client, &url).await
}

/// List non-obsolete specifications for a project.
pub async fn list_valid_project_specifications(
    client: &LaunchpadClient,
    project: &str,
) -> Result<Vec<Specification>> {
    let url = client.url(&format!("/{project}/valid_specifications"));
    Collection::fetch_all(client, &url).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specification_deserialise_minimal() {
        let json = r#"{
            "name": "cool-feature",
            "title": "A Cool Feature",
            "summary": "Makes things cool.",
            "priority": "High",
            "lifecycle_status": "Not started",
            "implementation_status": null,
            "definition_status": "Approved",
            "specification_url": null,
            "owner_link": null,
            "assignee_link": null,
            "drafter_link": null,
            "target_link": null,
            "milestone_link": null,
            "self_link": null,
            "web_link": null,
            "date_created": null
        }"#;
        let spec: Specification = serde_json::from_str(json).unwrap();
        assert_eq!(spec.name, "cool-feature");
        assert_eq!(spec.priority.as_deref(), Some("High"));
        assert_eq!(spec.lifecycle_status.as_deref(), Some("Not started"));
    }
}
