//! Launchpad Git repository operations.
//!
//! # Supported operations
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`get_git_repository`] | Fetch a Git repository by its Launchpad path |
//! | [`get_git_repository_by_unique_name`] | Fetch a repo by its unique name |
//! | [`get_default_git_repository`] | Fetch the default repo for a target |
//! | [`list_git_repositories`] | List repos for a target (incl. forks) |
//! | [`list_person_git_repositories`] | List repos owned by a person |
//! | [`list_git_refs`] | List branches and tags in a repository |
//! | [`list_merge_proposals`] | List merge proposals for a repository |
//! | [`get_merge_proposal`] | Fetch a single merge proposal by ID |
//! | [`get_merge_proposal_comments`] | Fetch review comments for a merge proposal |
//! | [`get_inline_comments`] | Fetch inline (diff) comments for a merge proposal |
//! | [`get_preview_diffs`] | List all preview diffs for a merge proposal |
//!
//! Every read also has a `*_raw` counterpart that keeps the exact JSON
//! Launchpad returned (see [`Raw`]). Collection readers return Launchpad's
//! collection shape with unmodified entries.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient, Raw};
use crate::error::{LpError, Result};

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
    pub date_created: Option<Timestamp>,
    /// Date last modified.
    pub date_last_modified: Option<Timestamp>,
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
    pub date_created: Option<Timestamp>,
    /// Date last updated.
    pub date_last_modified: Option<Timestamp>,
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
    pub date_created: Option<Timestamp>,
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
    pub date_created: Option<Timestamp>,
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

/// Like [`get_git_repository()`], but also keep the exact Launchpad JSON.
pub async fn get_git_repository_raw(
    client: &LaunchpadClient,
    path: &str,
) -> Result<Raw<GitRepository>> {
    let clean = path.trim_start_matches('/');
    client.get_raw(&format!("/{clean}")).await
}

/// Look up a Git repository by its unique Launchpad name.
///
/// `unique_name` is in `~person/project/+git/name` format.
pub async fn get_git_repository_by_unique_name(
    client: &LaunchpadClient,
    unique_name: &str,
) -> Result<GitRepository> {
    client
        .get_url(&repository_by_unique_name_url(client, unique_name))
        .await
}

/// Like [`get_git_repository_by_unique_name()`], but also keep the exact
/// Launchpad JSON.
pub async fn get_git_repository_by_unique_name_raw(
    client: &LaunchpadClient,
    unique_name: &str,
) -> Result<Raw<GitRepository>> {
    client
        .get_url_raw(&repository_by_unique_name_url(client, unique_name))
        .await
}

fn repository_by_unique_name_url(client: &LaunchpadClient, unique_name: &str) -> String {
    let enc: String = url::form_urlencoded::byte_serialize(unique_name.as_bytes()).collect();
    client.url(&format!("/+git?ws.op=getByPath&path={enc}"))
}

/// Return the default Git repository for a project or distribution source
/// package.
///
/// `target` is a project name (e.g. `"launchpad"`) or a distribution source
/// package slug (e.g. `"ubuntu/+source/unbound"`), with leading and trailing
/// slashes trimmed. Bare distribution names, distribution series, and
/// `~person`/`~team` targets do not have a target-level default repository on
/// Launchpad and are not accepted.
///
/// # Errors
///
/// Returns [`LpError::Usage`] for empty targets, `~person`/`~team` targets,
/// repository paths containing `/+git/`, or unsupported target types rejected
/// by Launchpad (`target: (None, None)`). Returns [`LpError::LocalNotFound`]
/// when the target does not exist on Launchpad or has no default Git
/// repository configured (`null`).
pub async fn get_default_git_repository(
    client: &LaunchpadClient,
    target: &str,
) -> Result<GitRepository> {
    get_default_git_repository_raw(client, target)
        .await
        .map(|raw| raw.value)
}

/// Like [`get_default_git_repository()`], but also keep the exact Launchpad
/// JSON.
///
/// See [`get_default_git_repository()`] for accepted `target` formats and error
/// semantics.
pub async fn get_default_git_repository_raw(
    client: &LaunchpadClient,
    target: &str,
) -> Result<Raw<GitRepository>> {
    let (clean, url) = default_repository_url(client, target)?;
    match client.get_url::<serde_json::Value>(&url).await {
        Ok(serde_json::Value::Null) => Err(no_default_repository_error(&clean)),
        Ok(value) => Raw::from_value(value),
        Err(err) => Err(map_default_repository_error(&clean, err)),
    }
}

