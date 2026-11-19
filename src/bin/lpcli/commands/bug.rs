//! Bug command handlers and their private helpers.
//!
//! Every bug command can print lpcli's JSON contract (`docs/json-output.md`):
//!
//! * `tasks` and `subscriptions` print Launchpad's collection unchanged.
//! * `show`, `comments`, `attachments`, and `watches` combine several
//!   requests into an lpcli-defined object whose members are unmodified
//!   Launchpad entries or collections, named after Launchpad's link names
//!   (`bug`, `bug_tasks`, `messages`, `attachments`, `bug_watches`). Values
//!   lpcli works out itself, such as resolved attachment download URLs, go
//!   under `lpcli`.
//! * Mutations print the entry Launchpad returned, or the entry read back
//!   from the reported location. Operations that change several tasks print
//!   `{"bug_tasks": {...}, "lpcli": {"failures": [...]}}`, and operations
//!   with no resulting entry print an lpcli-defined object under `lpcli`.
//!   With `--quiet`, mutations print only the `self_link` of each resulting
//!   entry, one per line.
//!
//! When an operation that changes several tasks fails for some of them, the
//! successful results and the failures are printed first and the command
//! then fails with [`lpcli::error::LpError::Partial`] (exit code 7).

use std::io::Write as _;

use crate::*;

use lpcli::error::{ErrorKind, LpError};

// ---------------------------------------------------------------------------
// Command outcomes
// ---------------------------------------------------------------------------

/// What a bug command writes to stdout in the selected mode.
#[derive(Debug)]
pub(crate) enum BugBody {
    /// A report rendered by the output layer in every mode.
    Report(Report),
    /// Nothing on stdout, because nothing was changed.
    Empty,
}

/// The complete result of a bug command.
#[derive(Debug)]
pub(crate) struct BugOutcome {
    pub(crate) body: BugBody,
    /// `rich`/`plain` lines for stderr, such as per-task failures. Structured
    /// modes report failures in the JSON instead.
    pub(crate) warnings: Vec<String>,
    /// For mutations: the `self_link`s printed by `--quiet`.
    pub(crate) quiet: Option<Vec<String>>,
    /// An error returned after the output has been printed.
    pub(crate) error: Option<LpError>,
}

impl BugOutcome {
    fn read_blocks(json: serde_json::Value, blocks: Vec<Block>) -> Self {
        Self::report(Report {
            blocks,
            json,
            records: Records {
                columns: Vec::new(),
                rows: Vec::new(),
            },
        })
    }

    fn report(report: Report) -> Self {
        Self {
            body: BugBody::Report(report),
            warnings: Vec::new(),
            quiet: None,
            error: None,
        }
    }

    fn list_blocks(json: serde_json::Value, records: Records, blocks: Vec<Block>) -> Self {
        Self::report(Report {
            blocks,
            json,
            records,
        })
    }

    /// A mutation whose result is `json`; `--quiet` prints `links`.
    pub(crate) fn mutation(
        json: serde_json::Value,
        blocks: Vec<Block>,
        links: Vec<String>,
    ) -> Self {
        Self {
            quiet: Some(links),
            ..Self::read_blocks(json, blocks)
        }
    }

    /// A mutation whose result is one Launchpad entry.
    fn entry<T>(entry: &Raw<T>, blocks: Vec<Block>) -> Self {
        Self::mutation(
            entry.raw.clone(),
            blocks,
            raw_self_link(&entry.raw).into_iter().collect(),
        )
    }

    /// The text written to stdout.
    pub(crate) fn render(&self, output: &Output, quiet: bool) -> String {
        if quiet && let Some(links) = &self.quiet {
            return links
                .iter()
                .map(|link| format!("{}\n", bug_terminal_safe(link)))
                .collect();
        }
        match &self.body {
            BugBody::Report(report) => output.render(report),
            BugBody::Empty => String::new(),
        }
    }

    /// The text written to stderr before any error.
    pub(crate) fn render_warnings(&self, output: &Output) -> String {
        if output.mode().is_structured() {
            return String::new();
        }
        self.warnings
            .iter()
            .map(|warning| output.render_warning(warning))
            .collect()
    }

    /// Print the outcome, then return its deferred error, if any.
    fn emit(self, output: &Output, quiet: bool) -> lpcli::error::Result<()> {
        if quiet {
            output.write_raw(&self.render(output, true))?;
        } else if let BugBody::Report(report) = &self.body {
            output.print(report)?;
        }
        let warnings = self.render_warnings(output);
        if !warnings.is_empty() {
            std::io::stderr().lock().write_all(warnings.as_bytes())?;
        }
        self.error.map_or(Ok(()), Err)
    }
}

/// Replace control characters so a Launchpad value printed on its own line
/// cannot inject terminal escape sequences or split the line.
fn bug_terminal_safe(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

// ---------------------------------------------------------------------------
// Raw JSON helpers
// ---------------------------------------------------------------------------

/// The `entries` of a Launchpad collection, or nothing.
fn raw_entries(collection: &serde_json::Value) -> &[serde_json::Value] {
    collection
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// A Launchpad attribute as text, unchanged: strings as they are, other
/// non-null values in their JSON form, and `None` for null or missing.
fn raw_text(entry: &serde_json::Value, key: &str) -> Option<String> {
    match entry.get(key)? {
        serde_json::Value::Null => None,
        serde_json::Value::String(text) => Some(text.clone()),
        value => Some(value.to_string()),
    }
}

fn raw_self_link(entry: &serde_json::Value) -> Option<String> {
    entry
        .get("self_link")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// A complete collection of entries lpcli assembled itself.
fn collection_json(entries: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "total_size": entries.len(),
        "start": 0,
        "entries": entries,
    })
}

fn column(label: &'static str, key: &'static str) -> Column {
    Column::new(label, key)
}

/// TSV records taken straight from Launchpad entries, so values are never
/// reformatted.
fn raw_records(entries: &[serde_json::Value], columns: Vec<Column>) -> Records {
    let rows = entries
        .iter()
        .map(|entry| {
            columns
                .iter()
                .map(|column| raw_text(entry, column.key))
                .collect()
        })
        .collect();
    Records { columns, rows }
}

/// The structured form of an error inside an lpcli object. Credential-like
/// values are redacted, as in the top-level error object.
fn bug_error_json(error: &LpError) -> serde_json::Value {
    serde_json::json!({
        "kind": error.kind().name(),
        "message": crate::output::redact_secrets(&error.to_string()),
        "http_status": error.http_status(),
    })
}

// ---------------------------------------------------------------------------
// Failures of operations on several tasks
// ---------------------------------------------------------------------------

/// One failed step of an operation that changes several things.
#[derive(Debug)]
pub(crate) struct BugFailure {
    /// The failed step, e.g. `set_status` or `add_task`.
    pub(crate) operation: &'static str,
    /// The task target or series the step applied to.
    pub(crate) target: Option<String>,
    /// The API link of the task the step applied to, when known.
    pub(crate) task_link: Option<String>,
    pub(crate) error: LpError,
    /// A human-readable description for messages.
    pub(crate) description: String,
}

impl BugFailure {
    pub(crate) fn new(
        operation: &'static str,
        target: Option<String>,
        task_link: Option<String>,
        error: LpError,
    ) -> Self {
        let description = match &target {
            Some(target) => format!("{operation} for '{target}': {error}"),
            None => format!("{operation}: {error}"),
        };
        Self {
            operation,
            target,
            task_link,
            error,
            description,
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "operation": self.operation,
            "target": self.target,
            "task_link": self.task_link,
            "error": bug_error_json(&self.error),
        })
    }

    fn warning(&self) -> String {
        format!("Failed: {}", self.description)
    }

    /// Whether Launchpad may already reflect this step.
    fn may_have_applied(&self) -> bool {
        self.error.kind() == ErrorKind::Partial
    }
}

fn failures_json(failures: &[BugFailure]) -> serde_json::Value {
    failures.iter().map(BugFailure::json).collect()
}

/// Finish an operation that changes several things.
///
/// * Without failures, `outcome` is returned unchanged.
/// * When something was (or may have been) applied, the output is kept,
///   failures are added as stderr warnings for `rich`/`plain`, and the
///   command fails with [`LpError::Partial`] using `message`.
/// * When nothing was applied, the first failure is returned unchanged,
///   keeping its exit code. A single failure prints nothing else; several
///   failures are still listed in the JSON and as `rich`/`plain` warnings.
pub(crate) fn finish_batch(
    mut outcome: BugOutcome,
    applied: usize,
    failures: Vec<BugFailure>,
    message: impl FnOnce(&[BugFailure]) -> String,
) -> BugOutcome {
    if failures.is_empty() {
        return outcome;
    }
    outcome
        .warnings
        .extend(failures.iter().map(BugFailure::warning));
    if applied > 0 || failures.iter().any(BugFailure::may_have_applied) {
        outcome.error = Some(LpError::Partial(message(&failures)));
        return outcome;
    }
    outcome.quiet = Some(Vec::new());
    if failures.len() == 1 {
        // The returned error already reports the only failure.
        outcome.body = BugBody::Empty;
        outcome.warnings.clear();
    } else if let BugBody::Report(report) = &mut outcome.body {
        report.blocks.clear();
    }
    outcome.error = failures.into_iter().next().map(|failure| failure.error);
    outcome
}

