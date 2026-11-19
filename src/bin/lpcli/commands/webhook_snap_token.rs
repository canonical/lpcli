//! Webhook, snap and access-token command handlers.
//!
//! Every read prints Launchpad's JSON unchanged in `json` mode, and every
//! list also supports `tsv`. Mutations that create a Launchpad entry print
//! that entry; `delete`, `revoke`, and `issue` have no entry to print, so they
//! print an lpcli-defined object under `lpcli`.

use crate::*;

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// What a webhook, snap, or access-token command writes to stdout.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CommandResult {
    /// A read, printed in the selected output mode.
    Read(Report),
    /// A mutation, printed in the selected output mode, or with `--quiet` as
    /// just `quiet`: the resulting entry's `self_link`, the issued secret,
    /// or nothing.
    Mutation {
        report: Report,
        quiet: Option<String>,
        sensitive: bool,
    },
}

impl CommandResult {
    /// The text written to stdout.
    pub(crate) fn render(&self, output: &Output, quiet: bool) -> String {
        match self {
            Self::Mutation { quiet: line, .. } if quiet => line
                .as_deref()
                .map(|line| format!("{}\n", terminal_safe(line)))
                .unwrap_or_default(),
            Self::Read(report) | Self::Mutation { report, .. } => output.render(report),
        }
    }

    pub(crate) fn print(&self, output: &Output, quiet: bool) -> lpcli::error::Result<()> {
        match self {
            Self::Mutation {
                sensitive: true, ..
            } if quiet => output
                .write_raw(&self.render(output, true))
                .map_err(|err| {
                    lpcli::error::LpError::Partial(format!(
                        "Launchpad issued an access token, but writing the secret to stdout failed: {err}."
                    ))
                }),
            Self::Mutation {
                report,
                sensitive: true,
                ..
            } => output.write_raw(&output.render(report)).map_err(|err| {
                lpcli::error::LpError::Partial(format!(
                    "Launchpad issued an access token, but writing the secret to stdout failed: {err}."
                ))
            }),
            Self::Mutation { .. } if quiet => output.write_raw(&self.render(output, true)),
            Self::Read(report) | Self::Mutation { report, .. } => output.print(report),
        }
    }
}

/// Replace control characters so a Launchpad value printed on its own line
/// cannot inject terminal escape sequences or split the line.
fn terminal_safe(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
        .collect()
}

fn column(label: &'static str, key: &'static str) -> Column {
    Column { label, key }
}

fn list_report(
    noun: Noun,
    empty: &str,
    hint: Option<&str>,
    columns: Vec<Column>,
    rows: Vec<Vec<Option<String>>>,
    raw: &serde_json::Value,
) -> CommandResult {
    CommandResult::Read(Report {
        blocks: vec![Block::List {
            noun,
            empty: empty.into(),
            hint: hint.map(str::to_string),
        }],
        json: raw.clone(),
        records: Records { columns, rows },
    })
}

/// A mutation whose result is a Launchpad entry: JSON is the entry
/// unchanged, and `--quiet` prints its `self_link`.
fn entry_mutation<T>(entry: &Raw<T>, blocks: Vec<Block>) -> CommandResult {
    CommandResult::Mutation {
        quiet: entry
            .raw
            .get("self_link")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        report: Report {
            blocks,
            ..Report::json(entry.raw.clone())
        },
        sensitive: false,
    }
}

/// A mutation without a Launchpad entry: JSON is an lpcli-defined object and
/// `--quiet` prints nothing.
fn lpcli_mutation(blocks: Vec<Block>, lpcli: serde_json::Value) -> CommandResult {
    CommandResult::Mutation {
        quiet: None,
        report: Report {
            blocks,
            ..Report::json(serde_json::json!({ "lpcli": lpcli }))
        },
        sensitive: false,
    }
}

fn ok(text: &str) -> Block {
    Block::Status(Level::Ok, vec![Span::Text(text.into())])
}

fn day(timestamp: Option<jiff::Timestamp>) -> Option<String> {
    output::format_date(timestamp)
}

// ---------------------------------------------------------------------------
// Webhook handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_webhook(
    cmd: WebhookCommand,
    output: &Output,
    quiet: bool,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = authenticated_client(cache_timeout)?;
    webhook_result(&client, cmd).await?.print(output, quiet)
}

