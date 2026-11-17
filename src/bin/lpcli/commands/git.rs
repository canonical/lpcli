//! Git command handlers.
//!
//! Every read prints Launchpad's JSON unchanged in `json` mode. List commands
//! build a [`Report`] whose records also drive `tsv` output. `git comments`
//! combines two Launchpad responses into an lpcli-defined object.

use crate::*;

// ---------------------------------------------------------------------------
// Git handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_git(
    cmd: GitCommand,
    output: &Output,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    output.print(&git_report(&client, cmd).await?)
}

/// Read the data for a git command and build its report.
pub(crate) async fn git_report(
    client: &LaunchpadClient,
    cmd: GitCommand,
) -> lpcli::error::Result<Report> {
    Ok(match cmd {
        GitCommand::Show { path } => {
            git_repository_report(&git::get_git_repository_raw(client, &path).await?)
        }
        GitCommand::Default { target } => {
            git_repository_report(&git::get_default_git_repository_raw(client, &target).await?)
        }
        GitCommand::ListPersonRepos { name } => {
            git_repositories_report(&git::list_person_git_repositories_raw(client, &name).await?)
        }
        GitCommand::List { target } => {
            git_repositories_report(&git::list_git_repositories_raw(client, &target).await?)
        }
        GitCommand::Refs { path } => git_refs_report(&git::list_git_refs_raw(client, &path).await?),
        GitCommand::Proposals { path, status } => git_proposals_report(
            &git::list_merge_proposals_raw(client, &path, status.as_deref()).await?,
        ),
        GitCommand::Comments { path, id, diff } => {
            git_comments_report(&fetch_git_comments(client, &path, id, diff).await?)
        }
        GitCommand::Diffs { path, id } => {
            git_diffs_report(&git::get_preview_diffs_raw(client, &path, id).await?)
        }
    })
}

pub(crate) fn git_supports_json(_command: &GitCommand) -> bool {
    true
}