fn describe_failures(failures: &[BugFailure]) -> String {
    failures
        .iter()
        .map(|failure| failure.description.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

// ---------------------------------------------------------------------------
// Task changes
// ---------------------------------------------------------------------------

/// A change to one bug task attribute.
pub(crate) enum TaskChange {
    Status(bugs::BugTaskStatus),
    Importance(bugs::BugImportance),
    /// The assignee's API URL.
    Assignee(String),
}

impl TaskChange {
    fn operation(&self) -> &'static str {
        match self {
            Self::Status(_) => "set_status",
            Self::Importance(_) => "set_importance",
            Self::Assignee(_) => "set_assignee",
        }
    }

    async fn apply(
        &self,
        client: &LaunchpadClient,
        task_url: &str,
    ) -> lpcli::error::Result<Raw<bugs::BugTask>> {
        match self {
            Self::Status(status) => bugs::set_bug_status_raw(client, task_url, status).await,
            Self::Importance(importance) => {
                bugs::set_bug_importance_raw(client, task_url, importance).await
            }
            Self::Assignee(url) => bugs::set_bug_assignee_raw(client, task_url, url).await,
        }
    }
}

fn task_label(task: &bugs::BugTask) -> String {
    task.bug_target_display_name
        .clone()
        .or_else(|| task.target_link.clone())
        .or_else(|| task.self_link.clone())
        .unwrap_or_else(|| "(unknown)".to_string())
}

/// Apply `change` to every matched task, continuing after failures.
async fn update_tasks(
    client: &LaunchpadClient,
    bug_id: u64,
    matched: &[&bugs::BugTask],
    change: &TaskChange,
    spans: Vec<Span>,
) -> BugOutcome {
    let mut entries = Vec::new();
    let mut failures = Vec::new();
    for task in matched {
        let label = task_label(task);
        let Some(task_url) = task.self_link.as_deref() else {
            failures.push(BugFailure::new(
                change.operation(),
                Some(label),
                None,
                LpError::Other("Bug task has no self_link".into()),
            ));
            continue;
        };
        match change.apply(client, task_url).await {
            Ok(updated) => entries.push(updated.raw),
            Err(error) => failures.push(BugFailure::new(
                change.operation(),
                Some(label),
                Some(task_url.to_string()),
                error,
            )),
        }
    }
    let applied = entries.len();
    let links = entries.iter().filter_map(raw_self_link).collect();
    let json = serde_json::json!({
        "bug_tasks": collection_json(entries),
        "lpcli": {"failures": failures_json(&failures)},
    });
    let mut status_spans = spans;
    status_spans.push(Span::Text(format!(
        " for {applied} task(s) on bug #{bug_id}."
    )));
    let blocks = vec![Block::Status(Level::Ok, status_spans)];
    let total = matched.len();
    finish_batch(
        BugOutcome::mutation(json, blocks, links),
        applied,
        failures,
        |failures| {
            format!(
                "Updated {applied} of {total} task(s) on bug #{bug_id}; {} failed: {}",
                failures.len(),
                describe_failures(failures)
            )
        },
    )
}

// ---------------------------------------------------------------------------
// Report block builders
// ---------------------------------------------------------------------------

struct ParsedTask<'a> {
    series: Option<String>,
    task: &'a bugs::BugTask,
}

fn group_bug_tasks<'a>(
    tasks: &'a [bugs::BugTask],
    missing: &str,
) -> std::collections::BTreeMap<String, Vec<ParsedTask<'a>>> {
    let mut by_target: std::collections::BTreeMap<String, Vec<ParsedTask<'a>>> =
        std::collections::BTreeMap::new();
    for t in tasks {
        let (target_name, series) = bugs::parse_target_link(t.target_link.as_deref().unwrap_or(""));
        let key = if !target_name.is_empty() {
            target_name
        } else {
            t.bug_target_display_name
                .as_deref()
                .unwrap_or(missing)
                .to_string()
        };
        by_target
            .entry(key)
            .or_default()
            .push(ParsedTask { series, task: t });
    }

    // Within each target group: the series-less task first, then series tasks
    // ordered alphabetically.
    for tasks_for_target in by_target.values_mut() {
        tasks_for_target.sort_by(|a, b| match (&a.series, &b.series) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(sa), Some(sb)) => sa.cmp(sb),
        });
    }
    by_target
}

fn task_display_target(target_name: &str, series: Option<&str>, task: &bugs::BugTask) -> String {
    if series.is_none()
        && let Some(disp) = task.bug_target_display_name.as_deref()
        && disp.contains('(')
    {
        disp.to_string()
    } else {
        target_name.to_string()
    }
}

fn ok_block(message: impl Into<String>) -> Block {
    Block::Status(Level::Ok, vec![Span::Text(message.into())])
}

fn note_block(message: impl Into<String>) -> Block {
    Block::Note(vec![Span::Text(message.into())])
}

fn bug_tasks_table_block(tasks: &[bugs::BugTask], empty: String) -> Block {
    let by_target = group_bug_tasks(tasks, "none");
    let has_any_watch = tasks.iter().any(|t| t.bug_watch_link.is_some());
    let mut columns = vec![
        column("Target", "target"),
        column("Series", "series"),
        column("Status", "status"),
        column("Importance", "importance"),
        column("Assignee", "assignee"),
    ];
    if has_any_watch {
        columns.push(column("Watch", "watch"));
    }
    let mut rows = Vec::new();
    for (target_name, tasks_for_target) in &by_target {
        for pt in tasks_for_target {
            let display_target = task_display_target(target_name, pt.series.as_deref(), pt.task);
            let mut row = vec![
                Some(display_target),
                Some(pt.series.clone().unwrap_or_default()),
                pt.task.status.clone(),
                pt.task.importance.clone(),
                output::link_name(pt.task.assignee_link.as_deref()),
            ];
            if has_any_watch {
                row.push(pt.task.bug_watch_id().map(|id| format!("#{id}")));
            }
            rows.push(row);
        }
    }
    Block::Table {
        records: Records { columns, rows },
        noun: Noun {
            singular: "bug task",
            plural: "bug tasks",
        },
        empty,
        hint: None,
    }
}

fn bug_attachments_table_block(
    attachments: &[bugs::BugAttachment],
    download_urls: &[Option<String>],
    empty: String,
) -> Block {
    let columns = vec![
        column("Title", "title"),
        column("Type", "type"),
        column("URL", "download_url"),
    ];
    let rows = attachments
        .iter()
        .zip(download_urls.iter())
        .map(|(a, url)| vec![a.title.clone(), a.attachment_type.clone(), url.clone()])
        .collect();
    Block::Table {
        records: Records { columns, rows },
        noun: Noun {
            singular: "attachment",
            plural: "attachments",
        },
        empty,
        hint: None,
    }
}

fn bug_show_blocks(read: &BugReportRead) -> Vec<Block> {
    let bug = &read.bug;
    let mut blocks = vec![
        Block::Heading(format!("Bug #{}", bug.id)),
        Block::Field("Title", bug.title.clone()),
    ];
    if !bug.tags.is_empty() {
        blocks.push(Block::Field("Tags", bug.tags.join(", ")));
    }
    if let Some(count) = bug.users_affected_count {
        blocks.push(Block::Field("Affects", format!("{count} user(s)")));
    }
    if let Some(link) = &bug.web_link {
        blocks.push(Block::Field("URL", link.clone()));
    }
    if let Some(desc) = &bug.description {
        blocks.push(Block::Field("Description", desc.clone()));
    }
    blocks.push(Block::Break);
    blocks.push(bug_tasks_table_block(
        &read.tasks,
        "No bug tasks found.".into(),
    ));
    if !read.attachments.is_empty() {
        blocks.push(Block::Break);
        blocks.push(Block::Heading("Attachments".into()));
        blocks.push(bug_attachments_table_block(
            &read.attachments,
            &read.download_urls,
            "No attachments found.".into(),
        ));
    }
    blocks
}

fn bug_comments_blocks(read: &BugCommentsRead, bug_id: u64) -> Vec<Block> {
    let entries = read
        .comments
        .iter()
        .enumerate()
        .filter(|(_, c)| c.date_deleted.is_none())
        .map(|(i, c)| {
            let idx = c.index.unwrap_or(i as u64);
            let attachments = read
                .attachments
                .iter()
                .zip(&read.download_urls)
                .filter(|(a, _)| {
                    c.self_link.is_some() && a.message_link.as_deref() == c.self_link.as_deref()
                })
                .map(|(a, url)| CommentAttachment {
                    title: a.title.as_deref().unwrap_or("attachment").to_string(),
                    kind: a
                        .attachment_type
                        .as_deref()
                        .unwrap_or("Unspecified")
                        .to_string(),
                    url: url.clone(),
                })
                .collect();
            CommentEntry {
                label_field: "Comment",
                label: format!("#{idx}"),
                author: output::link_name(c.owner_link.as_deref()),
                date: c.date_created.map(CommentDate::Timestamp),
                details: Vec::new(),
                content_field: "Content",
                content: c.content.clone().unwrap_or_default(),
                attachments,
            }
        })
        .collect();
    vec![Block::Comments {
        entries,
        noun: Noun {
            singular: "comment",
            plural: "comments",
        },
        empty: format!("No comments found for bug #{bug_id}."),
    }]
}

fn bug_subscriptions_table_block(subs: &[bugs::BugSubscription], bug_id: u64) -> Block {
    let columns = vec![
        column("Person", "person"),
        column("Subscribed By", "subscribed_by"),
        Column::date("Date", "date_created"),
    ];
    let rows = subs
        .iter()
        .map(|s| {
            vec![
                output::link_name(s.person_link.as_deref()),
                output::link_name(s.subscribed_by_link.as_deref()),
                s.date_created.map(|ts| ts.to_string()),
            ]
        })
        .collect();
    Block::Table {
        records: Records { columns, rows },
        noun: Noun {
            singular: "subscription",
            plural: "subscriptions",
        },
        empty: format!("No subscriptions found for bug #{bug_id}."),
        hint: None,
    }
}

fn bug_watches_table_block(watches: &[bugs::BugWatch], linked: &[String], bug_id: u64) -> Block {
    let columns = vec![
        column("Watch ID", "id"),
        column("Tracker", "bug_tracker"),
        column("Remote Bug", "remote_bug"),
        column("Status", "remote_status"),
        column("URL", "url"),
        column("Linked Tasks", "linked_tasks"),
    ];
    let rows = watches
        .iter()
        .zip(linked)
        .map(|(w, linked_tasks)| {
            vec![
                w.id().map(str::to_string),
                output::link_name(w.bug_tracker_link.as_deref()),
                w.remote_bug.clone(),
                w.remote_status.clone(),
                w.url.clone(),
                Some(linked_tasks.clone()),
            ]
        })
        .collect();
    Block::Table {
        records: Records { columns, rows },
        noun: Noun {
            singular: "bug watch",
            plural: "bug watches",
        },
        empty: format!("No bug watches found for bug #{bug_id}."),
        hint: None,
    }
}

