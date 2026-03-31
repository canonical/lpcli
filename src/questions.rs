//! Launchpad question-and-answer (support) operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_question_by_id`] | Fetch a question by its global numeric ID |
//! | [`get_question`] | Fetch a question from a specific target |
//! | [`search_questions`] | Search questions on a project or distribution |
//! | [`get_question_messages`] | List messages (answers) on a question |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A Launchpad support question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// Numeric identifier.
    pub id: u64,
    /// One-line title / summary.
    pub title: Option<String>,
    /// Full question description.
    pub description: Option<String>,
    /// Status: `"Open"`, `"Needs information"`, `"Answered"`, `"Solved"`,
    /// `"Expired"`, or `"Invalid"`.
    pub status: Option<String>,
    /// API link to the question owner.
    pub owner_link: Option<String>,
    /// API link to the target (project or distribution).
    pub target_link: Option<String>,
    /// API link to the person who answered, if any.
    pub answerer_link: Option<String>,
    /// When the question was created.
    pub date_created: Option<DateTime<Utc>>,
    /// When the question was last updated.
    pub date_last_updated: Option<DateTime<Utc>>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

/// A message attached to a Launchpad question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionMessage {
    /// Sequential index within the question's message list.
    pub index: Option<u64>,
    /// Message body text.
    pub content: Option<String>,
    /// Action taken: `"Question"`, `"Answer"`, `"Comment"`, `"Expire"`,
    /// `"Reopen"`, `"Confirm"`.
    pub action: Option<String>,
    /// API link to the message author.
    pub owner_link: Option<String>,
    /// When the message was posted.
    pub date_created: Option<DateTime<Utc>>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

/// Parameters for searching questions.
#[derive(Debug, Clone, Default)]
pub struct QuestionSearchParams<'a> {
    /// Full-text keyword search against question titles and descriptions.
    pub search_text: Option<&'a str>,
    /// Filter by status (e.g. `"Open"`, `"Answered"`, `"Solved"`).
    pub status: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch a question by its global numeric ID.
pub async fn get_question_by_id(
    client: &LaunchpadClient,
    question_id: u64,
) -> Result<Question> {
    let url =
        client.url(&format!("/questions?ws.op=getByID&question_id={question_id}"));
    client.get_url(&url).await
}

/// Fetch a question from a specific target by its ID.
///
/// `target` is a project or distribution name.
pub async fn get_question(
    client: &LaunchpadClient,
    target: &str,
    question_id: u64,
) -> Result<Question> {
    client.get(&format!("/{target}/+question/{question_id}")).await
}

/// Search questions on a project or distribution.
///
/// `target` is a project or distribution name.
pub async fn search_questions(
    client: &LaunchpadClient,
    target: &str,
    params: &QuestionSearchParams<'_>,
) -> Result<Vec<Question>> {
    let mut query = format!("/{target}?ws.op=searchQuestions");
    if let Some(text) = params.search_text {
        let enc: String =
            url::form_urlencoded::byte_serialize(text.as_bytes()).collect();
        query.push_str(&format!("&search={enc}"));
    }
    if let Some(status) = params.status {
        let enc: String =
            url::form_urlencoded::byte_serialize(status.as_bytes()).collect();
        query.push_str(&format!("&status={enc}"));
    }
    let url = client.url(&query);
    Collection::fetch_all(client, &url).await
}

/// List messages on a question.
pub async fn get_question_messages(
    client: &LaunchpadClient,
    target: &str,
    question_id: u64,
) -> Result<Vec<QuestionMessage>> {
    let url = client.url(&format!("/{target}/+question/{question_id}/messages"));
    Collection::fetch_all(client, &url).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_deserialise_minimal() {
        let json = r#"{
            "id": 42,
            "title": "How do I do X?",
            "description": "I tried Y but it didn't work.",
            "status": "Open",
            "owner_link": null,
            "target_link": null,
            "answerer_link": null,
            "date_created": null,
            "date_last_updated": null,
            "self_link": null,
            "web_link": null
        }"#;
        let q: Question = serde_json::from_str(json).unwrap();
        assert_eq!(q.id, 42);
        assert_eq!(q.status.as_deref(), Some("Open"));
    }

    #[test]
    fn question_search_params_default() {
        let p = QuestionSearchParams::default();
        assert!(p.search_text.is_none());
        assert!(p.status.is_none());
    }
}
