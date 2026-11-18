//! Upload-queue command handlers.

use crate::*;

// ---------------------------------------------------------------------------
// Queue handler
// ---------------------------------------------------------------------------

/// A queue archive resolved to a Launchpad API URL and a display label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueueArchive {
    /// Full Launchpad API URL passed to `getPackageUploads`.
    pub(crate) api_url: String,
    /// Short label used in output, e.g. `ubuntu` or `~owner/ppa`.
    pub(crate) label: String,
}

impl QueueArchive {
    /// Describe the target as `distro/series`, appending the archive when it
    /// is not the distribution's primary archive.
    pub(crate) fn target_label(&self, distro: &str, series: &str) -> String {
        if self.label.eq_ignore_ascii_case(distro) {
            format!("{distro}/{series}")
        } else {
            format!("{distro}/{series} ({})", self.label)
        }
    }
}

/// Resolve `--archive`/`--ppa` to the Launchpad archive API URL.
pub(crate) fn resolve_queue_archive(
    client: &LaunchpadClient,
    distro: &str,
    archive: Option<&str>,
    ppa: Option<&PpaRef>,
) -> lpcli::error::Result<QueueArchive> {
    if let Some(ppa) = ppa {
        return ppa_queue_archive(client, distro, ppa);
    }
    let Some(archive) = archive else {
        return Ok(primary_queue_archive(client, distro));
    };
    let value = archive.trim();
    if value.is_empty() {
        return Err(lpcli::error::LpError::Other(
            "--archive must not be empty".to_string(),
        ));
    }
    if let Some(ppa) = value.strip_prefix("ppa:") {
        let ppa = parse_ppa_ref(ppa).map_err(lpcli::error::LpError::Other)?;
        return ppa_queue_archive(client, distro, &ppa);
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        let label = archive_label(value).unwrap_or_else(|| value.trim_end_matches('/').to_string());
        if label.starts_with('~') {
            validate_ppa_distro(distro)?;
        }
        return Ok(QueueArchive {
            api_url: value.to_string(),
            label,
        });
    }
    if value.contains("/+archive/") {
        let trimmed = value.trim_matches('/');
        let label = archive_label(trimmed).unwrap_or_else(|| trimmed.to_string());
        if label.starts_with('~') {
            validate_ppa_distro(distro)?;
        }
        return Ok(QueueArchive {
            api_url: client.url(&format!("/{trimmed}")),
            label,
        });
    }
    if value.eq_ignore_ascii_case("primary") {
        return Ok(primary_queue_archive(client, distro));
    }
    Ok(QueueArchive {
        api_url: client.url(&format!("/{}/+archive/{}", distro, value)),
        label: format!("{distro}/{value}"),
    })
}

/// The primary archive for a distribution.
pub(crate) fn primary_queue_archive(client: &LaunchpadClient, distro: &str) -> QueueArchive {
    QueueArchive {
        api_url: client.url(&format!("/{distro}/+archive/primary")),
        label: distro.to_string(),
    }
}

/// Reject PPA-shaped archive targets for non-Ubuntu distributions.
pub(crate) fn validate_ppa_distro(distro: &str) -> lpcli::error::Result<()> {
    if distro.eq_ignore_ascii_case("ubuntu") {
        Ok(())
    } else {
        Err(lpcli::error::LpError::Other(format!(
            "PPA archives are only available for Ubuntu, not '{distro}'"
        )))
    }
}

/// A PPA archive; PPAs live under the Ubuntu distribution.
pub(crate) fn ppa_queue_archive(
    client: &LaunchpadClient,
    distro: &str,
    ppa: &PpaRef,
) -> lpcli::error::Result<QueueArchive> {
    validate_ppa_distro(distro)?;
    Ok(QueueArchive {
        api_url: client.url(&ppa.api_path()),
        label: ppa.label(),
    })
}

/// Derive a short label from an archive reference or API URL.
///
/// Recognises both `distro/+archive/name` and `~owner/+archive/distro/name`.
pub(crate) fn archive_label(reference: &str) -> Option<String> {
    let path = url::Url::parse(reference)
        .ok()
        .map(|url| url.path().to_string())
        .unwrap_or_else(|| reference.to_string());
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let archive_index = segments.iter().position(|segment| *segment == "+archive")?;
    let distro = segments.get(archive_index.checked_sub(1)?)?;
    if distro.starts_with('~') {
        let name = segments.get(archive_index + 2)?;
        return Some(format!("{distro}/{name}"));
    }
    let name = segments.get(archive_index + 1)?;
    if name.eq_ignore_ascii_case("primary") {
        Some((*distro).to_string())
    } else {
        Some(format!("{distro}/{name}"))
    }
}

/// Add archive context to an HTTP 401/403 error from a queue mutation.
pub(crate) fn queue_mutation_permission_error(
    err: lpcli::error::LpError,
    action: &str,
    archive: &QueueArchive,
    distro: &str,
    series: &str,
) -> lpcli::error::LpError {
    match err {
        lpcli::error::LpError::Api { status, message } if status == 401 || status == 403 => {
            lpcli::error::LpError::Unauthorized(format!(
                "You do not have queue-admin permission to {action} packages in {}. {message}",
                archive.target_label(distro, series),
            ))
        }
        other => other,
    }
}