fn ensure_tracker_blocks(tracker: &bugs::BugTracker, created: bool) -> Vec<Block> {
    let name = tracker.name.as_deref().unwrap_or("(unknown name)");
    let state = if created {
        "created"
    } else {
        "reused (already existed)"
    };
    let mut blocks = vec![ok_block(format!("Bug tracker '{name}' {state}."))];
    if let Some(tracker_type) = tracker.bug_tracker_type.as_deref() {
        blocks.push(Block::Field("Type", tracker_type.to_string()));
    }
    if let Some(base_url) = tracker.base_url.as_deref() {
        blocks.push(Block::Field("Base URL", base_url.to_string()));
    }
    if let Some(self_link) = tracker.self_link.as_deref() {
        blocks.push(Block::Field("API", self_link.to_string()));
    }
    blocks
}

// ---------------------------------------------------------------------------
// Attachments
// ---------------------------------------------------------------------------

/// Resolve the download URL for a bug attachment.
///
/// The `data_link` field on a bug attachment points to the Launchpad API
/// file resource which 303-redirects to the librarian URL containing the
/// actual filename. This function resolves that redirect to obtain the
/// direct download URL, or `None` when it cannot.
async fn resolve_attachment_download_url(
    client: &LaunchpadClient,
    attachment: &bugs::BugAttachment,
) -> Option<String> {
    let data_link = attachment.data_link.as_deref()?;
    // The library's classic OAuth client signs absolute URLs indiscriminately.
    // Only enrich API-origin links with auth; download hosts remain unsigned.
    let same_origin = url::Url::parse(&client.url("/"))
        .ok()
        .zip(url::Url::parse(data_link).ok())
        .is_some_and(|(base, link)| base.origin() == link.origin());
    let anonymous = LaunchpadClient::new(None);
    let resolver = if same_origin { client } else { &anonymous };
    resolver.resolve_redirect(data_link).await.ok().flatten()
}

/// Optional URL enrichment must not prevent displaying the underlying read.
async fn resolve_attachment_download_urls(
    client: &LaunchpadClient,
    attachments: &[bugs::BugAttachment],
) -> Vec<Option<String>> {
    futures_util::future::join_all(
        attachments
            .iter()
            .map(|a| resolve_attachment_download_url(client, a)),
    )
    .await
}

/// Like [`resolve_attachment_download_urls`], with `—` for unresolved URLs.
#[cfg(test)]
pub(crate) async fn resolve_all_attachment_urls(
    client: &LaunchpadClient,
    attachments: &[bugs::BugAttachment],
) -> Vec<String> {
    resolve_attachment_download_urls(client, attachments)
        .await
        .into_iter()
        .map(|url| url.unwrap_or_else(|| "—".to_string()))
        .collect()
}

/// The `lpcli` download URLs, one per attachment in Launchpad's order.
fn attachment_urls_json(
    attachments: &[bugs::BugAttachment],
    download_urls: &[Option<String>],
) -> serde_json::Value {
    attachments
        .iter()
        .zip(download_urls)
        .map(|(attachment, url)| {
            serde_json::json!({
                "attachment_link": attachment.self_link,
                "download_url": url,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Combined reads
// ---------------------------------------------------------------------------

pub(crate) struct BugReportRead {
    pub(crate) bug: bugs::Bug,
    pub(crate) tasks: Vec<bugs::BugTask>,
    pub(crate) attachments: Vec<bugs::BugAttachment>,
    pub(crate) download_urls: Vec<Option<String>>,
    pub(crate) bug_raw: serde_json::Value,
    pub(crate) tasks_raw: serde_json::Value,
    pub(crate) attachments_raw: serde_json::Value,
}

impl BugReportRead {
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "bug": self.bug_raw,
            "bug_tasks": self.tasks_raw,
            "attachments": self.attachments_raw,
            "lpcli": {
                "attachment_download_urls":
                    attachment_urls_json(&self.attachments, &self.download_urls),
            },
        })
    }
}

pub(crate) async fn fetch_bug_report(
    client: &LaunchpadClient,
    bug_id: u64,
) -> lpcli::error::Result<BugReportRead> {
    let (bug, tasks, attachments) = tokio::try_join!(
        bugs::get_bug_raw(client, bug_id),
        bugs::get_bug_tasks_raw(client, bug_id),
        bugs::get_bug_attachments_raw(client, bug_id),
    )?;
    let download_urls = resolve_attachment_download_urls(client, &attachments.value).await;
    Ok(BugReportRead {
        bug: bug.value,
        tasks: tasks.value,
        attachments: attachments.value,
        download_urls,
        bug_raw: bug.raw,
        tasks_raw: tasks.raw,
        attachments_raw: attachments.raw,
    })
}

pub(crate) struct BugCommentsRead {
    pub(crate) comments: Vec<bugs::BugComment>,
    pub(crate) attachments: Vec<bugs::BugAttachment>,
    pub(crate) download_urls: Vec<Option<String>>,
    pub(crate) messages_raw: serde_json::Value,
    pub(crate) attachments_raw: serde_json::Value,
}

impl BugCommentsRead {
    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "messages": self.messages_raw,
            "attachments": self.attachments_raw,
            "lpcli": {
                "attachment_download_urls":
                    attachment_urls_json(&self.attachments, &self.download_urls),
            },
        })
    }

    fn records(&self) -> Records {
        raw_records(
            raw_entries(&self.messages_raw),
            vec![
                column("Author", "owner_link"),
                column("Date", "date_created"),
                column("Deleted", "date_deleted"),
                column("Subject", "subject"),
                column("Content", "content"),
                column("Comment URL", "self_link"),
            ],
        )
    }
}

pub(crate) async fn fetch_bug_comments(
    client: &LaunchpadClient,
    bug_id: u64,
) -> lpcli::error::Result<BugCommentsRead> {
    let (comments, attachments) = tokio::try_join!(
        bugs::get_bug_comments_raw(client, bug_id),
        bugs::get_bug_attachments_raw(client, bug_id),
    )?;
    let download_urls = resolve_attachment_download_urls(client, &attachments.value).await;
    Ok(BugCommentsRead {
        comments: comments.value,
        attachments: attachments.value,
        download_urls,
        messages_raw: comments.raw,
        attachments_raw: attachments.raw,
    })
}

fn tasks_records(tasks_raw: &serde_json::Value) -> Records {
    raw_records(
        raw_entries(tasks_raw),
        vec![
            column("Target", "bug_target_display_name"),
            column("Status", "status"),
            column("Importance", "importance"),
            column("Assignee", "assignee_link"),
            column("Watch", "bug_watch_link"),
            column("Target URL", "target_link"),
            column("Task URL", "self_link"),
        ],
    )
}

fn subscriptions_records(subs_raw: &serde_json::Value) -> Records {
    raw_records(
        raw_entries(subs_raw),
        vec![
            column("Person", "person_link"),
            column("Subscribed By", "subscribed_by_link"),
            column("Date", "date_created"),
            column("Subscription URL", "self_link"),
        ],
    )
}

fn attachments_records(
    attachments_raw: &serde_json::Value,
    download_urls: &[Option<String>],
) -> Records {
    let columns = vec![
        column("Title", "title"),
        column("Type", "type"),
        column("Download URL", "download_url"),
        column("Attachment URL", "self_link"),
    ];
    let rows = raw_entries(attachments_raw)
        .iter()
        .zip(download_urls)
        .map(|(entry, url)| {
            vec![
                raw_text(entry, "title"),
                raw_text(entry, "type"),
                url.clone(),
                raw_text(entry, "self_link"),
            ]
        })
        .collect();
    Records { columns, rows }
}

/// For each watch, the `self_link`s of the tasks linked to it, matched by
/// watch ID so links under any API version match.
fn watch_task_links(
    watches: &[bugs::BugWatch],
    tasks: &[bugs::BugTask],
) -> Vec<Option<Vec<String>>> {
    watches
        .iter()
        .map(|watch| {
            let id = watch.id()?;
            Some(
                tasks
                    .iter()
                    .filter(|task| task.bug_watch_id() == Some(id))
                    .filter_map(|task| task.self_link.clone())
                    .collect(),
            )
        })
        .collect()
}

pub(crate) struct BugWatchesRead {
    pub(crate) watches: Raw<Vec<bugs::BugWatch>>,
    pub(crate) tasks: Raw<Vec<bugs::BugTask>>,
}

impl BugWatchesRead {
    pub(crate) fn json(&self) -> serde_json::Value {
        let linked: Vec<serde_json::Value> = self
            .watches
            .value
            .iter()
            .zip(watch_task_links(&self.watches.value, &self.tasks.value))
            .map(|(watch, links)| {
                serde_json::json!({
                    "bug_watch_link": watch.self_link,
                    "bug_task_links": links,
                })
            })
            .collect();
        serde_json::json!({
            "bug_watches": self.watches.raw,
            "bug_tasks": self.tasks.raw,
            "lpcli": {"linked_bug_tasks": linked},
        })
    }

    /// The "Linked Tasks" cell of each watch, in watch order.
    fn linked_cells(&self) -> Vec<String> {
        let links = collect_watch_task_links(&self.tasks.value);
        self.watches
            .value
            .iter()
            .map(|watch| watch_linked_tasks_cell(watch.id(), &links))
            .collect()
    }

