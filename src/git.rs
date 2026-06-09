//! Launchpad Git repository operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_git_repository`] | Fetch a Git repository by its Launchpad path |
//! | [`get_git_repository_by_unique_name`] | Fetch a repo by its unique name |
//! | [`get_default_git_repository`] | Fetch the default repo for a target |
//! | [`list_person_git_repositories`] | List repos owned by a person |
//! | [`list_git_refs`] | List branches and tags in a repository |
//! | [`list_merge_proposals`] | List merge proposals for a repository |
//! | [`get_merge_proposal`] | Fetch a single merge proposal by ID |
//! | [`get_merge_proposal_comments`] | Fetch review comments for a merge proposal |
//! | [`get_inline_comments`] | Fetch inline (diff) comments for a merge proposal |
//! | [`get_preview_diffs`] | List all preview diffs for a merge proposal |

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A Launchpad-hosted Git repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepository {
    /// Repository name (the last segment of the unique name).
    pub name: Option<String>,
    /// Unique name, e.g. `"~person/project/+git/repo"`.
    pub unique_name: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Whether this is the default repo for the owner+target combination.
    pub owner_default: Option<bool>,
    /// Whether this is the target's globally default repo.
    pub target_default: Option<bool>,
    /// Repository type: `"Hosted"`, `"Imported"`, or `"Remote"`.
    pub repository_type: Option<String>,
    /// Information type: `"Public"`, `"Private"`, etc.
    pub information_type: Option<String>,
    /// Whether the repository is private.
    pub private: Option<bool>,
    /// API link to the owner.
    pub owner_link: Option<String>,
    /// API link to the target project, distribution, or source package.
    pub target_link: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// Date created.
    pub date_created: Option<DateTime<Utc>>,
    /// Date last modified.
    pub date_last_modified: Option<DateTime<Utc>>,
    /// Number of loose objects (indicates whether a repack would help).
    pub loose_object_count: Option<u64>,
    /// Number of pack files.
    pub pack_count: Option<u64>,
}

/// A reference (branch or tag) within a Git repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRef {
    /// The full ref path (e.g. `"refs/heads/main"`).
    pub path: Option<String>,
    /// A human-readable display name (usually the branch/tag name).
    pub display_name: Option<String>,
    /// The commit SHA1 at this ref.
    pub commit_sha1: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// API link to the containing repository.
    pub repository_link: Option<String>,
}

/// A merge proposal for merging one Git branch into another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeProposal {
    /// Status: `"Work in progress"`, `"Needs review"`, `"Approved"`, etc.
    pub queue_status: Option<String>,
    /// Proposed commit message.
    pub commit_message: Option<String>,
    /// Description of the change.
    pub description: Option<String>,
    /// API link to the source repository.
    pub source_git_repository_link: Option<String>,
    /// Source branch path.
    pub source_git_path: Option<String>,
    /// API link to the target repository.
    pub target_git_repository_link: Option<String>,
    /// Target branch path.
    pub target_git_path: Option<String>,
    /// API link to the person who registered the proposal.
    pub registrant_link: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// Link to the collection of all review comments on this proposal.
    pub all_comments_collection_link: Option<String>,
    /// Link to the current preview diff.
    pub preview_diff_link: Option<String>,
    /// Link to the collection of all preview diffs.
    pub preview_diffs_collection_link: Option<String>,
    /// Date the proposal was created.
    pub date_created: Option<DateTime<Utc>>,
    /// Date last updated.
    pub date_last_modified: Option<DateTime<Utc>>,
}

/// A code review comment on a merge proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeReviewComment {
    /// Database ID / tracking number.
    pub id: Option<u64>,
    /// Comment title.
    pub title: Option<String>,
    /// Comment body (all text/plain chunks joined).
    pub content: Option<String>,
    /// Review vote: `"Approve"`, `"Needs Fixing"`, `"Abstain"`, etc.
    pub vote: Option<String>,
    /// Free-form vote tag.
    pub vote_tag: Option<String>,
    /// API link to the comment author.
    pub author_link: Option<String>,
    /// When the comment was posted.
    pub date_created: Option<DateTime<Utc>>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
}