fn default_repository_url(client: &LaunchpadClient, target: &str) -> Result<(String, String)> {
    let clean = target.trim().trim_matches('/');
    if clean.is_empty() {
        return Err(LpError::Usage(
            "Git repository target must not be empty; pass a project name \
             (e.g. \"launchpad\") or a distribution source package slug \
             (e.g. \"ubuntu/+source/unbound\")."
                .to_string(),
        ));
    }
    if clean.contains("/+git/") {
        return Err(LpError::Usage(format!(
            "Git target '{clean}' is a repository path, not a target; pass a project \
             name (e.g. \"launchpad\") or a distribution source package slug \
             (e.g. \"ubuntu/+source/unbound\")."
        )));
    }
    if clean.starts_with('~') {
        return Err(LpError::Usage(format!(
            "Person or team target '{clean}' does not have a target-level default \
             Git repository; pass a project name (e.g. \"launchpad\") or a \
             distribution source package slug (e.g. \"ubuntu/+source/unbound\")."
        )));
    }
    let target_url = client.url(&format!("/{clean}"));
    let enc: String = url::form_urlencoded::byte_serialize(target_url.as_bytes()).collect();
    Ok((
        clean.to_string(),
        client.url(&format!("/+git?ws.op=getDefaultRepository&target={enc}")),
    ))
}

fn no_default_repository_error(target: &str) -> LpError {
    if target.contains("/+source/") {
        LpError::LocalNotFound(format!(
            "No default Git repository is configured for '{target}'."
        ))
    } else {
        LpError::LocalNotFound(format!(
            "No default Git repository is configured for '{target}'. If '{target}' is a \
             distribution source package, use '<distro>/+source/{target}' (for example, \
             'ubuntu/+source/{target}')."
        ))
    }
}

fn map_default_repository_error(target: &str, err: LpError) -> LpError {
    match &err {
        LpError::Api {
            status: 400,
            message,
        } if message.contains("target: (None, None)") => LpError::Usage(format!(
            "Target '{target}' cannot have a default Git repository. Pass a project name \
             (e.g. \"launchpad\") or a distribution source package slug \
             (e.g. \"ubuntu/+source/unbound\"); bare distributions and distribution \
             series are not valid Git targets."
        )),
        LpError::Api {
            status: 400,
            message,
        } if message.contains("target: No such object") => {
            if target.contains("/+source/") {
                LpError::LocalNotFound(format!("Launchpad target '{target}' does not exist."))
            } else {
                LpError::LocalNotFound(format!(
                    "Launchpad target '{target}' does not exist. Pass a project name \
                     (e.g. \"launchpad\") or a distribution source package slug \
                     (e.g. \"ubuntu/+source/unbound\")."
                ))
            }
        }
        _ => err,
    }
}

/// List Git repositories for a target, including personal forks.
///
/// `target` is a project name (e.g. `"launchpad"`), a distribution source
/// package slug (e.g. `"ubuntu/+source/apt"`), or a person or team name
/// with a leading `~` (e.g. `"~desktop-snappers"`, which lists only that
/// owner's repositories). Bare distribution names and distribution series
/// are rejected by the API, as are non-existent targets.
pub async fn list_git_repositories(
    client: &LaunchpadClient,
    target: &str,
) -> Result<Vec<GitRepository>> {
    Collection::fetch_all(client, &repositories_target_url(client, target)?).await
}

/// Like [`list_git_repositories()`], but return Launchpad's collection
/// shape with every page's entries unmodified.
pub async fn list_git_repositories_raw(
    client: &LaunchpadClient,
    target: &str,
) -> Result<Raw<Vec<GitRepository>>> {
    Collection::fetch_all_raw(client, &repositories_target_url(client, target)?).await
}

/// List Git repositories owned by a Launchpad person or team.
///
/// `person_name` is the person or team name, with or without the leading
/// `~` (e.g. `"desktop-snappers"`). Only repositories owned by that
/// person or team are returned; results reflect the caller's visibility,
/// so authenticated requests can include private repositories.
pub async fn list_person_git_repositories(
    client: &LaunchpadClient,
    person_name: &str,
) -> Result<Vec<GitRepository>> {
    Collection::fetch_all(client, &person_repositories_url(client, person_name)?).await
}