    fn records(&self) -> Records {
        let columns = vec![
            column("Watch URL", "self_link"),
            column("Tracker", "bug_tracker_link"),
            column("Remote Bug", "remote_bug"),
            column("Status", "remote_status"),
            column("URL", "url"),
            column("Linked Tasks", "linked_tasks"),
        ];
        let rows = raw_entries(&self.watches.raw)
            .iter()
            .zip(self.linked_cells())
            .map(|(entry, linked)| {
                let mut row: Vec<Option<String>> = columns[..5]
                    .iter()
                    .map(|column| raw_text(entry, column.key))
                    .collect();
                row.push(Some(linked));
                row
            })
            .collect();
        Records { columns, rows }
    }
}

// ---------------------------------------------------------------------------
// Bug handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_bug(
    cmd: BugCommand,
    output: &Output,
    quiet: bool,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    let client = if matches!(&cmd, BugCommand::RemoteBugTrackers { .. }) {
        client.with_max_retries(2)
    } else {
        client
    };
    bug_outcome(&client, cmd).await?.emit(output, quiet)
}

fn attachment_params<'a>(
    comment: &'a str,
    attachment_file: Option<&'a str>,
    attachment_url: Option<&'a str>,
    attachment_description: Option<&'a str>,
    attachment_type: Option<&str>,
) -> bugs::AddAttachmentParams<'a> {
    bugs::AddAttachmentParams {
        comment,
        file_path: attachment_file.map(std::path::Path::new),
        url: attachment_url,
        filename: None,
        description: attachment_description,
        is_patch: attachment_type.is_some_and(|t| t.eq_ignore_ascii_case("patch")),
    }
}