pub(crate) fn git_supports_tsv(command: &GitCommand) -> bool {
    matches!(
        command,
        GitCommand::ListPersonRepos { .. }
            | GitCommand::List { .. }
            | GitCommand::Refs { .. }
            | GitCommand::Proposals { .. }
            | GitCommand::Diffs { .. }
    )
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

fn day(timestamp: Option<jiff::Timestamp>) -> Option<String> {
    timestamp.map(|t| t.strftime("%Y-%m-%d").to_string())
}

fn minute(timestamp: Option<jiff::Timestamp>) -> Option<String> {
    timestamp.map(|t| t.strftime("%Y-%m-%d %H:%M").to_string())
}

fn records(columns: Vec<Column>, rows: Vec<Vec<Option<String>>>) -> Records {
    Records { columns, rows }
}

/// `git show` and `git default`: one `git_repository` entry.
pub(crate) fn git_repository_report(repo: &Raw<git::GitRepository>) -> Report {
    let value = &repo.value;
    let heading = value
        .unique_name
        .clone()
        .or_else(|| value.name.clone())
        .unwrap_or_else(|| "Git repository".to_string());
    let mut blocks = vec![Block::Heading(heading)];
    let fields = [
        ("Description", value.description.clone()),
        ("Type", value.repository_type.clone()),
        ("Access", value.information_type.clone()),
        ("Modified", day(value.date_last_modified)),
        ("URL", value.web_link.clone()),
    ];
    blocks.extend(
        fields
            .into_iter()
            .filter_map(|(label, value)| value.map(|value| Block::Field(label, value))),
    );
    Report {
        blocks,
        json: repo.raw.clone(),
        records: records(Vec::new(), Vec::new()),
    }
}

/// `git list-person-repos`: Launchpad's collection of `git_repository` entries.
pub(crate) fn git_repositories_report(repos: &Raw<Vec<git::GitRepository>>) -> Report {
    let rows = repos
        .value
        .iter()
        .map(|repo| {
            vec![
                repo.unique_name.clone().or_else(|| repo.name.clone()),
                repo.repository_type.clone(),
                day(repo.date_last_modified),
            ]
        })
        .collect();
    Report {
        blocks: vec![Block::List {
            noun: Noun {
                singular: "Git repository",
                plural: "Git repositories",
            },
            empty: "No Git repositories found.".into(),
            hint: None,
        }],
        json: repos.raw.clone(),
        records: records(
            vec![
                Column {
                    label: "Repository",
                    key: "unique_name",
                },
                Column {
                    label: "Type",
                    key: "repository_type",
                },
                Column {
                    label: "Modified",
                    key: "date_last_modified",
                },
            ],
            rows,
        ),
    }
}

/// `git refs`: Launchpad's collection of `git_ref` entries.
pub(crate) fn git_refs_report(refs: &Raw<Vec<git::GitRef>>) -> Report {
    let rows = refs
        .value
        .iter()
        .map(|r| {
            vec![
                r.path.clone(),
                r.display_name.clone(),
                r.commit_sha1.clone(),
            ]
        })
        .collect();
    Report {
        blocks: vec![Block::List {
            noun: Noun {
                singular: "ref",
                plural: "refs",
            },
            empty: "No refs found.".into(),
            hint: None,
        }],
        json: refs.raw.clone(),
        records: records(
            vec![
                Column {
                    label: "Ref",
                    key: "path",
                },
                Column {
                    label: "Display Name",
                    key: "display_name",
                },
                Column {
                    label: "SHA1",
                    key: "commit_sha1",
                },
            ],
            rows,
        ),
    }
}

/// `git proposals`: Launchpad's collection of `branch_merge_proposal` entries.
pub(crate) fn git_proposals_report(proposals: &Raw<Vec<git::MergeProposal>>) -> Report {
    let rows = proposals
        .value
        .iter()
        .map(|p| {
            vec![
                p.self_link
                    .as_deref()
                    .and_then(git::extract_mp_id)
                    .map(|id| id.to_string()),
                p.queue_status.clone(),
                p.source_git_path.clone(),
                p.target_git_path.clone(),
                day(p.date_created),
            ]
        })
        .collect();
    Report {
        blocks: vec![Block::List {
            noun: Noun {
                singular: "merge proposal",
                plural: "merge proposals",
            },
            empty: "No merge proposals found.".into(),
            hint: Some("Use the ID value with --id for git comments or git diffs.".into()),
        }],
        json: proposals.raw.clone(),
        records: records(
            vec![
                Column {
                    label: "ID",
                    key: "id",
                },
                Column {
                    label: "Status",
                    key: "queue_status",
                },
                Column {
                    label: "Source Branch",
                    key: "source_git_path",
                },
                Column {
                    label: "Target Branch",
                    key: "target_git_path",
                },
                Column {
                    label: "Date",
                    key: "date_created",
                },
            ],
            rows,
        ),
    }
}

/// `git diffs`: Launchpad's collection of `preview_diff` entries.
pub(crate) fn git_diffs_report(diffs: &Raw<Vec<git::PreviewDiff>>) -> Report {
    let rows = diffs
        .value
        .iter()
        .map(|d| {
            vec![
                d.id.map(|id| id.to_string()),
                minute(d.date_created),
                d.diff_lines_count.map(|n| n.to_string()),
                d.added_lines_count.map(|n| n.to_string()),
                d.removed_lines_count.map(|n| n.to_string()),
                d.source_revision_id.clone(),
            ]
        })
        .collect();
    Report {
        blocks: vec![Block::List {
            noun: Noun {
                singular: "preview diff",
                plural: "preview diffs",
            },
            empty: "No preview diffs found.".into(),
            hint: Some("Use --diff <ID> with git comments to select a specific diff.".into()),
        }],
        json: diffs.raw.clone(),
        records: records(
            vec![
                Column {
                    label: "ID",
                    key: "id",
                },
                Column {
                    label: "Created",
                    key: "date_created",
                },
                Column {
                    label: "Lines",
                    key: "diff_lines_count",
                },
                Column {
                    label: "Added",
                    key: "added_lines_count",
                },
                Column {
                    label: "Removed",
                    key: "removed_lines_count",
                },
                Column {
                    label: "Source Revision",
                    key: "source_revision_id",
                },
            ],
            rows,
        ),
    }
}

// ---------------------------------------------------------------------------
// git comments
// ---------------------------------------------------------------------------

/// Review and inline comments for one merge proposal.
pub(crate) struct GitComments {
    /// Launchpad's `all_comments` collection, unmodified.
    pub(crate) comments: Raw<Vec<git::CodeReviewComment>>,
    /// The JSON array returned by `getInlineComments`, unmodified.
    pub(crate) inline: Raw<Vec<git::InlineComment>>,
    /// Diff-line context computed by lpcli from the preview diff text.
    pub(crate) diff_map: Vec<(u64, git::DiffLineContext)>,
}

pub(crate) async fn fetch_git_comments(
    client: &LaunchpadClient,
    path: &str,
    id: u64,
    diff: Option<u64>,
) -> lpcli::error::Result<GitComments> {
    let (comments, (inline, diff_map)) = tokio::try_join!(
        git::get_merge_proposal_comments_raw(client, path, id),
        git::get_inline_comments_raw(client, path, id, diff),
    )?;
    Ok(GitComments {
        comments,
        inline,
        diff_map,
    })
}

/// The file/line context of an inline comment, when the diff map has it.
fn inline_context<'a>(
    comment: &git::InlineComment,
    diff_map: &'a [(u64, git::DiffLineContext)],
) -> Option<&'a git::DiffLineContext> {
    let line: u64 = comment.line_number.as_deref()?.parse().ok()?;
    diff_map
        .iter()
        .find(|(number, _)| *number == line)
        .map(|(_, context)| context)
}