/// Like [`list_person_git_repositories()`], but return Launchpad's
/// collection shape with every page's entries unmodified.
pub async fn list_person_git_repositories_raw(
    client: &LaunchpadClient,
    person_name: &str,
) -> Result<Raw<Vec<GitRepository>>> {
    Collection::fetch_all_raw(client, &person_repositories_url(client, person_name)?).await
}

/// URL for the root `/+git` collection's `getRepositories` operation with
/// `target` expressed as an API URL (the only owner-filtered form Launchpad
/// offers; `~person/+git` itself is a web-only page that 404s on the API).
///
/// Blank targets and bare `/` are rejected so an empty target can never
/// drift into an unfiltered listing.
fn repositories_target_url(client: &LaunchpadClient, target: &str) -> Result<String> {
    let clean = target.trim().trim_matches('/');
    if clean.is_empty() {
        return Err(LpError::Other(
            "Git repository target must not be empty; pass a project name, \
             a distribution source package slug (e.g. \"ubuntu/+source/apt\"), \
             or a person/team name with a leading '~'"
                .to_string(),
        ));
    }
    let target_url = client.url(&format!("/{clean}"));
    let enc: String = url::form_urlencoded::byte_serialize(target_url.as_bytes()).collect();
    Ok(client.url(&format!("/+git?ws.op=getRepositories&target={enc}")))
}

/// URL for [`list_person_git_repositories()`]: the caller's name
/// normalised to a `~owner` API URL and passed as a `getRepositories`
/// target. Blank names are rejected before any request is made.
fn person_repositories_url(client: &LaunchpadClient, person_name: &str) -> Result<String> {
    let owner = person_name.trim().trim_start_matches('~');
    if owner.is_empty() {
        return Err(LpError::Other(
            "person name must not be empty; pass a Launchpad person or team \
             name, with or without the leading '~'"
                .to_string(),
        ));
    }
    repositories_target_url(client, &format!("~{owner}"))
}

/// List references (branches and tags) in a Git repository.
///
/// `repo_path` is the repository slug, e.g. `"~person/project/+git/name"`.
pub async fn list_git_refs(client: &LaunchpadClient, repo_path: &str) -> Result<Vec<GitRef>> {
    Collection::fetch_all(client, &refs_url(client, repo_path)).await
}

/// Like [`list_git_refs()`], but return Launchpad's collection shape with
/// every page's entries unmodified.
pub async fn list_git_refs_raw(
    client: &LaunchpadClient,
    repo_path: &str,
) -> Result<Raw<Vec<GitRef>>> {
    Collection::fetch_all_raw(client, &refs_url(client, repo_path)).await
}

fn refs_url(client: &LaunchpadClient, repo_path: &str) -> String {
    let clean = repo_path.trim_start_matches('/');
    client.url(&format!("/{clean}/refs"))
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
    Collection::fetch_all(client, &merge_proposals_url(client, repo_path, status)).await
}

/// Like [`list_merge_proposals()`], but return Launchpad's collection shape
/// with every page's entries unmodified.
pub async fn list_merge_proposals_raw(
    client: &LaunchpadClient,
    repo_path: &str,
    status: Option<&str>,
) -> Result<Raw<Vec<MergeProposal>>> {
    Collection::fetch_all_raw(client, &merge_proposals_url(client, repo_path, status)).await
}

fn merge_proposals_url(client: &LaunchpadClient, repo_path: &str, status: Option<&str>) -> String {
    let clean = repo_path.trim_start_matches('/');
    let mut url = client.url(&format!("/{clean}?ws.op=getMergeProposals"));
    if let Some(s) = status {
        let enc: String = url::form_urlencoded::byte_serialize(s.as_bytes()).collect();
        url.push_str(&format!("&status={enc}"));
    }
    url
}

fn merge_proposal_url(client: &LaunchpadClient, repo_path: &str, mp_id: u64) -> String {
    let clean = repo_path.trim_start_matches('/');
    client.url(&format!("/{clean}/+merge/{mp_id}"))
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
    let url = format!(
        "{}/all_comments",
        merge_proposal_url(client, repo_path, mp_id)
    );
    Collection::fetch_all(client, &url).await
}

