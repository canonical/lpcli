//! Read-mostly domain handlers: person, package, project, CVE, spec,
//! question and translation.
//!
//! Each command builds a [`Report`] from the library's `_raw` functions, so
//! `--format json` prints Launchpad's JSON unchanged: one entry for `show`
//! commands, and Launchpad's collection shape for lists. The typed values
//! drive the `rich`, `plain`, and (for lists) `tsv` output.

use crate::*;

use lpcli::error::LpError;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Report helpers
// ---------------------------------------------------------------------------

fn column(label: &'static str, key: &'static str) -> Column {
    Column { label, key }
}

fn noun(singular: &'static str, plural: &'static str) -> Noun {
    Noun { singular, plural }
}

fn no_records() -> Records {
    Records {
        columns: Vec::new(),
        rows: Vec::new(),
    }
}

/// A single Launchpad entry: its JSON is printed unchanged, and the present
/// fields are shown under a heading.
pub(crate) fn entry_report(
    json: Value,
    heading: String,
    fields: Vec<(&'static str, Option<String>)>,
) -> Report {
    let mut blocks = vec![Block::Heading(heading)];
    blocks.extend(
        fields
            .into_iter()
            .filter_map(|(label, value)| value.map(|value| Block::Field(label, value))),
    );
    Report {
        blocks,
        json,
        records: no_records(),
    }
}

/// A Launchpad collection: its JSON is printed unchanged, and its records
/// are shown as a table (`rich`), labeled records (`plain`), or `tsv`.
fn list_report(
    json: Value,
    noun: Noun,
    empty: &str,
    columns: Vec<Column>,
    rows: Vec<Vec<Option<String>>>,
) -> Report {
    Report {
        blocks: vec![Block::List {
            noun,
            empty: empty.into(),
            hint: None,
        }],
        json,
        records: Records { columns, rows },
    }
}

/// The raw entries of a Launchpad collection.
fn raw_entries(collection: &Value) -> &[Value] {
    collection
        .get("entries")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn flag(value: Option<bool>) -> Option<String> {
    value.map(|value| value.to_string())
}

fn day(value: Option<jiff::Timestamp>) -> Option<String> {
    value.map(|value| value.strftime("%Y-%m-%d").to_string())
}

/// The final path segment of a Launchpad link, without a leading `~`, e.g.
/// `https://api.launchpad.net/devel/~jdoe` -> `jdoe`.
fn link_name(link: Option<&str>) -> Option<String> {
    link.and_then(|url| url.rsplit('/').next())
        .map(|segment| segment.trim_start_matches('~'))
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
}

/// Whether an account is a person or a team. Launchpad sends `is_team`;
/// `account_type` is used when present.
fn person_type(person: &people::Person, raw: &Value) -> Option<String> {
    person
        .account_type
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| {
            raw.get("is_team")
                .and_then(Value::as_bool)
                .map(|team| if team { "Team" } else { "Person" }.to_string())
        })
}

fn people_report(found: &Raw<Vec<people::Person>>, noun: Noun, empty: &str) -> Report {
    let rows = found
        .value
        .iter()
        .zip(
            raw_entries(&found.raw)
                .iter()
                .chain(std::iter::repeat(&Value::Null)),
        )
        .map(|(person, raw)| {
            vec![
                Some(person.name.clone()),
                Some(person.display_name.clone()),
                person_type(person, raw),
            ]
        })
        .collect();
    list_report(
        found.raw.clone(),
        noun,
        empty,
        vec![
            column("Name", "name"),
            column("Display Name", "display_name"),
            column("Type", "type"),
        ],
        rows,
    )
}

fn cve_columns() -> Vec<Column> {
    vec![
        column("CVE", "sequence"),
        column("Status", "status"),
        column("Title", "title"),
    ]
}

fn cve_row(cve: &cves::Cve) -> Vec<Option<String>> {
    vec![
        Some(cve.sequence.clone()),
        cve.status.clone(),
        cve.title.clone(),
    ]
}

/// The `cve search` collection. The keyword filter runs locally, so no
/// Launchpad link can continue the filtered list: `next_collection_link` is
/// never included, and `total_size` only when every CVE was checked.
pub(crate) fn cve_search_report(found: &cves::CveSearch) -> Report {
    let mut json = serde_json::Map::new();
    if found.complete {
        json.insert("total_size".into(), found.cves.len().into());
    }
    json.insert("start".into(), 0.into());
    json.insert(
        "entries".into(),
        found
            .cves
            .iter()
            .map(|cve| cve.raw.clone())
            .collect::<Vec<_>>()
            .into(),
    );
    list_report(
        json.into(),
        noun("CVE", "CVEs"),
        "No CVEs found.",
        cve_columns(),
        found.cves.iter().map(|cve| cve_row(&cve.value)).collect(),
    )
}

fn source_columns() -> Vec<Column> {
    vec![
        column("Package", "source_package_name"),
        column("Version", "source_package_version"),
        column("Component", "component_name"),
        column("Pocket", "pocket"),
        column("Status", "status"),
    ]
}

fn sources_report(page: &Raw<Collection<packages::SourcePackagePublishingHistory>>) -> Report {
    let rows = page
        .value
        .entries
        .iter()
        .map(|p| {
            vec![
                p.source_package_name.clone(),
                p.source_package_version.clone(),
                p.component_name.clone(),
                p.pocket.clone(),
                p.status.clone(),
            ]
        })
        .collect();
    list_report(
        page.raw.clone(),
        noun("source publication", "source publications"),
        "No source publications found.",
        source_columns(),
        rows,
    )
}

// ---------------------------------------------------------------------------
// Person handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_person(
    cmd: PersonCommand,
    output: &Output,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    output.print(&person_report(&client, cmd).await?)
}