/// Run a bug command and collect what it prints.
pub(crate) async fn bug_outcome(
    client: &LaunchpadClient,
    cmd: BugCommand,
) -> lpcli::error::Result<BugOutcome> {
    Ok(match cmd {
        BugCommand::Show { bug_id } => {
            let read = fetch_bug_report(client, bug_id).await?;
            BugOutcome::read_blocks(read.json(), bug_show_blocks(&read))
        }

        BugCommand::Tasks { bug_id } => {
            let tasks = bugs::get_bug_tasks_raw(client, bug_id).await?;
            BugOutcome::list_blocks(
                tasks.raw.clone(),
                tasks_records(&tasks.raw),
                vec![bug_tasks_table_block(
                    &tasks.value,
                    format!("No bug tasks found for bug #{bug_id}."),
                )],
            )
        }

        BugCommand::Search {
            target,
            status,
            importance,
            tag,
            package,
            keyword,
            limit,
        } => {
            let params = BugSearchParams {
                status: status.as_deref(),
                importance: importance.as_deref(),
                tag: tag.as_deref(),
                package_name: package.as_deref(),
                search_text: keyword.as_deref(),
                limit: Some(limit),
                ..Default::default()
            };
            let page = bugs::search_bugs_raw(client, &target, &params).await?;
            BugOutcome::report(bug_search_report(&page))
        }

        BugCommand::AddComment {
            bug_id,
            message,
            attachment_file,
            attachment_url,
            attachment_description,
            attachment_type,
        } => {
            if attachment_file.is_some() || attachment_url.is_some() {
                let params = attachment_params(
                    &message,
                    attachment_file.as_deref(),
                    attachment_url.as_deref(),
                    attachment_description.as_deref(),
                    attachment_type.as_deref(),
                );
                let attachment = bugs::add_bug_attachment_raw(client, bug_id, &params).await?;
                let msg = format!("Comment with attachment added to bug #{bug_id}.");
                BugOutcome::entry(&attachment, vec![ok_block(msg)])
            } else {
                let comment = bugs::add_bug_comment_raw(client, bug_id, &message).await?;
                let msg = format!("Comment added to bug #{bug_id}.");
                BugOutcome::entry(&comment, vec![ok_block(msg)])
            }
        }

        BugCommand::Comments { bug_id } => {
            let read = fetch_bug_comments(client, bug_id).await?;
            BugOutcome::list_blocks(
                read.json(),
                read.records(),
                bug_comments_blocks(&read, bug_id),
            )
        }

        BugCommand::DeleteComment {
            bug_id,
            comment_index,
        } => {
            bugs::delete_bug_comment(client, bug_id, comment_index).await?;
            let msg = format!("Comment #{comment_index} deleted from bug #{bug_id}.");
            BugOutcome::mutation(
                serde_json::json!({"lpcli": {
                    "bug_id": bug_id,
                    "comment_index": comment_index,
                    "deleted": true,
                }}),
                vec![ok_block(msg)],
                Vec::new(),
            )
        }

        BugCommand::Create {
            target,
            package,
            title,
            description,
            attachment_file,
            attachment_url,
            attachment_description,
            attachment_type,
        } => {
            let effective_target = match &package {
                Some(pkg) => format!("{target}/+source/{pkg}"),
                None => target.clone(),
            };
            let bug = bugs::create_bug_raw(client, &effective_target, &title, &description).await?;
            let bug_id = bug.value.id;
            let mut blocks = vec![ok_block(format!("Bug #{bug_id} created."))];
            if let Some(link) = &bug.value.web_link {
                blocks.push(Block::Field("URL", link.clone()));
            }
            if attachment_file.is_none() && attachment_url.is_none() {
                return Ok(BugOutcome::entry(&bug, blocks));
            }

            let params = attachment_params(
                "",
                attachment_file.as_deref(),
                attachment_url.as_deref(),
                attachment_description.as_deref(),
                attachment_type.as_deref(),
            );
            let mut links: Vec<String> = raw_self_link(&bug.raw).into_iter().collect();
            let (attachment, failures) =
                match bugs::add_bug_attachment_raw(client, bug_id, &params).await {
                    Ok(attachment) => {
                        blocks.push(ok_block(format!("Attachment added to bug #{bug_id}.")));
                        links.extend(raw_self_link(&attachment.raw));
                        (attachment.raw, Vec::new())
                    }
                    Err(error) => (
                        serde_json::Value::Null,
                        vec![BugFailure::new("add_attachment", None, None, error)],
                    ),
                };
            let json = serde_json::json!({
                "bug": bug.raw,
                "attachment": attachment,
                "lpcli": {"failures": failures_json(&failures)},
            });
            // The bug was created, so any failure here is partial.
            finish_batch(
                BugOutcome::mutation(json, blocks, links),
                1,
                failures,
                |failures| {
                    format!(
                        "Bug #{bug_id} was created, but adding the attachment failed: {}. \
                         Retry with `lpcli bug add-comment -b {bug_id}` rather than \
                         filing the bug again.",
                        failures[0].error
                    )
                },
            )
        }

        BugCommand::SetDescription {
            bug_id,
            description,
            description_file,
        } => {
            let text = resolve_new_description(description, description_file.as_deref())?;
            let updated = bugs::set_bug_description_raw(client, bug_id, &text).await?;
            let mut blocks = vec![ok_block(format!("Description updated for bug #{bug_id}."))];
            if let Some(link) = &updated.value.web_link {
                blocks.push(Block::Field("URL", link.clone()));
            }
            BugOutcome::entry(&updated, blocks)
        }

        BugCommand::SetStatus {
            bug_id,
            target,
            many_targets,
            all_targets,
            series,
            many_series,
            all_series,
            no_series,
            status,
        } => {
            let (series, many_series) =
                canonical_bug_series(target.as_deref(), series, many_series);
            let target_filter = TargetFilter::from_args(target, many_targets, all_targets);
            let series_filter = SeriesFilter::from_args(series, many_series, all_series, no_series);
            let tasks = bugs::get_bug_tasks(client, bug_id).await?;
            let matched = collect_matching_tasks(&tasks, bug_id, &target_filter, &series_filter)?;
            update_tasks(
                client,
                bug_id,
                &matched,
                &TaskChange::Status(status.into()),
                vec![Span::Text(format!("Status updated to '{status}'"))],
            )
            .await
        }

        BugCommand::SetImportance {
            bug_id,
            target,
            many_targets,
            all_targets,
            series,
            many_series,
            all_series,
            no_series,
            importance,
        } => {
            let (series, many_series) =
                canonical_bug_series(target.as_deref(), series, many_series);
            let target_filter = TargetFilter::from_args(target, many_targets, all_targets);
            let series_filter = SeriesFilter::from_args(series, many_series, all_series, no_series);
            let tasks = bugs::get_bug_tasks(client, bug_id).await?;
            let matched = collect_matching_tasks(&tasks, bug_id, &target_filter, &series_filter)?;
            update_tasks(
                client,
                bug_id,
                &matched,
                &TaskChange::Importance(importance.into()),
                vec![Span::Text(format!("Importance updated to '{importance}'"))],
            )
            .await
        }

        BugCommand::SetAssignee {
            bug_id,
            target,
            many_targets,
            all_targets,
            series,
            many_series,
            all_series,
            no_series,
            name,
        } => {
            let (series, many_series) =
                canonical_bug_series(target.as_deref(), series, many_series);
            let target_filter = TargetFilter::from_args(target, many_targets, all_targets);
            let series_filter = SeriesFilter::from_args(series, many_series, all_series, no_series);
            let assignee_url = client.url(&format!("/~{name}"));
            let tasks = bugs::get_bug_tasks(client, bug_id).await?;
            let matched = collect_matching_tasks(&tasks, bug_id, &target_filter, &series_filter)?;
            update_tasks(
                client,
                bug_id,
                &matched,
                &TaskChange::Assignee(assignee_url),
                vec![
                    Span::Text("Assigned to ".into()),
                    Span::Name(format!("~{name}")),
                ],
            )
            .await
        }

        BugCommand::Subscribe { bug_id, name } => {
            let person_url = client.url(&format!("/~{name}"));
            let sub = bugs::subscribe_to_bug_raw(client, bug_id, &person_url).await?;
            let mut blocks = vec![Block::Status(
                Level::Ok,
                vec![
                    Span::Text("Subscribed ".into()),
                    Span::Name(format!("~{name}")),
                    Span::Text(format!(" to bug #{bug_id}.")),
                ],
            )];
            if let Some(link) = &sub.value.self_link {
                blocks.push(Block::Field("Subscription", link.clone()));
            }
            BugOutcome::entry(&sub, blocks)
        }

        BugCommand::Unsubscribe { bug_id, name } => {
            let person_url = client.url(&format!("/~{name}"));
            bugs::unsubscribe_from_bug(client, bug_id, &person_url).await?;
            let blocks = vec![Block::Status(
                Level::Ok,
                vec![
                    Span::Text("Unsubscribed ".into()),
                    Span::Name(format!("~{name}")),
                    Span::Text(format!(" from bug #{bug_id}.")),
                ],
            )];
            BugOutcome::mutation(
                serde_json::json!({"lpcli": {
                    "bug_id": bug_id,
                    "person_link": person_url,
                    "unsubscribed": true,
                }}),
                blocks,
                Vec::new(),
            )
        }

        BugCommand::Subscriptions { bug_id } => {
            let subs = bugs::get_bug_subscriptions_raw(client, bug_id).await?;
            BugOutcome::list_blocks(
                subs.raw.clone(),
                subscriptions_records(&subs.raw),
                vec![bug_subscriptions_table_block(&subs.value, bug_id)],
            )
        }

        BugCommand::Attachments { bug_id } => {
            let attachments = bugs::get_bug_attachments_raw(client, bug_id).await?;
            let download_urls = resolve_attachment_download_urls(client, &attachments.value).await;
            BugOutcome::list_blocks(
                serde_json::json!({
                    "attachments": attachments.raw,
                    "lpcli": {
                        "attachment_download_urls":
                            attachment_urls_json(&attachments.value, &download_urls),
                    },
                }),
                attachments_records(&attachments.raw, &download_urls),
                vec![bug_attachments_table_block(
                    &attachments.value,
                    &download_urls,
                    format!("No attachments for bug #{bug_id}."),
                )],
            )
        }

        BugCommand::AddTask {
            bug_id,
            target,
            package,
            series,
            many_series,
            importance,
            status,
            assignee,
            bug_tracker,
            remote_bug,
        } => {
            add_tasks(
                client,
                AddTasks {
                    bug_id,
                    target,
                    package,
                    series,
                    many_series,
                    importance,
                    status,
                    assignee,
                    remote: bug_tracker.zip(remote_bug),
                },
            )
            .await
        }

        BugCommand::DeleteTask {
            bug_id,
            target,
            package,
            series,
            many_series,
        } => delete_tasks(client, bug_id, target, package, series, many_series).await?,

        BugCommand::Tag {
            bug_id,
            add_tag,
            add_many_tags,
            remove_tag,
            remove_many_tags,
            remove_all_tags,
        } => {
            // Fetch the bug to get its current tags.
            let bug = bugs::get_bug(client, bug_id).await?;

            // Apply removals first (per spec), then additions.
            let after_remove =
                apply_tag_removals(bug.tags, remove_tag, remove_many_tags, remove_all_tags);
            let final_tags = apply_tag_additions(after_remove, add_tag, add_many_tags);

            let updated = bugs::set_bug_tags_raw(client, bug_id, &final_tags).await?;
            let blocks = if updated.value.tags.is_empty() {
                vec![ok_block(format!("Bug #{bug_id} tags cleared (no tags)."))]
            } else {
                let tags = updated.value.tags.join(", ");
                vec![Block::Status(
                    Level::Ok,
                    vec![
                        Span::Text(format!("Bug #{bug_id} tags updated: ")),
                        Span::Name(tags),
                    ],
                )]
            };
            BugOutcome::entry(&updated, blocks)
        }

        BugCommand::LinkRemote {
            bug_id,
            github,
            bug_tracker,
            remote_bug,
            target,
            package,
            series,
            task_url,
        } => {
            let series =
                series.map(|s| canonical_series(target.as_deref().unwrap_or(""), &s).to_string());
            let tasks = bugs::get_bug_tasks(client, bug_id).await?;
            let selector = TaskLinkSelector {
                target,
                package,
                series,
                task_url,
            };
            // Task selection is identical for the explicit tracker form and
            // the --github shorthand: the shorthand only changes how the
            // tracker and remote bug are resolved. Without any selector,
            // only a bug with exactly one task is linked; see
            // `find_task_to_link` for the series-less preference that
            // applies once --target/--package narrow the candidates.
            let task = find_task_to_link(&tasks, bug_id, &selector)?;
            let effective_task_url = task.self_link.as_deref().ok_or_else(|| {
                LpError::LocalNotFound(format!(
                    "Selected bug task has no self_link on bug #{bug_id}."
                ))
            })?;
            let display_name = task
                .bug_target_display_name
                .as_deref()
                .unwrap_or(effective_task_url);

            let ((linked_task, watch), remote_bug_label) = match &github {
                // Shorthand: one library call finds-or-registers the
                // correctly configured auto-github-<owner>-<repo> tracker,
                // reuses or creates the watch, validates the watch URL shape
                // before touching the task, and links it.
                Some(github_ref) => (
                    bugs::link_github_bug_task_raw(client, bug_id, effective_task_url, github_ref)
                        .await?,
                    github_ref.to_string(),
                ),
                None => {
                    // Clap's remote_link_spec group requires one of
                    // --github/--bug-tracker, and --bug-tracker requires
                    // --remote-bug, so both are present here.
                    let tracker_name = bug_tracker.as_deref().ok_or_else(|| {
                        LpError::Usage(
                            "--bug-tracker is required when --github is not given".into(),
                        )
                    })?;
                    let remote_bug_id = remote_bug.as_deref().ok_or_else(|| {
                        LpError::Usage("--remote-bug is required when --github is not given".into())
                    })?;
                    (
                        bugs::link_bug_task_remote_raw(
                            client,
                            bug_id,
                            effective_task_url,
                            tracker_name,
                            remote_bug_id,
                        )
                        .await?,
                        remote_bug_id.to_string(),
                    )
                }
            };

            let watch_id = watch.value.id().unwrap_or(&remote_bug_label);
            let watch_title = watch.value.title.as_deref().unwrap_or(watch_id);
            // The final watch URL is always printed: Launchpad derives it
            // from the tracker, so a missing URL must stay visible rather
            // than be silently skipped (mis-typed trackers generate junk
            // URLs that otherwise only show up in `bug watches`).
            let plain_url = watch
                .value
                .url
                .as_deref()
                .unwrap_or("(Launchpad returned no watch URL; check `lpcli bug watches`)")
                .to_string();
            let blocks = vec![
                ok_block(format!(
                    "Linked task '{display_name}' to remote bug: {watch_title}"
                )),
                Block::Field("URL", plain_url),
            ];
            let links = [&linked_task.raw, &watch.raw]
                .into_iter()
                .filter_map(raw_self_link)
                .collect();
            BugOutcome::mutation(
                serde_json::json!({"bug_task": linked_task.raw, "bug_watch": watch.raw}),
                blocks,
                links,
            )
        }

        BugCommand::UnlinkRemote {
            bug_id,
            target,
            package,
            series,
            task_url,
        } => {
            let series =
                series.map(|s| canonical_series(target.as_deref().unwrap_or(""), &s).to_string());
            let tasks = bugs::get_bug_tasks(client, bug_id).await?;
            let selector = TaskLinkSelector {
                target,
                package,
                series,
                task_url,
            };
            let task = find_task_to_link(&tasks, bug_id, &selector)?;
            let effective_task_url = task.self_link.as_deref().ok_or_else(|| {
                LpError::LocalNotFound(format!(
                    "Selected bug task has no self_link on bug #{bug_id}."
                ))
            })?;
            let display_name = task
                .bug_target_display_name
                .as_deref()
                .unwrap_or(effective_task_url);

            let updated = bugs::unset_bug_watch_raw(client, effective_task_url).await?;

            let mut blocks = vec![ok_block(format!(
                "Unlinked remote bug watch from task '{display_name}'."
            ))];
            // Unlinking only detaches the task; the watch object survives
            // (the REST API cannot delete watches) and becomes an orphan
            // that `bug watches` will report. Point at the web UI, which is
            // the only place a watch can be deleted.
            if let Some(watch_id) = task.bug_watch_id() {
                let note = format!(
                    "The watch itself remains on bug #{bug_id}; delete it in the web UI at https://bugs.launchpad.net/bugs/{bug_id}/+watch/{watch_id}"
                );
                blocks.push(note_block(note));
            }
            BugOutcome::entry(&updated, blocks)
        }

        BugCommand::Watches { bug_id } => {
            // A failure to fetch the tasks propagates: it must not be read
            // as "nothing is linked" and render every watch as an orphan.
            let (watches, tasks) = tokio::try_join!(
                bugs::get_bug_watches_raw(client, bug_id),
                bugs::get_bug_tasks_raw(client, bug_id),
            )?;
            let read = BugWatchesRead { watches, tasks };
            let linked = read.linked_cells();
            BugOutcome::list_blocks(
                read.json(),
                read.records(),
                vec![bug_watches_table_block(
                    &read.watches.value,
                    &linked,
                    bug_id,
                )],
            )
        }

        BugCommand::RemoteBugTrackers { limit, search } => {
            let found = bugs::search_bug_trackers_raw(client, search.as_deref(), limit).await?;
            BugOutcome::report(remote_bug_trackers_report(&found, search.is_some()))
        }

        BugCommand::Tracker {
            command:
                TrackerCommand::Create {
                    base_url,
                    tracker_type,
                    name,
                    title,
                    summary,
                },
        } => {
            // validate_cli already rejected invalid combinations; re-resolve
            // here (pure, no network) to obtain the requested tracker type
            // and name for dispatch.
            let resolved = resolve_tracker_create(&base_url, tracker_type, name.as_deref())
                .map_err(LpError::Usage)?;
            let params = bugs::EnsureBugTrackerParams {
                base_url: &resolved.base_url,
                bug_tracker_type: resolved.bug_tracker_type,
                name: resolved.name.as_deref(),
                title: title.as_deref(),
                summary: summary.as_deref(),
            };
            // The library validates the tracker it returns against the
            // request (exact type, matching base URL). The printed identity,
            // type and base URL come from the tracker entry Launchpad
            // returned — not from the requested arguments — together with
            // whether the tracker was created or reused.
            let bugs::EnsureBugTrackerRawOutcome { tracker, created } =
                bugs::ensure_bug_tracker_raw(client, &params).await?;
            BugOutcome::entry(&tracker, ensure_tracker_blocks(&tracker.value, created))
        }
    })
}

