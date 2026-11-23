//! Read-mostly domain handlers: person, package, project, CVE, spec,
//! question and translation.
//!
//! Each command builds a [`Report`] from the library's `_raw` functions, so
//! `--format json` prints Launchpad's JSON unchanged: one entry for `show`
//! commands, and Launchpad's collection shape for lists. The typed values
//! drive the `rich`, `plain`, and (for lists) `tsv` output.

use crate::*;

use futures_util::StreamExt;

use lpcli::error::LpError;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Report helpers
// ---------------------------------------------------------------------------

fn column(label: &'static str, key: &'static str) -> Column {
    Column::new(label, key)
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

/// The final path segment of a Launchpad link, without a leading `~`, e.g.
/// `https://api.launchpad.net/devel/~jdoe` -> `jdoe`.
fn link_name(link: Option<&str>) -> Option<String> {
    output::link_name(link)
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
        column("Maintainer", "maintainer"),
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
                link_name(p.package_maintainer_link.as_deref()),
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
                        m.date_joined.map(|ts| ts.to_string()),
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
                    Column::date("Since", "date_joined"),
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

        PackageCommand::Builds {
            owner,
            ppa,
            name,
            version,
            series,
            arch,
            state,
            distro,
            limit,
        } => {
            package_builds(
                client,
                &PackageBuildsRequest {
                    owner,
                    ppa,
                    name,
                    version,
                    series,
                    arch,
                    state,
                    distro,
                    limit,
                },
            )
            .await?
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

        PackageCommand::Changelog {
            name,
            series,
            version,
            pocket,
            ppa,
            distro,
            limit,
            full,
        } => {
            package_changelog(
                client,
                &PackageChangelogRequest {
                    name,
                    series,
                    version,
                    pocket,
                    ppa,
                    distro,
                    limit,
                    full,
                },
            )
            .await?
        }
    })
}

// ---------------------------------------------------------------------------
// Package build results
// ---------------------------------------------------------------------------

/// Parameters for `lpcli package builds`.
struct PackageBuildsRequest {
    owner: Option<String>,
    ppa: Option<String>,
    name: Option<String>,
    version: Option<String>,
    series: Option<String>,
    arch: Option<String>,
    state: Option<String>,
    distro: String,
    limit: u32,
}