pub(crate) async fn person_report(
    client: &LaunchpadClient,
    cmd: PersonCommand,
) -> lpcli::error::Result<Report> {
    Ok(match cmd {
        PersonCommand::Show { name } => {
            let found = people::get_person_raw(client, &name).await?;
            let person = &found.value;
            entry_report(
                found.raw.clone(),
                person.display_name.clone(),
                vec![
                    ("Name", Some(person.name.clone())),
                    ("Type", person_type(person, &found.raw)),
                    ("Karma", person.karma.map(|karma| karma.to_string())),
                    ("Description", person.description.clone()),
                    ("URL", person.web_link.clone()),
                ],
            )
        }

        PersonCommand::Search { query } => people_report(
            &people::search_people_raw(client, &query).await?,
            noun("person", "people"),
            "No people found.",
        ),

        PersonCommand::Members { team } => {
            let found = people::get_team_members_raw(client, &team).await?;
            let rows = found
                .value
                .iter()
                .map(|m| {
                    vec![
                        link_name(m.member_link.as_deref()),
                        m.status.clone(),
                        day(m.date_joined),
                    ]
                })
                .collect();
            list_report(
                found.raw,
                noun("member", "members"),
                "No members found.",
                vec![
                    column("Member", "member"),
                    column("Status", "status"),
                    column("Since", "date_joined"),
                ],
                rows,
            )
        }

        PersonCommand::Bugs { name } => {
            let found = people::get_person_bugs_raw(client, &name).await?;
            let rows = found
                .value
                .iter()
                .map(|b| {
                    vec![
                        link_name(b.bug_link.as_deref()),
                        b.title.clone(),
                        b.status.clone(),
                        b.importance.clone(),
                    ]
                })
                .collect();
            list_report(
                found.raw,
                noun("bug task", "bug tasks"),
                "No bug tasks found.",
                vec![
                    column("Bug", "id"),
                    column("Title", "title"),
                    column("Status", "status"),
                    column("Importance", "importance"),
                ],
                rows,
            )
        }

        PersonCommand::Ppas { name } => {
            let found = people::list_person_ppas_raw(client, &name).await?;
            let rows = found
                .value
                .iter()
                .map(|p| vec![p.name.clone(), p.description.clone(), flag(p.enabled)])
                .collect();
            list_report(
                found.raw,
                noun("PPA", "PPAs"),
                "No PPAs found.",
                vec![
                    column("Name", "name"),
                    column("Description", "description"),
                    column("Enabled", "enabled"),
                ],
                rows,
            )
        }

        PersonCommand::OwnedTeams { name } => people_report(
            &people::get_person_owned_teams_raw(client, &name).await?,
            noun("team", "teams"),
            "No owned teams found.",
        ),
    })
}

