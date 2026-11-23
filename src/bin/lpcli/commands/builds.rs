//! Build listing, inspection, log download, and retry handlers.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use futures_util::{StreamExt as _, TryStreamExt as _};
use lpcli::error::LpError;
use lpcli::packages::{BuildRecordSearchParams, SourceBuild};
use serde_json::Value;

use super::read_domains::{
    column, entry_report, flag, format_size, link_name, no_records, noun, raw_entries,
};
use crate::*;

// ---------------------------------------------------------------------------
// Shared request & filter types
// ---------------------------------------------------------------------------

/// Parameters for `lpcli package builds`.
pub(crate) struct PackageBuildsRequest {
    pub(crate) owner: Option<String>,
    pub(crate) ppa: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) series: Option<String>,
    pub(crate) arch: Vec<String>,
    pub(crate) state: Vec<String>,
    pub(crate) exclude_state: Vec<String>,
    pub(crate) pocket: Option<Pocket>,
    pub(crate) distro: String,
    pub(crate) limit: u32,
}

/// Target selection for `package build logs` and `package build retry`.
#[derive(Debug, Clone)]
pub(crate) enum BuildTarget {
    Single {
        build_id: Option<u64>,
        build_url: Option<String>,
        owner: Option<String>,
        ppa: Option<String>,
        distro: String,
    },
    Filter(BuildFilterRequest),
}

/// Bulk filter parameters for `package build logs` and `package build retry`.
#[derive(Debug, Clone)]
pub(crate) struct BuildFilterRequest {
    pub(crate) owner: Option<String>,
    pub(crate) ppa: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) series: Option<String>,
    pub(crate) arch: Vec<String>,
    pub(crate) state: Vec<String>,
    pub(crate) exclude_state: Vec<String>,
    pub(crate) pocket: Option<Pocket>,
    pub(crate) distro: String,
    pub(crate) limit: u32,
}

/// Request for `lpcli package build logs`.
#[derive(Debug, Clone)]
pub(crate) struct BuildLogsRequest {
    pub(crate) target: BuildTarget,
    pub(crate) output_dir: PathBuf,
    pub(crate) include_upload_logs: bool,
}