/// Like [`get_merge_proposal_comments()`], but return Launchpad's collection
/// shape with every page's entries unmodified.
pub async fn get_merge_proposal_comments_raw(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
) -> Result<Raw<Vec<CodeReviewComment>>> {
    let url = format!(
        "{}/all_comments",
        merge_proposal_url(client, repo_path, mp_id)
    );
    Collection::fetch_all_raw(client, &url).await
}

/// Fetch a single merge proposal by its repository path and ID.
pub async fn get_merge_proposal(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
) -> Result<MergeProposal> {
    client
        .get_url(&merge_proposal_url(client, repo_path, mp_id))
        .await
}

/// Like [`get_merge_proposal()`], but also keep the exact Launchpad JSON.
pub async fn get_merge_proposal_raw(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
) -> Result<Raw<MergeProposal>> {
    client
        .get_url_raw(&merge_proposal_url(client, repo_path, mp_id))
        .await
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

/// Like [`get_preview_diffs()`], but return Launchpad's collection shape with
/// every page's entries unmodified.
///
/// If the merge proposal has no `preview_diffs_collection_link`, this returns
/// an empty collection (`total_size` 0) without a further request.
pub async fn get_preview_diffs_raw(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
) -> Result<Raw<Vec<PreviewDiff>>> {
    let mp = get_merge_proposal(client, repo_path, mp_id).await?;
    match mp.preview_diffs_collection_link {
        Some(link) => Collection::fetch_all_raw(client, &link).await,
        None => Ok(Raw {
            value: Vec::new(),
            raw: serde_json::json!({"total_size": 0, "start": 0, "entries": []}),
        }),
    }
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
    let Some((diff_link, url)) = inline_comments_target(client, repo_path, mp_id, diff_id).await?
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    let comments: Vec<InlineComment> = client.get_url(&url).await?;
    let diff_map = diff_line_map(client, &diff_link).await;
    Ok((comments, diff_map))
}

/// Like [`get_inline_comments()`], but also keep the exact JSON array
/// Launchpad returned from `getInlineComments`.
///
/// `getInlineComments` returns a JSON array rather than a collection, so the
/// raw value is that array, unmodified. When the merge proposal has no
/// preview diff, the raw value is an empty array. The diff line map is
/// computed by lpcli and is not part of Launchpad's response.
pub async fn get_inline_comments_raw(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
    diff_id: Option<u64>,
) -> Result<(Raw<Vec<InlineComment>>, Vec<(u64, DiffLineContext)>)> {
    let Some((diff_link, url)) = inline_comments_target(client, repo_path, mp_id, diff_id).await?
    else {
        let empty = Raw {
            value: Vec::new(),
            raw: serde_json::Value::Array(Vec::new()),
        };
        return Ok((empty, Vec::new()));
    };
    let comments: Raw<Vec<InlineComment>> = client.get_url_raw(&url).await?;
    let diff_map = diff_line_map(client, &diff_link).await;
    Ok((comments, diff_map))
}

/// Resolve the preview diff link and the `getInlineComments` URL, or `None`
/// when no diff was requested and the merge proposal has no current diff.
async fn inline_comments_target(
    client: &LaunchpadClient,
    repo_path: &str,
    mp_id: u64,
    diff_id: Option<u64>,
) -> Result<Option<(String, String)>> {
    let mp = get_merge_proposal(client, repo_path, mp_id).await?;

    // Determine which diff to use.
    let diff_link = if let Some(requested_id) = diff_id {
        // Build the URL using the client's configured base URL so that
        // test/staging overrides via `with_base_url` are respected.
        let clean = repo_path.trim_start_matches('/');
        client.url(&format!(
            "/{clean}/+merge/{mp_id}/+preview-diff/{requested_id}"
        ))
    } else {
        // Default: use the latest diff (preview_diff_link points to the current one).
        match mp.preview_diff_link {
            Some(link) => link,
            None => return Ok(None),
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
    Ok(Some((diff_link, url)))
}

/// Fetch the preview diff text to build the line map, or return an empty map
/// if the diff or its text is unavailable.
async fn diff_line_map(client: &LaunchpadClient, diff_link: &str) -> Vec<(u64, DiffLineContext)> {
    match fetch_preview_diff(client, diff_link).await {
        Ok(pd) => match pd.diff_text_link {
            Some(ref text_url) => match client.get_text_url(text_url).await {
                Ok(text) => parse_diff_line_map(&text),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

/// Fetch a preview diff resource by its full API URL.
pub async fn fetch_preview_diff(client: &LaunchpadClient, diff_url: &str) -> Result<PreviewDiff> {
    client.get_url(diff_url).await
}

/// Like [`fetch_preview_diff()`], but also keep the exact Launchpad JSON.
pub async fn fetch_preview_diff_raw(
    client: &LaunchpadClient,
    diff_url: &str,
) -> Result<Raw<PreviewDiff>> {
    client.get_url_raw(diff_url).await
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

    /// The listing functions must query the root `/+git` collection's
    /// `getRepositories` operation with the target expressed as an API URL.
    /// The bug this guards against: `~person/+git` is a web-only page and
    /// 404s on the API, and a bare person name is not a valid target. The
    /// matchers compare the decoded parameter values, so an encoding or
    /// parameter-name regression fails the mock assertion.
    #[tokio::test]
    async fn list_git_repositories_queries_target_collection() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{
            "start": 0,
            "total_size": 1,
            "entries": [{
                "name": "launchpad",
                "unique_name": "~ikoruk/launchpad/+git/launchpad",
                "repository_type": "Hosted",
                "date_last_modified": "2024-06-15T10:30:00Z"
            }]
        }"#;
        let mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getRepositories".into()),
                mockito::Matcher::UrlEncoded(
                    "target".into(),
                    format!("{}/launchpad", server.url()),
                ),
            ]))
            .with_status(200)
            .with_body(body)
            .expect(1)
            .create_async()
            .await;

        let client = LaunchpadClient::new(None).with_base_url(server.url());
        let repos = list_git_repositories(&client, "launchpad")
            .await
            .expect("listing repositories for a target should succeed");

        mock.assert_async().await;
        assert_eq!(repos.len(), 1);
        assert_eq!(
            repos[0].unique_name.as_deref(),
            Some("~ikoruk/launchpad/+git/launchpad")
        );
        assert_eq!(repos[0].repository_type.as_deref(), Some("Hosted"));
        assert_eq!(
            repos[0]
                .date_last_modified
                .map(|t| t.strftime("%Y-%m-%d").to_string()),
            Some("2024-06-15".to_string())
        );
    }

    /// `list_person_git_repositories` must send a `~person` target URL
    /// (with the `~` preserved and any caller-supplied `~` normalised);
    /// a bare person name is not a valid API target.
    #[tokio::test]
    async fn list_person_git_repositories_uses_person_target() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{
            "start": 0,
            "total_size": 1,
            "entries": [{
                "name": "snaps",
                "unique_name": "~desktop-snappers/snappy/+git/snaps"
            }]
        }"#;
        let mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getRepositories".into()),
                mockito::Matcher::UrlEncoded(
                    "target".into(),
                    format!("{}/~desktop-snappers", server.url()),
                ),
            ]))
            .with_status(200)
            .with_body(body)
            .expect(1)
            .create_async()
            .await;

        let client = LaunchpadClient::new(None).with_base_url(server.url());
        let repos = list_person_git_repositories(&client, "desktop-snappers")
            .await
            .expect("listing repositories owned by a person should succeed");

        mock.assert_async().await;
        assert_eq!(repos.len(), 1);
        assert_eq!(
            repos[0].unique_name.as_deref(),
            Some("~desktop-snappers/snappy/+git/snaps")
        );
    }

    #[tokio::test]
    async fn list_repositories_reject_empty_targets() {
        let client = LaunchpadClient::new(None);
        for bad in ["", "   ", "/", "//"] {
            assert!(
                list_git_repositories(&client, bad).await.is_err(),
                "empty target {bad:?} must be rejected"
            );
        }
        for bad in ["", "   ", "~~", "~"] {
            assert!(
                list_person_git_repositories(&client, bad).await.is_err(),
                "empty person name {bad:?} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn get_default_git_repository_queries_project_and_source_package_targets() {
        let mut server = mockito::Server::new_async().await;
        let client = LaunchpadClient::new(None).with_base_url(server.url());

        let project_body = serde_json::json!({
            "name": "launchpad",
            "unique_name": "~launchpad/launchpad/+git/launchpad",
            "target_default": true,
            "extra_launchpad_field": "kept-verbatim"
        });
        let project_mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
                mockito::Matcher::UrlEncoded(
                    "target".into(),
                    format!("{}/launchpad", server.url()),
                ),
            ]))
            .with_status(200)
            .with_body(project_body.to_string())
            .expect(2)
            .create_async()
            .await;

        let typed = get_default_git_repository(&client, "launchpad")
            .await
            .unwrap();
        assert_eq!(
            typed.unique_name.as_deref(),
            Some("~launchpad/launchpad/+git/launchpad")
        );
        let raw = get_default_git_repository_raw(&client, " /launchpad/ ")
            .await
            .unwrap();
        assert_eq!(raw.raw, project_body);
        project_mock.assert_async().await;

        let pkg_body = serde_json::json!({
            "name": "unbound",
            "unique_name": "~git-ubuntu-import/ubuntu/+source/unbound/+git/unbound",
            "target_default": true,
            "extra_pkg_field": 42
        });
        let pkg_mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
                mockito::Matcher::UrlEncoded(
                    "target".into(),
                    format!("{}/ubuntu/+source/unbound", server.url()),
                ),
            ]))
            .with_status(200)
            .with_body(pkg_body.to_string())
            .expect(2)
            .create_async()
            .await;

        let pkg_typed = get_default_git_repository(&client, " ubuntu/+source/unbound ")
            .await
            .unwrap();
        assert_eq!(
            pkg_typed.unique_name.as_deref(),
            Some("~git-ubuntu-import/ubuntu/+source/unbound/+git/unbound")
        );
        let pkg_raw = get_default_git_repository_raw(&client, "/ubuntu/+source/unbound/")
            .await
            .unwrap();
        assert_eq!(pkg_raw.raw, pkg_body);
        pkg_mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_default_git_repository_null_response_returns_local_not_found() {
        let mut server = mockito::Server::new_async().await;
        let client = LaunchpadClient::new(None).with_base_url(server.url());

        let proj_mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
                mockito::Matcher::UrlEncoded("target".into(), format!("{}/unbound", server.url())),
            ]))
            .with_status(200)
            .with_body("null")
            .expect(2)
            .create_async()
            .await;

        for err in [
            get_default_git_repository(&client, "unbound")
                .await
                .unwrap_err(),
            get_default_git_repository_raw(&client, "unbound")
                .await
                .unwrap_err(),
        ] {
            assert!(matches!(err, LpError::LocalNotFound(_)), "{err:?}");
            assert_eq!(err.exit_code(), 4);
            assert_eq!(err.http_status(), None);
            let msg = err.to_string();
            assert!(msg.contains("ubuntu/+source/unbound"), "{msg}");
            assert!(!msg.contains("lpcli"), "{msg}");
        }
        proj_mock.assert_async().await;

        let pkg_mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
                mockito::Matcher::UrlEncoded(
                    "target".into(),
                    format!("{}/ubuntu/+source/unbound", server.url()),
                ),
            ]))
            .with_status(200)
            .with_body("null")
            .expect(2)
            .create_async()
            .await;

        for err in [
            get_default_git_repository(&client, "ubuntu/+source/unbound")
                .await
                .unwrap_err(),
            get_default_git_repository_raw(&client, "ubuntu/+source/unbound")
                .await
                .unwrap_err(),
        ] {
            assert!(matches!(err, LpError::LocalNotFound(_)), "{err:?}");
            assert_eq!(err.exit_code(), 4);
            assert_eq!(err.http_status(), None);
            let msg = err.to_string();
            assert!(msg.contains("'ubuntu/+source/unbound'"), "{msg}");
            assert!(!msg.contains("<distro>/+source/"), "{msg}");
        }
        pkg_mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_default_git_repository_maps_launchpad_400_target_errors() {
        let mut server = mockito::Server::new_async().await;
        let client = LaunchpadClient::new(None).with_base_url(server.url());

        let distro_mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
                mockito::Matcher::UrlEncoded("target".into(), format!("{}/ubuntu", server.url())),
            ]))
            .with_status(400)
            .with_body("target: (None, None)")
            .expect(2)
            .create_async()
            .await;

        for err in [
            get_default_git_repository(&client, "ubuntu")
                .await
                .unwrap_err(),
            get_default_git_repository_raw(&client, "ubuntu")
                .await
                .unwrap_err(),
        ] {
            assert!(matches!(err, LpError::Usage(_)), "{err:?}");
            assert_eq!(err.exit_code(), 2);
            let msg = err.to_string();
            assert!(msg.contains("bare distributions"), "{msg}");
            assert!(msg.contains("ubuntu/+source/unbound"), "{msg}");
            assert!(!msg.contains("lpcli"), "{msg}");
        }
        distro_mock.assert_async().await;

        let missing_proj_mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
                mockito::Matcher::UrlEncoded(
                    "target".into(),
                    format!("{}/tenacious", server.url()),
                ),
            ]))
            .with_status(400)
            .with_body(format!(
                "target: No such object \"{}/tenacious\".",
                server.url()
            ))
            .expect(1)
            .create_async()
            .await;

        let err = get_default_git_repository(&client, "tenacious")
            .await
            .unwrap_err();
        assert!(matches!(err, LpError::LocalNotFound(_)), "{err:?}");
        assert_eq!(err.exit_code(), 4);
        assert_eq!(err.http_status(), None);
        assert!(err.to_string().contains("ubuntu/+source/unbound"), "{err}");
        missing_proj_mock.assert_async().await;

        let missing_pkg_mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
                mockito::Matcher::UrlEncoded(
                    "target".into(),
                    format!("{}/ubuntu/+source/missing", server.url()),
                ),
            ]))
            .with_status(400)
            .with_body(format!(
                "target: No such object \"{}/ubuntu/+source/missing\".",
                server.url()
            ))
            .expect(1)
            .create_async()
            .await;

        let err = get_default_git_repository_raw(&client, "ubuntu/+source/missing")
            .await
            .unwrap_err();
        assert!(matches!(err, LpError::LocalNotFound(_)), "{err:?}");
        assert_eq!(err.exit_code(), 4);
        assert_eq!(err.http_status(), None);
        let msg = err.to_string();
        assert!(msg.contains("ubuntu/+source/missing"), "{msg}");
        assert!(!msg.contains("ubuntu/+source/unbound"), "{msg}");
        missing_pkg_mock.assert_async().await;

        let other_400_mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
                mockito::Matcher::UrlEncoded("target".into(), format!("{}/other400", server.url())),
            ]))
            .with_status(400)
            .with_body("unexpected parameter error")
            .expect(1)
            .create_async()
            .await;

        let err = get_default_git_repository(&client, "other400")
            .await
            .unwrap_err();
        assert!(
            matches!(err, LpError::Api { status: 400, ref message } if message == "unexpected parameter error"),
            "{err:?}"
        );
        other_400_mock.assert_async().await;

        let auth_mock = server
            .mock("GET", "/+git")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getDefaultRepository".into()),
                mockito::Matcher::UrlEncoded("target".into(), format!("{}/privproj", server.url())),
            ]))
            .with_status(401)
            .with_body("Unauthorized")
            .expect(1)
            .create_async()
            .await;

        let err = get_default_git_repository(&client, "privproj")
            .await
            .unwrap_err();
        assert!(matches!(err, LpError::Api { status: 401, .. }), "{err:?}");
        assert_eq!(err.exit_code(), 3);
        auth_mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_default_git_repository_rejects_invalid_targets_locally() {
        let mut server = mockito::Server::new_async().await;
        let zero_mock = server
            .mock("GET", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let client = LaunchpadClient::new(None).with_base_url(server.url());

        for bad in [
            "",
            "   ",
            "/",
            "//",
            "~ubuntu-archive",
            "/~jdoe/",
            "~owner/project/+git/repo",
            "launchpad/+git/launchpad",
        ] {
            for err in [
                get_default_git_repository(&client, bad).await.unwrap_err(),
                get_default_git_repository_raw(&client, bad)
                    .await
                    .unwrap_err(),
            ] {
                assert!(
                    matches!(err, LpError::Usage(_)),
                    "target {bad:?} should fail with Usage, got {err:?}"
                );
                assert_eq!(err.exit_code(), 2, "target {bad:?}: {err}");
                assert!(!err.to_string().contains("lpcli"), "{err}");
            }
        }
        zero_mock.assert_async().await;
    }
}