// ---------------------------------------------------------------------------
// Package handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_package(
    cmd: PackageCommand,
    output: &Output,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    if let PackageCommand::Download {
        name,
        version,
        series,
        distro,
        output: output_dir,
    } = cmd
    {
        let request = SourceDownloadRequest {
            series: canonical_series(&distro, &series).to_string(),
            name,
            version,
            distro,
            output_dir: output_dir.into(),
        };
        return handle_package_download(&client, output, &request).await;
    }
    output.print(&package_report(&client, cmd).await?)
}

/// Build the report for a read-only `package` command.
///
/// `package download` writes files and decides its own exit status, so it
/// is handled by [`handle_package_download()`] instead.
pub(crate) async fn package_report(
    client: &LaunchpadClient,
    cmd: PackageCommand,
) -> lpcli::error::Result<Report> {
    Ok(match cmd {
        PackageCommand::Series { series, distro } => {
            let series_name = canonical_series(&distro, &series);
            let found = packages::get_distro_series_raw(client, &distro, series_name).await?;
            let s = &found.value;
            entry_report(
                found.raw.clone(),
                s.display_name.clone().unwrap_or_else(|| s.name.clone()),
                vec![
                    ("Version", s.version.clone()),
                    ("Status", s.status.clone()),
                    ("Active", flag(s.active)),
                    ("URL", s.web_link.clone()),
                ],
            )
        }

        PackageCommand::ListSeries { distro } => {
            let found = packages::list_distro_series_raw(client, &distro).await?;
            let rows = found
                .value
                .iter()
                .map(|s| {
                    vec![
                        Some(s.name.clone()),
                        s.version.clone(),
                        s.status.clone(),
                        flag(s.active),
                    ]
                })
                .collect();
            list_report(
                found.raw,
                noun("series", "series"),
                "No series found.",
                vec![
                    column("Name", "name"),
                    column("Version", "version"),
                    column("Status", "status"),
                    column("Active", "active"),
                ],
                rows,
            )
        }

        PackageCommand::Search {
            series,
            name,
            version,
            pocket,
            distro,
            limit,
        } => {
            let series_name = canonical_series(&distro, &series);
            let params = SourceSearchParams {
                source_name: name.as_deref(),
                version: version.as_deref(),
                pocket: pocket.map(pocket_launchpad_name),
                status: Some("Published"),
                limit: Some(limit),
                ..Default::default()
            };
            sources_report(
                &packages::search_published_sources_raw(client, &distro, series_name, &params)
                    .await?,
            )
        }

        PackageCommand::Ppa { owner, ppa } => {
            let found = packages::get_ppa_raw(client, &owner, &ppa).await?;
            let archive = &found.value;
            entry_report(
                found.raw.clone(),
                archive.name.clone().unwrap_or_else(|| "PPA".into()),
                vec![
                    ("Description", archive.description.clone()),
                    ("Enabled", flag(archive.enabled)),
                    ("URL", archive.web_link.clone()),
                ],
            )
        }

        PackageCommand::PpaSources { owner, ppa, name } => {
            let params = SourceSearchParams {
                source_name: name.as_deref(),
                ..Default::default()
            };
            sources_report(&packages::list_ppa_sources_raw(client, &owner, &ppa, &params).await?)
        }

        PackageCommand::Distro { distro } => {
            let found = packages::get_distro_raw(client, &distro).await?;
            let d = &found.value;
            entry_report(
                found.raw.clone(),
                d.display_name.clone().unwrap_or_else(|| d.name.clone()),
                vec![
                    ("Title", d.title.clone()),
                    ("Summary", d.summary.clone()),
                    ("Official", flag(d.official_packages)),
                    ("URL", d.web_link.clone()),
                ],
            )
        }

        PackageCommand::Download { .. } => {
            return Err(LpError::Usage(
                "`package download` writes files and has no read-only report.".into(),
            ));
        }
    })
}

// ---------------------------------------------------------------------------
// Project handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_project(
    cmd: ProjectCommand,
    output: &Output,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    output.print(&project_report(&client, cmd).await?)
}