/// Build the report for `lpcli package builds`.
///
/// The source publications come from `getPublishedSources`; each source's
/// builds come from its `getBuilds` operation, scoped to one publication so
/// that the primary archive does not time out. Active binary publications are
/// checked per source, only for sources with visible rows and at least one
/// successful build.
async fn package_builds(
    client: &LaunchpadClient,
    request: &PackageBuildsRequest,
) -> lpcli::error::Result<Report> {
    use futures_util::TryStreamExt as _;

    if request.limit == 0 {
        return Err(LpError::Usage("--limit must be at least 1.".into()));
    }
    if (request.owner.is_some() || request.ppa.is_some())
        && !request.distro.eq_ignore_ascii_case("ubuntu")
    {
        return Err(LpError::Usage(format!(
            "PPA archives are only available for Ubuntu, not '{}'.",
            request.distro
        )));
    }
    if request.owner.is_some() != request.ppa.is_some() {
        return Err(LpError::Usage(
            "--owner and --ppa must be provided together.".into(),
        ));
    }
    let owner = request.owner.as_deref().map(str::trim);
    let ppa = request.ppa.as_deref().map(str::trim);
    if owner.is_some_and(str::is_empty) {
        return Err(LpError::Usage("--owner must not be empty.".into()));
    }
    if ppa.is_some_and(str::is_empty) {
        return Err(LpError::Usage("--ppa must not be empty.".into()));
    }

    let canonical_series_name = request
        .series
        .as_deref()
        .map(|s| canonical_series(&request.distro, s));
    let params = SourceSearchParams {
        source_name: request.name.as_deref(),
        version: request.version.as_deref(),
        exact_match: true,
        statuses: &["Published", "Superseded", "Pending"],
        limit: Some(request.limit),
        ..Default::default()
    };

    let (page, archive_label) = if let (Some(owner), Some(ppa)) = (owner, ppa) {
        if let Err(error) = packages::get_ppa_raw(client, owner, ppa).await {
            return Err(match error {
                LpError::NotFound(_) => LpError::NotFound(format!("PPA ~{owner}/{ppa} not found.")),
                other => other,
            });
        }
        let page =
            packages::search_ppa_sources_raw(client, owner, ppa, canonical_series_name, &params)
                .await?;
        (page, format!("~{owner}/{ppa}"))
    } else {
        let series = canonical_series_name.ok_or_else(|| {
            LpError::Usage(
                "specify --series for the primary archive or --owner and --ppa for a PPA".into(),
            )
        })?;
        let page = packages::search_published_sources_raw(client, &request.distro, series, &params)
            .await?;
        (page, request.distro.clone())
    };

    let sources = &page.value.entries;

    // Fetch each source's builds concurrently, preserving source order and
    // short-circuiting on the first failure (e.g. 429 or auth error).
    let fetched: Vec<Raw<Vec<packages::SourceBuild>>> = futures_util::stream::iter(sources)
        .map(|source| async move {
            let link = source
                .self_link
                .as_deref()
                .ok_or_else(|| LpError::Other("source publication has no self_link".into()))?;
            packages::get_source_builds_raw(client, link).await
        })
        .buffered(8)
        .try_collect()
        .await?;

    let mut source_builds: Vec<(Vec<packages::SourceBuild>, Vec<Value>)> =
        Vec::with_capacity(fetched.len());
    for raw in fetched {
        let entries = raw
            .raw
            .get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        source_builds.push((raw.value, entries));
    }

    let arch_filter = request.arch.as_deref();
    let state_filter = request.state.as_deref();
    let visible_per_source: Vec<Vec<usize>> = source_builds
        .iter()
        .map(|(builds, _)| {
            builds
                .iter()
                .enumerate()
                .filter(|(_, build)| {
                    arch_filter.is_none_or(|arch| build.arch_tag.as_deref() == Some(arch))
                })
                .filter(|(_, build)| {
                    state_filter.is_none_or(|state| build.buildstate.as_deref() == Some(state))
                })
                .map(|(idx, _)| idx)
                .collect()
        })
        .collect();

    // Only sources with at least one visible build row AND a successful build
    // can have active binaries worth querying.
    let checks: Vec<usize> = source_builds
        .iter()
        .enumerate()
        .filter(|(idx, (builds, _))| {
            !visible_per_source[*idx].is_empty()
                && builds
                    .iter()
                    .any(|build| build.buildstate.as_deref() == Some("Successfully built"))
        })
        .map(|(idx, _)| idx)
        .collect();
    let mut binaries = vec![false; sources.len()];
    let checked: Vec<(usize, bool)> = futures_util::stream::iter(checks)
        .map(|idx| async move {
            let link = sources[idx]
                .self_link
                .as_deref()
                .ok_or_else(|| LpError::Other("source publication has no self_link".into()))?;
            Ok::<_, LpError>((
                idx,
                packages::source_has_active_binaries(client, link).await?,
            ))
        })
        .buffered(8)
        .try_collect()
        .await?;
    for (idx, has_binaries) in checked {
        binaries[idx] = has_binaries;
    }

    let mut rows = Vec::new();
    let mut raw_builds = Vec::new();
    let mut log_rows = Vec::new();
    let mut source_map = serde_json::Map::new();

    for (idx, source) in sources.iter().enumerate() {
        let (builds, builds_json) = &source_builds[idx];
        let series = output::link_name(source.distro_series_link.as_deref());
        let visible = &visible_per_source[idx];

        let mut visible_links = Vec::with_capacity(visible.len());
        for &build_idx in visible {
            let build = &builds[build_idx];
            if let Some(raw) = builds_json.get(build_idx) {
                raw_builds.push(raw.clone());
            }
            let build_series =
                output::link_name(build.distro_series_link.as_deref()).or_else(|| series.clone());
            let package = build
                .source_package_name
                .as_deref()
                .or(source.source_package_name.as_deref());
            let version = build
                .source_package_version
                .as_deref()
                .or(source.source_package_version.as_deref());
            visible_links.push(
                build
                    .self_link
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            if let Some(log_url) = &build.build_log_url {
                let arch = build.arch_tag.as_deref().unwrap_or("");
                let build_series = build_series.as_deref().unwrap_or("");
                log_rows.push(vec![
                    Some(format!(
                        "{} {} ({build_series}/{arch})",
                        package.unwrap_or(""),
                        version.unwrap_or(""),
                    )),
                    Some(log_url.clone()),
                ]);
            }
            let row_binaries =
                build.buildstate.as_deref() == Some("Successfully built") && binaries[idx];
            rows.push(vec![
                package.map(str::to_string),
                version.map(str::to_string),
                build_series,
                build.arch_tag.clone(),
                source.status.clone(),
                build.buildstate.clone(),
                Some(binaries_word(row_binaries)),
                build.date_built.map(|ts| ts.to_string()),
                build.duration.clone(),
                build.build_log_url.clone(),
            ]);
        }

        let placeholder = builds.is_empty() && arch_filter.is_none() && state_filter.is_none();
        if placeholder {
            rows.push(vec![
                source.source_package_name.clone(),
                source.source_package_version.clone(),
                series.clone(),
                None,
                source.status.clone(),
                Some("No builds yet".into()),
                Some(binaries_word(false)),
                None,
                None,
                None,
            ]);
        }

        if (!visible.is_empty() || placeholder)
            && let Some(link) = &source.self_link
        {
            source_map.insert(
                link.clone(),
                serde_json::json!({
                    "builds": visible_links,
                    "binaries_published": binaries[idx],
                }),
            );
        }
    }

    let complete = page.value.next_collection_link.is_none();
    let json = serde_json::json!({
        "source_publications": page.raw.clone(),
        "builds": {"entries": raw_builds},
        "lpcli": {
            "archive": archive_label,
            "series": canonical_series_name,
            "complete": complete,
            "sources": source_map,
        },
    });

    let mut blocks = vec![Block::Heading(format!("Builds for {archive_label}"))];
    if let Some(series) = canonical_series_name {
        blocks.push(Block::Field("Series", series.to_string()));
    }
    let hint = (!complete)
        .then(|| "More source publications match; raise --limit to see them.".to_string());
    let empty = if sources.is_empty() {
        format!("No source publications found in {archive_label}.")
    } else if rows.is_empty() {
        "No builds matched the requested --arch/--state filters.".into()
    } else {
        "No builds found.".into()
    };
    blocks.push(Block::List {
        noun: noun("build result", "build results"),
        empty,
        hint,
    });
    if !log_rows.is_empty() {
        blocks.push(Block::RichTable {
            records: Records {
                columns: vec![column("Build", "build"), column("Log", "build_log_url")],
                rows: log_rows,
            },
            noun: noun("build log", "build logs"),
            empty: "No build logs found.".into(),
            hint: None,
        });
    }

    Ok(Report {
        blocks,
        json,
        records: Records {
            columns: vec![
                column("Package", "source_package_name"),
                column("Version", "source_package_version"),
                column("Series", "series"),
                column("Arch", "arch_tag"),
                column("Publication", "status"),
                column("Build State", "buildstate"),
                column("Binaries", "binaries_published"),
                Column::datetime("Finished", "datebuilt"),
                column("Duration", "duration"),
                Column::rich_hidden("Log", "build_log_url"),
            ],
            rows,
        },
    })
}

fn binaries_word(published: bool) -> String {
    if published { "yes" } else { "no" }.to_string()
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
                        r.date_released.map(|ts| ts.to_string()),
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
                    Column::date("Release Date", "date_released"),
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

/// Messages read as width-aware cards in `rich`, labeled records in `plain`,
/// and one record per message in `tsv`.
pub(crate) fn question_messages_report(found: &Raw<Vec<questions::QuestionMessage>>) -> Report {
    let mut entries = Vec::with_capacity(found.value.len());
    let mut rows = Vec::with_capacity(found.value.len());
    for m in &found.value {
        let index = m.index.map(|index| index.to_string());
        let author = link_name(m.owner_link.as_deref());
        let raw_date = m.date_created.map(|ts| ts.to_string());
        let content = m.content.clone().unwrap_or_default();
        entries.push(CommentEntry {
            label_field: "Message",
            label: format!("#{}", index.as_deref().unwrap_or("?")),
            author: author.clone(),
            date: m.date_created.map(CommentDate::Timestamp),
            details: m
                .action
                .as_ref()
                .map(|action| vec![("Action", action.clone())])
                .unwrap_or_default(),
            content_field: "Content",
            content: content.clone(),
            attachments: Vec::new(),
        });
        rows.push(vec![
            index,
            author,
            raw_date,
            m.action.clone(),
            m.content.is_some().then_some(content),
        ]);
    }
    Report {
        blocks: vec![Block::Comments {
            entries,
            noun: noun("message", "messages"),
            empty: "No messages found.".into(),
        }],
        json: found.raw.clone(),
        records: Records {
            columns: vec![
                column("Index", "index"),
                column("Author", "owner"),
                Column::datetime("Date", "date_created"),
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
                .map(|e| {
                    vec![
                        e.path.clone(),
                        e.status.clone(),
                        e.date_created.map(|ts| ts.to_string()),
                    ]
                })
                .collect();
            list_report(
                found.raw,
                noun("queue entry", "queue entries"),
                "The translation import queue is empty.",
                vec![
                    column("Path", "path"),
                    column("Status", "status"),
                    Column::date("Date", "date_created"),
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
// Package changelog handler
// ---------------------------------------------------------------------------

pub(crate) const DEFAULT_CHANGELOG_LIMIT: usize = 5;

#[derive(Debug, Clone)]
pub(crate) struct PackageChangelogRequest {
    pub(crate) name: String,
    pub(crate) series: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) pocket: Option<Pocket>,
    pub(crate) ppa: Option<PpaRef>,
    pub(crate) distro: String,
    pub(crate) limit: Option<usize>,
    pub(crate) full: bool,
}

async fn package_changelog(
    client: &LaunchpadClient,
    request: &PackageChangelogRequest,
) -> lpcli::error::Result<Report> {
    if request.limit == Some(0) {
        return Err(LpError::Usage("--limit must be at least 1.".into()));
    }
    if request.ppa.is_some() && !request.distro.eq_ignore_ascii_case("ubuntu") {
        return Err(LpError::Usage(format!(
            "PPA archives are only available for Ubuntu, not '{}'.",
            request.distro
        )));
    }

    let canonical_series_name = request
        .series
        .as_deref()
        .map(|s| canonical_series(&request.distro, s));
    let params = SourceSearchParams {
        source_name: Some(&request.name),
        version: request.version.as_deref(),
        pocket: request.pocket.map(pocket_launchpad_name),
        status: request.version.is_none().then_some("Published"),
        exact_match: true,
        limit: Some(1),
        ..Default::default()
    };

    let found = if let Some(ppa) = &request.ppa {
        packages::search_ppa_sources_raw(
            client,
            &ppa.owner,
            &ppa.name,
            canonical_series_name,
            &params,
        )
        .await?
    } else {
        let series_name = canonical_series_name.ok_or_else(|| {
            LpError::Usage("--series is required when --ppa is not given.".into())
        })?;
        packages::search_published_sources_raw(client, &request.distro, series_name, &params)
            .await?
    };

    let publication_json = raw_entries(&found.raw).first().cloned();
    let (Some(publication), Some(publication_json)) =
        (found.value.entries.into_iter().next(), publication_json)
    else {
        return Err(LpError::LocalNotFound(changelog_not_found_message(
            request,
            canonical_series_name,
        )));
    };

    let self_link = publication
        .self_link
        .as_deref()
        .ok_or_else(|| LpError::Other("Source package publication has no self_link.".into()))?;
    let changelog_url = packages::get_source_changelog_url(client, self_link)
        .await?
        .ok_or_else(|| {
            let pkg_version = publication
                .source_package_version
                .as_deref()
                .unwrap_or("unknown version");
            LpError::LocalNotFound(format!(
                "No changelog is available on Launchpad for '{}' ({pkg_version}) in {}.",
                request.name,
                changelog_target_label(request, canonical_series_name),
            ))
        })?;

    let raw_text = match packages::fetch_source_changelog(client, &changelog_url).await {
        Ok(text) => text,
        Err(err) => {
            if let Some(ppa) = &request.ppa {
                let pkg_version = publication
                    .source_package_version
                    .as_deref()
                    .unwrap_or("unknown version");
                let ppa_label = ppa.label();
                return Err(match err {
                    LpError::NotFound(_) => LpError::LocalNotFound(format!(
                        "Changelog for '{}' ({pkg_version}) in {ppa_label} could not be fetched without authentication; changelogs in private PPAs are not accessible via public librarian URLs.",
                        request.name,
                    )),
                    LpError::Unauthorized(_)
                    | LpError::Api {
                        status: 401 | 403, ..
                    } => LpError::Api {
                        status: 403,
                        message: format!(
                            "Changelog for '{}' ({pkg_version}) in {ppa_label} requires authentication; changelogs in private PPAs cannot be fetched via public librarian URLs.",
                            request.name,
                        ),
                    },
                    other => other,
                });
            }
            return Err(err);
        }
    };
    Ok(package_changelog_report(
        request,
        canonical_series_name,
        &publication,
        publication_json,
        &changelog_url,
        &raw_text,
    ))
}

fn changelog_target_label(
    request: &PackageChangelogRequest,
    canonical_series_name: Option<&str>,
) -> String {
    match (&request.ppa, canonical_series_name) {
        (Some(ppa), Some(series)) => format!("{} ({})", ppa.label(), series),
        (Some(ppa), None) => ppa.label(),
        (None, Some(series)) => format!("{}/{series}", request.distro),
        (None, None) => request.distro.clone(),
    }
}

fn changelog_not_found_message(
    request: &PackageChangelogRequest,
    canonical_series_name: Option<&str>,
) -> String {
    let qualifier = if request.version.is_none() {
        "published "
    } else {
        ""
    };
    let version = request
        .version
        .as_ref()
        .map(|v| format!(" ({v})"))
        .unwrap_or_default();
    let pocket = request
        .pocket
        .map(|p| format!(" [pocket: {}]", pocket_launchpad_name(p)))
        .unwrap_or_default();
    format!(
        "No {qualifier}source package '{}'{version}{pocket} found in {}.",
        request.name,
        changelog_target_label(request, canonical_series_name),
    )
}

/// Build the report for `lpcli package changelog`.
pub(crate) fn package_changelog_report(
    request: &PackageChangelogRequest,
    canonical_series_name: Option<&str>,
    publication: &packages::SourcePackagePublishingHistory,
    publication_json: Value,
    changelog_url: &str,
    raw_text: &str,
) -> Report {
    let pkg_name = publication
        .source_package_name
        .as_deref()
        .unwrap_or(&request.name);
    let pkg_version = publication
        .source_package_version
        .as_deref()
        .unwrap_or("unknown version");
    let archive_label = request
        .ppa
        .as_ref()
        .map_or_else(|| request.distro.clone(), PpaRef::label);
    let resolved_series = canonical_series_name
        .map(str::to_string)
        .or_else(|| output::link_name(publication.distro_series_link.as_deref()));

    let mut entries = packages::parse_debian_changelog(raw_text);
    let total_entries = entries.len();
    let effective_limit = (!request.full).then(|| request.limit.unwrap_or(DEFAULT_CHANGELOG_LIMIT));
    let displayed_text = match effective_limit {
        Some(limit) if total_entries > 0 => {
            entries.truncate(limit);
            packages::slice_debian_changelog(raw_text, limit)
        }
        _ => packages::slice_debian_changelog(raw_text, usize::MAX),
    };
    let truncated = entries.len() < total_entries;

    let mut blocks = vec![Block::Heading(format!("{pkg_name} ({pkg_version})"))];
    if let Some(ppa) = &request.ppa {
        blocks.push(Block::Field("Archive", ppa.label()));
    }
    if let Some(series) = &resolved_series {
        blocks.push(Block::Field("Series", series.clone()));
    }
    if let Some(pocket) = &publication.pocket {
        blocks.push(Block::Field("Pocket", pocket.clone()));
    }
    if let Some(status) = &publication.status {
        blocks.push(Block::Field("Status", status.clone()));
    }
    if let Some(published) = publication.date_published {
        blocks.push(Block::Timestamp(
            "Published",
            published,
            TimestampPrecision::DateTime,
        ));
    }
    blocks.push(Block::Field("Changelog", changelog_url.to_string()));
    blocks.push(Block::Break);

    if !entries.is_empty() {
        if truncated {
            blocks.push(Block::Note(vec![
                Span::Text(format!(
                    "Showing latest {} of {total_entries} changelog entries. Use ",
                    entries.len()
                )),
                Span::Command("--full".into()),
                Span::Text(" or ".into()),
                Span::Command("--limit".into()),
                Span::Text(" to see more.".into()),
            ]));
            blocks.push(Block::Break);
        }
        let comment_entries = entries
            .iter()
            .map(|entry| {
                let mut details = Vec::new();
                if !entry.distributions.is_empty() {
                    details.push(("Distribution", entry.distributions.join(", ")));
                }
                if let Some(urgency) = &entry.urgency {
                    details.push(("Urgency", urgency.clone()));
                }
                if !entry.metadata.is_empty() {
                    let formatted = entry
                        .metadata
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    details.push(("Metadata", formatted));
                }
                CommentEntry {
                    label_field: "Version",
                    label: entry.version.clone(),
                    author: entry.maintainer.clone(),
                    date: entry
                        .timestamp
                        .map(CommentDate::Timestamp)
                        .or_else(|| entry.date.clone().map(CommentDate::Text)),
                    details,
                    content_field: "Changes",
                    content: entry.changes.clone(),
                    attachments: Vec::new(),
                }
            })
            .collect();
        blocks.push(Block::Comments {
            entries: comment_entries,
            noun: noun("changelog entry", "changelog entries"),
            empty: "No changelog entries found.".into(),
        });
    } else if !displayed_text.trim().is_empty() {
        blocks.push(Block::Field("Changes", displayed_text.clone()));
    } else {
        blocks.push(Block::Note(vec![Span::Text(
            "No changelog entries found.".into(),
        )]));
    }

    Report {
        blocks,
        json: serde_json::json!({
            "source_package_publishing_history": publication_json,
            "lpcli": {
                "archive": archive_label,
                "series": resolved_series,
                "changelog_url": changelog_url,
                "changelog": displayed_text,
                "total_entries": total_entries,
                "truncated": truncated,
                "entries": entries,
            },
        }),
        records: no_records(),
    }
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
    let found =
        package_download_with_progress(client, request, output.progress(), |announcement| {
            if output.mode() == OutputMode::Rich
                && output.progress() == download::ProgressMode::Rich
            {
                output.write_stderr_report(announcement)
            } else {
                Ok(())
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
#[cfg(test)]
pub(crate) async fn package_download(
    client: &LaunchpadClient,
    request: &SourceDownloadRequest,
    announce: impl FnOnce(&Report) -> lpcli::error::Result<()>,
) -> lpcli::error::Result<SourceDownload> {
    package_download_with_progress(client, request, download::ProgressMode::None, announce).await
}

/// Find the publication, list its files, and download them with an explicit
/// stderr [`download::ProgressMode`].
pub(crate) async fn package_download_with_progress(
    client: &LaunchpadClient,
    request: &SourceDownloadRequest,
    progress: download::ProgressMode,
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
    download.outcomes =
        download_public_files_with_progress(&download.urls, &request.output_dir, progress).await?;
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
                let filename = output::redact_secrets(filename);
                let error = output::redact_secrets(error);
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
                | PackageCommand::Builds { .. }
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