// ---------------------------------------------------------------------------
// add-task and delete-task
// ---------------------------------------------------------------------------

struct AddTasks {
    bug_id: u64,
    target: String,
    package: Option<String>,
    series: Option<String>,
    many_series: Option<String>,
    importance: Option<BugTaskImportance>,
    status: Option<BugTaskStatus>,
    assignee: Option<String>,
    /// The remote bug tracker name and remote bug ID to link each task to.
    remote: Option<(String, String)>,
}

/// Add one task per requested series (or one for the base target), apply
/// the requested attributes, and link the remote bug, continuing after
/// failures.
async fn add_tasks(client: &LaunchpadClient, args: AddTasks) -> BugOutcome {
    let AddTasks {
        bug_id,
        target,
        package,
        series,
        many_series,
        importance,
        status,
        assignee,
        remote,
    } = args;
    let (series, many_series) = canonical_bug_series(Some(&target), series, many_series);
    let series_list = match (series, many_series) {
        (Some(s), _) => Some(vec![s]),
        (None, Some(many)) => Some(parse_comma_separated(&many)),
        (None, None) => None,
    };
    let package_path = package
        .as_deref()
        .map(|pkg| format!("/+source/{pkg}"))
        .unwrap_or_default();
    // Without a series a single task is added against the base target
    // (the distribution or project without a series component).
    let targets: Vec<(String, String)> = match series_list {
        None => vec![(
            target.clone(),
            client.url(&format!("/{target}{package_path}")),
        )],
        Some(list) => list
            .into_iter()
            .map(|s| {
                let url = client.url(&format!("/{target}/{s}{package_path}"));
                (s, url)
            })
            .collect(),
    };

    let mut changes = Vec::new();
    if let Some(importance) = importance {
        changes.push(TaskChange::Importance(importance.into()));
    }
    if let Some(status) = status {
        changes.push(TaskChange::Status(status.into()));
    }
    if let Some(name) = &assignee {
        changes.push(TaskChange::Assignee(client.url(&format!("/~{name}"))));
    }

    let mut blocks = Vec::new();
    let mut tasks = Vec::new();
    let mut watches = Vec::new();
    let mut failures = Vec::new();
    let mut remote_failures = Vec::new();
    for (series_name, target_url) in &targets {
        let task = match bugs::add_bug_task_raw(client, bug_id, target_url).await {
            Ok(task) => task,
            Err(error) => {
                failures.push(BugFailure::new(
                    "add_task",
                    Some(series_name.clone()),
                    None,
                    error,
                ));
                continue;
            }
        };
        let display_name = task
            .value
            .bug_target_display_name
            .clone()
            .unwrap_or_else(|| series_name.clone());
        blocks.push(ok_block(format!("Bug task added: {display_name}")));

        let Some(task_url) = task.value.self_link.clone() else {
            let operations = changes
                .iter()
                .map(TaskChange::operation)
                .chain(remote.as_ref().map(|_| "link_remote"));
            for operation in operations {
                let mut failure = BugFailure::new(
                    operation,
                    Some(series_name.clone()),
                    None,
                    LpError::Other("new task has no self_link".into()),
                );
                if let Some((tracker, remote_id)) = &remote
                    && operation == "link_remote"
                {
                    failure.description = remote_link_failure(
                        series_name,
                        tracker,
                        remote_id,
                        "new task has no self_link",
                    );
                    remote_failures.push(failure.description.clone());
                }
                failures.push(failure);
            }
            tasks.push(task.raw);
            continue;
        };

        let mut latest = task.raw;
        for change in &changes {
            match change.apply(client, &task_url).await {
                Ok(updated) => latest = updated.raw,
                Err(error) => failures.push(BugFailure::new(
                    change.operation(),
                    Some(series_name.clone()),
                    Some(task_url.clone()),
                    error,
                )),
            }
        }

        if let Some((tracker_name, remote_id)) = &remote {
            match bugs::link_bug_task_remote_raw(client, bug_id, &task_url, tracker_name, remote_id)
                .await
            {
                Ok((linked, watch)) => {
                    let watch_id = watch.value.id().unwrap_or(remote_id);
                    let watch_title = watch.value.title.as_deref().unwrap_or(watch_id);
                    blocks.push(ok_block(format!(
                        "Bug task linked to remote bug: {watch_title}"
                    )));
                    if let Some(url) = &watch.value.url {
                        blocks.push(Block::Field("Remote URL", url.clone()));
                    }
                    latest = linked.raw;
                    watches.push(watch.raw);
                }
                Err(error) => {
                    let description = remote_link_failure(
                        series_name,
                        tracker_name,
                        remote_id,
                        &error.to_string(),
                    );
                    remote_failures.push(description.clone());
                    let mut failure = BugFailure::new(
                        "link_remote",
                        Some(series_name.clone()),
                        Some(task_url.clone()),
                        error,
                    );
                    failure.description = description;
                    failures.push(failure);
                }
            }
        }
        tasks.push(latest);
    }

    let created = tasks.len();
    let mut links: Vec<String> = tasks.iter().filter_map(raw_self_link).collect();
    links.extend(watches.iter().filter_map(raw_self_link));
    let mut json = serde_json::json!({
        "bug_tasks": collection_json(tasks),
        "lpcli": {"failures": failures_json(&failures)},
    });
    if remote.is_some() {
        json["bug_watches"] = collection_json(watches);
    }
    let requested = targets.len();
    finish_batch(
        BugOutcome::mutation(json, blocks, links),
        created,
        failures,
        |failures| {
            let mut parts = vec![format!(
                "Created {created} of {requested} bug task(s) on bug #{bug_id}."
            )];
            let other: Vec<&BugFailure> = failures
                .iter()
                .filter(|failure| failure.operation != "link_remote")
                .collect();
            if !other.is_empty() {
                parts.push(format!(
                    "Failed: {}.",
                    other
                        .iter()
                        .map(|failure| failure.description.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            if let Some(error) = remote_link_failure_error(bug_id, &remote_failures) {
                parts.push(error.to_string());
            }
            parts.join(" ")
        },
    )
}

async fn delete_tasks(
    client: &LaunchpadClient,
    bug_id: u64,
    target: String,
    package: Option<String>,
    series: Option<String>,
    many_series: Option<String>,
) -> lpcli::error::Result<BugOutcome> {
    let (series, many_series) = canonical_bug_series(Some(&target), series, many_series);
    // The effective target name is the package name when a package is
    // provided, otherwise the distribution/project name — matching how
    // parse_target_link returns the package name (not the distro) for
    // source-package tasks.
    let effective_target = package.unwrap_or(target);
    let tasks = bugs::get_bug_tasks(client, bug_id).await?;

    // When no series is given, delete every task that matches the target
    // regardless of series. Otherwise apply the series filter.
    let matched: Vec<&bugs::BugTask> = match (series, many_series) {
        (None, None) => {
            let result: Vec<&bugs::BugTask> = tasks
                .iter()
                .filter(|t| {
                    let (tgt, _) = bugs::parse_target_link(t.target_link.as_deref().unwrap_or(""));
                    tgt.eq_ignore_ascii_case(&effective_target)
                })
                .collect();
            if result.is_empty() {
                return Err(LpError::LocalNotFound(format!(
                    "No tasks found for target '{effective_target}' on bug #{bug_id}."
                )));
            }
            result
        }
        (series, many_series) => {
            let series_filter = match (series, many_series) {
                (Some(s), _) => SeriesFilter::One(s),
                (None, Some(many)) => SeriesFilter::Many(parse_comma_separated(&many)),
                (None, None) => unreachable!("handled above"),
            };
            let target_filter = TargetFilter::One(effective_target);
            collect_matching_tasks(&tasks, bug_id, &target_filter, &series_filter)?
        }
    };

    let mut blocks = Vec::new();
    let mut deleted = Vec::new();
    let mut failures = Vec::new();
    for task in &matched {
        let display_name = task
            .bug_target_display_name
            .clone()
            .unwrap_or_else(|| "(unknown)".to_string());
        let Some(task_url) = task.self_link.as_deref() else {
            failures.push(BugFailure::new(
                "delete_task",
                Some(display_name),
                None,
                LpError::Other("Bug task has no self_link".into()),
            ));
            continue;
        };
        match bugs::delete_bug_task(client, task_url).await {
            Ok(()) => {
                blocks.push(ok_block(format!("Deleted bug task: {display_name}")));
                deleted.push(task_url.to_string());
            }
            Err(error) => failures.push(BugFailure::new(
                "delete_task",
                Some(display_name),
                Some(task_url.to_string()),
                error,
            )),
        }
    }

    let count = deleted.len();
    let summary_suffix = if failures.is_empty() {
        String::new()
    } else {
        format!(", {} error(s)", failures.len())
    };
    if !blocks.is_empty() {
        blocks.push(Block::Break);
    }
    blocks.push(Block::Status(
        if failures.is_empty() {
            Level::Ok
        } else {
            Level::Warning
        },
        vec![Span::Text(format!(
            "Deleted {count} task(s) on bug #{bug_id}{summary_suffix}."
        ))],
    ));
    let json = serde_json::json!({"lpcli": {
        "bug_id": bug_id,
        "deleted_task_links": deleted,
        "failures": failures_json(&failures),
    }});
    let total = matched.len();
    Ok(finish_batch(
        BugOutcome::mutation(json, blocks, Vec::new()),
        count,
        failures,
        |failures| {
            format!(
                "Deleted {count} of {total} task(s) on bug #{bug_id}; {} failed: {}",
                failures.len(),
                describe_failures(failures)
            )
        },
    ))
}

// ---------------------------------------------------------------------------
// Bug tracker registration output helpers
// ---------------------------------------------------------------------------

/// Build the output lines for `bug tracker create`.
///
/// The printed name, type and base URL come from the tracker entry
/// Launchpad returned — ensureBugTracker may return an existing tracker
/// found by base URL, silently ignoring the requested name and type — plus
/// whether the tracker was created or reused.
#[cfg(test)]
pub(crate) fn format_ensure_tracker_result(
    tracker: &bugs::BugTracker,
    created: bool,
) -> Vec<String> {
    let report = Report {
        blocks: ensure_tracker_blocks(tracker, created),
        json: serde_json::Value::Null,
        records: Records {
            columns: Vec::new(),
            rows: Vec::new(),
        },
    };
    Output::fixed(OutputMode::Rich, false, 100)
        .render(&report)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The final watch URL line printed after a successful link, for both the
/// explicit tracker form and the --github shorthand.
///
/// The URL is printed unconditionally: Launchpad derives it from the
/// tracker, so a missing URL must stay visible rather than be silently
/// skipped (mis-typed trackers generate junk URLs that otherwise only show
/// up in `bug watches`).
#[cfg(test)]
pub(crate) fn watch_url_line(watch: &bugs::BugWatch) -> String {
    match watch.url.as_deref() {
        Some(url) => format!("  URL: {url}"),
        None => "  URL: (Launchpad returned no watch URL; check `lpcli bug watches`)".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Target / series filter types and task collection helper
// ---------------------------------------------------------------------------

/// Split a comma-separated string into a trimmed, non-empty list of strings.
pub(crate) fn parse_comma_separated(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Which targets to apply a bug-task operation to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetFilter {
    /// Exactly one named target.
    One(String),
    /// An explicit set of named targets (OR logic within the set).
    Many(Vec<String>),
    /// Every target that already has a task on this bug.
    All,
}

impl TargetFilter {
    pub(crate) fn from_args(
        target: Option<String>,
        many_targets: Option<String>,
        all_targets: bool,
    ) -> Self {
        match (all_targets, many_targets, target) {
            (true, _, _) => TargetFilter::All,
            (false, Some(many), _) => TargetFilter::Many(parse_comma_separated(&many)),
            (false, None, Some(t)) => TargetFilter::One(t),
            // Clap's required ArgGroup prevents this branch; defensive fallback.
            (false, None, None) => TargetFilter::All,
        }
    }
}

/// Which Ubuntu series to apply a bug-task operation to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeriesFilter {
    /// Exactly one named series.
    One(String),
    /// An explicit set of named series (OR logic within the set).
    Many(Vec<String>),
    /// Every series that already has a task on this bug.
    All,
    /// Only series-less tasks (no Ubuntu series component in the target link).
    NoSeries,
}

impl SeriesFilter {
    pub(crate) fn from_args(
        series: Option<String>,
        many_series: Option<String>,
        all_series: bool,
        no_series: bool,
    ) -> Self {
        match (all_series, no_series, many_series, series) {
            (true, _, _, _) => SeriesFilter::All,
            (_, true, _, _) => SeriesFilter::NoSeries,
            (false, false, Some(many), _) => SeriesFilter::Many(parse_comma_separated(&many)),
            (false, false, None, Some(s)) => SeriesFilter::One(s),
            // Clap's required ArgGroup prevents this branch; defensive fallback.
            (false, false, None, None) => SeriesFilter::All,
        }
    }
}

/// Return all bug tasks that match `target_filter` AND `series_filter`.
///
/// Every explicitly named target and series is validated against what is
/// actually present on the bug before filtering, so callers receive an
/// actionable error message instead of a silent empty result.
///
/// When `SeriesFilter::All` is used, both series-specific tasks
/// (e.g. `/ubuntu/noble/+source/pkg`) and series-less tasks
/// (e.g. the distribution-level `/ubuntu/+source/pkg` row or an upstream
/// project task) are included.  `SeriesFilter::NoSeries` matches only
/// series-less tasks.
pub(crate) fn collect_matching_tasks<'a>(
    tasks: &'a [bugs::BugTask],
    bug_id: u64,
    target_filter: &TargetFilter,
    series_filter: &SeriesFilter,
) -> std::result::Result<Vec<&'a bugs::BugTask>, lpcli::error::LpError> {
    // Parse every task's target link once.
    let parsed: Vec<(&bugs::BugTask, String, Option<String>)> = tasks
        .iter()
        .map(|t| {
            let (tgt, ser) = bugs::parse_target_link(t.target_link.as_deref().unwrap_or(""));
            (t, tgt, ser)
        })
        .collect();

    // Sorted, de-duplicated lists of targets and series present on this bug —
    // used to produce helpful "did you mean…?" error messages.
    let available_targets: Vec<String> = parsed
        .iter()
        .filter(|(_, t, _)| !t.is_empty())
        .map(|(_, t, _)| t.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    let available_series: Vec<String> = parsed
        .iter()
        .filter_map(|(_, _, ser)| ser.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    // Validate every explicitly named target.
    match target_filter {
        TargetFilter::One(t) => {
            if !available_targets.iter().any(|a| a.eq_ignore_ascii_case(t)) {
                return Err(lpcli::error::LpError::LocalNotFound(format!(
                    "Target '{t}' not found on bug #{bug_id}. \
                     Available targets: {}.",
                    available_targets.join(", ")
                )));
            }
        }
        TargetFilter::Many(ts) => {
            let missing: Vec<&str> = ts
                .iter()
                .filter(|t| !available_targets.iter().any(|a| a.eq_ignore_ascii_case(t)))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                return Err(lpcli::error::LpError::LocalNotFound(format!(
                    "Targets not found on bug #{bug_id}: {}. \
                     Available targets: {}.",
                    missing.join(", "),
                    available_targets.join(", ")
                )));
            }
        }
        TargetFilter::All => {}
    }

    // Validate every explicitly named series.
    match series_filter {
        SeriesFilter::One(s) => {
            if !available_series.iter().any(|a| a.eq_ignore_ascii_case(s)) {
                return Err(lpcli::error::LpError::LocalNotFound(format!(
                    "Series '{s}' not found on bug #{bug_id}. \
                     Available series: {}.",
                    available_series.join(", ")
                )));
            }
        }
        SeriesFilter::Many(ss) => {
            let missing: Vec<&str> = ss
                .iter()
                .filter(|s| !available_series.iter().any(|a| a.eq_ignore_ascii_case(s)))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                return Err(lpcli::error::LpError::LocalNotFound(format!(
                    "Series not found on bug #{bug_id}: {}. \
                     Available series: {}.",
                    missing.join(", "),
                    available_series.join(", ")
                )));
            }
        }
        SeriesFilter::All => {}
        SeriesFilter::NoSeries => {}
    }

    // Apply the combined filter (target AND series must both match).
    let matched: Vec<&bugs::BugTask> = parsed
        .iter()
        .filter(|(_, task_tgt, task_ser)| {
            let target_ok = match target_filter {
                TargetFilter::One(t) => task_tgt.eq_ignore_ascii_case(t),
                TargetFilter::Many(ts) => ts.iter().any(|t| task_tgt.eq_ignore_ascii_case(t)),
                TargetFilter::All => true,
            };
            let series_ok = match series_filter {
                SeriesFilter::One(s) => task_ser
                    .as_deref()
                    .map(|ts| ts.eq_ignore_ascii_case(s))
                    .unwrap_or(false),
                SeriesFilter::Many(ss) => task_ser
                    .as_deref()
                    .map(|ts| ss.iter().any(|s| ts.eq_ignore_ascii_case(s)))
                    .unwrap_or(false),
                SeriesFilter::All => true,
                SeriesFilter::NoSeries => task_ser.is_none(),
            };
            target_ok && series_ok
        })
        .map(|(t, _, _)| *t)
        .collect();

    if matched.is_empty() {
        return Err(lpcli::error::LpError::LocalNotFound(format!(
            "No tasks found matching the given target and series filters on bug #{bug_id}."
        )));
    }

    Ok(matched)
}

/// Find a single bug task from a list of tasks matching the given criteria.
///
/// If `task_url` is provided, matches directly by `self_link`.
/// Otherwise, filters by `target` (matching package name, distro in URL, or display name),
/// `package`, and `series`.
///
/// Without any selector, the bug must have exactly one task: a mutation is
/// never applied to a task chosen among several (for example a series-less
/// Debian task over an Ubuntu release task). When `target` or `package`
/// narrows the candidates and `series` was not specified, a single
/// series-less task (e.g. distribution-level or upstream) is preferred.
pub(crate) fn find_task_to_link<'a>(
    tasks: &'a [bugs::BugTask],
    bug_id: u64,
    selector: &TaskLinkSelector,
) -> lpcli::error::Result<&'a bugs::BugTask> {
    if tasks.is_empty() {
        return Err(lpcli::error::LpError::LocalNotFound(format!(
            "Bug #{bug_id} has no bug tasks."
        )));
    }

    if let Some(url) = selector.task_url.as_deref() {
        return tasks
            .iter()
            .find(|t| t.self_link.as_deref() == Some(url))
            .ok_or_else(|| {
                lpcli::error::LpError::LocalNotFound(format!(
                    "No task with self_link '{url}' found on bug #{bug_id}."
                ))
            });
    }

    let has_selector =
        selector.target.is_some() || selector.package.is_some() || selector.series.is_some();
    if !has_selector && tasks.len() > 1 {
        let names: Vec<&str> = tasks
            .iter()
            .filter_map(|t| {
                t.bug_target_display_name
                    .as_deref()
                    .or(t.target_link.as_deref())
            })
            .collect();
        return Err(lpcli::error::LpError::Other(format!(
            "Multiple matching tasks on bug #{bug_id}: {}. No task was selected or \
             modified; specify --target, --package, --series, or --task-url.",
            names.join(", ")
        )));
    }

    // Filter tasks based on target, package, and series.
    let mut candidates: Vec<&bugs::BugTask> = tasks
        .iter()
        .filter(|t| {
            let target_link = t.target_link.as_deref().unwrap_or("");
            let (task_target, task_series) = bugs::parse_target_link(target_link);
            let display_name = t.bug_target_display_name.as_deref().unwrap_or("");

            // Series check
            if let Some(ser) = selector.series.as_deref() {
                let matches_series = task_series
                    .as_deref()
                    .map(|ts| ts.eq_ignore_ascii_case(ser))
                    .unwrap_or(false);
                if !matches_series {
                    return false;
                }
            }

            // Package check
            if let Some(pkg) = selector.package.as_deref() {
                let matches_pkg = task_target.eq_ignore_ascii_case(pkg)
                    || target_link
                        .to_ascii_lowercase()
                        .contains(&format!("/+source/{}", pkg.to_ascii_lowercase()));
                if !matches_pkg {
                    return false;
                }
            }

            // Target check: matches target name, distro in target_link, or substring in display_name.
            if let Some(tgt) = selector.target.as_deref() {
                let tgt_lower = tgt.to_ascii_lowercase();
                let matches_target = task_target.eq_ignore_ascii_case(tgt)
                    || target_link
                        .to_ascii_lowercase()
                        .starts_with(&format!("https://api.launchpad.net/devel/{tgt_lower}"))
                    || target_link
                        .to_ascii_lowercase()
                        .starts_with(&format!("/{tgt_lower}"))
                    || target_link
                        .to_ascii_lowercase()
                        .contains(&format!("/{tgt_lower}/"))
                    || display_name.to_ascii_lowercase().contains(&tgt_lower);
                if !matches_target {
                    return false;
                }
            }

            true
        })
        .collect();

    if candidates.is_empty() {
        let available: Vec<String> = tasks
            .iter()
            .filter_map(|t| {
                t.bug_target_display_name
                    .as_deref()
                    .or(t.target_link.as_deref())
            })
            .map(|s| s.to_string())
            .collect();
        return Err(lpcli::error::LpError::LocalNotFound(format!(
            "No task matching criteria on bug #{bug_id}. Available tasks: {}.",
            if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            }
        )));
    }

    // If no series was specified and multiple candidates matched, prefer series-less tasks
    // (e.g. the distro-level /debian/+source/pkg or upstream task).
    if selector.series.is_none() && candidates.len() > 1 {
        let series_less: Vec<&bugs::BugTask> = candidates
            .iter()
            .copied()
            .filter(|t| {
                let (_, s) = bugs::parse_target_link(t.target_link.as_deref().unwrap_or(""));
                s.is_none()
            })
            .collect();
        if series_less.len() == 1 {
            return Ok(series_less[0]);
        } else if !series_less.is_empty() {
            candidates = series_less;
        }
    }

    if candidates.len() == 1 {
        Ok(candidates[0])
    } else {
        let names: Vec<String> = candidates
            .iter()
            .filter_map(|t| {
                t.bug_target_display_name
                    .as_deref()
                    .or(t.target_link.as_deref())
            })
            .map(|s| s.to_string())
            .collect();
        Err(lpcli::error::LpError::Other(format!(
            "Multiple matching tasks on bug #{bug_id}: {}. Please specify --target, --package, or --series to disambiguate.",
            names.join(", ")
        )))
    }
}

pub(crate) fn remote_link_failure(
    series: &str,
    tracker: &str,
    remote_bug: &str,
    reason: &str,
) -> String {
    format!("series '{series}', tracker '{tracker}', remote bug '{remote_bug}': {reason}")
}

pub(crate) fn remote_link_failure_error(
    bug_id: u64,
    failures: &[String],
) -> Option<lpcli::error::LpError> {
    if failures.is_empty() {
        return None;
    }

    Some(lpcli::error::LpError::Other(format!(
        "Failed to link remote bugs for one or more tasks on bug #{bug_id}; \
         task creation may already have succeeded: {}",
        failures.join("; ")
    )))
}

/// Parse a list of tags from a `Vec<String>` where each element may contain
/// comma-separated values (e.g. `"homestar,"` or `"homestar, cheat, strongbad"`).
///
/// This handles all three user-facing input forms:
/// * Space-separated on the command line: `--add-many-tags homestar cheat strongbad`
///   → each word is a separate element already, just needs trailing comma stripping.
/// * Quoted comma-separated: `--add-many-tags "homestar, cheat, strongbad"`
///   → a single element containing commas that must be split.
/// * Shell-split with trailing commas: `--add-many-tags homestar, cheat, strongbad`
///   → elements like `"homestar,"` that need trimming.
pub(crate) fn parse_tags_from_vec(args: &[String]) -> Vec<String> {
    args.iter()
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Resolve the new bug description from `--description` or
/// `--description-file`.
///
/// Exactly one of the two must be supplied (enforced by clap). The resolved
/// text is rejected when it is empty or whitespace-only: Launchpad does not
/// accept an empty bug description, and an empty pipe or file silently
/// wiping the current description would be an easy mistake to make.
pub(crate) fn resolve_new_description(
    description: Option<String>,
    description_file: Option<&str>,
) -> lpcli::error::Result<String> {
    let text = match (description, description_file) {
        (Some(text), _) => text,
        (None, Some(path)) => read_description_file(path)?,
        (None, None) => unreachable!("clap requires --description or --description-file"),
    };
    if text.trim().is_empty() {
        return Err(lpcli::error::LpError::Other(
            "the new bug description must not be empty".into(),
        ));
    }
    Ok(text)
}

/// Read the new bug description from the file given by `--description-file`.
///
/// A path of `-` reads the description from standard input, mirroring
/// `gh issue edit --body-file -`.
pub(crate) fn read_description_file(path: &str) -> lpcli::error::Result<String> {
    if path == "-" {
        read_description_from_reader(std::io::stdin()).map_err(|e| {
            lpcli::error::LpError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read description from standard input: {e}"),
            ))
        })
    } else {
        std::fs::read_to_string(path).map_err(|e| {
            lpcli::error::LpError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read description file '{}': {e}", path),
            ))
        })
    }
}