pub(crate) async fn project_report(
    client: &LaunchpadClient,
    cmd: ProjectCommand,
) -> lpcli::error::Result<Report> {
    Ok(match cmd {
        ProjectCommand::Show { name } => {
            let found = projects::get_project_raw(client, &name).await?;
            let project = &found.value;
            entry_report(
                found.raw.clone(),
                project
                    .display_name
                    .clone()
                    .unwrap_or_else(|| project.name.clone()),
                vec![
                    ("Name", Some(project.name.clone())),
                    ("Summary", project.summary.clone()),
                    ("Website", project.homepage_url.clone()),
                    ("Launchpad", project.web_link.clone()),
                ],
            )
        }

        ProjectCommand::Search { query } => {
            let found = projects::search_projects_raw(client, &query).await?;
            let rows = found
                .value
                .iter()
                .map(|p| {
                    vec![
                        Some(p.name.clone()),
                        p.display_name.clone(),
                        p.summary.clone(),
                    ]
                })
                .collect();
            list_report(
                found.raw,
                noun("project", "projects"),
                "No projects found.",
                vec![
                    column("Name", "name"),
                    column("Display Name", "display_name"),
                    column("Summary", "summary"),
                ],
                rows,
            )
        }

        ProjectCommand::Milestones { project, active } => {
            let found = if active {
                projects::list_active_milestones_raw(client, &project).await?
            } else {
                projects::list_milestones_raw(client, &project).await?
            };
            let rows = found
                .value
                .iter()
                .map(|m| {
                    vec![
                        Some(m.name.clone()),
                        m.title.clone(),
                        flag(m.is_active),
                        m.date_targeted.map(|date| date.to_string()),
                    ]
                })
                .collect();
            list_report(
                found.raw,
                noun("milestone", "milestones"),
                "No milestones found.",
                vec![
                    column("Name", "name"),
                    column("Title", "title"),
                    column("Active", "is_active"),
                    column("Target Date", "date_targeted"),
                ],
                rows,
            )
        }

        ProjectCommand::ShowMilestone { project, name } => {
            let found = projects::get_milestone_raw(client, &project, &name).await?;
            let m = &found.value;
            entry_report(
                found.raw.clone(),
                m.name.clone(),
                vec![
                    ("Title", m.title.clone()),
                    ("Active", flag(m.is_active)),
                    ("Target", m.date_targeted.map(|date| date.to_string())),
                    ("URL", m.web_link.clone()),
                ],
            )
        }

        ProjectCommand::SeriesShow { project, series } => {
            let found = projects::get_project_series_raw(client, &project, &series).await?;
            let s = &found.value;
            entry_report(
                found.raw.clone(),
                s.name.clone(),
                vec![
                    ("Title", s.title.clone()),
                    ("Status", s.status.clone()),
                    ("Summary", s.summary.clone()),
                    ("URL", s.web_link.clone()),
                ],
            )
        }

        ProjectCommand::ListSeries { project } => {
            let found = projects::list_project_series_raw(client, &project).await?;
            let rows = found
                .value
                .iter()
                .map(|s| vec![Some(s.name.clone()), s.title.clone(), s.status.clone()])
                .collect();
            list_report(
                found.raw,
                noun("series", "series"),
                "No series found.",
                vec![
                    column("Name", "name"),
                    column("Title", "title"),
                    column("Status", "status"),
                ],
                rows,
            )
        }

        ProjectCommand::SeriesReleases { project, series } => {
            let found = projects::list_series_releases_raw(client, &project, &series).await?;
            let rows = found
                .value
                .iter()
                .map(|r| {
                    vec![
                        r.version.clone(),
                        day(r.date_released),
                        r.release_notes.clone(),
                    ]
                })
                .collect();
            list_report(
                found.raw,
                noun("release", "releases"),
                "No releases found.",
                vec![
                    column("Version", "version"),
                    column("Release Date", "date_released"),
                    column("Notes", "release_notes"),
                ],
                rows,
            )
        }
    })
}

// ---------------------------------------------------------------------------
// CVE handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_cve(
    cmd: CveCommand,
    output: &Output,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    output.print(&cve_report(&client, cmd).await?)
}

pub(crate) async fn cve_report(
    client: &LaunchpadClient,
    cmd: CveCommand,
) -> lpcli::error::Result<Report> {
    Ok(match cmd {
        CveCommand::Show { sequence } => {
            let found = cves::get_cve_raw(client, &sequence).await?;
            let cve = &found.value;
            entry_report(
                found.raw.clone(),
                cve.sequence.clone(),
                vec![
                    ("Title", cve.title.clone()),
                    ("Status", cve.status.clone()),
                    ("Description", cve.description.clone()),
                    ("URL", cve.web_link.clone()),
                ],
            )
        }

        CveCommand::Search {
            distro,
            keyword,
            limit,
        } => {
            let params = CveSearchParams {
                in_distribution: Some(distro.as_str()),
                limit: Some(limit),
                search_text: keyword.as_deref(),
                ..Default::default()
            };
            cve_search_report(&cves::search_cves_raw(client, &params).await?)
        }

        CveCommand::BugCves { bug_id } => {
            let found = cves::get_bug_cves_raw(client, bug_id).await?;
            list_report(
                found.raw,
                noun("CVE", "CVEs"),
                "No CVEs are linked to this bug.",
                cve_columns(),
                found.value.iter().map(cve_row).collect(),
            )
        }
    })
}