pub(crate) async fn webhook_result(
    client: &LaunchpadClient,
    cmd: WebhookCommand,
) -> lpcli::error::Result<CommandResult> {
    Ok(match cmd {
        WebhookCommand::List { target } => {
            webhooks_report(&webhooks::list_target_webhooks_raw(client, &target).await?)
        }

        WebhookCommand::Create {
            target,
            delivery_url,
            event_types,
            inactive,
            secret,
        } => {
            let types: Vec<&str> = event_types.split(',').map(str::trim).collect();
            let hook = webhooks::create_webhook_raw(
                client,
                &target,
                &delivery_url,
                &types,
                !inactive,
                secret.as_deref(),
            )
            .await?;
            webhook_created_report(&hook)
        }

        WebhookCommand::Delete { webhook_url } => {
            webhooks::delete_webhook(client, &webhook_url).await?;
            webhook_deleted_report(&webhook_url)
        }

        WebhookCommand::Ping { webhook_url } => {
            webhook_pinged_report(&webhooks::ping_webhook_raw(client, &webhook_url).await?)
        }

        WebhookCommand::Deliveries { webhook_url } => {
            deliveries_report(&webhooks::list_deliveries_raw(client, &webhook_url).await?)
        }
    })
}

fn webhooks_report(hooks: &Raw<Vec<webhooks::Webhook>>) -> CommandResult {
    let rows = hooks
        .value
        .iter()
        .map(|hook| {
            vec![
                hook.delivery_url.clone(),
                hook.active.map(|active| active.to_string()),
                hook.event_types.as_ref().map(|types| types.join(", ")),
                day(hook.date_created),
                hook.self_link.clone(),
            ]
        })
        .collect();
    list_report(
        Noun {
            singular: "webhook",
            plural: "webhooks",
        },
        "No webhooks found.",
        Some("Use the Webhook URL value with --webhook-url."),
        vec![
            column("Delivery URL", "delivery_url"),
            column("Active", "active"),
            column("Events", "event_types"),
            column("Created", "date_created"),
            column("Webhook URL", "self_link"),
        ],
        rows,
        &hooks.raw,
    )
}

fn deliveries_report(deliveries: &Raw<Vec<webhooks::WebhookDelivery>>) -> CommandResult {
    let rows = deliveries
        .value
        .iter()
        .map(|delivery| {
            vec![
                output::format_datetime(delivery.date_sent),
                delivery.successful.map(|successful| successful.to_string()),
                delivery.response_status_code.map(|code| code.to_string()),
                delivery.pending.map(|pending| pending.to_string()),
            ]
        })
        .collect();
    list_report(
        Noun {
            singular: "delivery",
            plural: "deliveries",
        },
        "No deliveries found.",
        None,
        vec![
            column("Sent", "date_sent"),
            column("Successful", "successful"),
            column("Status Code", "response_status_code"),
            column("Pending", "pending"),
        ],
        rows,
        &deliveries.raw,
    )
}

fn webhook_created_report(hook: &Raw<webhooks::Webhook>) -> CommandResult {
    let mut blocks = vec![ok("Webhook created.")];
    if let Some(link) = &hook.value.self_link {
        blocks.push(Block::Field("Webhook URL", link.clone()));
    }
    entry_mutation(hook, blocks)
}

fn webhook_pinged_report(delivery: &Raw<webhooks::WebhookDelivery>) -> CommandResult {
    let mut blocks = vec![ok("Test delivery sent.")];
    if let Some(status) = delivery.value.response_status_code {
        blocks.push(Block::Field("Response status", status.to_string()));
    }
    if let Some(successful) = delivery.value.successful {
        blocks.push(Block::Field("Successful", successful.to_string()));
    }
    entry_mutation(delivery, blocks)
}

pub(crate) fn webhook_deleted_report(webhook_url: &str) -> CommandResult {
    lpcli_mutation(
        vec![
            ok("Webhook deleted."),
            Block::Field("Webhook URL", webhook_url.into()),
        ],
        serde_json::json!({"deleted": true, "webhook_link": webhook_url}),
    )
}

