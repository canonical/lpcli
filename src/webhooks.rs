//! Launchpad webhook management.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`create_webhook`] | Create a new webhook on a target |
//! | [`list_target_webhooks`] | List webhooks for a project, distribution, or Git repo |
//! | [`delete_webhook`] | Delete a webhook by URL |
//! | [`ping_webhook`] | Send a test ping delivery |
//! | [`list_deliveries`] | List recent deliveries for a webhook |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A Launchpad webhook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Webhook {
    /// URL that receives event payloads.
    pub delivery_url: Option<String>,
    /// Whether the webhook is active.
    pub active: Option<bool>,
    /// The event type slugs subscribed to (e.g. `["git:push:0.1"]`).
    pub event_types: Option<Vec<String>>,
    /// API link to the target object.
    pub target_link: Option<String>,
    /// API link to the person who created the webhook.
    pub registrant_link: Option<String>,
    /// Date created.
    pub date_created: Option<DateTime<Utc>>,
    /// Date last modified.
    pub date_last_modified: Option<DateTime<Utc>>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

/// A single delivery attempt for a webhook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookDelivery {
    /// Whether this delivery has been successfully acknowledged.
    pub successful: Option<bool>,
    /// HTTP response status code returned by the delivery target.
    pub response_status_code: Option<u64>,
    /// Whether the delivery is still pending.
    pub pending: Option<bool>,
    /// When the delivery was dispatched.
    pub date_sent: Option<DateTime<Utc>>,
    /// API link to the parent webhook.
    pub webhook_link: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Create a new webhook on a Launchpad target.
///
/// `target_path` is the API path for the target, e.g.:
/// - `"/launchpad"` for a project
/// - `"/ubuntu"` for a distribution
/// - `"/~person/project/+git/name"` for a Git repository
///
/// `event_types` is a slice of event type slugs such as
/// `["git:push:0.1", "merge-proposal:0.1"]`.
pub async fn create_webhook(
    client: &LaunchpadClient,
    target_path: &str,
    delivery_url: &str,
    event_types: &[&str],
    active: bool,
    secret: Option<&str>,
) -> Result<Webhook> {
    let mut pairs: Vec<(&str, &str)> = vec![
        ("ws.op", "newWebhook"),
        ("delivery_url", delivery_url),
        ("active", if active { "true" } else { "false" }),
    ];
    for et in event_types {
        pairs.push(("event_types", et));
    }
    if let Some(s) = secret {
        pairs.push(("secret", s));
    }
    let clean = target_path.trim_start_matches('/');
    let location = client
        .post_pairs_created_location(&format!("/{clean}"), &pairs)
        .await?;
    client.get_url(&location).await
}

/// List webhooks registered on a Launchpad target.
///
/// `target_path` format is the same as for [`create_webhook`].
pub async fn list_target_webhooks(
    client: &LaunchpadClient,
    target_path: &str,
) -> Result<Vec<Webhook>> {
    let clean = target_path.trim_start_matches('/');
    let url = client.url(&format!("/{clean}/webhooks"));
    Collection::fetch_all(client, &url).await
}

/// Delete a webhook.
///
/// `webhook_url` is the `self_link` of the webhook.
pub async fn delete_webhook(client: &LaunchpadClient, webhook_url: &str) -> Result<()> {
    client.delete_url_ok(webhook_url).await
}

/// Send a test ping to a webhook.
///
/// Returns the resulting [`WebhookDelivery`] record.
pub async fn ping_webhook(
    client: &LaunchpadClient,
    webhook_url: &str,
) -> Result<WebhookDelivery> {
    let location = client
        .post_pairs_url_created_location(webhook_url, &[("ws.op", "ping")])
        .await?;
    client.get_url(&location).await
}

/// List recent deliveries for a webhook.
pub async fn list_deliveries(
    client: &LaunchpadClient,
    webhook_url: &str,
) -> Result<Vec<WebhookDelivery>> {
    let url = format!("{webhook_url}/deliveries");
    Collection::fetch_all(client, &url).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_deserialise_minimal() {
        let json = r#"{
            "delivery_url": "https://example.com/hook",
            "active": true,
            "event_types": ["git:push:0.1"],
            "target_link": null,
            "registrant_link": null,
            "date_created": null,
            "date_last_modified": null,
            "self_link": "https://api.launchpad.net/devel/launchpad/+webhook/1",
            "web_link": null
        }"#;
        let wh: Webhook = serde_json::from_str(json).unwrap();
        assert_eq!(wh.delivery_url.as_deref(), Some("https://example.com/hook"));
        assert_eq!(wh.active, Some(true));
        assert_eq!(
            wh.event_types.as_ref().map(|v| v.iter().map(String::as_str).collect::<Vec<_>>()),
            Some(vec!["git:push:0.1"])
        );
    }
}
