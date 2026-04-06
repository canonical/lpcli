//! Connectivity and authentication status checks.
//!
//! Provides lightweight checks to verify:
//!
//! * Whether the user has stored OAuth credentials on disk.
//! * Whether the Launchpad REST API is reachable and returning valid responses.
//!
//! These checks are intentionally separate from the other domain modules so
//! that the `status` command can surface helpful diagnostics even when
//! something is misconfigured.
//!
//! # Example
//!
//! ```no_run
//! use lpcli::status;
//!
//! #[tokio::main]
//! async fn main() {
//!     let server = status::check_server().await;
//!     let auth = status::check_auth().await;
//!     println!("Server reachable: {}", server.reachable);
//!     println!("Logged in: {}", auth.logged_in);
//! }
//! ```

use serde::Deserialize;

use crate::auth;
use crate::client::LaunchpadClient;
use crate::error::LpError;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Outcome of checking the Launchpad REST API server availability.
#[derive(Debug, Clone)]
pub struct ServerStatus {
    /// `true` when the API root endpoint returned a successful JSON response.
    pub reachable: bool,
    /// HTTP status code returned by the server, if a response was received.
    pub http_status: Option<u16>,
    /// The `resource_type_link` field from the API root JSON, confirming that
    /// the endpoint is serving a valid Launchpad API response.
    pub resource_type_link: Option<String>,
    /// Human-readable error description when `reachable` is `false`.
    pub error: Option<String>,
}

/// Outcome of checking the user's local authentication credentials.
#[derive(Debug, Clone)]
pub struct AuthStatus {
    /// `true` when a credential file was found and successfully parsed.
    pub logged_in: bool,
    /// Launchpad username (e.g. `"~someuser"`) obtained from a successful
    /// authenticated API call; `None` when not logged in or when the server
    /// is unreachable.
    pub username: Option<String>,
}

// ---------------------------------------------------------------------------
// Minimal deserialization of the Launchpad service-root
// ---------------------------------------------------------------------------

/// Subset of the Launchpad service-root JSON response used for status checks.
///
/// All other fields returned by the API are silently ignored.
#[derive(Debug, Deserialize)]
struct ApiRoot {
    /// Identifies this endpoint as a valid Launchpad service root.
    resource_type_link: Option<String>,
    /// Points to the authenticated user's person resource.
    /// Present and non-null only for authenticated requests.
    me_link: Option<String>,
}

/// Minimal deserialisation of a Launchpad person resource used to extract the
/// authenticated user's Launchpad username.
#[derive(Debug, Deserialize)]
struct MePerson {
    /// Launchpad username (e.g. `"someuser"`), without the leading `~`.
    name: Option<String>,
}

// ---------------------------------------------------------------------------
// Public functions
// ---------------------------------------------------------------------------

/// Check whether the Launchpad REST API server is reachable and functional.
///
/// Makes an **unauthenticated** GET request to the API root
/// (`https://api.launchpad.net/devel/`) and verifies that a valid JSON
/// response is received.
///
/// This function does not return `Err`; all failures are encoded in the
/// returned [`ServerStatus`] so callers can display them without propagating
/// errors.
pub async fn check_server() -> ServerStatus {
    let client = LaunchpadClient::new(None);
    match client.get::<ApiRoot>("").await {
        Ok(root) => ServerStatus {
            reachable: true,
            http_status: Some(200),
            resource_type_link: root.resource_type_link,
            error: None,
        },
        Err(e) => {
            let http_status = match &e {
                LpError::Api { status, .. } => Some(*status),
                _ => None,
            };
            ServerStatus {
                reachable: false,
                http_status,
                resource_type_link: None,
                error: Some(e.to_string()),
            }
        }
    }
}

/// Check whether the user has stored OAuth credentials and, if so, verify
/// them against the Launchpad API.
///
/// Behaviour:
///
/// * If no credential file exists, returns
///   `AuthStatus { logged_in: false, username: None }`.
/// * If credentials exist and the server is reachable, an authenticated
///   request is made to the API root to obtain the Launchpad username from
///   the `me_link` field.
/// * If credentials exist but the server cannot be reached (or the token has
///   been revoked), returns `AuthStatus { logged_in: true, username: None }`
///   to indicate that local credentials are present but unverified.
pub async fn check_auth() -> AuthStatus {
    // 1. Try to load credentials from disk.
    let creds = match auth::load_credentials() {
        Ok(c) => c,
        Err(LpError::NotAuthenticated) | Err(LpError::Config(_)) => {
            return AuthStatus {
                logged_in: false,
                username: None,
            };
        }
        Err(_) => {
            return AuthStatus {
                logged_in: false,
                username: None,
            };
        }
    };

    // 2. Credentials are present; make an authenticated request to the API
    //    root to obtain the `me_link` and extract the Launchpad username.
    let client = LaunchpadClient::new(Some(creds));
    let username = match client.get::<ApiRoot>("").await {
        Ok(root) => {
            // `me_link` is "https://api.launchpad.net/devel/+me", which
            // redirects to the authenticated user's person resource.  Follow
            // it with another authenticated GET to obtain the real username
            // from the `name` field.
            if let Some(me_url) = root.me_link {
                client
                    .get_url::<MePerson>(&me_url)
                    .await
                    .ok()
                    .and_then(|p| p.name)
                    .map(|n| format!("~{n}"))
            } else {
                None
            }
        }
        Err(_) => None,
    };

    AuthStatus {
        logged_in: true,
        username,
    }
}