// ---------------------------------------------------------------------------
// Spec handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_spec(
    cmd: SpecCommand,
    output: &Output,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    output.print(&spec_report(&client, cmd).await?)
}

pub(crate) async fn spec_report(
    client: &LaunchpadClient,
    cmd: SpecCommand,
) -> lpcli::error::Result<Report> {
    Ok(match cmd {
        SpecCommand::Show { target, name } => {
            let found = specifications::get_specification_raw(client, &target, &name).await?;
            let spec = &found.value;
            entry_report(
                found.raw.clone(),
                spec.name.clone(),
                vec![
                    ("Title", spec.title.clone()),
                    ("Priority", spec.priority.clone()),
                    ("Lifecycle", spec.lifecycle_status.clone()),
                    ("Implementation", spec.implementation_status.clone()),
                    ("Definition", spec.definition_status.clone()),
                    ("Summary", spec.summary.clone()),
                    ("URL", spec.web_link.clone()),
                ],
            )
        }

        SpecCommand::List { target, all } => {
            let found = if all {
                specifications::list_project_specifications_raw(client, &target).await?
            } else {
                specifications::list_valid_project_specifications_raw(client, &target).await?
            };
            let rows = found
                .value
                .iter()
                .map(|s| {
                    vec![
                        Some(s.name.clone()),
                        s.title.clone(),
                        s.priority.clone(),
                        s.lifecycle_status.clone(),
                        s.implementation_status.clone(),
                    ]
                })
                .collect();
            list_report(
                found.raw,
                noun("spec", "specs"),
                "No specs found.",
                vec![
                    column("Name", "name"),
                    column("Title", "title"),
                    column("Priority", "priority"),
                    column("Lifecycle", "lifecycle_status"),
                    column("Impl", "implementation_status"),
                ],
                rows,
            )
        }
    })
}

// ---------------------------------------------------------------------------
// Question handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_question(
    cmd: QuestionCommand,
    output: &Output,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    output.print(&question_report(&client, cmd).await?)
}

pub(crate) async fn question_report(
    client: &LaunchpadClient,
    cmd: QuestionCommand,
) -> lpcli::error::Result<Report> {
    Ok(match cmd {
        QuestionCommand::Show { question_id } => {
            let found = questions::get_question_by_id_raw(client, question_id).await?;
            let q = &found.value;
            entry_report(
                found.raw.clone(),
                format!("Question #{}", q.id),
                vec![
                    ("Title", q.title.clone()),
                    ("Status", q.status.clone()),
                    ("Description", q.description.clone()),
                    ("URL", q.web_link.clone()),
                ],
            )
        }

        QuestionCommand::Search {
            target,
            query,
            status,
        } => {
            let params = QuestionSearchParams {
                search_text: query.as_deref(),
                status: status.as_deref(),
            };
            let found = questions::search_questions_raw(client, &target, &params).await?;
            let rows = found
                .value
                .iter()
                .map(|q| vec![Some(q.id.to_string()), q.title.clone(), q.status.clone()])
                .collect();
            list_report(
                found.raw,
                noun("question", "questions"),
                "No questions found.",
                vec![
                    column("ID", "id"),
                    column("Title", "title"),
                    column("Status", "status"),
                ],
                rows,
            )
        }

        QuestionCommand::Messages {
            target,
            question_id,
        } => question_messages_report(
            &questions::get_question_messages_raw(client, &target, question_id).await?,
        ),
    })
}