/// An inline comment attached to a specific line in a merge proposal diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InlineComment {
    /// The diff line number this comment is attached to (returned as a string by the API).
    pub line_number: Option<String>,
    /// The person who posted the comment (embedded object or link string).
    pub person: Option<serde_json::Value>,
    /// Comment text.
    pub text: Option<String>,
    /// When the comment was posted.
    pub date: Option<String>,
}

impl InlineComment {
    /// Extract a human-readable author name from the `person` field.
    ///
    /// The API may return a full person object (with `name`, `self_link`, etc.)
    /// or a plain URL string.
    pub fn author_display(&self) -> &str {
        match &self.person {
            Some(serde_json::Value::Object(obj)) => obj
                .get("name")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    obj.get("self_link")
                        .and_then(|v| v.as_str())
                        .and_then(|u| u.rsplit('/').next())
                })
                .unwrap_or("unknown"),
            Some(serde_json::Value::String(s)) => s.rsplit('/').next().unwrap_or(s),
            _ => "unknown",
        }
    }
}

/// A preview diff for a merge proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewDiff {
    /// Database ID.
    pub id: Option<u64>,
    /// Link to the diff text content (a librarian file URL).
    pub diff_text_link: Option<String>,
    /// API self-link.
    pub self_link: Option<String>,
    /// When this diff was created.
    pub date_created: Option<DateTime<Utc>>,
    /// Title / description.
    pub title: Option<String>,
    /// Number of lines added.
    pub added_lines_count: Option<u64>,
    /// Number of lines removed.
    pub removed_lines_count: Option<u64>,
    /// Number of lines in the diff.
    pub diff_lines_count: Option<u64>,
    /// Source revision ID used to generate this diff.
    pub source_revision_id: Option<String>,
}

/// Location context for a diff line number — maps a diff-level line to a
/// specific file, source line, and content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLineContext {
    /// File path from the diff header (e.g. `"lib/lp/archivepublisher/model.py"`).
    pub file: String,
    /// The source-code line number in the new (post-patch) file, if applicable.
    /// `None` for removed lines (lines starting with `-`).
    pub new_line: Option<u64>,
    /// The actual text content of the diff line (without the leading +/-/space).
    pub content: String,
}