/// Request for `lpcli package build retry`.
#[derive(Debug, Clone)]
pub(crate) struct BuildRetryRequest {
    pub(crate) target: BuildTarget,
    pub(crate) dry_run: bool,
    pub(crate) yes: bool,
    pub(crate) save_logs: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub(crate) async fn handle_build(
    client: &LaunchpadClient,
    output: &Output,
    quiet: bool,
    no_input: bool,
    cmd: BuildCommand,
) -> lpcli::error::Result<()> {
    match cmd {
        BuildCommand::Show {
            build_id,
            build_url,
            owner,
            ppa,
            distro,
        } => {
            let report = build_show_report(
                client,
                build_id,
                build_url.as_deref(),
                owner.as_deref(),
                ppa.as_deref(),
                &distro,
            )
            .await?;
            output.print(&report)
        }
        BuildCommand::Logs {
            build_id,
            build_url,
            owner,
            ppa,
            name,
            version,
            series,
            arch,
            state,
            exclude_state,
            pocket,
            distro,
            limit,
            output: output_dir,
            include_upload_logs,
        } => {
            let target = build_target_from_args(
                build_id,
                build_url,
                owner,
                ppa,
                name,
                version,
                series,
                arch,
                state,
                exclude_state,
                pocket,
                distro,
                limit,
            );
            let request = BuildLogsRequest {
                target,
                output_dir: output_dir.into(),
                include_upload_logs,
            };
            handle_build_logs(client, output, &request).await
        }
        BuildCommand::Retry {
            build_id,
            build_url,
            owner,
            ppa,
            name,
            version,
            series,
            arch,
            state,
            exclude_state,
            pocket,
            distro,
            limit,
            dry_run,
            yes,
            save_logs,
        } => {
            let target = build_target_from_args(
                build_id,
                build_url,
                owner,
                ppa,
                name,
                version,
                series,
                arch,
                state,
                exclude_state,
                pocket,
                distro,
                limit,
            );
            let request = BuildRetryRequest {
                target,
                dry_run,
                yes,
                save_logs: save_logs.map(PathBuf::from),
            };
            handle_build_retry(client, output, quiet, no_input, &request).await
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_target_from_args(
    build_id: Option<u64>,
    build_url: Option<String>,
    owner: Option<String>,
    ppa: Option<String>,
    name: Option<String>,
    version: Option<String>,
    series: Option<String>,
    arch: Vec<String>,
    state: Vec<String>,
    exclude_state: Vec<String>,
    pocket: Option<Pocket>,
    distro: String,
    limit: u32,
) -> BuildTarget {
    if build_id.is_some() || build_url.is_some() {
        BuildTarget::Single {
            build_id,
            build_url,
            owner,
            ppa,
            distro,
        }
    } else {
        BuildTarget::Filter(BuildFilterRequest {
            owner,
            ppa,
            name,
            version,
            series,
            arch,
            state,
            exclude_state,
            pocket,
            distro,
            limit,
        })
    }
}

// ---------------------------------------------------------------------------
// Archive & build fetching helpers
// ---------------------------------------------------------------------------

struct ArchiveBuildSources {
    archive_label: String,
    canonical_series_name: Option<String>,
    page: Raw<Collection<packages::SourcePackagePublishingHistory>>,
    source_builds: Vec<(Vec<SourceBuild>, Vec<Value>)>,
}

fn validate_archive_target(
    owner: Option<&str>,
    ppa: Option<&str>,
    series: Option<&str>,
    distro: &str,
    limit: u32,
) -> lpcli::error::Result<(Option<PpaRef>, Option<String>)> {
    if limit == 0 {
        return Err(LpError::Usage("--limit must be at least 1.".into()));
    }
    let ppa_ref = resolve_ppa_args(owner, ppa).map_err(LpError::Usage)?;
    if ppa_ref.is_some() && !distro.eq_ignore_ascii_case("ubuntu") {
        return Err(LpError::Usage(format!(
            "PPA archives are only available for Ubuntu, not '{distro}'."
        )));
    }
    let canonical_series_name = series.map(|s| canonical_series(distro, s).to_string());
    Ok((ppa_ref, canonical_series_name))
}

async fn verify_ppa_exists(client: &LaunchpadClient, ppa: &PpaRef) -> lpcli::error::Result<()> {
    if let Err(error) = packages::get_ppa_raw(client, &ppa.owner, &ppa.name).await {
        return Err(match error {
            LpError::NotFound(_) => LpError::NotFound(format!("PPA {} not found.", ppa.label())),
            other => other,
        });
    }
    Ok(())
}

struct SourceBuildQuery<'a> {
    ppa_ref: Option<&'a PpaRef>,
    canonical_series_name: Option<&'a str>,
    distro: &'a str,
    name: Option<&'a str>,
    version: Option<&'a str>,
    pocket: Option<Pocket>,
    limit: u32,
}

async fn fetch_source_publications_and_builds(
    client: &LaunchpadClient,
    query: &SourceBuildQuery<'_>,
) -> lpcli::error::Result<ArchiveBuildSources> {
    let params = SourceSearchParams {
        source_name: query.name,
        version: query.version,
        pocket: query.pocket.map(pocket_launchpad_name),
        exact_match: true,
        statuses: &["Published", "Superseded", "Pending"],
        limit: Some(query.limit),
        ..Default::default()
    };

    let (page, archive_label) = if let Some(ppa) = query.ppa_ref {
        verify_ppa_exists(client, ppa).await?;
        let page = packages::search_ppa_sources_raw(
            client,
            &ppa.owner,
            &ppa.name,
            query.canonical_series_name,
            &params,
        )
        .await?;
        (page, ppa.label())
    } else {
        let series = query.canonical_series_name.ok_or_else(|| {
            LpError::Usage(
                "specify --series for the primary archive or --owner and --ppa for a PPA".into(),
            )
        })?;
        let page =
            packages::search_published_sources_raw(client, query.distro, series, &params).await?;
        (page, query.distro.to_string())
    };

    // Fetch each source's builds concurrently, preserving source order and
    // short-circuiting on the first failure (e.g. 429 or auth error).
    let fetched: Vec<Raw<Vec<SourceBuild>>> = futures_util::stream::iter(&page.value.entries)
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

    let source_builds = fetched
        .into_iter()
        .map(|raw| {
            let entries = raw_entries(&raw.raw).to_vec();
            (raw.value, entries)
        })
        .collect();

    Ok(ArchiveBuildSources {
        archive_label,
        canonical_series_name: query.canonical_series_name.map(str::to_string),
        page,
        source_builds,
    })
}

fn build_matches_filters(
    build: &SourceBuild,
    exact_name: Option<&str>,
    canonical_series_name: Option<&str>,
    arches: &[String],
    states: &[String],
    exclude_states: &[String],
    pocket: Option<Pocket>,
) -> bool {
    if let Some(name) = exact_name
        && build.source_package_name.as_deref() != Some(name)
    {
        return false;
    }
    if let Some(series) = canonical_series_name
        && link_name(build.distro_series_link.as_deref()).as_deref() != Some(series)
    {
        return false;
    }
    if let Some(pocket) = pocket
        && build.pocket.as_deref() != Some(pocket_launchpad_name(pocket))
    {
        return false;
    }
    if !arches.is_empty()
        && !build
            .arch_tag
            .as_deref()
            .is_some_and(|arch| arches.iter().any(|wanted| wanted == arch))
    {
        return false;
    }
    if !states.is_empty()
        && !build
            .buildstate
            .as_deref()
            .is_some_and(|state| states.iter().any(|wanted| wanted == state))
    {
        return false;
    }
    if !exclude_states.is_empty()
        && build
            .buildstate
            .as_deref()
            .is_some_and(|state| exclude_states.iter().any(|excluded| excluded == state))
    {
        return false;
    }
    true
}

// ---------------------------------------------------------------------------
// `package builds` handler
// ---------------------------------------------------------------------------

/// Build the report for `lpcli package builds`.
///
/// The source publications come from `getPublishedSources`; each source's
/// builds come from its `getBuilds` operation, scoped to one publication so
/// that the primary archive does not time out. Active binary publications are
/// checked per source, only for sources with visible rows and at least one
/// successful build.
pub(crate) async fn package_builds(
    client: &LaunchpadClient,
    request: &PackageBuildsRequest,
) -> lpcli::error::Result<Report> {
    let (ppa_ref, canonical_series_name) = validate_archive_target(
        request.owner.as_deref(),
        request.ppa.as_deref(),
        request.series.as_deref(),
        &request.distro,
        request.limit,
    )?;

    let fetched = fetch_source_publications_and_builds(
        client,
        &SourceBuildQuery {
            ppa_ref: ppa_ref.as_ref(),
            canonical_series_name: canonical_series_name.as_deref(),
            distro: &request.distro,
            name: request.name.as_deref(),
            version: request.version.as_deref(),
            pocket: request.pocket,
            limit: request.limit,
        },
    )
    .await?;

    let sources = &fetched.page.value.entries;
    let source_builds = &fetched.source_builds;

    let visible_per_source: Vec<Vec<usize>> = source_builds
        .iter()
        .map(|(builds, _)| {
            builds
                .iter()
                .enumerate()
                .filter(|(_, build)| {
                    build_matches_filters(
                        build,
                        None,
                        None,
                        &request.arch,
                        &request.state,
                        &request.exclude_state,
                        None,
                    )
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
    let has_local_build_filters =
        !request.arch.is_empty() || !request.state.is_empty() || !request.exclude_state.is_empty();

    for (idx, source) in sources.iter().enumerate() {
        let (builds, builds_json) = &source_builds[idx];
        let series = link_name(source.distro_series_link.as_deref());
        let visible = &visible_per_source[idx];

        let mut visible_links = Vec::with_capacity(visible.len());
        for &build_idx in visible {
            let build = &builds[build_idx];
            if let Some(raw) = builds_json.get(build_idx) {
                raw_builds.push(raw.clone());
            }
            let build_series =
                link_name(build.distro_series_link.as_deref()).or_else(|| series.clone());
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
                build.build_id().map(str::to_string),
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

        let placeholder = builds.is_empty() && !has_local_build_filters;
        if placeholder {
            rows.push(vec![
                None,
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

    let complete = fetched.page.value.next_collection_link.is_none();
    let json = serde_json::json!({
        "source_publications": fetched.page.raw,
        "builds": {"entries": raw_builds},
        "lpcli": {
            "archive": fetched.archive_label,
            "series": fetched.canonical_series_name,
            "complete": complete,
            "sources": source_map,
        },
    });

    let mut blocks = vec![Block::Heading(format!(
        "Builds for {}",
        fetched.archive_label
    ))];
    if let Some(series) = &fetched.canonical_series_name {
        blocks.push(Block::Field("Series", series.clone()));
    }
    let hint = (!complete)
        .then(|| "More source publications match; raise --limit to see them.".to_string());
    let empty = if sources.is_empty() {
        format!("No source publications found in {}.", fetched.archive_label)
    } else if rows.is_empty() {
        "No builds matched the requested filters.".into()
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
                column("ID", "build_id"),
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
// Single / filtered build resolution
// ---------------------------------------------------------------------------

struct ResolvedBuilds {
    is_single: bool,
    archive_label: String,
    series: Option<String>,
    complete: bool,
    builds: Vec<Raw<SourceBuild>>,
}

async fn resolve_single_build(
    client: &LaunchpadClient,
    build_id: Option<u64>,
    build_url: Option<&str>,
    owner: Option<&str>,
    ppa: Option<&str>,
    distro: &str,
) -> lpcli::error::Result<(Raw<SourceBuild>, String)> {
    if let Some(url) = build_url {
        let found = packages::get_build_raw(client, url.trim()).await?;
        let label = found
            .value
            .archive_link
            .as_deref()
            .and_then(archive_label)
            .unwrap_or_else(|| distro.to_string());
        return Ok((found, label));
    }

    let id = build_id.ok_or_else(|| {
        LpError::Usage("specify --build-id or --build-url to select a build".into())
    })?;
    let ppa_ref = resolve_ppa_args(owner, ppa).map_err(LpError::Usage)?;
    if ppa_ref.is_some() && !distro.eq_ignore_ascii_case("ubuntu") {
        return Err(LpError::Usage(format!(
            "PPA archives are only available for Ubuntu, not '{distro}'."
        )));
    }
    if let Some(ppa) = ppa_ref {
        let found = packages::get_ppa_build_raw(client, &ppa.owner, &ppa.name, id)
            .await
            .map_err(|err| match err {
                LpError::NotFound(_) => {
                    LpError::NotFound(format!("Build #{id} not found in PPA {}.", ppa.label()))
                }
                other => other,
            })?;
        Ok((found, ppa.label()))
    } else {
        let found = packages::get_primary_build_raw(client, distro, id)
            .await
            .map_err(|err| match err {
                LpError::NotFound(_) => {
                    LpError::NotFound(format!("Build #{id} not found in {distro}."))
                }
                other => other,
            })?;
        Ok((found, distro.to_string()))
    }
}

async fn resolve_ppa_build_records_filtered(
    client: &LaunchpadClient,
    ppa: &PpaRef,
    filter: &BuildFilterRequest,
    canonical_series_name: Option<&str>,
) -> lpcli::error::Result<ResolvedBuilds> {
    verify_ppa_exists(client, ppa).await?;

    let limit = filter.limit as usize;
    let params = BuildRecordSearchParams {
        build_state: filter.state.first().map(String::as_str),
        pocket: filter.pocket.map(pocket_launchpad_name),
        source_name: filter.name.as_deref(),
        // Fetch pages of reasonable size so local filtering by arch/series
        // still finds matches without loading thousands of historical builds.
        limit: Some(filter.limit.clamp(50, 100)),
    };

    let mut page =
        packages::get_ppa_build_records_raw(client, &ppa.owner, &ppa.name, &params).await?;
    let mut matched = Vec::new();
    let mut hit_limit_with_more = false;

    loop {
        let raw_list = raw_entries(&page.raw);
        let next_link = page.value.next_collection_link.clone();
        for (idx, build) in page.value.entries.into_iter().enumerate() {
            let matches = build_matches_filters(
                &build,
                filter.name.as_deref(),
                canonical_series_name,
                &filter.arch,
                &filter.state,
                &filter.exclude_state,
                filter.pocket,
            );
            if !matches {
                continue;
            }
            if matched.len() == limit {
                hit_limit_with_more = true;
                break;
            }
            let raw = raw_list
                .get(idx)
                .cloned()
                .ok_or_else(|| LpError::Other("missing raw JSON for build record".into()))?;
            matched.push(Raw { value: build, raw });
        }

        if hit_limit_with_more || matched.len() >= limit {
            if next_link.is_some() {
                hit_limit_with_more = true;
            }
            break;
        }
        let Some(next) = next_link else {
            break;
        };
        page = packages::get_build_records_page_raw(client, &next).await?;
    }

    Ok(ResolvedBuilds {
        is_single: false,
        archive_label: ppa.label(),
        series: canonical_series_name.map(str::to_string),
        complete: !hit_limit_with_more,
        builds: matched,
    })
}

async fn resolve_target_builds(
    client: &LaunchpadClient,
    target: &BuildTarget,
) -> lpcli::error::Result<ResolvedBuilds> {
    match target {
        BuildTarget::Single {
            build_id,
            build_url,
            owner,
            ppa,
            distro,
        } => {
            let (found, archive_label) = resolve_single_build(
                client,
                *build_id,
                build_url.as_deref(),
                owner.as_deref(),
                ppa.as_deref(),
                distro,
            )
            .await?;
            let series = link_name(found.value.distro_series_link.as_deref());
            Ok(ResolvedBuilds {
                is_single: true,
                archive_label,
                series,
                complete: true,
                builds: vec![found],
            })
        }
        BuildTarget::Filter(filter) => {
            let (ppa_ref, canonical_series_name) = validate_archive_target(
                filter.owner.as_deref(),
                filter.ppa.as_deref(),
                filter.series.as_deref(),
                &filter.distro,
                filter.limit,
            )?;

            // Use PPA-level `getBuildRecords` when possible (avoids N+1 per
            // source publication and finds older failed builds across versions).
            if let Some(ppa) = ppa_ref.as_ref()
                && filter.version.is_none()
                && filter.state.len() <= 1
            {
                return resolve_ppa_build_records_filtered(
                    client,
                    ppa,
                    filter,
                    canonical_series_name.as_deref(),
                )
                .await;
            }

            let fetched = fetch_source_publications_and_builds(
                client,
                &SourceBuildQuery {
                    ppa_ref: ppa_ref.as_ref(),
                    canonical_series_name: canonical_series_name.as_deref(),
                    distro: &filter.distro,
                    name: filter.name.as_deref(),
                    version: filter.version.as_deref(),
                    pocket: filter.pocket,
                    limit: filter.limit,
                },
            )
            .await?;

            let mut matched = Vec::new();
            let mut hit_limit_with_more = fetched.page.value.next_collection_link.is_some();
            for (builds, raw_entries) in fetched.source_builds {
                for (idx, build) in builds.into_iter().enumerate() {
                    if !build_matches_filters(
                        &build,
                        filter.name.as_deref(),
                        canonical_series_name.as_deref(),
                        &filter.arch,
                        &filter.state,
                        &filter.exclude_state,
                        filter.pocket,
                    ) {
                        continue;
                    }
                    if matched.len() >= filter.limit as usize {
                        hit_limit_with_more = true;
                        break;
                    }
                    let raw = raw_entries.get(idx).cloned().ok_or_else(|| {
                        LpError::Other("missing raw JSON for build record".into())
                    })?;
                    matched.push(Raw { value: build, raw });
                }
                if matched.len() >= filter.limit as usize {
                    break;
                }
            }

            Ok(ResolvedBuilds {
                is_single: false,
                archive_label: fetched.archive_label,
                series: fetched.canonical_series_name,
                complete: !hit_limit_with_more,
                builds: matched,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// `package build show`
// ---------------------------------------------------------------------------

pub(crate) async fn build_show_report(
    client: &LaunchpadClient,
    build_id: Option<u64>,
    build_url: Option<&str>,
    owner: Option<&str>,
    ppa: Option<&str>,
    distro: &str,
) -> lpcli::error::Result<Report> {
    let (found, archive_label) =
        resolve_single_build(client, build_id, build_url, owner, ppa, distro).await?;
    Ok(format_build_show_report(&found, &archive_label))
}

pub(crate) fn format_build_show_report(found: &Raw<SourceBuild>, default_archive: &str) -> Report {
    let b = &found.value;
    let id = b.build_id().unwrap_or("?");
    let heading = b.title.clone().unwrap_or_else(|| {
        format!(
            "Build #{id} ({} {})",
            b.source_package_name.as_deref().unwrap_or("unknown"),
            b.source_package_version.as_deref().unwrap_or("unknown"),
        )
    });

    let mut report = entry_report(
        found.raw.clone(),
        heading,
        vec![
            ("Build ID", b.build_id().map(str::to_string)),
            ("Package", b.source_package_name.clone()),
            ("Version", b.source_package_version.clone()),
            (
                "Archive",
                b.archive_link
                    .as_deref()
                    .and_then(archive_label)
                    .or_else(|| Some(default_archive.to_string())),
            ),
            ("Series", link_name(b.distro_series_link.as_deref())),
            ("Pocket", b.pocket.clone()),
            ("Arch", b.arch_tag.clone()),
            ("State", b.buildstate.clone()),
            ("Can Retry", flag(b.can_be_retried)),
            ("Can Cancel", flag(b.can_be_cancelled)),
            ("Builder", link_name(b.builder_link.as_deref())),
        ],
    );

    for (label, ts) in [
        ("Created", b.date_created),
        ("Dispatched", b.date_first_dispatched),
        ("Started", b.date_started),
        ("Finished", b.date_built),
    ] {
        if let Some(ts) = ts {
            report
                .blocks
                .push(Block::Timestamp(label, ts, TimestampPrecision::DateTime));
        }
    }

    for (label, value) in [
        ("Duration", b.duration.clone()),
        ("Score", b.score.map(|s| s.to_string())),
        ("Dependencies", b.dependencies.clone()),
        ("External Deps", b.external_dependencies.clone()),
        ("Build Log", b.build_log_url.clone()),
        ("Upload Log", b.upload_log_url.clone()),
        ("Buildinfo", b.buildinfo_url.clone()),
        ("Changes", b.changesfile_url.clone()),
        ("URL", b.web_link.clone()),
    ] {
        if let Some(value) = value {
            report.blocks.push(Block::Field(label, value));
        }
    }

    report
}

// ---------------------------------------------------------------------------
// `package build logs`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct PlannedLogItem {
    pub(crate) build_self_link: Option<String>,
    pub(crate) build_id: Option<String>,
    pub(crate) kind: &'static str,
    pub(crate) url: String,
    pub(crate) filename_error: Option<String>,
}

/// Outcome of executing `package build logs`.
#[derive(Debug)]
pub(crate) struct BuildLogsExecution {
    pub(crate) archive_label: String,
    pub(crate) series: Option<String>,
    pub(crate) complete: bool,
    pub(crate) output_dir: PathBuf,
    pub(crate) raw_builds: Vec<Value>,
    pub(crate) items: Vec<PlannedLogItem>,
    pub(crate) outcomes: Vec<download::DownloadOutcome>,
    pub(crate) skipped_no_log: usize,
}

pub(crate) async fn handle_build_logs(
    client: &LaunchpadClient,
    output: &Output,
    request: &BuildLogsRequest,
) -> lpcli::error::Result<()> {
    let executed = execute_build_logs(client, request, output.progress(), |announcement| {
        if output.mode() == OutputMode::Rich && output.progress() == download::ProgressMode::Rich {
            output.write_stderr_report(announcement)
        } else {
            Ok(())
        }
    })
    .await?;
    output.print(&build_logs_report(&executed))?;
    build_logs_result(&executed)
}

pub(crate) async fn execute_build_logs(
    client: &LaunchpadClient,
    request: &BuildLogsRequest,
    progress: download::ProgressMode,
    announce: impl FnOnce(&Report) -> lpcli::error::Result<()>,
) -> lpcli::error::Result<BuildLogsExecution> {
    let resolved = resolve_target_builds(client, &request.target).await?;
    let mut raw_builds = Vec::with_capacity(resolved.builds.len());
    let mut raw_urls: Vec<(Option<String>, Option<String>, &'static str, String)> = Vec::new();
    let mut skipped_no_log = 0usize;

    for raw_build in resolved.builds {
        let b = &raw_build.value;
        let build_id = b.build_id().map(str::to_string);
        let self_link = b.self_link.clone();
        let mut build_had_log = false;

        if let Some(log_url) = &b.build_log_url {
            raw_urls.push((
                self_link.clone(),
                build_id.clone(),
                "build_log",
                log_url.clone(),
            ));
            build_had_log = true;
        }
        if request.include_upload_logs
            && let Some(upload_url) = &b.upload_log_url
        {
            raw_urls.push((self_link, build_id, "upload_log", upload_url.clone()));
            build_had_log = true;
        }
        if !build_had_log {
            skipped_no_log += 1;
        }
        raw_builds.push(raw_build.raw);
    }

    // Disambiguate filenames within the batch so multiple builds (or two uploads
    // sharing the same filename) never overwrite one another.
    let mut base_counts: HashMap<String, usize> = HashMap::new();
    let mut extracted = Vec::with_capacity(raw_urls.len());
    for (_, _, _, url) in &raw_urls {
        let res = download::extract_safe_filename(url);
        if let Ok(name) = &res {
            *base_counts.entry(name.clone()).or_insert(0) += 1;
        }
        extracted.push(res);
    }

    let mut used_filenames: HashSet<String> = HashSet::new();
    let mut items = Vec::with_capacity(raw_urls.len());
    let mut named_downloads = Vec::new();
    let mut valid_indices = Vec::new();

    for ((build_self_link, build_id, kind, url), name_res) in raw_urls.into_iter().zip(extracted) {
        match name_res {
            Ok(base_name) => {
                let needs_prefix = base_counts.get(&base_name).copied().unwrap_or(0) > 1;
                let mut candidate = if needs_prefix {
                    match build_id.as_deref() {
                        Some(id) => format!("build-{id}_{base_name}"),
                        None => base_name.clone(),
                    }
                } else {
                    base_name
                };
                if !used_filenames.insert(candidate.clone()) {
                    candidate = disambiguate_in_set(&candidate, &mut used_filenames);
                }
                valid_indices.push(items.len());
                named_downloads.push(download::NamedDownload {
                    url: url.clone(),
                    filename: candidate.clone(),
                });
                items.push(PlannedLogItem {
                    build_self_link,
                    build_id,
                    kind,
                    url,
                    filename_error: None,
                });
            }
            Err(err) => {
                items.push(PlannedLogItem {
                    build_self_link,
                    build_id,
                    kind,
                    url,
                    filename_error: Some(output::redact_secrets(&err.to_string())),
                });
            }
        }
    }

    if !named_downloads.is_empty() {
        announce(&Report {
            blocks: vec![Block::Note(vec![Span::Text(format!(
                "Downloading {} build log(s) to '{}'...",
                named_downloads.len(),
                request.output_dir.display()
            ))])],
            json: Value::Null,
            records: no_records(),
        })?;
    }

    let batch_outcomes = download::download_named_public_files_with_progress(
        client,
        &named_downloads,
        &request.output_dir,
        progress,
    )
    .await?;

    let mut batch_iter = batch_outcomes.into_iter();
    let mut outcomes = Vec::with_capacity(items.len());
    let is_ppa = resolved.archive_label.starts_with('~');

    for item in &items {
        if let Some(err) = &item.filename_error {
            outcomes.push(download::DownloadOutcome::Failed {
                filename: download_fallback_name(&item.url),
                error: err.clone(),
            });
            continue;
        }
        let outcome = batch_iter.next().expect("matching outcome per valid item");
        outcomes.push(match outcome {
            download::DownloadOutcome::Failed { filename, error }
                if is_ppa
                    && (error.contains("HTTP 401")
                        || error.contains("HTTP 403")
                        || error.contains("does not exist on Launchpad")) =>
            {
                download::DownloadOutcome::Failed {
                    filename,
                    error: format!(
                        "{error} (Private PPA build logs cannot be fetched without librarian authentication.)"
                    ),
                }
            }
            other => other,
        });
    }

    Ok(BuildLogsExecution {
        archive_label: resolved.archive_label,
        series: resolved.series,
        complete: resolved.complete,
        output_dir: request.output_dir.clone(),
        raw_builds,
        items,
        outcomes,
        skipped_no_log,
    })
}

fn download_fallback_name(url: &str) -> String {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    output::redact_secrets(without_query)
}

fn disambiguate_in_set(candidate: &str, used: &mut HashSet<String>) -> String {
    let (stem, ext) = split_archive_extension(candidate);
    let mut counter = 2usize;
    loop {
        let next = format!("{stem}-{counter}{ext}");
        if used.insert(next.clone()) {
            return next;
        }
        counter += 1;
    }
}

fn split_archive_extension(filename: &str) -> (&str, &str) {
    for suffix in [".txt.gz", ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"] {
        if let Some(stem) = filename.strip_suffix(suffix) {
            return (stem, suffix);
        }
    }
    match filename.rsplit_once('.') {
        Some((stem, _ext)) if !stem.is_empty() => (stem, &filename[stem.len()..]),
        _ => (filename, ""),
    }
}

pub(crate) fn build_logs_report(executed: &BuildLogsExecution) -> Report {
    let output_dir = executed.output_dir.to_string_lossy().into_owned();
    let mut blocks = vec![Block::Heading(format!(
        "Build logs for {}",
        executed.archive_label
    ))];
    if let Some(series) = &executed.series {
        blocks.push(Block::Field("Series", series.clone()));
    }

    let mut downloads_json = Vec::with_capacity(executed.outcomes.len());
    let mut downloaded = 0usize;
    let mut failed = 0usize;

    for (item, outcome) in executed.items.iter().zip(executed.outcomes.iter()) {
        match outcome {
            download::DownloadOutcome::Success(file) => {
                downloaded += 1;
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
                downloads_json.push(serde_json::json!({
                    "build_self_link": item.build_self_link,
                    "build_id": item.build_id,
                    "kind": item.kind,
                    "filename": filename,
                    "status": "downloaded",
                    "path": file.path.to_string_lossy(),
                    "size": file.size,
                }));
            }
            download::DownloadOutcome::Failed { filename, error } => {
                failed += 1;
                let filename = output::redact_secrets(filename);
                let error = output::redact_secrets(error);
                blocks.push(Block::Status(
                    Level::Error,
                    vec![
                        Span::Name(filename.clone()),
                        Span::Text(format!(": {error}")),
                    ],
                ));
                downloads_json.push(serde_json::json!({
                    "build_self_link": item.build_self_link,
                    "build_id": item.build_id,
                    "kind": item.kind,
                    "filename": filename,
                    "status": "failed",
                    "error": error,
                }));
            }
        }
    }

    if executed.raw_builds.is_empty() {
        blocks.push(Block::Note(vec![Span::Text(
            "No matching builds found.".into(),
        )]));
    } else if executed.outcomes.is_empty() {
        blocks.push(Block::Status(
            Level::Warning,
            vec![Span::Text(format!(
                "No build logs available ({} build(s) skipped with no log URL).",
                executed.skipped_no_log
            ))],
        ));
    } else {
        blocks.push(Block::Break);
        let (level, mut summary) = match (downloaded, failed) {
            (_, 0) => (
                Level::Ok,
                format!("Downloaded {downloaded} build log(s) to '{output_dir}'."),
            ),
            (0, _) => (
                Level::Error,
                format!("No build logs downloaded; {failed} failed."),
            ),
            _ => (
                Level::Warning,
                format!("{downloaded} build log(s) downloaded to '{output_dir}', {failed} failed."),
            ),
        };
        if executed.skipped_no_log > 0 {
            summary.push_str(&format!(
                " ({} build(s) had no log URL.)",
                executed.skipped_no_log
            ));
        }
        blocks.push(Block::Status(level, vec![Span::Text(summary)]));
    }

    Report {
        blocks,
        json: serde_json::json!({
            "builds": {
                "entries": executed.raw_builds,
            },
            "lpcli": {
                "archive": executed.archive_label,
                "series": executed.series,
                "complete": executed.complete,
                "output_directory": output_dir,
                "downloads": downloads_json,
                "downloaded": downloaded,
                "skipped_no_log": executed.skipped_no_log,
                "failed": failed,
            },
        }),
        records: no_records(),
    }
}

pub(crate) fn build_logs_result(executed: &BuildLogsExecution) -> lpcli::error::Result<()> {
    let mut downloaded = 0usize;
    let mut failures = Vec::new();
    for outcome in &executed.outcomes {
        match outcome {
            download::DownloadOutcome::Success(_) => downloaded += 1,
            download::DownloadOutcome::Failed { filename, error } => {
                failures.push(format!("{filename} ({error})"));
            }
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    let total = executed.outcomes.len();
    let list = failures.join("; ");
    if downloaded == 0 {
        Err(LpError::Other(format!(
            "Failed to download all {total} build log(s): {list}"
        )))
    } else {
        Err(LpError::Partial(format!(
            "Downloaded {downloaded} of {total} build log(s); failed: {list}"
        )))
    }
}

// ---------------------------------------------------------------------------
// `package build retry`
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct RetryCandidate {
    pub(crate) build: SourceBuild,
    pub(crate) raw: Value,
    pub(crate) self_link: String,
    pub(crate) series: Option<String>,
}

#[derive(Debug, Clone)]
struct SkippedRetry {
    self_link: Option<String>,
    build_id: Option<String>,
    package: Option<String>,
    version: Option<String>,
    series: Option<String>,
    arch: Option<String>,
    state: Option<String>,
    reason: String,
}

#[derive(Debug)]
struct RetryFailure {
    operation: &'static str,
    build_self_link: String,
    build_id: Option<String>,
    error: LpError,
}

impl RetryFailure {
    fn to_json(&self) -> Value {
        serde_json::json!({
            "operation": self.operation,
            "build_self_link": self.build_self_link,
            "build_id": self.build_id,
            "error": {
                "kind": self.error.kind().name(),
                "message": output::redact_secrets(&self.error.to_string()),
                "http_status": self.error.http_status(),
            },
        })
    }
}

#[derive(Debug, Clone)]
struct SavedLogRecord {
    build_self_link: String,
    build_id: Option<String>,
    kind: &'static str,
    path: Option<PathBuf>,
    url: String,
    size: Option<u64>,
}

impl SavedLogRecord {
    fn to_json(&self) -> Value {
        serde_json::json!({
            "build_self_link": self.build_self_link,
            "build_id": self.build_id,
            "kind": self.kind,
            "url": self.url,
            "path": self.path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "size": self.size,
        })
    }
}

/// Outcome of executing `package build retry`.
#[derive(Debug)]
pub(crate) struct BuildRetryOutcome {
    pub(crate) report: Report,
    pub(crate) retried_links: Vec<String>,
    pub(crate) result: lpcli::error::Result<()>,
}

pub(crate) async fn handle_build_retry(
    client: &LaunchpadClient,
    output: &Output,
    quiet: bool,
    no_input: bool,
    request: &BuildRetryRequest,
) -> lpcli::error::Result<()> {
    let outcome = execute_build_retry(client, no_input, request, |candidates| {
        confirm_bulk_retry(output, candidates, request.save_logs.as_deref())
    })
    .await?;
    if quiet {
        for link in &outcome.retried_links {
            output.print_quiet(&serde_json::json!({ "self_link": link }))?;
        }
    } else {
        output.print(&outcome.report)?;
    }
    outcome.result
}

fn confirm_bulk_retry(
    output: &Output,
    candidates: &[RetryCandidate],
    save_logs: Option<&Path>,
) -> lpcli::error::Result<bool> {
    use std::io::{BufRead as _, Read as _};

    let _ = output.write_stderr_line(&format!("Builds to retry ({}):", candidates.len()));
    for candidate in candidates {
        let b = &candidate.build;
        let id = b.build_id().unwrap_or("?");
        let pkg = b.source_package_name.as_deref().unwrap_or("unknown");
        let ver = b.source_package_version.as_deref().unwrap_or("unknown");
        let series = candidate.series.as_deref().unwrap_or("unknown");
        let arch = b.arch_tag.as_deref().unwrap_or("unknown");
        let state = b.buildstate.as_deref().unwrap_or("unknown");
        let _ =
            output.write_stderr_line(&format!("  - #{id} {pkg} {ver} ({series}/{arch}, {state})"));
    }
    if let Some(dir) = save_logs {
        let _ = output.write_stderr_line(&format!(
            "Existing logs will be saved to '{}' before retrying.",
            dir.display()
        ));
    }

    let mut stderr = std::io::stderr().lock();
    write!(stderr, "Retry {} build(s)? [y/N]: ", candidates.len())?;
    stderr.flush()?;
    drop(stderr);

    let mut input = String::new();
    let bytes = std::io::stdin().lock().take(64).read_line(&mut input)?;
    if bytes == 0 {
        return Ok(false);
    }
    let answer = input.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

pub(crate) async fn execute_build_retry<F>(
    client: &LaunchpadClient,
    no_input: bool,
    request: &BuildRetryRequest,
    confirm: F,
) -> lpcli::error::Result<BuildRetryOutcome>
where
    F: FnOnce(&[RetryCandidate]) -> lpcli::error::Result<bool>,
{
    if let Some(save_dir) = &request.save_logs
        && save_dir.as_os_str().to_string_lossy().trim().is_empty()
    {
        return Err(LpError::Usage(
            "--save-logs directory must not be empty.".into(),
        ));
    }

    let resolved = resolve_target_builds(client, &request.target).await?;

    let mut eligible = Vec::new();
    let mut skipped = Vec::new();

    for raw_build in resolved.builds {
        let b = raw_build.value;
        let series = link_name(b.distro_series_link.as_deref()).or_else(|| resolved.series.clone());
        let state = b.buildstate.clone();
        if b.can_be_retried == Some(true) {
            let self_link = b
                .self_link
                .clone()
                .ok_or_else(|| LpError::Other("build record has no self_link".into()))?;
            eligible.push(RetryCandidate {
                build: b,
                raw: raw_build.raw,
                self_link,
                series,
            });
        } else {
            let reason = match state.as_deref() {
                Some(s) => format!("not retryable (state: {s}, can_be_retried is false)"),
                None => "not retryable (can_be_retried is false)".to_string(),
            };
            skipped.push(SkippedRetry {
                self_link: b.self_link.clone(),
                build_id: b.build_id().map(str::to_string),
                package: b.source_package_name.clone(),
                version: b.source_package_version.clone(),
                series,
                arch: b.arch_tag.clone(),
                state,
                reason,
            });
        }
    }

    if resolved.is_single && !request.dry_run && eligible.is_empty() {
        let skip = skipped
            .first()
            .expect("single build query yielded one build");
        let id = skip.build_id.as_deref().unwrap_or("?");
        return Err(LpError::Usage(format!(
            "Build #{id} ({} {}, {}/{}) is in state '{}' and cannot be retried (can_be_retried is false).",
            skip.package.as_deref().unwrap_or("unknown"),
            skip.version.as_deref().unwrap_or("unknown"),
            skip.series.as_deref().unwrap_or("unknown"),
            skip.arch.as_deref().unwrap_or("unknown"),
            skip.state.as_deref().unwrap_or("unknown"),
        )));
    }

    if request.dry_run {
        let planned_saves = if let Some(save_dir) = &request.save_logs {
            collect_planned_log_saves(&eligible, save_dir)
        } else {
            Vec::new()
        };
        let report = build_retry_report(
            &resolved.archive_label,
            resolved.series.as_deref(),
            true,
            false,
            &eligible,
            &[],
            &skipped,
            &planned_saves,
            &[],
        );
        return Ok(BuildRetryOutcome {
            report,
            retried_links: Vec::new(),
            result: Ok(()),
        });
    }

    if eligible.is_empty() {
        let report = build_retry_report(
            &resolved.archive_label,
            resolved.series.as_deref(),
            false,
            false,
            &eligible,
            &[],
            &skipped,
            &[],
            &[],
        );
        return Ok(BuildRetryOutcome {
            report,
            retried_links: Vec::new(),
            result: Ok(()),
        });
    }

    if !resolved.is_single && !request.yes {
        if no_input {
            return Err(LpError::Usage(
                "bulk build retry needs interactive confirmation; rerun from a terminal, pass --yes, or use --dry-run to preview"
                    .into(),
            ));
        }
        if !confirm(&eligible)? {
            let report = build_retry_report(
                &resolved.archive_label,
                resolved.series.as_deref(),
                false,
                true,
                &eligible,
                &[],
                &skipped,
                &[],
                &[],
            );
            return Ok(BuildRetryOutcome {
                report,
                retried_links: Vec::new(),
                result: Ok(()),
            });
        }
    }

    let mut retried_links = Vec::new();
    let mut retried_raw = Vec::new();
    let mut saved_logs = Vec::new();
    let mut failures = Vec::new();
    let mut used_backup_names = HashSet::new();
    let mut any_mutated = false;

    for candidate in &eligible {
        let build_id = candidate.build.build_id().map(str::to_string);
        if let Some(save_dir) = &request.save_logs {
            match save_build_logs_before_retry(
                client,
                candidate,
                save_dir,
                &mut used_backup_names,
                &mut saved_logs,
            )
            .await
            {
                Ok(()) => {}
                Err(error) => {
                    failures.push(RetryFailure {
                        operation: "save_log",
                        build_self_link: candidate.self_link.clone(),
                        build_id,
                        error,
                    });
                    // Do not call POST retry when backing up an available log fails.
                    continue;
                }
            }
        }

        match packages::retry_build_raw(client, &candidate.self_link).await {
            Ok(updated) => {
                any_mutated = true;
                retried_links.push(candidate.self_link.clone());
                retried_raw.push(updated.raw);
            }
            Err(err @ LpError::Partial(_)) => {
                any_mutated = true;
                failures.push(RetryFailure {
                    operation: "retry_readback",
                    build_self_link: candidate.self_link.clone(),
                    build_id,
                    error: err,
                });
            }
            Err(err) => {
                failures.push(RetryFailure {
                    operation: "retry",
                    build_self_link: candidate.self_link.clone(),
                    build_id,
                    error: err,
                });
            }
        }
    }

    let report = build_retry_report(
        &resolved.archive_label,
        resolved.series.as_deref(),
        false,
        false,
        &eligible,
        &retried_raw,
        &skipped,
        &saved_logs,
        &failures,
    );

    let result = if failures.is_empty() {
        Ok(())
    } else if any_mutated || !retried_links.is_empty() {
        let list = failures
            .iter()
            .map(|f| format!("{} ({})", f.build_self_link, f.error))
            .collect::<Vec<_>>()
            .join("; ");
        Err(LpError::Partial(format!(
            "Retried {} of {} eligible build(s); failed: {list}",
            retried_links.len(),
            eligible.len()
        )))
    } else if failures.len() == 1 {
        Err(failures.into_iter().next().expect("one failure").error)
    } else {
        let list = failures
            .iter()
            .map(|f| format!("{} ({})", f.build_self_link, f.error))
            .collect::<Vec<_>>()
            .join("; ");
        Err(LpError::Other(format!(
            "Failed to retry all {} eligible build(s): {list}",
            eligible.len()
        )))
    };

    Ok(BuildRetryOutcome {
        report,
        retried_links,
        result,
    })
}

fn collect_planned_log_saves(
    candidates: &[RetryCandidate],
    save_dir: &Path,
) -> Vec<SavedLogRecord> {
    let mut used = HashSet::new();
    let mut planned = Vec::new();
    for candidate in candidates {
        let b = &candidate.build;
        let build_id = b.build_id().map(str::to_string);
        for (kind, url_opt) in [
            ("build_log", b.build_log_url.as_deref()),
            ("upload_log", b.upload_log_url.as_deref()),
        ] {
            let Some(url) = url_opt else {
                continue;
            };
            let path = download::extract_safe_filename(url)
                .ok()
                .map(|base| unique_backup_path(save_dir, build_id.as_deref(), &base, &mut used));
            planned.push(SavedLogRecord {
                build_self_link: candidate.self_link.clone(),
                build_id: build_id.clone(),
                kind,
                path,
                url: url.to_string(),
                size: None,
            });
        }
    }
    planned
}

async fn save_build_logs_before_retry(
    client: &LaunchpadClient,
    candidate: &RetryCandidate,
    save_dir: &Path,
    used_names: &mut HashSet<String>,
    saved_logs: &mut Vec<SavedLogRecord>,
) -> lpcli::error::Result<()> {
    let b = &candidate.build;
    let build_id = b.build_id().map(str::to_string);
    for (kind, url_opt) in [
        ("build_log", b.build_log_url.as_deref()),
        ("upload_log", b.upload_log_url.as_deref()),
    ] {
        let Some(url) = url_opt else {
            continue;
        };
        let base_name = download::extract_safe_filename(url)?;
        let dest = unique_backup_path(save_dir, build_id.as_deref(), &base_name, used_names);
        let downloaded = download::download_public_file(client, url, &dest).await?;
        saved_logs.push(SavedLogRecord {
            build_self_link: candidate.self_link.clone(),
            build_id: build_id.clone(),
            kind,
            path: Some(downloaded.path),
            url: url.to_string(),
            size: Some(downloaded.size),
        });
    }
    Ok(())
}

/// Choose a collision-free backup path under `dir` without overwriting any
/// existing file on disk or any filename already claimed in the current run.
fn unique_backup_path(
    dir: &Path,
    build_id: Option<&str>,
    base_name: &str,
    used_names: &mut HashSet<String>,
) -> PathBuf {
    let prefixed = match build_id {
        Some(id) if !base_name.starts_with(&format!("build-{id}_")) => {
            format!("build-{id}_{base_name}")
        }
        _ => base_name.to_string(),
    };
    let (stem, ext) = split_archive_extension(&prefixed);
    let mut candidate = prefixed.clone();
    let mut counter = 2usize;
    while used_names.contains(&candidate) || dir.join(&candidate).exists() {
        candidate = format!("{stem}-{counter}{ext}");
        counter += 1;
    }
    used_names.insert(candidate.clone());
    dir.join(candidate)
}

#[allow(clippy::too_many_arguments)]
fn build_retry_report(
    archive_label: &str,
    series: Option<&str>,
    dry_run: bool,
    cancelled: bool,
    eligible: &[RetryCandidate],
    retried_raw: &[Value],
    skipped: &[SkippedRetry],
    saved_logs: &[SavedLogRecord],
    failures: &[RetryFailure],
) -> Report {
    let heading = if dry_run {
        format!("Build retry dry run for {archive_label}")
    } else if cancelled {
        format!("Build retry cancelled for {archive_label}")
    } else {
        format!("Build retry for {archive_label}")
    };
    let mut blocks = vec![Block::Heading(heading)];
    if let Some(series) = series {
        blocks.push(Block::Field("Series", series.to_string()));
    }

    if dry_run {
        if eligible.is_empty() && skipped.is_empty() {
            blocks.push(Block::Note(vec![Span::Text(
                "No matching builds found.".into(),
            )]));
        } else {
            blocks.push(Block::Status(
                Level::Ok,
                vec![Span::Text(format!(
                    "Dry run: {} eligible build(s) would be retried, {} skipped.",
                    eligible.len(),
                    skipped.len()
                ))],
            ));
            for candidate in eligible {
                let b = &candidate.build;
                blocks.push(Block::Note(vec![Span::Text(format!(
                    "Would retry #{} {} {} ({}/{}, {})",
                    b.build_id().unwrap_or("?"),
                    b.source_package_name.as_deref().unwrap_or("unknown"),
                    b.source_package_version.as_deref().unwrap_or("unknown"),
                    candidate.series.as_deref().unwrap_or("unknown"),
                    b.arch_tag.as_deref().unwrap_or("unknown"),
                    b.buildstate.as_deref().unwrap_or("unknown"),
                ))]));
            }
            if !saved_logs.is_empty() {
                blocks.push(Block::Note(vec![Span::Text(format!(
                    "Would back up {} existing log(s) before retrying.",
                    saved_logs.len()
                ))]));
            }
        }
    } else if cancelled {
        blocks.push(Block::Status(
            Level::Warning,
            vec![Span::Text(format!(
                "Cancelled: {} eligible build(s) not retried.",
                eligible.len()
            ))],
        ));
    } else if eligible.is_empty() && skipped.is_empty() {
        blocks.push(Block::Note(vec![Span::Text(
            "No matching builds found.".into(),
        )]));
    } else {
        for saved in saved_logs {
            if let (Some(path), Some(size)) = (&saved.path, saved.size) {
                let filename = path.file_name().map_or_else(
                    || path.to_string_lossy().into_owned(),
                    |n| n.to_string_lossy().into_owned(),
                );
                blocks.push(Block::Status(
                    Level::Ok,
                    vec![
                        Span::Text("Saved log ".into()),
                        Span::Name(filename),
                        Span::Text(format!(" ({})", format_size(size))),
                    ],
                ));
            }
        }
        for raw in retried_raw {
            let link = raw
                .get("self_link")
                .and_then(Value::as_str)
                .unwrap_or("build");
            let state = raw
                .get("buildstate")
                .and_then(Value::as_str)
                .unwrap_or("Needs building");
            blocks.push(Block::Status(
                Level::Ok,
                vec![
                    Span::Text("Retried ".into()),
                    Span::Name(link.to_string()),
                    Span::Text(format!(" ({state})")),
                ],
            ));
        }
        for failure in failures {
            blocks.push(Block::Status(
                Level::Error,
                vec![
                    Span::Text(format!(
                        "{} {}: ",
                        failure.operation, failure.build_self_link
                    )),
                    Span::Text(output::redact_secrets(&failure.error.to_string())),
                ],
            ));
        }
        blocks.push(Block::Break);
        let summary_level = if failures.is_empty() {
            Level::Ok
        } else if retried_raw.is_empty() {
            Level::Error
        } else {
            Level::Warning
        };
        blocks.push(Block::Status(
            summary_level,
            vec![Span::Text(format!(
                "Retried {} build(s), skipped {}, failed {}.",
                retried_raw.len(),
                skipped.len(),
                failures.len()
            ))],
        ));
    }

    for skip in skipped {
        blocks.push(Block::Note(vec![Span::Text(format!(
            "Skipped #{} {} {} ({}/{}): {}",
            skip.build_id.as_deref().unwrap_or("?"),
            skip.package.as_deref().unwrap_or("unknown"),
            skip.version.as_deref().unwrap_or("unknown"),
            skip.series.as_deref().unwrap_or("unknown"),
            skip.arch.as_deref().unwrap_or("unknown"),
            skip.reason,
        ))]));
    }

    let eligible_links: Vec<Value> = eligible
        .iter()
        .map(|c| Value::String(c.self_link.clone()))
        .collect();
    let retried_links: Vec<Value> = retried_raw
        .iter()
        .filter_map(|raw| raw.get("self_link").cloned())
        .collect();
    let skipped_json: Vec<Value> = skipped
        .iter()
        .map(|s| {
            serde_json::json!({
                "build_self_link": s.self_link,
                "build_id": s.build_id,
                "source_package_name": s.package,
                "source_package_version": s.version,
                "series": s.series,
                "arch_tag": s.arch,
                "buildstate": s.state,
                "reason": s.reason,
            })
        })
        .collect();
    let saved_logs_json: Vec<Value> = saved_logs.iter().map(SavedLogRecord::to_json).collect();
    let failures_json: Vec<Value> = failures.iter().map(RetryFailure::to_json).collect();
    let builds_entries: Vec<Value> = if dry_run || cancelled {
        eligible.iter().map(|c| c.raw.clone()).collect()
    } else {
        retried_raw.to_vec()
    };

    Report {
        blocks,
        json: serde_json::json!({
            "builds": {
                "entries": builds_entries,
            },
            "lpcli": {
                "archive": archive_label,
                "series": series,
                "dry_run": dry_run,
                "cancelled": cancelled,
                "eligible": eligible_links,
                "retried": retried_links,
                "skipped": skipped_json,
                "saved_logs": saved_logs_json,
                "failures": failures_json,
            },
        }),
        records: no_records(),
    }
}