/// Messages read as a conversation in `rich` and `plain`, and as one record
/// per message in `tsv`.
fn question_messages_report(found: &Raw<Vec<questions::QuestionMessage>>) -> Report {
    let mut blocks = Vec::new();
    let mut rows = Vec::new();
    for m in &found.value {
        let index = m.index.map(|index| index.to_string());
        let author = link_name(m.owner_link.as_deref());
        let date = m
            .date_created
            .map(|date| date.strftime("%Y-%m-%d %H:%M").to_string());
        blocks.push(Block::Heading(format!(
            "Message #{}: {}, {}",
            index.as_deref().unwrap_or("?"),
            author.as_deref().unwrap_or("unknown"),
            date.as_deref().unwrap_or("unknown date"),
        )));
        if let Some(action) = &m.action {
            blocks.push(Block::Field("Action", action.clone()));
        }
        blocks.push(Block::Note(vec![Span::Text(
            m.content.clone().unwrap_or_default(),
        )]));
        blocks.push(Block::Break);
        rows.push(vec![
            index,
            author,
            date,
            m.action.clone(),
            m.content.clone(),
        ]);
    }
    if found.value.is_empty() {
        blocks.push(Block::Note(vec![Span::Text("No messages found.".into())]));
    }
    Report {
        blocks,
        json: found.raw.clone(),
        records: Records {
            columns: vec![
                column("Index", "index"),
                column("Author", "owner"),
                column("Date", "date_created"),
                column("Action", "action"),
                column("Content", "content"),
            ],
            rows,
        },
    }
}

// ---------------------------------------------------------------------------
// Translation handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_translation(
    cmd: TranslationCommand,
    output: &Output,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    output.print(&translation_report(&client, cmd).await?)
}

pub(crate) async fn translation_report(
    client: &LaunchpadClient,
    cmd: TranslationCommand,
) -> lpcli::error::Result<Report> {
    Ok(match cmd {
        TranslationCommand::Queue { distro, series } => {
            let series_name = canonical_series(&distro, &series);
            let found =
                translations::get_distro_series_import_queue_raw(client, &distro, series_name)
                    .await?;
            let rows = found
                .value
                .iter()
                .map(|e| vec![e.path.clone(), e.status.clone(), day(e.date_created)])
                .collect();
            list_report(
                found.raw,
                noun("queue entry", "queue entries"),
                "The translation import queue is empty.",
                vec![
                    column("Path", "path"),
                    column("Status", "status"),
                    column("Date", "date_created"),
                ],
                rows,
            )
        }

        TranslationCommand::Templates { distro, series } => {
            let series_name = canonical_series(&distro, &series);
            let found =
                translations::get_distro_series_templates_raw(client, &distro, series_name).await?;
            let rows = found
                .value
                .iter()
                .map(|t| {
                    vec![
                        t.name.clone(),
                        t.path.clone(),
                        t.priority.map(|priority| priority.to_string()),
                        flag(t.is_current),
                    ]
                })
                .collect();
            list_report(
                found.raw,
                noun("template", "templates"),
                "No translation templates found.",
                vec![
                    column("Name", "name"),
                    column("Path", "path"),
                    column("Priority", "priority"),
                    column("Current", "is_current"),
                ],
                rows,
            )
        }
    })
}

// ---------------------------------------------------------------------------
// Package download handler
// ---------------------------------------------------------------------------

/// What `package download` should fetch, and where to save it.
#[derive(Debug, Clone)]
pub(crate) struct SourceDownloadRequest {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    /// The canonical series name.
    pub(crate) series: String,
    pub(crate) distro: String,
    pub(crate) output_dir: std::path::PathBuf,
}

/// The publication `package download` found, and every file outcome.
#[derive(Debug)]
pub(crate) struct SourceDownload {
    pub(crate) request: SourceDownloadRequest,
    pub(crate) publication: packages::SourcePackagePublishingHistory,
    /// The publication exactly as Launchpad returned it.
    pub(crate) publication_json: Value,
    pub(crate) urls: Vec<String>,
    /// One outcome per URL; empty when the publication has no files.
    pub(crate) outcomes: Vec<download::DownloadOutcome>,
}

impl SourceDownload {
    fn version(&self) -> &str {
        self.publication
            .source_package_version
            .as_deref()
            .unwrap_or("unknown version")
    }

    fn failures(&self) -> Vec<(&str, &str)> {
        self.outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                download::DownloadOutcome::Failed { filename, error } => {
                    Some((filename.as_str(), error.as_str()))
                }
                download::DownloadOutcome::Success(_) => None,
            })
            .collect()
    }
}