// ---------------------------------------------------------------------------
// Snap handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_snap(
    cmd: SnapCommand,
    output: &Output,
    quiet: bool,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    snap_result(&client, cmd).await?.print(output, quiet)
}

pub(crate) async fn snap_result(
    client: &LaunchpadClient,
    cmd: SnapCommand,
) -> lpcli::error::Result<CommandResult> {
    Ok(match cmd {
        SnapCommand::Show { owner, name } => {
            snap_report(&snaps::get_snap_raw(client, &owner, &name).await?, &name)
        }

        SnapCommand::Find { owner } => {
            snaps_report(&snaps::find_snaps_by_owner_raw(client, &owner).await?)
        }

        SnapCommand::Builds { owner, name } => {
            snap_builds_report(&snaps::get_snap_pending_builds_raw(client, &owner, &name).await?)
        }

        SnapCommand::RequestBuilds {
            owner,
            name,
            pocket,
            distro,
        } => {
            let archive_url = client.url(&format!("/{distro}/+archive/primary"));
            let request = snaps::request_snap_builds_raw(
                client,
                &owner,
                &name,
                &archive_url,
                pocket_launchpad_name(pocket),
            )
            .await?;
            snap_builds_requested_report(&request)
        }
    })
}

fn snap_report(snap: &Raw<snaps::Snap>, requested_name: &str) -> CommandResult {
    let value = &snap.value;
    let mut blocks = vec![Block::Heading(
        value.name.as_deref().unwrap_or(requested_name).into(),
    )];
    if let Some(description) = &value.description {
        blocks.push(Block::Note(vec![Span::Text(description.clone())]));
    }
    let fields = [
        ("Store name", value.store_name.clone()),
        ("Private", value.private.map(|private| private.to_string())),
        (
            "Auto-upload",
            value.store_upload.map(|upload| upload.to_string()),
        ),
        ("Git URL", value.git_repository_url.clone()),
        ("Git path", value.git_path.clone()),
        ("URL", value.web_link.clone()),
    ];
    blocks.extend(
        fields
            .into_iter()
            .filter_map(|(label, value)| value.map(|value| Block::Field(label, value))),
    );
    CommandResult::Read(Report {
        blocks,
        ..Report::json(snap.raw.clone())
    })
}

fn snaps_report(snaps: &Raw<Vec<snaps::Snap>>) -> CommandResult {
    let rows = snaps
        .value
        .iter()
        .map(|snap| {
            vec![
                snap.name.clone(),
                snap.store_name.clone(),
                snap.private.map(|private| private.to_string()),
            ]
        })
        .collect();
    list_report(
        Noun {
            singular: "snap recipe",
            plural: "snap recipes",
        },
        "No snap recipes found.",
        None,
        vec![
            column("Name", "name"),
            column("Store Name", "store_name"),
            column("Private", "private"),
        ],
        rows,
        &snaps.raw,
    )
}

fn snap_builds_report(builds: &Raw<Vec<snaps::SnapBuild>>) -> CommandResult {
    let rows = builds
        .value
        .iter()
        .map(|build| {
            vec![
                build.title.clone(),
                build.pocket.clone(),
                build.score.map(|score| score.to_string()),
                build.store_upload_status.clone(),
            ]
        })
        .collect();
    list_report(
        Noun {
            singular: "pending build",
            plural: "pending builds",
        },
        "No pending builds found.",
        None,
        vec![
            column("Title", "title"),
            column("Pocket", "pocket"),
            column("Score", "score"),
            column("Upload Status", "store_upload_status"),
        ],
        rows,
        &builds.raw,
    )
}

fn snap_builds_requested_report(request: &Raw<snaps::SnapBuildRequest>) -> CommandResult {
    let mut blocks = vec![ok("Build request submitted.")];
    if let Some(status) = &request.value.status {
        blocks.push(Block::Field("Status", status.clone()));
    }
    if let Some(link) = &request.value.self_link {
        blocks.push(Block::Field("Request URL", link.clone()));
    }
    entry_mutation(request, blocks)
}