/// Read description text from an arbitrary reader.
///
/// Separated from [`read_description_file`] so the stdin path can be unit
/// tested with a [`std::io::Cursor`] stand-in.
pub(crate) fn read_description_from_reader<R: std::io::Read>(
    mut reader: R,
) -> std::io::Result<String> {
    let mut buf = String::new();
    reader.read_to_string(&mut buf)?;
    Ok(buf)
}

/// Remove tags from `current_tags` according to the supplied remove flags.
///
/// `remove_all_tags` takes precedence: when `true`, the result is always empty.
/// Otherwise, `remove_tag` (single) or `remove_many_tags` (multiple) specify
/// which tags to drop (case-insensitive comparison).
pub(crate) fn apply_tag_removals(
    current_tags: Vec<String>,
    remove_tag: Option<String>,
    remove_many_tags: Option<Vec<String>>,
    remove_all_tags: bool,
) -> Vec<String> {
    if remove_all_tags {
        return Vec::new();
    }
    let to_remove: Vec<String> = match (remove_tag, remove_many_tags) {
        (Some(t), _) => vec![t],
        (None, Some(many)) => parse_tags_from_vec(&many),
        (None, None) => return current_tags,
    };
    current_tags
        .into_iter()
        .filter(|t| !to_remove.iter().any(|r| r.eq_ignore_ascii_case(t)))
        .collect()
}

