//! Launchpad people (users and teams) operations.
//!
//! This module provides types and functions for querying Launchpad people and
//! teams via the REST API (`https://api.launchpad.net/devel/~{name}`).
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_person`] | Fetch a person or team by Launchpad name |
//! | [`search_people`] | Search for people by display name |
//! | [`get_team_members`] | List members of a team |
//! | [`get_person_bugs`] | Bugs assigned to or filed by a person |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Whether a Launchpad account is a person or a team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountType {
    /// An individual user account.
    Person,
    /// A team account.
    Team,
}

impl std::fmt::Display for AccountType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Person => write!(f, "Person"),
            Self::Team => write!(f, "Team"),
        }
    }
}

/// A Launchpad person or team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    /// Launchpad username (e.g. `"ubuntu-dev"`).
    pub name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Whether this is a person or a team.
    #[serde(rename = "account_type")]
    pub account_type: Option<AccountType>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web profile URL.
    pub web_link: Option<String>,
    /// Date the account was created.
    pub date_created: Option<DateTime<Utc>>,
    /// Email address (only visible to the authenticated user themselves).
    pub preferred_email_address_link: Option<String>,
    /// Short description / biography.
    pub description: Option<String>,
    /// Karma score.
    pub karma: Option<i64>,
}

/// A team membership entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamMembership {
    /// API self-link.
    pub self_link: Option<String>,
    /// Link to the member person.
    pub member_link: Option<String>,
    /// Link to the team.
    pub team_link: Option<String>,
    /// Membership status (e.g. "Approved", "Administrator").
    pub status: Option<String>,
    /// When the membership was first made active.
    pub date_joined: Option<DateTime<Utc>>,
    /// When the membership expires.
    pub date_expires: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch a Launchpad person or team by their Launchpad name.
///
/// `name` is the Launchpad account name without the leading `~`
/// (e.g. `"ubuntu-dev"`, not `"~ubuntu-dev"`).
///
/// # Errors
///
/// Returns [`crate::error::LpError::NotFound`] when the person does not exist.
pub async fn get_person(client: &LaunchpadClient, name: &str) -> Result<Person> {
    client.get(&format!("/~{name}")).await
}

/// Search for people by display name or Launchpad name.
pub async fn search_people(
    client: &LaunchpadClient,
    query: &str,
) -> Result<Vec<Person>> {
    let encoded = url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>();
    let url = client.url(&format!("/people?ws.op=find&text={encoded}"));
    Collection::fetch_all(client, &url).await
}

/// List the (approved) members of a Launchpad team.
///
/// Uses the `members_details` collection link which returns `TeamMembership`
/// resources (with status and join date), rather than the `members` link which
/// returns plain `Person` resources.
pub async fn get_team_members(
    client: &LaunchpadClient,
    team_name: &str,
) -> Result<Vec<TeamMembership>> {
    let url = client.url(&format!("/~{team_name}/members_details"));
    Collection::fetch_all(client, &url).await
}

/// List bugs filed by a person (filed by `~{name}`).
pub async fn get_person_bugs(
    client: &LaunchpadClient,
    name: &str,
) -> Result<Vec<crate::bugs::Bug>> {
    let url = client.url(&format!("/~{name}/+bugs?ws.op=searchTasks&bug_reporter=~{name}"));
    Collection::fetch_all(client, &url).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_type_display() {
        assert_eq!(AccountType::Person.to_string(), "Person");
        assert_eq!(AccountType::Team.to_string(), "Team");
    }

    #[test]
    fn person_deserialise_minimal() {
        let json = r#"{
            "name": "ubuntu-dev",
            "display_name": "Ubuntu Developers",
            "account_type": "team",
            "self_link": null,
            "web_link": null,
            "date_created": null,
            "preferred_email_address_link": null,
            "description": null,
            "karma": null
        }"#;
        let person: Person = serde_json::from_str(json).unwrap();
        assert_eq!(person.name, "ubuntu-dev");
        assert_eq!(person.account_type, Some(AccountType::Team));
    }

    #[test]
    fn person_deserialise_person_type() {
        let json = r#"{
            "name": "jdoe",
            "display_name": "Jane Doe",
            "account_type": "person",
            "self_link": "https://api.launchpad.net/devel/~jdoe",
            "web_link": "https://launchpad.net/~jdoe",
            "date_created": null,
            "preferred_email_address_link": null,
            "description": "A developer",
            "karma": 1234
        }"#;
        let person: Person = serde_json::from_str(json).unwrap();
        assert_eq!(person.name, "jdoe");
        assert_eq!(person.karma, Some(1234));
        assert_eq!(person.description.as_deref(), Some("A developer"));
    }

    #[test]
    fn team_membership_deserialise() {
        let json = r#"{
            "self_link": "https://api.launchpad.net/devel/~ubuntu-dev/+member/jdoe",
            "member_link": "https://api.launchpad.net/devel/~jdoe",
            "team_link": "https://api.launchpad.net/devel/~ubuntu-dev",
            "status": "Approved",
            "date_joined": null,
            "date_expires": null
        }"#;
        let membership: TeamMembership = serde_json::from_str(json).unwrap();
        assert_eq!(membership.status.as_deref(), Some("Approved"));
        assert_eq!(membership.member_link.as_deref(), Some("https://api.launchpad.net/devel/~jdoe"));
    }
}
