//! Launchpad personal access token management.
//!
//! Personal access tokens allow pushing to Git repositories over HTTPS
//! without using the full OAuth flow.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`issue_project_access_token`] | Issue a token scoped to a project |
//! | [`issue_git_access_token`] | Issue a token scoped to a Git repository |
//! | [`list_project_access_tokens`] | List tokens for a project |
//! | [`list_git_access_tokens`] | List tokens for a Git repository |
//! | [`revoke_access_token`] | Revoke a token by its `self_link` URL |
//!
//! The list operations also have `*_raw` counterparts that keep the exact
//! JSON Launchpad returned (see [`Raw`]). Issuing a token returns only the
//! plaintext secret, not a Launchpad entry, and revoking returns nothing, so
//! those operations have no raw counterparts.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient, Raw};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A personal access token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessToken {
    /// Short description of what the token is used for.
    pub description: Option<String>,
    /// Scopes granted to this token (e.g. `["repository:push"]`).
    pub scopes: Option<Vec<String>>,
    /// Whether the token has expired.
    pub is_expired: Option<bool>,
    /// When the token was created.
    pub date_created: Option<Timestamp>,
    /// When the token was last used, if ever.
    pub date_last_used: Option<Timestamp>,
    /// When the token expires, if it has an expiry.
    pub date_expires: Option<Timestamp>,
    /// API self-link (used to revoke the token).
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Issue a new personal access token for a project.
///
/// Returns the plaintext token secret. **This secret is only available at
/// creation time** — Launchpad stores only a hash.
///
/// `project` is the project name. `scopes` is a slice of scope strings
/// (e.g. `["repository:push", "repository:build_status"]`).
pub async fn issue_project_access_token(
    client: &LaunchpadClient,
    project: &str,
    description: &str,
    scopes: &[&str],
) -> Result<String> {
    let mut pairs: Vec<(&str, &str)> =
        vec![("ws.op", "issueAccessToken"), ("description", description)];
    for scope in scopes {
        pairs.push(("scopes", scope));
    }
    client
        .post_pairs(&format!("/{}", urlenc(project)), &pairs)
        .await
}

/// Issue a new personal access token for a Git repository.
///
/// `repo_path` is the repository slug without a leading `/`,
/// e.g. `"~person/project/+git/name"`.
pub async fn issue_git_access_token(
    client: &LaunchpadClient,
    repo_path: &str,
    description: &str,
    scopes: &[&str],
) -> Result<String> {
    let clean = repo_path.trim_start_matches('/');
    let mut pairs: Vec<(&str, &str)> =
        vec![("ws.op", "issueAccessToken"), ("description", description)];
    for scope in scopes {
        pairs.push(("scopes", scope));
    }
    client.post_pairs(&format!("/{clean}"), &pairs).await
}

/// List personal access tokens for a project.
pub async fn list_project_access_tokens(
    client: &LaunchpadClient,
    project: &str,
) -> Result<Vec<AccessToken>> {
    Collection::fetch_all(client, &project_tokens_url(client, project)).await
}

/// Like [`list_project_access_tokens()`], but return Launchpad's collection
/// shape with every page's entries unmodified.
pub async fn list_project_access_tokens_raw(
    client: &LaunchpadClient,
    project: &str,
) -> Result<Raw<Vec<AccessToken>>> {
    Collection::fetch_all_raw(client, &project_tokens_url(client, project)).await
}

fn project_tokens_url(client: &LaunchpadClient, project: &str) -> String {
    client.url(&format!("/{}?ws.op=getAccessTokens", urlenc(project)))
}

/// List personal access tokens for a Git repository.
pub async fn list_git_access_tokens(
    client: &LaunchpadClient,
    repo_path: &str,
) -> Result<Vec<AccessToken>> {
    Collection::fetch_all(client, &git_tokens_url(client, repo_path)).await
}

/// Like [`list_git_access_tokens()`], but return Launchpad's collection shape
/// with every page's entries unmodified.
pub async fn list_git_access_tokens_raw(
    client: &LaunchpadClient,
    repo_path: &str,
) -> Result<Raw<Vec<AccessToken>>> {
    Collection::fetch_all_raw(client, &git_tokens_url(client, repo_path)).await
}

fn git_tokens_url(client: &LaunchpadClient, repo_path: &str) -> String {
    let clean = repo_path.trim_start_matches('/');
    client.url(&format!("/{clean}?ws.op=getAccessTokens"))
}

/// Revoke a personal access token.
///
/// `token_url` is the `self_link` of the token to revoke.
pub async fn revoke_access_token(client: &LaunchpadClient, token_url: &str) -> Result<()> {
    client
        .post_pairs_url_ok(token_url, &[("ws.op", "revoke")])
        .await
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
    fn access_token_deserialise_minimal() {
        let json = r#"{
            "description": "My push token",
            "scopes": ["repository:push"],
            "is_expired": false,
            "date_created": null,
            "date_last_used": null,
            "date_expires": null,
            "self_link": "https://api.launchpad.net/devel/launchpad/+access-token/1",
            "web_link": null
        }"#;
        let tok: AccessToken = serde_json::from_str(json).unwrap();
        assert_eq!(tok.description.as_deref(), Some("My push token"));
        assert_eq!(
            tok.scopes
                .as_ref()
                .map(|v| v.iter().map(String::as_str).collect::<Vec<_>>()),
            Some(vec!["repository:push"])
        );
    }
}