/// Download source files for a published source package, print every
/// outcome, then report the exit status (see [`package_download_result()`]).
pub(crate) async fn handle_package_download(
    client: &LaunchpadClient,
    output: &Output,
    request: &SourceDownloadRequest,
) -> lpcli::error::Result<()> {
    let found = package_download(client, request, |announcement| {
        if output.mode().is_structured() {
            Ok(())
        } else {
            output.print(announcement)
        }
    })
    .await?;
    output.print(&package_download_report(&found))?;
    package_download_result(&found)
}

/// Find the publication, list its files, and download them.
///
/// `announce` receives a progress message just before the downloads start.
/// Metadata failures (no publication, no file list) are returned as errors
/// before anything is downloaded. Per-file failures are recorded in
/// [`SourceDownload::outcomes`] instead.
pub(crate) async fn package_download(
    client: &LaunchpadClient,
    request: &SourceDownloadRequest,
    announce: impl FnOnce(&Report) -> lpcli::error::Result<()>,
) -> lpcli::error::Result<SourceDownload> {
    // Exact match avoids substring hits such as "libtest-valgrind-perl" when
    // searching for "valgrind".
    let params = SourceSearchParams {
        source_name: Some(&request.name),
        version: request.version.as_deref(),
        status: Some("Published"),
        exact_match: true,
        ..Default::default()
    };
    let found =
        packages::search_published_sources_raw(client, &request.distro, &request.series, &params)
            .await?;
    let publication_json = raw_entries(&found.raw).first().cloned();
    let (Some(publication), Some(publication_json)) =
        (found.value.entries.into_iter().next(), publication_json)
    else {
        return Err(LpError::LocalNotFound(format!(
            "No published source package '{}'{} found in {}/{}.",
            request.name,
            request
                .version
                .as_ref()
                .map(|v| format!(" ({v})"))
                .unwrap_or_default(),
            request.distro,
            request.series,
        )));
    };

    let self_link = publication
        .self_link
        .as_deref()
        .ok_or_else(|| LpError::Other("Source package publication has no self_link.".into()))?;
    let urls = packages::get_source_file_urls(client, self_link).await?;

    let mut download = SourceDownload {
        request: request.clone(),
        publication,
        publication_json,
        urls,
        outcomes: Vec::new(),
    };
    if download.urls.is_empty() {
        return Ok(download);
    }
    announce(&Report {
        blocks: vec![Block::Note(vec![
            Span::Text(format!("Downloading {} file(s) for ", download.urls.len())),
            Span::Name(request.name.clone()),
            Span::Text(format!(" ({}) ...", download.version())),
        ])],
        json: Value::Null,
        records: no_records(),
    })?;
    download.outcomes = download_public_files(&download.urls, &request.output_dir).await?;
    Ok(download)
}

/// The `package download` report.
///
/// The JSON is an lpcli-defined object: the unmodified Launchpad publication
/// under `source_package_publishing_history`, and lpcli's file list and
/// per-file outcomes under `lpcli`.
pub(crate) fn package_download_report(found: &SourceDownload) -> Report {
    let request = &found.request;
    let output_dir = request.output_dir.to_string_lossy().into_owned();
    let mut blocks = Vec::new();
    let mut downloads = Vec::new();
    for outcome in &found.outcomes {
        match outcome {
            download::DownloadOutcome::Success(file) => {
                let filename = file.path.file_name().map_or_else(
                    || file.path.to_string_lossy().into_owned(),
                    |name| name.to_string_lossy().into_owned(),
                );
                blocks.push(Block::Status(
                    Level::Ok,
                    vec![
                        Span::Name(filename.clone()),
                        Span::Text(format!(" ({})", format_size(file.size))),
                    ],
                ));
                downloads.push(serde_json::json!({
                    "filename": filename,
                    "status": "downloaded",
                    "path": file.path.to_string_lossy(),
                    "size": file.size,
                }));
            }
            download::DownloadOutcome::Failed { filename, error } => {
                blocks.push(Block::Status(
                    Level::Error,
                    vec![
                        Span::Name(filename.clone()),
                        Span::Text(format!(": {error}")),
                    ],
                ));
                downloads.push(serde_json::json!({
                    "filename": filename,
                    "status": "failed",
                    "error": error,
                }));
            }
        }
    }

    let failed = found.failures().len();
    let downloaded = found.outcomes.len() - failed;
    if found.urls.is_empty() {
        blocks.push(Block::Status(
            Level::Warning,
            vec![
                Span::Text("No source files found for ".into()),
                Span::Name(request.name.clone()),
                Span::Text(format!(
                    " ({}) in {}/{}.",
                    found.version(),
                    request.distro,
                    request.series
                )),
            ],
        ));
    } else {
        blocks.push(Block::Break);
        let (level, summary) = match (downloaded, failed) {
            (_, 0) => (
                Level::Ok,
                format!("Downloaded {downloaded} file(s) to '{output_dir}'."),
            ),
            (0, _) => (
                Level::Error,
                format!("No files downloaded; {failed} failed."),
            ),
            _ => (
                Level::Warning,
                format!("{downloaded} file(s) downloaded to '{output_dir}', {failed} failed."),
            ),
        };
        blocks.push(Block::Status(level, vec![Span::Text(summary)]));
    }

    Report {
        blocks,
        json: serde_json::json!({
            "source_package_publishing_history": found.publication_json,
            "lpcli": {
                "source_file_urls": found.urls,
                "output_directory": output_dir,
                "downloads": downloads,
                "downloaded": downloaded,
                "failed": failed,
            },
        }),
        records: no_records(),
    }
}