/// Add tags to `current_tags` according to the supplied add flags.
///
/// `add_tag` (single) or `add_many_tags` (multiple) specify which tags to add.
/// Tags that already exist in `current_tags` (case-insensitive) are not
/// duplicated.
pub(crate) fn apply_tag_additions(
    mut current_tags: Vec<String>,
    add_tag: Option<String>,
    add_many_tags: Option<Vec<String>>,
) -> Vec<String> {
    let to_add: Vec<String> = match (add_tag, add_many_tags) {
        (Some(t), _) => vec![t],
        (None, Some(many)) => parse_tags_from_vec(&many),
        (None, None) => return current_tags,
    };
    for tag in to_add {
        if !tag.is_empty() && !current_tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            current_tags.push(tag);
        }
    }
    current_tags
}

/// Every bug read and mutation prints contract JSON.
pub(crate) fn bug_supports_json(command: &BugCommand) -> bool {
    match command {
        BugCommand::Show { .. }
        | BugCommand::Tasks { .. }
        | BugCommand::Search { .. }
        | BugCommand::AddComment { .. }
        | BugCommand::Comments { .. }
        | BugCommand::DeleteComment { .. }
        | BugCommand::Create { .. }
        | BugCommand::SetDescription { .. }
        | BugCommand::SetStatus { .. }
        | BugCommand::SetImportance { .. }
        | BugCommand::SetAssignee { .. }
        | BugCommand::Subscribe { .. }
        | BugCommand::Unsubscribe { .. }
        | BugCommand::Subscriptions { .. }
        | BugCommand::Attachments { .. }
        | BugCommand::AddTask { .. }
        | BugCommand::DeleteTask { .. }
        | BugCommand::Tag { .. }
        | BugCommand::LinkRemote { .. }
        | BugCommand::UnlinkRemote { .. }
        | BugCommand::Watches { .. }
        | BugCommand::RemoteBugTrackers { .. }
        | BugCommand::Tracker { .. } => true,
    }
}

/// TSV is only available for list-like reads (`show` is a single bug).
pub(crate) fn bug_supports_tsv(command: &BugCommand) -> bool {
    matches!(
        command,
        BugCommand::Tasks { .. }
            | BugCommand::Search { .. }
            | BugCommand::Comments { .. }
            | BugCommand::Subscriptions { .. }
            | BugCommand::Attachments { .. }
            | BugCommand::Watches { .. }
            | BugCommand::RemoteBugTrackers { .. }
    )
}