/// The lpcli-defined `git comments` object.
///
/// `all_comments` and `inline_comments` are Launchpad's responses, unchanged.
/// `lpcli.inline_comment_locations` has one element per inline comment, in
/// the same order: the file, new-file line, and diff line content that lpcli
/// worked out from the preview diff, or `null` when it is unknown.
pub(crate) fn git_comments_json(comments: &GitComments) -> serde_json::Value {
    let locations: Vec<serde_json::Value> = comments
        .inline
        .value
        .iter()
        .map(|comment| {
            inline_context(comment, &comments.diff_map).map_or(serde_json::Value::Null, |ctx| {
                serde_json::json!({
                    "file": ctx.file,
                    "new_line": ctx.new_line,
                    "content": ctx.content,
                })
            })
        })
        .collect();
    serde_json::json!({
        "all_comments": comments.comments.raw,
        "inline_comments": comments.inline.raw,
        "lpcli": {"inline_comment_locations": locations},
    })
}

/// The `git comments` report: review comments, then inline comments, each
/// as labeled fields. Its JSON is [`git_comments_json()`].
pub(crate) fn git_comments_report(comments: &GitComments) -> Report {
    let GitComments {
        comments: review,
        inline,
        diff_map,
    } = comments;
    let mut blocks = vec![Block::Heading("Review comments".into())];
    if review.value.is_empty() {
        blocks.push(Block::Note(vec![Span::Text("No review comments.".into())]));
    }
    for (i, c) in review.value.iter().enumerate() {
        blocks.push(Block::Break);
        blocks.push(Block::Field(
            "Comment",
            format!("#{}", c.id.unwrap_or(i as u64)),
        ));
        let author = c
            .author_link
            .as_deref()
            .and_then(|u| u.rsplit('/').next())
            .unwrap_or("unknown");
        blocks.push(Block::Field("Author", author.to_string()));
        if let Some(date) = minute(c.date_created) {
            blocks.push(Block::Field("Date", date));
        }
        if let Some(vote) = &c.vote {
            blocks.push(Block::Field("Vote", vote.clone()));
        }
        blocks.push(Block::Field("Text", c.content.clone().unwrap_or_default()));
    }

    if !inline.value.is_empty() {
        blocks.push(Block::Break);
        blocks.push(Block::Heading("Inline comments".into()));
    }
    for ic in &inline.value {
        let context = inline_context(ic, diff_map);
        let location = match context {
            Some(ctx) => match ctx.new_line {
                Some(line) => format!("{}:{line}", ctx.file),
                None => ctx.file.clone(),
            },
            None => ic
                .line_number
                .as_deref()
                .map_or_else(|| "unknown".to_string(), |n| format!("diff line {n}")),
        };
        blocks.push(Block::Break);
        blocks.push(Block::Field("Location", location));
        blocks.push(Block::Field("Author", ic.author_display().to_string()));
        if let Some(date) = &ic.date {
            blocks.push(Block::Field("Date", date.clone()));
        }
        if let Some(ctx) = context {
            blocks.push(Block::Field("Diff line", ctx.content.clone()));
        }
        blocks.push(Block::Field("Text", ic.text.clone().unwrap_or_default()));
    }

    Report {
        blocks,
        json: git_comments_json(comments),
        records: records(Vec::new(), Vec::new()),
    }
}