// ---------------------------------------------------------------------------
// AccessToken handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_access_token(
    cmd: AccessTokenCommand,
    output: &Output,
    quiet: bool,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = authenticated_client(cache_timeout)?;
    access_token_result(&client, cmd)
        .await?
        .print(output, quiet)
}

pub(crate) async fn access_token_result(
    client: &LaunchpadClient,
    cmd: AccessTokenCommand,
) -> lpcli::error::Result<CommandResult> {
    Ok(match cmd {
        AccessTokenCommand::Issue {
            project,
            description,
            scopes,
        } => {
            let scope_list: Vec<&str> = scopes.split(',').map(str::trim).collect();
            let secret = access_tokens::issue_project_access_token(
                client,
                &project,
                &description,
                &scope_list,
            )
            .await?;
            access_token_issued_report(secret)
        }

        AccessTokenCommand::IssueGit {
            repo,
            description,
            scopes,
        } => {
            let scope_list: Vec<&str> = scopes.split(',').map(str::trim).collect();
            let secret =
                access_tokens::issue_git_access_token(client, &repo, &description, &scope_list)
                    .await?;
            access_token_issued_report(secret)
        }

        AccessTokenCommand::List { project } => access_tokens_report(
            &access_tokens::list_project_access_tokens_raw(client, &project).await?,
        ),

        AccessTokenCommand::ListGit { repo } => {
            access_tokens_report(&access_tokens::list_git_access_tokens_raw(client, &repo).await?)
        }

        AccessTokenCommand::Revoke { token_url } => {
            access_tokens::revoke_access_token(client, &token_url).await?;
            lpcli_mutation(
                vec![
                    ok("Access token revoked."),
                    Block::Field("Token URL", token_url.clone()),
                ],
                serde_json::json!({"revoked": true, "access_token_link": token_url}),
            )
        }
    })
}

/// Launchpad returns only the plaintext secret, which is the whole result:
/// JSON wraps it in an lpcli-defined object, and `--quiet` still prints it,
/// because there is no `self_link` and the secret cannot be read again.
pub(crate) fn access_token_issued_report(secret: String) -> CommandResult {
    CommandResult::Mutation {
        report: Report {
            blocks: vec![
                ok("Access token issued."),
                Block::Status(
                    Level::Warning,
                    vec![Span::Text(
                        "Save this secret; it will not be shown again.".into(),
                    )],
                ),
                Block::Field("Secret", secret.clone()),
            ],
            ..Report::json(serde_json::json!({"lpcli": {"secret": secret}}))
        },
        quiet: Some(secret),
        sensitive: true,
    }
}

fn access_tokens_report(tokens: &Raw<Vec<access_tokens::AccessToken>>) -> CommandResult {
    let rows = tokens
        .value
        .iter()
        .map(|token| {
            vec![
                token.description.clone(),
                token.scopes.as_ref().map(|scopes| scopes.join(", ")),
                day(token.date_created),
                day(token.date_last_used),
                token.self_link.clone(),
            ]
        })
        .collect();
    list_report(
        Noun {
            singular: "access token",
            plural: "access tokens",
        },
        "No access tokens found.",
        Some("Use the Token URL value with --token-url."),
        vec![
            column("Description", "description"),
            column("Scopes", "scopes"),
            column("Created", "date_created"),
            column("Last Used", "date_last_used"),
            column("Token URL", "self_link"),
        ],
        rows,
        &tokens.raw,
    )
}

// ---------------------------------------------------------------------------
// Structured output support
// ---------------------------------------------------------------------------

/// Every webhook, snap, and access-token command has JSON output.
pub(crate) fn webhook_snap_token_supports_json(command: &Command) -> bool {
    matches!(
        command,
        Command::Webhook(_) | Command::Snap(_) | Command::AccessToken(_)
    )
}

/// The list commands have one-record-per-line TSV output.
pub(crate) fn webhook_snap_token_supports_tsv(command: &Command) -> bool {
    matches!(
        command,
        Command::Webhook(WebhookCommand::List { .. } | WebhookCommand::Deliveries { .. })
            | Command::Snap(SnapCommand::Find { .. } | SnapCommand::Builds { .. })
            | Command::AccessToken(
                AccessTokenCommand::List { .. } | AccessTokenCommand::ListGit { .. }
            )
    )
}