/// The exit status of `package download`, decided after its report is
/// printed.
///
/// - Every file downloaded, or the publication has no files: success.
/// - Some files downloaded and some failed: [`LpError::Partial`] (exit 7),
///   because files were written to disk.
/// - Every file failed: [`LpError::Other`] (exit 1). Looking up the
///   publication and its file list is only preparation, so it does not make
///   the download a partial success, and nothing was written.
pub(crate) fn package_download_result(found: &SourceDownload) -> lpcli::error::Result<()> {
    let failures = found.failures();
    if failures.is_empty() {
        return Ok(());
    }
    let list = failures
        .iter()
        .map(|(filename, error)| format!("{filename} ({error})"))
        .collect::<Vec<_>>()
        .join("; ");
    let total = found.outcomes.len();
    let downloaded = total - failures.len();
    let name = &found.request.name;
    if downloaded == 0 {
        Err(LpError::Other(format!(
            "Failed to download all {total} source file(s) for '{name}': {list}"
        )))
    } else {
        Err(LpError::Partial(format!(
            "Downloaded {downloaded} of {total} source file(s) for '{name}'; failed: {list}"
        )))
    }
}

/// Format a byte size in a human-friendly way using binary prefixes.
pub(crate) fn format_size(bytes: u64) -> String {
    match unit_prefix::NumberPrefix::binary(bytes as f64) {
        unit_prefix::NumberPrefix::Standalone(bytes) => format!("{bytes} B"),
        unit_prefix::NumberPrefix::Prefixed(prefix, n) => format!("{n:.1} {prefix}B"),
    }
}

// ---------------------------------------------------------------------------
// Supported output modes
// ---------------------------------------------------------------------------

/// Every person, package, project, CVE, spec, question, and translation
/// command prints Launchpad's JSON (or, for `package download`, an
/// lpcli-defined object).
pub(crate) fn read_domain_supports_json(command: &Command) -> bool {
    matches!(
        command,
        Command::Person(_)
            | Command::Package(_)
            | Command::Project(_)
            | Command::Cve(_)
            | Command::Spec(_)
            | Command::Question(_)
            | Command::Translation(_)
    )
}

/// List commands also print `tsv`; single entries and downloads do not.
pub(crate) fn read_domain_supports_tsv(command: &Command) -> bool {
    matches!(
        command,
        Command::Person(
            PersonCommand::Search { .. }
                | PersonCommand::Members { .. }
                | PersonCommand::Bugs { .. }
                | PersonCommand::Ppas { .. }
                | PersonCommand::OwnedTeams { .. }
        ) | Command::Package(
            PackageCommand::ListSeries { .. }
                | PackageCommand::Search { .. }
                | PackageCommand::PpaSources { .. }
        ) | Command::Project(
            ProjectCommand::Search { .. }
                | ProjectCommand::Milestones { .. }
                | ProjectCommand::ListSeries { .. }
                | ProjectCommand::SeriesReleases { .. }
        ) | Command::Cve(CveCommand::Search { .. } | CveCommand::BugCves { .. })
            | Command::Spec(SpecCommand::List { .. })
            | Command::Question(QuestionCommand::Search { .. } | QuestionCommand::Messages { .. })
            | Command::Translation(_)
    )
}