/// Parse a unified diff and build a mapping from 1-based diff line numbers to
/// file/line context.
///
/// Launchpad's `getInlineComments` uses 1-based line numbers counting from the
/// first line of the full diff text.
pub fn parse_diff_line_map(diff_text: &str) -> Vec<(u64, DiffLineContext)> {
    let mut result = Vec::new();
    let mut current_file: Option<String> = None;
    // Line counter in the "new" (post-patch) file.
    let mut new_lineno: u64 = 0;

    for (i, line) in diff_text.lines().enumerate() {
        let diff_line = (i + 1) as u64; // 1-based

        if line.starts_with("diff --git ") {
            // New file boundary — reset context so inter-file header lines
            // (index, ---, etc.) are not attributed to the previous file.
            current_file = None;
            new_lineno = 0;
            continue;
        }
        
        if line.starts_with("+++ b/") || line.starts_with("+++ ") {
            // New file header, extract path.
            let path = line
                .strip_prefix("+++ b/")
                .or_else(|| line.strip_prefix("+++ "))
                .unwrap_or(line);
            current_file = Some(path.to_string());
        } else if line.starts_with("@@ ") {
            // Hunk header: @@ -old_start,old_count +new_start,new_count @@
            // Extract new_start.
            if let Some(plus_part) = line.split('+').nth(1) {
                let num_str = plus_part.split(',').next().unwrap_or("1");
                new_lineno = num_str.parse::<u64>().unwrap_or(1).saturating_sub(1);
            }
        } else if let Some(ref file) = current_file {
            if let Some(stripped) = line.strip_prefix('+') {
                new_lineno += 1;
                result.push((
                    diff_line,
                    DiffLineContext {
                        file: file.clone(),
                        new_line: Some(new_lineno),
                        content: stripped.to_string(),
                    },
                ));
            } else if let Some(stripped) = line.strip_prefix('-') {
                result.push((
                    diff_line,
                    DiffLineContext {
                        file: file.clone(),
                        new_line: None,
                        content: stripped.to_string(),
                    },
                ));
            } else if !line.starts_with('\\') {
                // Context line (starts with space or is the line itself).
                new_lineno += 1;
                result.push((
                    diff_line,
                    DiffLineContext {
                        file: file.clone(),
                        new_line: Some(new_lineno),
                        content: line.get(1..).unwrap_or(line).to_string(),
                    },
                ));
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch a Git repository by its Launchpad API path.
///
/// `path` is the repository slug, with or without a leading `/`,
/// e.g. `"~person/project/+git/name"` or `"/~person/+git/name"`.
pub async fn get_git_repository(client: &LaunchpadClient, path: &str) -> Result<GitRepository> {
    let clean = path.trim_start_matches('/');
    client.get(&format!("/{clean}")).await
}

/// Look up a Git repository by its unique Launchpad name.
///
/// `unique_name` is in `~person/project/+git/name` format.
pub async fn get_git_repository_by_unique_name(
    client: &LaunchpadClient,
    unique_name: &str,
) -> Result<GitRepository> {
    let enc: String = url::form_urlencoded::byte_serialize(unique_name.as_bytes()).collect();
    let url = client.url(&format!("/+git?ws.op=getByPath&path={enc}"));
    client.get_url(&url).await
}

/// Return the default Git repository for a project, distribution, or
/// distribution source package.
///
/// `target` is a project or distribution name (e.g. `"launchpad"`, `"ubuntu"`).
pub async fn get_default_git_repository(
    client: &LaunchpadClient,
    target: &str,
) -> Result<GitRepository> {
    let target_url = client.url(&format!("/{target}"));
    let enc: String = url::form_urlencoded::byte_serialize(target_url.as_bytes()).collect();
    let url = client.url(&format!("/+git?ws.op=getDefaultRepository&target={enc}"));
    client.get_url(&url).await
}

/// List Git repositories owned by a Launchpad person or team.
pub async fn list_person_git_repositories(
    client: &LaunchpadClient,
    person_name: &str,
) -> Result<Vec<GitRepository>> {
    let enc: String = url::form_urlencoded::byte_serialize(person_name.as_bytes()).collect();
    let url = client.url(&format!("/~{enc}/+git"));
    Collection::fetch_all(client, &url).await
}

/// List references (branches and tags) in a Git repository.
///
/// `repo_path` is the repository slug, e.g. `"~person/project/+git/name"`.
pub async fn list_git_refs(client: &LaunchpadClient, repo_path: &str) -> Result<Vec<GitRef>> {
    let clean = repo_path.trim_start_matches('/');
    let url = client.url(&format!("/{clean}/refs"));
    Collection::fetch_all(client, &url).await
}

/// List merge proposals for a Git repository, optionally filtered by status.
///
/// `status` values include `"Work in progress"`, `"Needs review"`,
/// `"Approved"`, `"Rejected"`, `"Merged"`.
pub async fn list_merge_proposals(
    client: &LaunchpadClient,
    repo_path: &str,
    status: Option<&str>,
) -> Result<Vec<MergeProposal>> {
    let clean = repo_path.trim_start_matches('/');
    let mut url = client.url(&format!("/{clean}?ws.op=getMergeProposals"));
    if let Some(s) = status {
        let enc: String = url::form_urlencoded::byte_serialize(s.as_bytes()).collect();
        url.push_str(&format!("&status={enc}"));
    }
    Collection::fetch_all(client, &url).await
}

/// Fetch all code review comments for a merge proposal.
///
/// `repo_path` is the repository slug (e.g. `"~person/project/+git/name"`).
/// `mp_id` is the numeric merge proposal ID (last segment of the MP's self_link).
pub async fn get_merge_proposal_comments(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
) -> Result<Vec<CodeReviewComment>> {
    let clean = repo_path.trim_start_matches('/');
    let url = client.url(&format!("/{clean}/+merge/{mp_id}/all_comments"));
    Collection::fetch_all(client, &url).await
}

/// Fetch a single merge proposal by its repository path and ID.
pub async fn get_merge_proposal(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
) -> Result<MergeProposal> {
    let clean = repo_path.trim_start_matches('/');
    let url = client.url(&format!("/{clean}/+merge/{mp_id}"));
    client.get_url(&url).await
}

/// List all preview diffs for a merge proposal, sorted by creation date.
pub async fn get_preview_diffs(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
) -> Result<Vec<PreviewDiff>> {
    let mp = get_merge_proposal(client, repo_path, mp_id).await?;
    let collection_link = match mp.preview_diffs_collection_link {
        Some(link) => link,
        None => return Ok(Vec::new()),
    };
    let coll: Vec<PreviewDiff> = Collection::fetch_all(client, &collection_link).await?;
    Ok(coll)
}

/// Fetch inline (diff-level) comments for a merge proposal.
///
/// If `diff_id` is `Some`, uses that specific preview diff. Otherwise uses the
/// most recent (last) preview diff from the collection.
/// Returns `(comments, diff_line_map)` — the map is empty if the preview diff
/// text is unavailable.
pub async fn get_inline_comments(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
    diff_id: Option<u64>,
) -> Result<(Vec<InlineComment>, Vec<(u64, DiffLineContext)>)> {
    let mp = get_merge_proposal(client, repo_path, mp_id).await?;

    // Determine which diff to use.
    let diff_link = if let Some(requested_id) = diff_id {
        // Build the URL for the specific diff.
        let clean = repo_path.trim_start_matches('/');
        format!(
            "{}/{clean}/+merge/{mp_id}/+preview-diff/{requested_id}",
            crate::client::API_BASE
        )
    } else {
        // Default: use the latest diff (preview_diff_link points to the current one).
        match mp.preview_diff_link {
            Some(link) => link,
            None => return Ok((Vec::new(), Vec::new())),
        }
    };
    // The preview_diff_link looks like
    // "https://api.launchpad.net/devel/~user/project/+git/repo/+merge/123/+preview-diff/456"
    // We need the numeric ID (last segment).
    let clean = repo_path.trim_start_matches('/');
    let diff_id = diff_link.rsplit('/').next().unwrap_or_default();
    let url = client.url(&format!(
        "/{clean}/+merge/{mp_id}?ws.op=getInlineComments&previewdiff_id={diff_id}"
    ));
    let comments: Vec<InlineComment> = client.get_url(&url).await?;

    // Fetch the preview diff text to build the line map.
    let diff_map = match fetch_preview_diff(client, &diff_link).await {
        Ok(pd) => match pd.diff_text_link {
            Some(ref text_url) => match client.get_text_url(text_url).await {
                Ok(text) => parse_diff_line_map(&text),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        },
        Err(_) => Vec::new(),
    };

    Ok((comments, diff_map))
}

/// Fetch a preview diff resource by its full API URL.
pub async fn fetch_preview_diff(client: &LaunchpadClient, diff_url: &str) -> Result<PreviewDiff> {
    client.get_url(diff_url).await
}

/// Extract the numeric merge proposal ID from a self_link URL.
///
/// For example, given
/// `"https://api.launchpad.net/devel/~user/project/+git/repo/+merge/12345"`
/// this returns `Some(12345)`.
pub fn extract_mp_id(self_link: &str) -> Option<u64> {
    self_link
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_repository_deserialise_minimal() {
        let json = r#"{
            "name": "lpcli",
            "unique_name": "~jdoe/lpcli/+git/lpcli",
            "description": "lpcli git repo",
            "owner_default": true,
            "target_default": true,
            "repository_type": "Hosted",
            "information_type": "Public",
            "private": false,
            "owner_link": null,
            "target_link": null,
            "self_link": null,
            "web_link": null,
            "date_created": null,
            "date_last_modified": null,
            "loose_object_count": 0,
            "pack_count": 1
        }"#;
        let repo: GitRepository = serde_json::from_str(json).unwrap();
        assert_eq!(repo.name.as_deref(), Some("lpcli"));
        assert_eq!(repo.private, Some(false));
    }

    #[test]
    fn git_ref_deserialise() {
        let json = r#"{
            "path": "refs/heads/main",
            "display_name": "main",
            "commit_sha1": "abc123",
            "self_link": null,
            "web_link": null,
            "repository_link": null
        }"#;
        let r: GitRef = serde_json::from_str(json).unwrap();
        assert_eq!(r.path.as_deref(), Some("refs/heads/main"));
        assert_eq!(r.display_name.as_deref(), Some("main"));
    }

    #[test]
    fn code_review_comment_deserialise() {
        let json = r#"{
            "id": 42,
            "title": "Review comment",
            "content": "Looks good to me!",
            "vote": "Approve",
            "vote_tag": null,
            "author_link": "https://api.launchpad.net/devel/~reviewer",
            "date_created": "2024-06-15T10:30:00Z",
            "self_link": "https://api.launchpad.net/devel/~user/project/+git/repo/+merge/123/comments/42",
            "web_link": "https://code.launchpad.net/~user/project/+git/repo/+merge/123/comments/42"
        }"#;
        let c: CodeReviewComment = serde_json::from_str(json).unwrap();
        assert_eq!(c.id, Some(42));
        assert_eq!(c.content.as_deref(), Some("Looks good to me!"));
        assert_eq!(c.vote.as_deref(), Some("Approve"));
        assert_eq!(
            c.author_link.as_deref(),
            Some("https://api.launchpad.net/devel/~reviewer")
        );
    }

    #[test]
    fn extract_mp_id_from_self_link() {
        let url = "https://api.launchpad.net/devel/~user/project/+git/repo/+merge/12345";
        assert_eq!(super::extract_mp_id(url), Some(12345));
    }

    #[test]
    fn extract_mp_id_invalid() {
        assert_eq!(super::extract_mp_id("not-a-url"), None);
        assert_eq!(super::extract_mp_id(""), None);
    }

    #[test]
    fn inline_comment_deserialise_object_person() {
        let json = r#"{
            "line_number": "42",
            "person": {"name": "reviewer", "self_link": "https://api.launchpad.net/devel/~reviewer"},
            "text": "Nit: extra whitespace here",
            "date": "2024-06-15 10:30:00 UTC"
        }"#;
        let ic: InlineComment = serde_json::from_str(json).unwrap();
        assert_eq!(ic.line_number.as_deref(), Some("42"));
        assert_eq!(ic.text.as_deref(), Some("Nit: extra whitespace here"));
        assert_eq!(ic.author_display(), "reviewer");
    }

    #[test]
    fn inline_comment_deserialise_string_person() {
        let json = r#"{
            "line_number": "10",
            "person": "https://api.launchpad.net/devel/~reviewer",
            "text": "Fix this",
            "date": "2024-06-15 10:30:00 UTC"
        }"#;
        let ic: InlineComment = serde_json::from_str(json).unwrap();
        assert_eq!(ic.author_display(), "~reviewer");
    }

    #[test]
    fn parse_diff_line_map_basic() {
        let diff = "\
diff --git a/lib/model.py b/lib/model.py
--- a/lib/model.py
+++ b/lib/model.py
@@ -10,6 +10,7 @@ class Foo:
     def existing(self):
         pass
 
+    def new_method(self):
+        return True
 
     def other(self):
";
        let map = super::parse_diff_line_map(diff);
        // Line 4 of the diff is the @@ hunk header → not in map.
        // Line 5 = context "    def existing(self):" → new_line=10
        let l5 = map.iter().find(|(n, _)| *n == 5);
        assert!(l5.is_some());
        let ctx5 = &l5.unwrap().1;
        assert_eq!(ctx5.file, "lib/model.py");
        assert_eq!(ctx5.new_line, Some(10));

        // Line 8 = "+    def new_method(self):" → new_line=13
        let l8 = map.iter().find(|(n, _)| *n == 8);
        assert!(l8.is_some());
        let ctx8 = &l8.unwrap().1;
        assert_eq!(ctx8.file, "lib/model.py");
        assert_eq!(ctx8.new_line, Some(13));
        assert_eq!(ctx8.content, "    def new_method(self):");
    }
}