/// Add archive context to an HTTP 401/403 error from a queue read.
pub(crate) fn queue_read_permission_error(
    err: lpcli::error::LpError,
    archive: &QueueArchive,
    distro: &str,
    series: &str,
) -> lpcli::error::LpError {
    match err {
        lpcli::error::LpError::Api { status, message } if status == 401 || status == 403 => {
            lpcli::error::LpError::Unauthorized(format!(
                "You do not have permission to read the upload queue for {}. {message}",
                archive.target_label(distro, series),
            ))
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Output targets
// ---------------------------------------------------------------------------

/// Where a queue handler writes its results.
pub(crate) trait QueueOutput {
    fn queue_output(&self) -> std::borrow::Cow<'_, Output>;
}

impl QueueOutput for Output {
    fn queue_output(&self) -> std::borrow::Cow<'_, Output> {
        std::borrow::Cow::Borrowed(self)
    }
}

/// Handler tests written before the output layer pass a [`Display`]; it
/// selects uncolored `rich` output at that width.
#[cfg(test)]
impl QueueOutput for Display {
    fn queue_output(&self) -> std::borrow::Cow<'_, Output> {
        std::borrow::Cow::Owned(Output::fixed(OutputMode::Rich, false, self.width()))
    }
}

// ---------------------------------------------------------------------------
// Queue handler
// ---------------------------------------------------------------------------

pub(crate) async fn handle_queue(
    cmd: QueueCommand,
    output: &Output,
    quiet: bool,
    policy: ReadAuthPolicy,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let client = read_client(policy, cache_timeout)?;
    match cmd {
        QueueCommand::Search {
            pocket,
            status,
            name,
            version,
            exact_match,
            archive,
            ppa,
            distro,
            series,
            limit,
        } => {
            let archive =
                resolve_queue_archive(&client, &distro, archive.as_deref(), ppa.as_ref())?;
            handle_queue_search(
                output,
                &client,
                pocket,
                status,
                name,
                version,
                exact_match,
                &archive,
                distro,
                series,
                limit,
            )
            .await
        }
        QueueCommand::Download {
            name,
            version,
            status,
            arch,
            archive,
            ppa,
            distro,
            series,
            output: output_dir,
        } => {
            let archive =
                resolve_queue_archive(&client, &distro, archive.as_deref(), ppa.as_ref())?;
            handle_queue_download(
                output, &client, name, version, status, arch, &archive, distro, series, output_dir,
            )
            .await
        }
        QueueCommand::Info {
            name,
            version,
            status,
            arch,
            archive,
            ppa,
            distro,
            series,
        } => {
            let archive =
                resolve_queue_archive(&client, &distro, archive.as_deref(), ppa.as_ref())?;
            handle_queue_info(
                output, &client, name, version, status, arch, &archive, distro, series,
            )
            .await
        }
        QueueCommand::Accept {
            name,
            version,
            status,
            arch,
            archive,
            ppa,
            distro,
            series,
        } => {
            let archive =
                resolve_queue_archive(&client, &distro, archive.as_deref(), ppa.as_ref())?;
            let accepted = handle_queue_accept(
                &client, name, version, status, arch, &archive, distro, series,
            )
            .await?;
            print_queue_mutation(output, quiet, &client, &accepted).await
        }
        QueueCommand::Reject {
            name,
            version,
            status,
            arch,
            archive,
            ppa,
            comment,
            distro,
            series,
        } => {
            let archive =
                resolve_queue_archive(&client, &distro, archive.as_deref(), ppa.as_ref())?;
            let rejected = handle_queue_reject(
                &client, name, version, status, arch, &archive, comment, distro, series,
            )
            .await?;
            print_queue_mutation(output, quiet, &client, &rejected).await
        }
    }
}

pub(crate) fn queue_supports_json(_command: &QueueCommand) -> bool {
    true
}

pub(crate) fn queue_supports_tsv(command: &QueueCommand) -> bool {
    matches!(command, QueueCommand::Search { .. })
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The requested series, or the distribution's current development series.
async fn resolve_queue_series(
    client: &LaunchpadClient,
    distro: &str,
    series: Option<String>,
) -> lpcli::error::Result<String> {
    match series {
        Some(s) => Ok(canonical_series(distro, &s).to_string()),
        None => queue::get_devel_series_name(client, distro).await,
    }
}

/// Whether an upload's `display_arches` include `arch`.
fn upload_has_arch(upload: &queue::PackageUpload, arch: QueueArch) -> bool {
    upload
        .display_arches
        .as_deref()
        .unwrap_or("")
        .split(',')
        .any(|a| a.trim().eq_ignore_ascii_case(arch.as_str()))
}

/// Search parameters for one exactly named queue upload.
fn exact_upload_params<'a>(
    archive: &'a QueueArchive,
    name: &'a str,
    version: Option<&'a str>,
    status: &'a str,
) -> QueueSearchParams<'a> {
    QueueSearchParams {
        archive: Some(&archive.api_url),
        status: Some(status),
        name: Some(name),
        version,
        exact_match: true,
        limit: Some(30),
        ..Default::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn upload_not_found(
    name: &str,
    version: Option<&str>,
    arch: QueueArch,
    status: &str,
    archive: &QueueArchive,
    distro: &str,
    series: &str,
) -> lpcli::error::LpError {
    lpcli::error::LpError::LocalNotFound(format!(
        "No package '{}'{} [arch={}] found in queue with status '{}' for {}.",
        name,
        version.map(|v| format!(" ({v})")).unwrap_or_default(),
        arch,
        status,
        archive.target_label(distro, series),
    ))
}

fn missing_self_link() -> lpcli::error::LpError {
    lpcli::error::LpError::Other("Package upload has no self_link.".to_string())
}

/// Find the first upload for `arch`, keeping its exact Launchpad entry.
#[allow(clippy::too_many_arguments)]
async fn find_queue_upload(
    client: &LaunchpadClient,
    name: &str,
    version: Option<&str>,
    status: &str,
    arch: QueueArch,
    archive: &QueueArchive,
    distro: &str,
    series: &str,
) -> lpcli::error::Result<Raw<queue::PackageUpload>> {
    let params = exact_upload_params(archive, name, version, status);
    let uploads = queue::get_package_uploads_raw(client, distro, series, &params)
        .await
        .map_err(|err| queue_read_permission_error(err, archive, distro, series))?;
    uploads
        .value
        .iter()
        .position(|upload| upload_has_arch(upload, arch))
        .map(|index| Raw {
            value: uploads.value[index].clone(),
            raw: uploads.raw["entries"][index].clone(),
        })
        .ok_or_else(|| upload_not_found(name, version, arch, status, archive, distro, series))
}

/// The `lpcli` member shared by queue reads that resolve one upload.
fn queue_lpcli_json(archive_label: &str, series: &str) -> serde_json::Value {
    serde_json::json!({"archive": archive_label, "series": series})
}

fn empty_records() -> Records {
    Records {
        columns: Vec::new(),
        rows: Vec::new(),
    }
}

fn text_report(blocks: Vec<Block>) -> Report {
    Report {
        blocks,
        json: serde_json::Value::Null,
        records: empty_records(),
    }
}

fn upload_name(upload: &queue::PackageUpload) -> String {
    upload
        .package_name
        .clone()
        .or_else(|| upload.display_name.clone())
        .unwrap_or_else(|| "?".to_string())
}

fn upload_version(upload: &queue::PackageUpload) -> String {
    upload
        .package_version
        .clone()
        .or_else(|| upload.display_version.clone())
        .unwrap_or_else(|| "?".to_string())
}

// ---------------------------------------------------------------------------
// Queue search
// ---------------------------------------------------------------------------

/// One page of `getPackageUploads` results.
pub(crate) struct QueueSearch {
    /// The searched archive and series, e.g. `ubuntu/noble (~owner/ppa)`.
    pub(crate) target: String,
    /// Launchpad's result page, including `next_collection_link` when
    /// `--limit` cut the results short.
    pub(crate) page: Raw<Collection<queue::PackageUpload>>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn queue_search(
    client: &LaunchpadClient,
    pocket: Option<Pocket>,
    status: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
    exact_match: bool,
    archive: &QueueArchive,
    distro: &str,
    series: Option<String>,
    limit: u32,
) -> lpcli::error::Result<QueueSearch> {
    let series_name = resolve_queue_series(client, distro, series).await?;
    let params = QueueSearchParams {
        pocket: pocket.map(pocket_launchpad_name),
        status,
        name,
        version,
        exact_match,
        archive: Some(&archive.api_url),
        limit: Some(limit),
    };
    let page = queue::get_package_uploads_page_raw(client, distro, &series_name, &params)
        .await
        .map_err(|err| queue_read_permission_error(err, archive, distro, &series_name))?;
    Ok(QueueSearch {
        target: archive.target_label(distro, &series_name),
        page,
    })
}

/// Build the `queue search` report. The JSON is Launchpad's result page,
/// unmodified.
pub(crate) fn queue_search_report(search: &QueueSearch) -> Report {
    let rows = search
        .page
        .value
        .entries
        .iter()
        .map(|upload| {
            vec![
                upload
                    .package_name
                    .clone()
                    .or_else(|| upload.display_name.clone()),
                upload
                    .package_version
                    .clone()
                    .or_else(|| upload.display_version.clone()),
                upload.component_name.clone(),
                upload.pocket.clone(),
                upload.status.clone(),
                upload.display_arches.clone(),
                upload
                    .date_created
                    .map(|d| d.strftime("%Y-%m-%d %H:%M").to_string()),
            ]
        })
        .collect();
    let hint = search
        .page
        .value
        .next_collection_link
        .is_some()
        .then(|| "More uploads match; raise --limit to see them.".to_string());
    Report {
        blocks: vec![
            Block::Heading(format!("Upload queue for {}", search.target)),
            Block::List {
                noun: Noun {
                    singular: "upload",
                    plural: "uploads",
                },
                empty: "No packages found in the queue.".into(),
                hint,
            },
        ],
        json: search.page.raw.clone(),
        records: Records {
            columns: vec![
                Column {
                    label: "Package",
                    key: "package_name",
                },
                Column {
                    label: "Version",
                    key: "package_version",
                },
                Column {
                    label: "Component",
                    key: "component_name",
                },
                Column {
                    label: "Pocket",
                    key: "pocket",
                },
                Column {
                    label: "Status",
                    key: "status",
                },
                Column {
                    label: "Archs",
                    key: "display_arches",
                },
                Column {
                    label: "Created",
                    key: "date_created",
                },
            ],
            rows,
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_queue_search<O: QueueOutput + ?Sized>(
    output: &O,
    client: &LaunchpadClient,
    pocket: Option<Pocket>,
    status: Option<String>,
    name: Option<String>,
    version: Option<String>,
    exact_match: bool,
    archive: &QueueArchive,
    distro: String,
    series: Option<String>,
    limit: u32,
) -> lpcli::error::Result<()> {
    let search = queue_search(
        client,
        pocket,
        status.as_deref(),
        name.as_deref(),
        version.as_deref(),
        exact_match,
        archive,
        &distro,
        series,
        limit,
    )
    .await?;
    output.queue_output().print(&queue_search_report(&search))
}

// ---------------------------------------------------------------------------
// Queue download
// ---------------------------------------------------------------------------

/// A queue upload and the file URLs Launchpad listed for it.
pub(crate) struct QueueDownloadTarget {
    pub(crate) target: String,
    pub(crate) archive_label: String,
    pub(crate) series: String,
    pub(crate) upload: Raw<queue::PackageUpload>,
    /// `sourceFileUrls`, when it was requested.
    pub(crate) source_file_urls: Option<Raw<Vec<String>>>,
    /// `binaryFileUrls`, when it was requested.
    pub(crate) binary_file_urls: Option<Raw<Vec<String>>>,
}

impl QueueDownloadTarget {
    /// Every file URL to download, sources first.
    pub(crate) fn urls(&self) -> Vec<String> {
        self.source_file_urls
            .iter()
            .chain(&self.binary_file_urls)
            .flat_map(|urls| urls.value.iter().cloned())
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn queue_download_target(
    client: &LaunchpadClient,
    name: &str,
    version: Option<&str>,
    status: &str,
    arch: QueueArch,
    archive: &QueueArchive,
    distro: &str,
    series: Option<String>,
) -> lpcli::error::Result<QueueDownloadTarget> {
    let series_name = resolve_queue_series(client, distro, series).await?;
    let upload = find_queue_upload(
        client,
        name,
        version,
        status,
        arch,
        archive,
        distro,
        &series_name,
    )
    .await?;
    let self_link = upload
        .value
        .self_link
        .clone()
        .ok_or_else(missing_self_link)?;

    let mut target = QueueDownloadTarget {
        target: archive.target_label(distro, &series_name),
        archive_label: archive.label.clone(),
        series: series_name,
        source_file_urls: None,
        binary_file_urls: None,
        upload,
    };
    // Collect file URLs based on what the upload contains.
    if target.upload.value.contains_source.unwrap_or(false) {
        target.source_file_urls = Some(queue::get_source_file_urls_raw(client, &self_link).await?);
    }
    if target.upload.value.contains_build.unwrap_or(false) {
        target.binary_file_urls = Some(queue::get_binary_file_urls_raw(client, &self_link).await?);
    }
    // If neither listed anything (e.g. a copy), try the endpoints not yet asked.
    if target.urls().is_empty() && target.source_file_urls.is_none() {
        target.source_file_urls = Some(queue::get_source_file_urls_raw(client, &self_link).await?);
    }
    if target.urls().is_empty() && target.binary_file_urls.is_none() {
        target.binary_file_urls = Some(queue::get_binary_file_urls_raw(client, &self_link).await?);
    }
    Ok(target)
}

/// The result of downloading a queue upload's files.
pub(crate) struct QueueDownload {
    pub(crate) target: QueueDownloadTarget,
    pub(crate) output_dir: std::path::PathBuf,
    pub(crate) outcomes: Vec<download::DownloadOutcome>,
}

fn outcome_file(outcome: &download::DownloadOutcome) -> String {
    match outcome {
        download::DownloadOutcome::Success(file) => file
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.path.display().to_string()),
        download::DownloadOutcome::Failed { filename, .. } => filename.clone(),
    }
}

impl QueueDownload {
    fn counts(&self) -> (usize, usize) {
        let ok = self
            .outcomes
            .iter()
            .filter(|o| matches!(o, download::DownloadOutcome::Success(_)))
            .count();
        (ok, self.outcomes.len() - ok)
    }

    /// The lpcli-defined `queue download` object.
    ///
    /// `package_upload`, `source_file_urls`, and `binary_file_urls` are
    /// Launchpad's responses, unchanged; the URL lists appear only when
    /// lpcli requested them. `lpcli.downloads` has one element per file, in
    /// the same shape as `package download`.
    pub(crate) fn json(&self) -> serde_json::Value {
        let downloads: Vec<serde_json::Value> = self
            .outcomes
            .iter()
            .map(|outcome| match outcome {
                download::DownloadOutcome::Success(file) => serde_json::json!({
                    "filename": outcome_file(outcome),
                    "status": "downloaded",
                    "path": file.path.to_string_lossy(),
                    "size": file.size,
                }),
                download::DownloadOutcome::Failed { filename, error } => serde_json::json!({
                    "filename": filename,
                    "status": "failed",
                    "error": error,
                }),
            })
            .collect();
        let (downloaded, failed) = self.counts();
        let mut lpcli = queue_lpcli_json(&self.target.archive_label, &self.target.series);
        lpcli["output_directory"] = self.output_dir.to_string_lossy().into();
        lpcli["downloads"] = downloads.into();
        lpcli["downloaded"] = downloaded.into();
        lpcli["failed"] = failed.into();
        let mut json = serde_json::json!({
            "package_upload": self.target.upload.raw,
            "lpcli": lpcli,
        });
        if let Some(urls) = &self.target.source_file_urls {
            json["source_file_urls"] = urls.raw.clone();
        }
        if let Some(urls) = &self.target.binary_file_urls {
            json["binary_file_urls"] = urls.raw.clone();
        }
        json
    }

    /// Per-file results, then a summary.
    pub(crate) fn report(&self) -> Report {
        let upload = &self.target.upload.value;
        let mut blocks = Vec::new();
        if self.outcomes.is_empty() {
            blocks.push(Block::Status(
                Level::Warning,
                vec![
                    Span::Text("No files found for ".into()),
                    Span::Name(upload_name(upload)),
                    Span::Text(format!(
                        " ({}) in the queue for {}.",
                        upload_version(upload),
                        self.target.target
                    )),
                ],
            ));
        }
        for outcome in &self.outcomes {
            match outcome {
                download::DownloadOutcome::Success(file) => blocks.push(Block::Status(
                    Level::Ok,
                    vec![
                        Span::Name(outcome_file(outcome)),
                        Span::Text(format!(" ({} bytes)", file.size)),
                    ],
                )),
                download::DownloadOutcome::Failed { filename, error } => {
                    blocks.push(Block::Status(
                        Level::Error,
                        vec![
                            Span::Name(filename.clone()),
                            Span::Text(format!(": {error}")),
                        ],
                    ))
                }
            }
        }
        let (ok, failed) = self.counts();
        if !self.outcomes.is_empty() {
            let target = &self.target.target;
            let (level, summary) = match (ok, failed) {
                (_, 0) => (
                    Level::Ok,
                    format!(
                        "Downloaded {ok} file(s) from {target} to '{}'.",
                        self.output_dir.display()
                    ),
                ),
                (0, _) => (
                    Level::Error,
                    format!("No files downloaded from {target}; {failed} failed."),
                ),
                _ => (
                    Level::Warning,
                    format!("{ok} file(s) downloaded from {target}, {failed} failed."),
                ),
            };
            blocks.push(Block::Break);
            blocks.push(Block::Status(level, vec![Span::Text(summary)]));
        }
        Report {
            blocks,
            json: self.json(),
            records: empty_records(),
        }
    }

    /// The exit status, decided after the report is printed.
    ///
    /// - Every file downloaded, or the upload lists no files: success.
    /// - Some files downloaded and some failed: [`lpcli::error::LpError::Partial`]
    ///   (exit 7), because files were written to disk.
    /// - Every file failed: [`lpcli::error::LpError::Other`] (exit 1), because
    ///   nothing was written; finding the upload is only preparation.
    pub(crate) fn status(&self) -> lpcli::error::Result<()> {
        let (ok, failed) = self.counts();
        if failed == 0 {
            return Ok(());
        }
        let list = self
            .outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                download::DownloadOutcome::Failed { filename, error } => {
                    Some(format!("{filename} ({error})"))
                }
                download::DownloadOutcome::Success(_) => None,
            })
            .collect::<Vec<_>>()
            .join("; ");
        let total = self.outcomes.len();
        let name = upload_name(&self.target.upload.value);
        Err(if ok == 0 {
            lpcli::error::LpError::Other(format!(
                "Failed to download all {total} file(s) for '{name}': {list}"
            ))
        } else {
            lpcli::error::LpError::Partial(format!(
                "Downloaded {ok} of {total} file(s) for '{name}'; failed: {list}"
            ))
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_queue_download<O: QueueOutput + ?Sized>(
    output: &O,
    client: &LaunchpadClient,
    name: String,
    version: Option<String>,
    status: String,
    arch: QueueArch,
    archive: &QueueArchive,
    distro: String,
    series: Option<String>,
    output_dir: String,
) -> lpcli::error::Result<()> {
    let output = output.queue_output();
    let target = queue_download_target(
        client,
        &name,
        version.as_deref(),
        &status,
        arch,
        archive,
        &distro,
        series,
    )
    .await?;
    let urls = target.urls();
    let output_dir = std::path::PathBuf::from(output_dir);
    let outcomes = if urls.is_empty() {
        Vec::new()
    } else {
        if output.mode() == OutputMode::Rich && output.progress() == download::ProgressMode::Rich {
            let upload = &target.upload.value;
            output.print(&text_report(vec![Block::Heading(format!(
                "Downloading {} file(s) for {} ({})",
                urls.len(),
                upload_name(upload),
                upload_version(upload),
            ))]))?;
        }
        download_public_files_with_progress(&urls, &output_dir, output.progress()).await?
    };
    let download = QueueDownload {
        target,
        output_dir,
        outcomes,
    };
    output.print(&download.report())?;
    download.status()
}

// ---------------------------------------------------------------------------
// Queue accept and reject
// ---------------------------------------------------------------------------

/// A queue decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueueAction {
    Accept,
    Reject,
}

/// A completed accept or reject, before the upload is read back.
#[derive(Debug, Clone)]
pub(crate) struct QueueMutation {
    pub(crate) action: QueueAction,
    /// The upload as found in the queue before the change.
    pub(crate) upload: queue::PackageUpload,
    pub(crate) self_link: String,
    pub(crate) target: String,
    pub(crate) comment: Option<String>,
}

/// Find exactly one upload for `arch` and apply `action` to it.
#[allow(clippy::too_many_arguments)]
async fn apply_queue_action(
    action: QueueAction,
    client: &LaunchpadClient,
    name: String,
    version: Option<String>,
    status: String,
    arch: QueueArch,
    archive: &QueueArchive,
    comment: Option<String>,
    distro: String,
    series: Option<String>,
) -> lpcli::error::Result<QueueMutation> {
    let series_name = resolve_queue_series(client, &distro, series).await?;
    let params = exact_upload_params(archive, &name, version.as_deref(), &status);
    let uploads = queue::get_package_uploads(client, &distro, &series_name, &params)
        .await
        .map_err(|err| queue_read_permission_error(err, archive, &distro, &series_name))?;
    let matching: Vec<_> = uploads
        .into_iter()
        .filter(|upload| upload_has_arch(upload, arch))
        .collect();

    // Only a single package may be changed at a time. If multiple results
    // match, require the user to disambiguate with --version.
    if matching.len() > 1 && version.is_none() {
        let versions: Vec<String> = matching
            .iter()
            .filter_map(|u| u.package_version.clone())
            .collect();
        return Err(lpcli::error::LpError::Other(format!(
            "Multiple uploads match '{}' [arch={}] with status '{}' in {}: {}. \
             Use --version to select exactly one.",
            name,
            arch,
            status,
            archive.target_label(&distro, &series_name),
            versions.join(", "),
        )));
    }
    let upload = matching.into_iter().next().ok_or_else(|| {
        upload_not_found(
            &name,
            version.as_deref(),
            arch,
            &status,
            archive,
            &distro,
            &series_name,
        )
    })?;
    let self_link = upload.self_link.clone().ok_or_else(missing_self_link)?;

    let (verb, result) = match action {
        QueueAction::Accept => ("accept", queue::accept_from_queue(client, &self_link).await),
        QueueAction::Reject => (
            "reject",
            queue::reject_from_queue(client, &self_link, comment.as_deref()).await,
        ),
    };
    result.map_err(|err| {
        queue_mutation_permission_error(err, verb, archive, &distro, &series_name)
    })?;

    Ok(QueueMutation {
        action,
        upload,
        self_link,
        target: archive.target_label(&distro, &series_name),
        comment,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_queue_accept(
    client: &LaunchpadClient,
    name: String,
    version: Option<String>,
    status: String,
    arch: QueueArch,
    archive: &QueueArchive,
    distro: String,
    series: Option<String>,
) -> lpcli::error::Result<QueueMutation> {
    apply_queue_action(
        QueueAction::Accept,
        client,
        name,
        version,
        status,
        arch,
        archive,
        None,
        distro,
        series,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_queue_reject(
    client: &LaunchpadClient,
    name: String,
    version: Option<String>,
    status: String,
    arch: QueueArch,
    archive: &QueueArchive,
    comment: Option<String>,
    distro: String,
    series: Option<String>,
) -> lpcli::error::Result<QueueMutation> {
    apply_queue_action(
        QueueAction::Reject,
        client,
        name,
        version,
        status,
        arch,
        archive,
        comment,
        distro,
        series,
    )
    .await
}

/// Read the changed upload back from Launchpad.
///
/// `acceptFromQueue` and `rejectFromQueue` return no entry. A failed read
/// is [`lpcli::error::LpError::Partial`], because the change was applied.
pub(crate) async fn refresh_queue_upload(
    client: &LaunchpadClient,
    mutation: &QueueMutation,
) -> lpcli::error::Result<Raw<queue::PackageUpload>> {
    let description = match mutation.action {
        QueueAction::Accept => "Accepting the upload",
        QueueAction::Reject => "Rejecting the upload",
    };
    client
        .get_created_raw(&mutation.self_link, description)
        .await
}

/// The accept/reject report; its JSON is the refreshed Launchpad entry.
pub(crate) fn queue_mutation_report(
    mutation: &QueueMutation,
    entry: &Raw<queue::PackageUpload>,
) -> Report {
    let upload = &entry.value;
    let verb = match mutation.action {
        QueueAction::Accept => "Accepted",
        QueueAction::Reject => "Rejected",
    };
    let arches = upload
        .display_arches
        .clone()
        .or_else(|| mutation.upload.display_arches.clone())
        .unwrap_or_else(|| "?".to_string());
    let mut blocks = vec![Block::Status(
        Level::Ok,
        vec![
            Span::Text(format!("{verb} ")),
            Span::Name(upload_name(upload)),
            Span::Text(format!(
                " ({}) [{arches}] in {}.",
                upload_version(upload),
                mutation.target
            )),
        ],
    )];
    if let Some(status) = &upload.status {
        blocks.push(Block::Field("Status", status.clone()));
    }
    if let Some(comment) = &mutation.comment {
        blocks.push(Block::Field("Comment", comment.clone()));
    }
    Report {
        blocks,
        json: entry.raw.clone(),
        records: empty_records(),
    }
}

/// The text an accept or reject writes to stdout: the report in the selected
/// mode, or with `--quiet` only the refreshed entry's `self_link`.
pub(crate) fn render_queue_mutation(
    output: &Output,
    quiet: bool,
    mutation: &QueueMutation,
    entry: &Raw<queue::PackageUpload>,
) -> String {
    if !quiet {
        return output.render(&queue_mutation_report(mutation, entry));
    }
    entry
        .raw
        .get("self_link")
        .and_then(serde_json::Value::as_str)
        .map(|link| {
            // A Launchpad value on its own line must not inject terminal
            // controls or split the line.
            let link: String = link
                .chars()
                .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
                .collect();
            format!("{link}\n")
        })
        .unwrap_or_default()
}

/// Read the changed upload back and print it, or only its `self_link` with
/// `--quiet`.
pub(crate) async fn print_queue_mutation(
    output: &Output,
    quiet: bool,
    client: &LaunchpadClient,
    mutation: &QueueMutation,
) -> lpcli::error::Result<()> {
    let entry = match refresh_queue_upload(client, mutation).await {
        Ok(entry) => entry,
        Err(error) => {
            let action = match mutation.action {
                QueueAction::Accept => "accept",
                QueueAction::Reject => "reject",
            };
            let result = serde_json::json!({"lpcli": {
                "action": action,
                "self_link": mutation.self_link,
                "refreshed": false,
                "error": {
                    "kind": error.kind().name(),
                    "message": crate::output::redact_secrets(&error.to_string()),
                    "http_status": error.http_status(),
                },
            }});
            let mut stdout = std::io::stdout().lock();
            if quiet {
                writeln!(
                    stdout,
                    "{}",
                    mutation
                        .self_link
                        .chars()
                        .map(|c| if c.is_control() { '\u{FFFD}' } else { c })
                        .collect::<String>()
                )?;
            } else if output.mode() == crate::output::OutputMode::Json {
                writeln!(stdout, "{result}")?;
            } else {
                output.warn(&format!(
                    "{action} succeeded for {}, but the updated entry could not be read back.",
                    mutation.self_link
                ))?;
            }
            stdout.flush()?;
            return Err(lpcli::error::LpError::Partial(format!(
                "{} succeeded, but refreshing the result failed: {}",
                action,
                crate::output::redact_secrets(&error.to_string())
            )));
        }
    };
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(render_queue_mutation(output, quiet, mutation, &entry).as_bytes())?;
    stdout.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Queue info
// ---------------------------------------------------------------------------

/// An upload with its binary package details.
pub(crate) struct QueueInfo {
    pub(crate) archive_label: String,
    pub(crate) series: String,
    /// Whether the archive is the distribution's primary archive.
    pub(crate) primary: bool,
    pub(crate) upload: Raw<queue::PackageUpload>,
    /// The upload's status log, read for rejected uploads.
    pub(crate) logs: Option<Raw<Vec<queue::PackageUploadLog>>>,
    /// `getBinaryProperties`, unchanged.
    pub(crate) binaries: Raw<Vec<queue::BinaryProperties>>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn queue_info(
    client: &LaunchpadClient,
    name: &str,
    version: Option<&str>,
    status: &str,
    arch: QueueArch,
    archive: &QueueArchive,
    distro: &str,
    series: Option<String>,
) -> lpcli::error::Result<QueueInfo> {
    let series_name = resolve_queue_series(client, distro, series).await?;
    let upload = find_queue_upload(
        client,
        name,
        version,
        status,
        arch,
        archive,
        distro,
        &series_name,
    )
    .await?;
    let self_link = upload
        .value
        .self_link
        .clone()
        .ok_or_else(missing_self_link)?;

    // The rejection reason is optional detail, so a failed log read is not
    // an error.
    let logs = match &upload.value.logs_collection_link {
        Some(logs_url) if upload.value.status.as_deref() == Some("Rejected") => {
            queue::get_upload_logs_raw(client, logs_url).await.ok()
        }
        _ => None,
    };
    let binaries = queue::get_binary_properties_raw(client, &self_link).await?;
    Ok(QueueInfo {
        archive_label: archive.label.clone(),
        series: series_name,
        primary: archive.label.eq_ignore_ascii_case(distro),
        upload,
        logs,
        binaries,
    })
}

fn binary_records(binaries: &[&queue::BinaryProperties]) -> Records {
    Records {
        columns: vec![
            Column {
                label: "Package",
                key: "name",
            },
            Column {
                label: "Version",
                key: "version",
            },
            Column {
                label: "Architecture",
                key: "architecture",
            },
            Column {
                label: "Component",
                key: "component",
            },
            Column {
                label: "Section",
                key: "section",
            },
            Column {
                label: "Priority",
                key: "priority",
            },
        ],
        rows: binaries
            .iter()
            .map(|b| {
                vec![
                    b.name.clone(),
                    b.version.clone(),
                    b.architecture.clone(),
                    b.component.clone(),
                    b.section.clone(),
                    b.priority.clone(),
                ]
            })
            .collect(),
    }
}

impl QueueInfo {
    /// The lpcli-defined `queue info` object.
    ///
    /// `package_upload` and `binary_properties` are Launchpad's responses,
    /// unchanged. `logs` is the upload's log collection, present only for
    /// rejected uploads.
    pub(crate) fn json(&self) -> serde_json::Value {
        let mut json = serde_json::json!({
            "package_upload": self.upload.raw,
            "binary_properties": self.binaries.raw,
            "lpcli": queue_lpcli_json(&self.archive_label, &self.series),
        });
        if let Some(logs) = &self.logs {
            json["logs"] = logs.raw.clone();
        }
        json
    }

    /// The upload details, then one list each for `.deb` and `.ddeb`
    /// packages, then a total.
    pub(crate) fn reports(&self) -> Vec<Report> {
        let upload = &self.upload.value;
        let mut blocks = vec![Block::Heading(format!(
            "{} ({})",
            upload_name(upload),
            upload_version(upload)
        ))];
        let mut field = |label, value: Option<String>| {
            if let Some(value) = value {
                blocks.push(Block::Field(label, value));
            }
        };
        field("Status", upload.status.clone());
        field("Architectures", upload.display_arches.clone());
        field(
            "Archive",
            (!self.primary).then(|| self.archive_label.clone()),
        );
        field("Component", upload.component_name.clone());
        field("Pocket", upload.pocket.clone());
        field(
            "Created",
            upload
                .date_created
                .map(|d| d.strftime("%Y-%m-%d %H:%M UTC").to_string()),
        );
        if let Some(log) = self.logs.as_ref().and_then(|logs| {
            logs.value
                .iter()
                .find(|log| log.new_status.as_deref() == Some("Rejected"))
        }) {
            field(
                "Reason",
                log.comment.clone().filter(|comment| !comment.is_empty()),
            );
            field(
                "Rejected",
                log.date_created
                    .map(|d| d.strftime("%Y-%m-%d %H:%M UTC").to_string()),
            );
        }
        let mut reports = vec![text_report(blocks)];

        let binaries = &self.binaries.value;
        if binaries.is_empty() {
            reports.push(text_report(vec![
                Block::Break,
                Block::Note(vec![Span::Text(
                    "No binary packages (.deb/.ddeb) found for this upload.".into(),
                )]),
            ]));
            return reports;
        }
        let (ddebs, debs): (Vec<_>, Vec<_>) = binaries.iter().partition(|b| b.is_debug_package());
        for (heading, noun, list) in [
            (
                "Binary packages (.deb)",
                Noun {
                    singular: "binary package",
                    plural: "binary packages",
                },
                &debs,
            ),
            (
                "Debug symbol packages (.ddeb)",
                Noun {
                    singular: "debug symbol package",
                    plural: "debug symbol packages",
                },
                &ddebs,
            ),
        ] {
            if list.is_empty() {
                continue;
            }
            reports.push(Report {
                blocks: vec![
                    Block::Break,
                    Block::Heading(heading.into()),
                    Block::List {
                        noun,
                        empty: String::new(),
                        hint: None,
                    },
                ],
                json: serde_json::Value::Null,
                records: binary_records(list),
            });
        }
        reports.push(text_report(vec![
            Block::Break,
            Block::Field(
                "Total",
                format!(
                    "{} binary package(s) ({} .deb, {} .ddeb)",
                    binaries.len(),
                    debs.len(),
                    ddebs.len()
                ),
            ),
        ]));
        reports
    }

    /// The text written to stdout in the selected mode.
    pub(crate) fn render(&self, output: &Output) -> String {
        if output.mode() == OutputMode::Json {
            return output.render(&Report::json(self.json()));
        }
        self.reports()
            .iter()
            .map(|report| output.render(report))
            .collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_queue_info<O: QueueOutput + ?Sized>(
    output: &O,
    client: &LaunchpadClient,
    name: String,
    version: Option<String>,
    status: String,
    arch: QueueArch,
    archive: &QueueArchive,
    distro: String,
    series: Option<String>,
) -> lpcli::error::Result<()> {
    let output = output.queue_output();
    let info = queue_info(
        client,
        &name,
        version.as_deref(),
        &status,
        arch,
        archive,
        &distro,
        series,
    )
    .await?;
    if output.mode() == OutputMode::Json {
        return output.print_json(&info.json());
    }
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(info.render(&output).as_bytes())?;
    stdout.flush()?;
    Ok(())
}
