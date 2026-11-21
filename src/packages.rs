//! Launchpad source packages and Ubuntu distribution operations.
//!
//! This module exposes functions for querying:
//!
//! * Ubuntu distributions and their series (e.g. `jammy`, `noble`).
//! * Source packages, publishing history, PPAs, and Debian changelogs.
//! * Binary packages built from a source package.
//! * Copying or syncing packages between series (PPAs).
//!
//! # API roots
//!
//! | Resource | Path |
//! |----------|------|
//! | Ubuntu distribution | `/ubuntu` |
//! | Series | `/ubuntu/jammy` |
//! | Source publications | `/ubuntu/+archive/primary?ws.op=getPublishedSources` |
//! | Source builds | `/+sourcepub/{id}?ws.op=getBuilds` |
//! | Published binaries | `/+sourcepub/{id}?ws.op=getPublishedBinaries` |
//! | PPAs | `/~{person}/+archive/ubuntu/{ppa}` |
//!
//! Each read function that returns Launchpad data has a `_raw` counterpart
//! that also returns the exact JSON Launchpad sent: an entry, or a collection
//! with `start`, `entries`, and `total_size` or `total_size_link`.
//! [`get_source_file_urls`], [`get_source_changelog_url`],
//! [`fetch_source_changelog`], and [`source_has_active_binaries`] have no
//! `_raw` counterparts because Launchpad returns a URL list, an optional URL
//! string, plain changelog text, or a value derived from a collection rather
//! than an entry or collection.
//!
//! Build results are read per source publication with `getBuilds`. The
//! archive-level `getBuildRecords` operation is not used because it times out
//! on the primary archive; `getBuilds` is scoped to one source publication and
//! works on both PPAs and the primary archive. `getBuilds` and
//! `getPublishedBinaries` are ordinary `devel` GET operations; no beta header
//! or beta service root is needed.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::client::{Collection, LaunchpadClient, Raw};
use crate::error::Result;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// An Ubuntu distribution series (e.g. Jammy Jellyfish / 22.04).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DistroSeries {
    /// Short name, e.g. `"jammy"`.
    pub name: String,
    /// Full display name, e.g. `"Ubuntu Jammy Jellyfish"`.
    pub display_name: Option<String>,
    /// Version string, e.g. `"22.04"`.
    pub version: Option<String>,
    /// Whether this series is currently active.
    pub active: Option<bool>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Web link.
    pub web_link: Option<String>,
    /// Current status of the series (e.g. "Active Development", "Supported").
    pub status: Option<String>,
}

/// A source package publication record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourcePackagePublishingHistory {
    /// API self-link.
    pub self_link: Option<String>,
    /// Display name of the source package.
    pub source_package_name: Option<String>,
    /// Version string.
    pub source_package_version: Option<String>,
    /// Component (e.g. `"main"`, `"universe"`).
    pub component_name: Option<String>,
    /// Section (e.g. `"devel"`, `"libs"`).
    pub section_name: Option<String>,
    /// Publishing status (e.g. "Published", "Superseded").
    pub status: Option<String>,
    /// When this record was created.
    pub date_published: Option<Timestamp>,
    /// When this record was superseded, if applicable.
    pub date_superseded: Option<Timestamp>,
    /// Pocket (e.g. `"Release"`, `"Updates"`, `"Security"`).
    pub pocket: Option<String>,
    /// Archive this was published in.
    pub archive_link: Option<String>,
    /// Series API link.
    pub distro_series_link: Option<String>,
    /// API link to the package maintainer (e.g.
    /// `"https://api.launchpad.net/devel/~ubuntu-devel-discuss-lists"`).
    pub package_maintainer_link: Option<String>,
}

/// A binary package publication record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryPackagePublishingHistory {
    /// API self-link.
    pub self_link: Option<String>,
    /// Binary package name.
    pub binary_package_name: Option<String>,
    /// Binary package version.
    pub binary_package_version: Option<String>,
    /// Architecture tag (e.g. `"amd64"`, `"arm64"`, `"all"`).
    pub architecture_specific: Option<bool>,
    /// Component (e.g. `"main"`).
    pub component_name: Option<String>,
    /// Status.
    pub status: Option<String>,
    /// When published.
    pub date_published: Option<Timestamp>,
}

/// A source package build record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBuild {
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// Architecture tag (e.g. `"amd64"`, `"arm64"`).
    pub arch_tag: Option<String>,
    /// Build state (e.g. `"Successfully built"`, `"Failed to build"`).
    pub buildstate: Option<String>,
    /// When the build finished, when it has finished.
    #[serde(rename = "datebuilt")]
    pub date_built: Option<Timestamp>,
    /// Build duration, as Launchpad formats it (e.g. `"0:02:59.187629"`).
    pub duration: Option<String>,
    /// URL of the build log.
    pub build_log_url: Option<String>,
    /// Source package name.
    pub source_package_name: Option<String>,
    /// Source package version.
    pub source_package_version: Option<String>,
    /// API link to the build's distro series.
    pub distro_series_link: Option<String>,
    /// Pocket (e.g. `"Release"`, `"Updates"`).
    pub pocket: Option<String>,
}

/// A Launchpad Personal Package Archive (PPA).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archive {
    /// API self-link.
    pub self_link: Option<String>,
    /// Short archive name.
    pub name: Option<String>,
    /// Human-readable description.
    pub description: Option<String>,
    /// Whether the archive is enabled.
    pub enabled: Option<bool>,
    /// Number of source packages.
    pub num_pkgs: Option<u64>,
    /// Web link.
    pub web_link: Option<String>,
    /// Owner API link.
    pub owner_link: Option<String>,
}

/// A parsed Debian changelog entry (`deb-changelog(5)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangelogEntry {
    /// Source package name from the stanza header.
    pub package: String,
    /// Package version from the stanza header.
    pub version: String,
    /// Target distribution(s) listed on the header line.
    pub distributions: Vec<String>,
    /// Urgency metadata value (e.g. `"medium"`, `"high"`), when present.
    pub urgency: Option<String>,
    /// Any additional `key=value` metadata attributes from the header line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata: Vec<(String, String)>,
    /// Changelog body text between header and trailer, preserving indentation.
    pub changes: String,
    /// Maintainer name and email from the `--` trailer, when present.
    pub maintainer: Option<String>,
    /// Raw RFC 2822 timestamp string from the `--` trailer, when present.
    pub date: Option<String>,
    /// Parsed UTC timestamp from `date`, when it parses as RFC 2822.
    pub timestamp: Option<Timestamp>,
}

/// Parameters for source package publication searches.
#[derive(Debug, Clone, Default)]
pub struct SourceSearchParams<'a> {
    /// Source package name filter.
    pub source_name: Option<&'a str>,
    /// Version filter.
    pub version: Option<&'a str>,
    /// Pocket filter (e.g. "Release", "Updates").
    pub pocket: Option<&'a str>,
    /// Status filter (e.g. "Published").
    pub status: Option<&'a str>,
    /// Status filters to send as repeated `status` parameters.
    ///
    /// When non-empty, each value is sent as its own `status` parameter and
    /// [`Self::status`] is ignored. Launchpad returns publications in every
    /// status when neither is set, including `Deleted` and `Obsolete`.
    pub statuses: &'a [&'a str],
    /// Whether to require an exact match for the source name and version.
    pub exact_match: bool,
    /// Maximum number of results to return.
    ///
    /// When set, it is sent as Launchpad's `ws.size` page size and only the
    /// first page is fetched. When unset, every page is fetched.
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------------------
// API functions
// ---------------------------------------------------------------------------

/// Fetch metadata for an Ubuntu distro series.
///
/// `distro` is typically `"ubuntu"` and `series` is the codename, e.g. `"jammy"`.
pub async fn get_distro_series(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
) -> Result<DistroSeries> {
    Ok(get_distro_series_raw(client, distro, series).await?.value)
}

/// Like [`get_distro_series()`], but also keep the exact JSON Launchpad
/// returned.
pub async fn get_distro_series_raw(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
) -> Result<Raw<DistroSeries>> {
    client
        .get_raw(&format!("/{}/{}", enc(distro), enc(series)))
        .await
}

/// List all known distro series for a distribution.
pub async fn list_distro_series(
    client: &LaunchpadClient,
    distro: &str,
) -> Result<Vec<DistroSeries>> {
    Ok(list_distro_series_raw(client, distro).await?.value)
}

/// Like [`list_distro_series()`], but also keep Launchpad's collection JSON.
pub async fn list_distro_series_raw(
    client: &LaunchpadClient,
    distro: &str,
) -> Result<Raw<Vec<DistroSeries>>> {
    let url = client.url(&format!("/{}/series", enc(distro)));
    Collection::fetch_all_raw(client, &url).await
}

/// Search for source package publications in a distro series.
///
/// `distro` is typically `"ubuntu"`, `series` is the codename (e.g. `"jammy"`).
///
/// `getPublishedSources` is an operation on the **archive** resource, not on
/// `distro_series`. This function targets the distribution's primary archive
/// (`/{distro}/+archive/primary`) and passes the series as a full API URL
/// via the `distro_series` link parameter.
///
/// [`SourceSearchParams::limit`] caps the number of results returned.
pub async fn search_published_sources(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
    params: &SourceSearchParams<'_>,
) -> Result<Vec<SourcePackagePublishingHistory>> {
    Ok(search_published_sources_raw(client, distro, series, params)
        .await?
        .value
        .entries)
}

/// Like [`search_published_sources()`], but also keep Launchpad's collection
/// JSON.
///
/// With [`SourceSearchParams::limit`], exactly one page of at most `limit`
/// entries is fetched and Launchpad's page is returned unchanged, including
/// `next_collection_link` when more results exist. Without a limit, every
/// page is fetched and `next_collection_link` is omitted.
pub async fn search_published_sources_raw(
    client: &LaunchpadClient,
    distro: &str,
    series: &str,
    params: &SourceSearchParams<'_>,
) -> Result<Raw<Collection<SourcePackagePublishingHistory>>> {
    // getPublishedSources lives on archive, not distro_series.
    let archive_url = client.url(&format!("/{}/+archive/primary", enc(distro)));
    // The distro_series parameter must be the full API URL of the series
    // resource, URL-encoded as a query parameter value.
    let series_url = client.url(&format!("/{}/{}", enc(distro), enc(series)));
    let mut query = format!(
        "{archive_url}?ws.op=getPublishedSources&distro_series={}",
        enc(&series_url),
    );
    append_source_search_params(&mut query, params, true);
    fetch_sources_raw(client, query, params.limit).await
}

/// Fetch details of a PPA by owner and archive name.
///
/// # Example
///
/// ```no_run
/// # use lpcli::client::LaunchpadClient;
/// # use lpcli::packages::get_ppa;
/// # tokio_test::block_on(async {
/// let client = LaunchpadClient::new(None);
/// let ppa = get_ppa(&client, "canonical-kernel-team", "ppa").await.unwrap();
/// # });
/// ```
pub async fn get_ppa(client: &LaunchpadClient, owner: &str, ppa_name: &str) -> Result<Archive> {
    Ok(get_ppa_raw(client, owner, ppa_name).await?.value)
}

/// Like [`get_ppa()`], but also keep the exact JSON Launchpad returned.
pub async fn get_ppa_raw(
    client: &LaunchpadClient,
    owner: &str,
    ppa_name: &str,
) -> Result<Raw<Archive>> {
    client
        .get_raw(&format!(
            "/~{}/+archive/ubuntu/{}",
            enc(owner),
            enc(ppa_name)
        ))
        .await
}

/// List source package publications in a PPA.
///
/// [`SourceSearchParams::pocket`] is not sent. [`SourceSearchParams::limit`]
/// caps the number of results returned.
pub async fn list_ppa_sources(
    client: &LaunchpadClient,
    owner: &str,
    ppa_name: &str,
    params: &SourceSearchParams<'_>,
) -> Result<Vec<SourcePackagePublishingHistory>> {
    Ok(list_ppa_sources_raw(client, owner, ppa_name, params)
        .await?
        .value
        .entries)
}

/// Like [`list_ppa_sources()`], but also keep Launchpad's collection JSON.
///
/// Pagination follows [`search_published_sources_raw()`].
pub async fn list_ppa_sources_raw(
    client: &LaunchpadClient,
    owner: &str,
    ppa_name: &str,
    params: &SourceSearchParams<'_>,
) -> Result<Raw<Collection<SourcePackagePublishingHistory>>> {
    search_ppa_sources_inner_raw(client, owner, ppa_name, None, params, false).await
}

/// Search for source package publications in an Ubuntu PPA, optionally
/// filtered to a distro `series`, keeping Launchpad's collection JSON.
///
/// Unlike [`list_ppa_sources_raw()`], this forwards [`SourceSearchParams::pocket`]
/// and an optional Ubuntu series filter when supplied.
pub async fn search_ppa_sources_raw(
    client: &LaunchpadClient,
    owner: &str,
    ppa_name: &str,
    series: Option<&str>,
    params: &SourceSearchParams<'_>,
) -> Result<Raw<Collection<SourcePackagePublishingHistory>>> {
    search_ppa_sources_inner_raw(client, owner, ppa_name, series, params, true).await
}

async fn search_ppa_sources_inner_raw(
    client: &LaunchpadClient,
    owner: &str,
    ppa_name: &str,
    series: Option<&str>,
    params: &SourceSearchParams<'_>,
    include_pocket: bool,
) -> Result<Raw<Collection<SourcePackagePublishingHistory>>> {
    let archive_url = client.url(&format!(
        "/~{}/+archive/ubuntu/{}",
        enc(owner),
        enc(ppa_name)
    ));
    let mut query = format!("{archive_url}?ws.op=getPublishedSources");
    if let Some(series) = series {
        let series_url = client.url(&format!("/ubuntu/{}", enc(series)));
        query.push_str(&format!("&distro_series={}", enc(&series_url)));
    }
    append_source_search_params(&mut query, params, include_pocket);
    fetch_sources_raw(client, query, params.limit).await
}

fn append_source_search_params(
    query: &mut String,
    params: &SourceSearchParams<'_>,
    include_pocket: bool,
) {
    if let Some(name) = params.source_name {
        query.push_str(&format!("&source_name={}", enc(name)));
    }
    if let Some(version) = params.version {
        query.push_str(&format!("&version={}", enc(version)));
    }
    if include_pocket && let Some(pocket) = params.pocket {
        query.push_str(&format!("&pocket={}", enc(pocket)));
    }
    if !params.statuses.is_empty() {
        for status in params.statuses {
            query.push_str(&format!("&status={}", enc(status)));
        }
    } else if let Some(status) = params.status {
        query.push_str(&format!("&status={}", enc(status)));
    }
    if params.exact_match {
        query.push_str("&exact_match=true");
    }
}

/// Fetch a `getPublishedSources` collection, honouring an optional limit.
///
/// `ws.size` is only Launchpad's page size, so a limited search fetches a
/// single page; an unlimited one fetches every page.
async fn fetch_sources_raw(
    client: &LaunchpadClient,
    mut url: String,
    limit: Option<u32>,
) -> Result<Raw<Collection<SourcePackagePublishingHistory>>> {
    if let Some(limit) = limit {
        url.push_str(&format!("&ws.size={limit}"));
        return Collection::fetch_page_raw(client, &url).await;
    }
    let all = Collection::fetch_all_raw(client, &url).await?;
    Ok(Raw {
        value: Collection {
            entries: all.value,
            next_collection_link: None,
            total_size: all
                .raw
                .get("total_size")
                .and_then(serde_json::Value::as_u64),
        },
        raw: all.raw,
    })
}

/// Get every build record created for a source package publication.
///
/// Calls `getBuilds` on a `source_package_publishing_history` entry. The
/// collection is ordered by architecture tag and includes the build state,
/// finish time, duration, and build log URL for each architecture.
///
/// `pub_self_link` is the API self-link of the
/// `SourcePackagePublishingHistory`.
pub async fn get_source_builds(
    client: &LaunchpadClient,
    pub_self_link: &str,
) -> Result<Vec<SourceBuild>> {
    Ok(get_source_builds_raw(client, pub_self_link).await?.value)
}

/// Like [`get_source_builds()`], but also keep Launchpad's collection JSON.
pub async fn get_source_builds_raw(
    client: &LaunchpadClient,
    pub_self_link: &str,
) -> Result<Raw<Vec<SourceBuild>>> {
    client.ensure_same_api_origin(pub_self_link, "publication self_link")?;
    let url = format!("{pub_self_link}?ws.op=getBuilds");
    Collection::fetch_all_raw(client, &url).await
}

/// Whether a source package publication has active binary publications.
///
/// `getPublishedBinaries` defaults to active (`Published` or `Pending`) binary
/// publications. Only one entry is requested: callers need the existence of
/// binaries, not their details. A source that has not been built yet, whose
/// builds failed, or whose binaries were superseded returns `false`.
pub async fn source_has_active_binaries(
    client: &LaunchpadClient,
    pub_self_link: &str,
) -> Result<bool> {
    client.ensure_same_api_origin(pub_self_link, "publication self_link")?;
    let url = format!("{pub_self_link}?ws.op=getPublishedBinaries&ws.size=1");
    let page: Raw<Collection<BinaryPackagePublishingHistory>> =
        Collection::fetch_page_raw(client, &url).await?;
    Ok(!page.value.entries.is_empty())
}

/// Get the source file URLs for a source package publishing history entry.
///
/// Calls the `sourceFileUrls` custom GET method on a
/// `source_package_publishing_history` entry to retrieve download URLs for
/// the source files (`.dsc`, `.orig.tar.*`, `.debian.tar.*`, etc.).
///
/// `pub_self_link` is the API self-link of the `SourcePackagePublishingHistory`.
pub async fn get_source_file_urls(
    client: &LaunchpadClient,
    pub_self_link: &str,
) -> Result<Vec<String>> {
    client.ensure_same_api_origin(pub_self_link, "publication self_link")?;
    let url = format!("{pub_self_link}?ws.op=sourceFileUrls");
    client.get_url_string_list(&url).await
}

/// Resolve the public changelog download URL for a source package publication.
///
/// Calls `changelogUrl` on a `source_package_publishing_history` resource.
/// Returns `Ok(None)` when Launchpad answers JSON `null` (no changelog stored
/// for the publication).
pub async fn get_source_changelog_url(
    client: &LaunchpadClient,
    pub_self_link: &str,
) -> Result<Option<String>> {
    client.ensure_same_api_origin(pub_self_link, "publication self_link")?;
    let url = format!("{pub_self_link}?ws.op=changelogUrl");
    client.get_url(&url).await
}

/// Fetch the raw text of a Debian changelog from a URL returned by
/// [`get_source_changelog_url()`].
///
/// Downloads without sending OAuth or hardware-bound credentials, validates the
/// initial URL and redirect hops, and caps the response size.
pub async fn fetch_source_changelog(
    client: &LaunchpadClient,
    changelog_url: &str,
) -> Result<String> {
    client.get_public_text_url(changelog_url).await
}

/// Parse a Debian `debian/changelog` document into structured [`ChangelogEntry`]
/// stanzas in newest-first file order.
///
/// Follows Debian Policy §4.4 (`deb-changelog(5)`): handles multi-distribution
/// headers, case-insensitive `urgency=...` plus arbitrary `key=value` metadata,
/// CRLF line endings, indented multi-line bodies, RFC 2822 trailer dates, and
/// trailing Emacs `Local variables:` footers. Returns an empty vector when no
/// structured stanzas are recognized so callers can fall back to raw text.
pub fn parse_debian_changelog(raw: &str) -> Vec<ChangelogEntry> {
    let normalized = normalize_line_endings(raw);
    parse_debian_changelog_stanzas(&normalized)
        .into_iter()
        .map(|(entry, _)| entry)
        .collect()
}

/// Return the raw Debian changelog text truncated to at most the first `limit`
/// parsed stanzas, preserving exact stanza formatting.
///
/// When `limit == 0`, returns an empty string. When the changelog contains no
/// recognized stanzas or `limit` covers all stanzas, returns the full normalized
/// text with trailing whitespace trimmed.
pub fn slice_debian_changelog(raw: &str, limit: usize) -> String {
    if limit == 0 {
        return String::new();
    }
    let normalized = normalize_line_endings(raw);
    let stanzas = parse_debian_changelog_stanzas(&normalized);
    if stanzas.is_empty() || limit >= stanzas.len() {
        return normalized.trim_end().to_string();
    }
    let end = stanzas[limit - 1].1.end;
    normalized[..end].trim_end().to_string()
}

fn normalize_line_endings(raw: &str) -> String {
    if !raw.contains('\r') {
        return raw.to_string();
    }
    raw.replace("\r\n", "\n").replace('\r', "\n")
}

struct ParsedHeader {
    package: String,
    version: String,
    distributions: Vec<String>,
    urgency: Option<String>,
    metadata: Vec<(String, String)>,
}

struct OpenStanza {
    start: usize,
    last_non_empty_end: usize,
    header: ParsedHeader,
    body_lines: Vec<String>,
}

fn finish_open_stanza(
    stanza: OpenStanza,
    end_offset: usize,
    trailer: Option<(Option<String>, Option<String>, Option<Timestamp>)>,
) -> (ChangelogEntry, std::ops::Range<usize>) {
    let body = trim_blank_boundary_lines(&stanza.body_lines).join("\n");
    let (maintainer, date, timestamp) = trailer.unwrap_or((None, None, None));
    (
        ChangelogEntry {
            package: stanza.header.package,
            version: stanza.header.version,
            distributions: stanza.header.distributions,
            urgency: stanza.header.urgency,
            metadata: stanza.header.metadata,
            changes: body,
            maintainer,
            date,
            timestamp,
        },
        stanza.start..end_offset,
    )
}

fn trim_blank_boundary_lines(lines: &[String]) -> &[String] {
    let start = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(start, |idx| idx + 1);
    &lines[start..end]
}

fn parse_debian_changelog_stanzas(
    normalized: &str,
) -> Vec<(ChangelogEntry, std::ops::Range<usize>)> {
    let mut results = Vec::new();
    let mut current: Option<OpenStanza> = None;
    let mut in_emacs_footer = false;
    let mut offset = 0usize;

    for raw_line in normalized.split_inclusive('\n') {
        let line_start = offset;
        let line_end = offset + raw_line.len();
        offset = line_end;
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let content_end = line_start + line.len();

        if in_emacs_footer {
            if line.trim().eq_ignore_ascii_case("end:") {
                in_emacs_footer = false;
            }
            continue;
        }

        if line
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("local variables:")
        {
            if let Some(prev) = current.take() {
                let end = prev.last_non_empty_end;
                results.push(finish_open_stanza(prev, end, None));
            }
            in_emacs_footer = true;
            continue;
        }

        if let Some(header) = parse_changelog_header(line) {
            if let Some(prev) = current.take() {
                let end = prev.last_non_empty_end;
                results.push(finish_open_stanza(prev, end, None));
            }
            current = Some(OpenStanza {
                start: line_start,
                last_non_empty_end: content_end,
                header,
                body_lines: Vec::new(),
            });
            continue;
        }

        let Some(ref mut active) = current else {
            continue;
        };

        if let Some(trailer) = parse_changelog_trailer(line) {
            let finished = current.take().expect("active stanza present");
            results.push(finish_open_stanza(finished, content_end, Some(trailer)));
            continue;
        }

        if !line.trim().is_empty() {
            active.last_non_empty_end = content_end;
        }
        active.body_lines.push(line.to_string());
    }

    if let Some(prev) = current.take() {
        let end = prev.last_non_empty_end;
        results.push(finish_open_stanza(prev, end, None));
    }

    results
}

fn parse_changelog_header(line: &str) -> Option<ParsedHeader> {
    if line.is_empty() || line.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }
    let open_idx = line.find(" (")?;
    let package = &line[..open_idx];
    let mut pkg_chars = package.chars();
    let first = pkg_chars.next()?;
    if !first.is_ascii_alphanumeric()
        || !pkg_chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    {
        return None;
    }

    let after_open = &line[open_idx + 2..];
    let close_rel = after_open.find(')')?;
    let version = &after_open[..close_rel];
    if version.is_empty() || version.chars().any(char::is_whitespace) {
        return None;
    }

    let after_close = &after_open[close_rel + 1..];
    if !after_close.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }

    let (dists_raw, meta_raw) = match after_close.split_once(';') {
        Some((dists, meta)) => (dists, Some(meta)),
        None => (after_close, None),
    };
    let distributions: Vec<String> = dists_raw.split_whitespace().map(str::to_string).collect();
    if distributions.is_empty() {
        return None;
    }

    let mut urgency = None;
    let mut metadata = Vec::new();
    if let Some(meta_raw) = meta_raw {
        for pair in meta_raw.split(',') {
            let trimmed = pair.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((key, val)) = trimmed.split_once('=') {
                let k = key.trim();
                let v = val.trim();
                if k.is_empty() {
                    continue;
                }
                if k.eq_ignore_ascii_case("urgency") {
                    urgency = Some(v.to_string());
                } else {
                    metadata.push((k.to_string(), v.to_string()));
                }
            }
        }
    }

    Some(ParsedHeader {
        package: package.to_string(),
        version: version.to_string(),
        distributions,
        urgency,
        metadata,
    })
}

fn parse_changelog_trailer(
    line: &str,
) -> Option<(Option<String>, Option<String>, Option<Timestamp>)> {
    let rest = line
        .strip_prefix(" -- ")
        .or_else(|| line.strip_prefix("-- "))?;

    // Debian policy separates the maintainer email and RFC 2822 date with two
    // or more spaces. Anchor after '>' when present so double spaces in names
    // do not split prematurely.
    let split_idx = rest
        .find('>')
        .and_then(|email_end| {
            rest[email_end + 1..]
                .find("  ")
                .map(|rel| email_end + 1 + rel)
        })
        .or_else(|| rest.find("  "));

    match split_idx {
        Some(idx) => {
            let maint = rest[..idx].trim();
            let date_str = rest[idx..].trim();
            let maintainer = (!maint.is_empty()).then(|| maint.to_string());
            let date = (!date_str.is_empty()).then(|| date_str.to_string());
            let timestamp = date.as_deref().and_then(|d| {
                jiff::fmt::rfc2822::DateTimeParser::new()
                    .parse_timestamp(d)
                    .ok()
            });
            Some((maintainer, date, timestamp))
        }
        None => {
            let trimmed = rest.trim();
            let maintainer = (!trimmed.is_empty()).then(|| trimmed.to_string());
            Some((maintainer, None, None))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ---------------------------------------------------------------------------
// Top-level distribution
// ---------------------------------------------------------------------------

/// Core metadata for an Ubuntu distribution (or any Launchpad distribution).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Distribution {
    /// Short identifying name (e.g. `"ubuntu"`).
    pub name: String,
    /// Human-readable display name (e.g. `"Ubuntu"`).
    pub display_name: Option<String>,
    /// Short title.
    pub title: Option<String>,
    /// One-paragraph summary.
    pub summary: Option<String>,
    /// Whether packages are tracked in Launchpad.
    pub official_packages: Option<bool>,
    /// API self-link.
    pub self_link: Option<String>,
    /// Launchpad web link.
    pub web_link: Option<String>,
    /// API link to the distribution owner.
    pub owner_link: Option<String>,
    /// API link to the bug supervisor.
    pub bug_supervisor_link: Option<String>,
}

/// Fetch top-level metadata for a distribution.
///
/// `distro` is the distribution identifier, e.g. `"ubuntu"` or `"debian"`.
pub async fn get_distro(client: &LaunchpadClient, distro: &str) -> Result<Distribution> {
    Ok(get_distro_raw(client, distro).await?.value)
}

/// Like [`get_distro()`], but also keep the exact JSON Launchpad returned.
pub async fn get_distro_raw(client: &LaunchpadClient, distro: &str) -> Result<Raw<Distribution>> {
    client.get_raw(&format!("/{}", enc(distro))).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distro_series_deserialise() {
        let json = r#"{
            "name": "jammy",
            "display_name": "Ubuntu Jammy Jellyfish",
            "version": "22.04",
            "active": true,
            "self_link": "https://api.launchpad.net/devel/ubuntu/jammy",
            "web_link": "https://launchpad.net/ubuntu/jammy",
            "status": "Current Stable Release"
        }"#;
        let series: DistroSeries = serde_json::from_str(json).unwrap();
        assert_eq!(series.name, "jammy");
        assert_eq!(series.version.as_deref(), Some("22.04"));
        assert_eq!(series.active, Some(true));
    }

    #[test]
    fn source_publication_deserialise_minimal() {
        let json = r#"{
            "self_link": null,
            "source_package_name": "linux",
            "source_package_version": "5.15.0-1.1",
            "component_name": "main",
            "section_name": "devel",
            "status": "Published",
            "date_published": null,
            "date_superseded": null,
            "pocket": "Release",
            "archive_link": null,
            "distro_series_link": null
        }"#;
        let pub_history: SourcePackagePublishingHistory = serde_json::from_str(json).unwrap();
        assert_eq!(pub_history.source_package_name.as_deref(), Some("linux"));
        assert_eq!(pub_history.pocket.as_deref(), Some("Release"));
        assert_eq!(pub_history.package_maintainer_link, None);
    }

    #[test]
    fn source_publication_deserialise_with_package_maintainer_link() {
        let with_maintainer = r#"{
            "source_package_name": "unbound",
            "package_maintainer_link": "https://api.launchpad.net/devel/~ubuntu-devel-discuss-lists"
        }"#;
        let parsed: SourcePackagePublishingHistory = serde_json::from_str(with_maintainer).unwrap();
        assert_eq!(
            parsed.package_maintainer_link.as_deref(),
            Some("https://api.launchpad.net/devel/~ubuntu-devel-discuss-lists")
        );

        let with_null = r#"{
            "source_package_name": "unbound",
            "package_maintainer_link": null
        }"#;
        let parsed_null: SourcePackagePublishingHistory = serde_json::from_str(with_null).unwrap();
        assert_eq!(parsed_null.package_maintainer_link, None);
    }

    #[test]
    fn archive_deserialise() {
        let json = r#"{
            "self_link": "https://api.launchpad.net/devel/~canonical-kernel-team/+archive/ubuntu/ppa",
            "name": "ppa",
            "description": "Canonical Kernel Team PPA",
            "enabled": true,
            "num_pkgs": 42,
            "web_link": "https://launchpad.net/~canonical-kernel-team/+archive/ubuntu/ppa",
            "owner_link": "https://api.launchpad.net/devel/~canonical-kernel-team"
        }"#;
        let archive: Archive = serde_json::from_str(json).unwrap();
        assert_eq!(archive.name.as_deref(), Some("ppa"));
        assert_eq!(archive.enabled, Some(true));
    }

    #[test]
    fn source_search_params_default() {
        let p = SourceSearchParams::default();
        assert!(p.source_name.is_none());
        assert!(p.pocket.is_none());
    }

    #[test]
    fn enc_encodes_spaces() {
        let out = enc("Fix Released");
        assert!(!out.contains(' '));
    }

    #[tokio::test]
    async fn search_ppa_sources_raw_sends_optional_series_and_pocket() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let body = serde_json::json!({
            "start": 0,
            "total_size": 1,
            "entries": [{
                "self_link": format!("{base}/~mozillateam/+archive/ubuntu/ppa/+sourcepub/1"),
                "source_package_name": "firefox",
                "source_package_version": "160.0-1",
                "status": "Published",
                "pocket": "Release",
            }]
        });
        let mock = server
            .mock("GET", "/~mozillateam/+archive/ubuntu/ppa")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getPublishedSources".into()),
                mockito::Matcher::UrlEncoded(
                    "distro_series".into(),
                    format!("{base}/ubuntu/noble"),
                ),
                mockito::Matcher::UrlEncoded("source_name".into(), "firefox".into()),
                mockito::Matcher::UrlEncoded("pocket".into(), "Release".into()),
                mockito::Matcher::UrlEncoded("status".into(), "Published".into()),
                mockito::Matcher::UrlEncoded("exact_match".into(), "true".into()),
                mockito::Matcher::UrlEncoded("ws.size".into(), "1".into()),
            ]))
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;

        let client = LaunchpadClient::new(None).with_base_url(base);
        let params = SourceSearchParams {
            source_name: Some("firefox"),
            pocket: Some("Release"),
            status: Some("Published"),
            exact_match: true,
            limit: Some(1),
            ..Default::default()
        };
        let found = search_ppa_sources_raw(&client, "mozillateam", "ppa", Some("noble"), &params)
            .await
            .unwrap();
        assert_eq!(found.value.entries.len(), 1);
        assert_eq!(
            found.value.entries[0].source_package_version.as_deref(),
            Some("160.0-1")
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_source_changelog_url_handles_string_and_null_and_fetches_text() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let pub_link = format!("{base}/ubuntu/+archive/primary/+sourcepub/18891390");
        let missing_pub_link = format!("{base}/ubuntu/+archive/primary/+sourcepub/999");

        let url_mock = server
            .mock("GET", "/ubuntu/+archive/primary/+sourcepub/18891390")
            .match_query(mockito::Matcher::UrlEncoded(
                "ws.op".into(),
                "changelogUrl".into(),
            ))
            .with_status(200)
            .with_body(serde_json::Value::String(format!("{base}/+files/changelog")).to_string())
            .create_async()
            .await;
        let null_mock = server
            .mock("GET", "/ubuntu/+archive/primary/+sourcepub/999")
            .match_query(mockito::Matcher::UrlEncoded(
                "ws.op".into(),
                "changelogUrl".into(),
            ))
            .with_status(200)
            .with_body("null")
            .create_async()
            .await;
        let text_mock = server
            .mock("GET", "/+files/changelog")
            .with_status(200)
            .with_body("unbound (1.19.2-1ubuntu3.12) noble-security; urgency=medium\n\n  * Fix bug.\n\n -- Allen Huang <allen.huang@canonical.com>  Wed, 14 Oct 2026 22:26:11 +0100\n")
            .create_async()
            .await;

        let client = LaunchpadClient::new(None).with_base_url(base);
        let url = get_source_changelog_url(&client, &pub_link)
            .await
            .unwrap()
            .unwrap();
        let text = fetch_source_changelog(&client, &url).await.unwrap();
        assert!(text.contains("unbound (1.19.2-1ubuntu3.12)"));
        assert_eq!(
            get_source_changelog_url(&client, &missing_pub_link)
                .await
                .unwrap(),
            None
        );

        url_mock.assert_async().await;
        null_mock.assert_async().await;
        text_mock.assert_async().await;
    }

    #[test]
    fn parse_debian_changelog_handles_stanzas_metadata_crlf_and_slice() {
        let raw = concat!(
            "unbound (1.19.2-1ubuntu3.12) noble-security noble-updates; urgency=HIGH, binary-only=yes\r\n",
            "\r\n",
            "  * SECURITY UPDATE: Allocation fix\r\n",
            "    - detail line\r\n",
            "\r\n",
            " -- Allen Huang <allen.huang@canonical.com>  Wed, 14 Oct 2026 22:26:11 +0100\r\n",
            "\r\n",
            "unbound (1.19.2-1ubuntu3.10) noble; urgency=medium\r\n",
            "\r\n",
            "  * Rebuild without change\r\n",
            "\r\n",
            "unbound (1.19.2-1) unstable; urgency=low\r\n",
            "\r\n",
            "  * New upstream release.\r\n",
            "\r\n",
            " -- Maintainer <m@example.org>  not-a-valid-date\r\n",
            "\r\n",
            "Local variables:\r\n",
            "mode: debian-changelog\r\n",
            "End:\r\n",
        );

        let entries = parse_debian_changelog(raw);
        assert_eq!(entries.len(), 3, "{entries:?}");

        assert_eq!(entries[0].package, "unbound");
        assert_eq!(entries[0].version, "1.19.2-1ubuntu3.12");
        assert_eq!(
            entries[0].distributions,
            vec!["noble-security", "noble-updates"]
        );
        assert_eq!(entries[0].urgency.as_deref(), Some("HIGH"));
        assert_eq!(
            entries[0].metadata,
            vec![("binary-only".to_string(), "yes".to_string())]
        );
        assert_eq!(
            entries[0].changes,
            "  * SECURITY UPDATE: Allocation fix\n    - detail line"
        );
        assert_eq!(
            entries[0].maintainer.as_deref(),
            Some("Allen Huang <allen.huang@canonical.com>")
        );
        assert_eq!(
            entries[0].date.as_deref(),
            Some("Wed, 14 Oct 2026 22:26:11 +0100")
        );
        assert_eq!(
            entries[0].timestamp.map(|ts| ts.to_string()),
            Some("2026-10-14T21:26:11Z".to_string())
        );

        // Missing trailer stanza is still recovered cleanly before next header.
        assert_eq!(entries[1].version, "1.19.2-1ubuntu3.10");
        assert_eq!(entries[1].maintainer, None);
        assert_eq!(entries[1].timestamp, None);
        assert_eq!(entries[1].changes, "  * Rebuild without change");

        // Invalid RFC 2822 date keeps raw date while setting timestamp = None,
        // and trailing Emacs Local variables block is ignored.
        assert_eq!(entries[2].version, "1.19.2-1");
        assert_eq!(entries[2].date.as_deref(), Some("not-a-valid-date"));
        assert_eq!(entries[2].timestamp, None);
        assert_eq!(entries[2].changes, "  * New upstream release.");

        // Slicing returns exact top N stanzas without trailing footer.
        assert_eq!(slice_debian_changelog(raw, 0), "");
        let sliced_one = slice_debian_changelog(raw, 1);
        assert!(sliced_one.contains("1.19.2-1ubuntu3.12"));
        assert!(!sliced_one.contains("1.19.2-1ubuntu3.10"));
        assert!(!sliced_one.contains("Local variables:"));

        let sliced_two = slice_debian_changelog(raw, 2);
        assert!(sliced_two.contains("1.19.2-1ubuntu3.10"));
        assert!(!sliced_two.contains("1.19.2-1) unstable"));

        // Non-standard changelog yields zero stanzas and slice preserves raw text.
        let unparsed = "Plain release notes without debian stanza headers\n";
        assert!(parse_debian_changelog(unparsed).is_empty());
        assert_eq!(
            slice_debian_changelog(unparsed, 5),
            "Plain release notes without debian stanza headers"
        );
    }

    #[tokio::test]
    async fn get_source_file_urls_and_changelog_url_reject_cross_origin_self_link() {
        let mut api = mockito::Server::new_async().await;
        let mut foreign = mockito::Server::new_async().await;
        let api_hit = api
            .mock("GET", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let foreign_hit = foreign
            .mock("GET", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;

        let client = LaunchpadClient::new(Some(crate::auth::Credentials::new(
            "lpcli",
            "tok",
            "topsecret",
        )))
        .with_base_url(api.url());
        let foreign_link = format!("{}/ubuntu/+sourcepub/1", foreign.url());

        let files_err = get_source_file_urls(&client, &foreign_link)
            .await
            .unwrap_err()
            .to_string();
        assert!(files_err.contains("cross-origin"), "{files_err}");
        assert!(!files_err.contains("topsecret"), "{files_err}");

        let changelog_err = get_source_changelog_url(&client, &foreign_link)
            .await
            .unwrap_err()
            .to_string();
        assert!(changelog_err.contains("cross-origin"), "{changelog_err}");
        assert!(!changelog_err.contains("topsecret"), "{changelog_err}");

        let builds_err = get_source_builds_raw(&client, &foreign_link)
            .await
            .unwrap_err()
            .to_string();
        assert!(builds_err.contains("cross-origin"), "{builds_err}");
        assert!(!builds_err.contains("topsecret"), "{builds_err}");

        let binaries_err = source_has_active_binaries(&client, &foreign_link)
            .await
            .unwrap_err()
            .to_string();
        assert!(binaries_err.contains("cross-origin"), "{binaries_err}");
        assert!(!binaries_err.contains("topsecret"), "{binaries_err}");

        api_hit.assert_async().await;
        foreign_hit.assert_async().await;
    }

    #[test]
    fn source_build_deserialise() {
        let json = r#"{
            "self_link": "https://api.launchpad.net/devel/ubuntu/+archive/primary/+build/33824576",
            "web_link": "https://launchpad.net/ubuntu/+source/mutter/50.1-0ubuntu2.5/+build/33824576",
            "arch_tag": "amd64",
            "buildstate": "Successfully built",
            "datebuilt": "2026-10-08T09:04:54.281155+00:00",
            "duration": "0:24:28.874821",
            "build_log_url": "https://launchpad.net/ubuntu/+source/mutter/50.1-0ubuntu2.5/+build/33824576/+files/buildlog_ubuntu-resolute-amd64.mutter_50.1-0ubuntu2.5_BUILDING.txt.gz",
            "source_package_name": "mutter",
            "source_package_version": "50.1-0ubuntu2.5",
            "distro_series_link": "https://api.launchpad.net/devel/ubuntu/resolute",
            "pocket": "Release"
        }"#;
        let build: SourceBuild = serde_json::from_str(json).unwrap();
        assert_eq!(build.arch_tag.as_deref(), Some("amd64"));
        assert_eq!(build.buildstate.as_deref(), Some("Successfully built"));
        assert_eq!(
            build.date_built.map(|ts| ts.to_string()),
            Some("2026-10-08T09:04:54.281155Z".to_string())
        );
        assert_eq!(build.duration.as_deref(), Some("0:24:28.874821"));
        assert!(build.build_log_url.is_some());
    }

    #[tokio::test]
    async fn search_published_sources_sends_repeated_status_filters() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({"start": 0, "total_size": 0, "entries": []});
        let mock = server
            .mock("GET", "/ubuntu/+archive/primary")
            .match_query(mockito::Matcher::Regex(
                "status=Published&status=Superseded&status=Pending".into(),
            ))
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;

        let client = LaunchpadClient::new(None).with_base_url(server.url());
        let params = SourceSearchParams {
            statuses: &["Published", "Superseded", "Pending"],
            ..Default::default()
        };
        let found = search_published_sources_raw(&client, "ubuntu", "resolute", &params)
            .await
            .unwrap();
        assert!(found.value.entries.is_empty());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_source_builds_raw_requests_getbuilds() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let pub_link = format!("{base}/ubuntu/+archive/primary/+sourcepub/18893206");
        let body = serde_json::json!({
            "start": 0,
            "total_size": 2,
            "entries": [
                {
                    "self_link": format!("{base}/ubuntu/+archive/primary/+build/1"),
                    "arch_tag": "amd64",
                    "buildstate": "Successfully built",
                    "datebuilt": "2026-10-08T09:04:54.281155+00:00",
                    "duration": "0:24:28.874821",
                    "build_log_url": format!("{base}/+files/buildlog"),
                    "source_package_name": "mutter",
                    "source_package_version": "50.1-0ubuntu2.5",
                    "distro_series_link": format!("{base}/ubuntu/resolute"),
                    "pocket": "Release"
                },
                {
                    "self_link": format!("{base}/ubuntu/+archive/primary/+build/2"),
                    "arch_tag": "arm64",
                    "buildstate": "Failed to build",
                    "datebuilt": null,
                    "duration": null,
                    "build_log_url": null,
                    "source_package_name": "mutter",
                    "source_package_version": "50.1-0ubuntu2.5",
                    "distro_series_link": format!("{base}/ubuntu/resolute"),
                    "pocket": "Release"
                }
            ]
        });
        let mock = server
            .mock("GET", "/ubuntu/+archive/primary/+sourcepub/18893206")
            .match_query(mockito::Matcher::UrlEncoded(
                "ws.op".into(),
                "getBuilds".into(),
            ))
            .with_status(200)
            .with_body(body.to_string())
            .create_async()
            .await;

        let client = LaunchpadClient::new(None).with_base_url(base);
        let found = get_source_builds_raw(&client, &pub_link).await.unwrap();
        assert_eq!(found.value.len(), 2);
        assert_eq!(found.value[1].arch_tag.as_deref(), Some("arm64"));
        assert_eq!(
            found.value[1].buildstate.as_deref(),
            Some("Failed to build")
        );
        assert_eq!(found.value[1].date_built, None);
        assert_eq!(found.raw, body);

        let builds = get_source_builds(&client, &pub_link).await.unwrap();
        assert_eq!(builds, found.value);
        mock.expect(2).assert_async().await;
    }

    #[tokio::test]
    async fn source_has_active_binaries_uses_one_entry_page() {
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let with_binaries = "/ubuntu/+archive/primary/+sourcepub/18893206";
        let without_binaries = "/ubuntu/+archive/primary/+sourcepub/18973252";

        let has_mock = server
            .mock("GET", with_binaries)
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getPublishedBinaries".into()),
                mockito::Matcher::UrlEncoded("ws.size".into(), "1".into()),
            ]))
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "start": 0,
                    "entries": [{"binary_package_name": "mutter"}]
                })
                .to_string(),
            )
            .create_async()
            .await;
        let none_mock = server
            .mock("GET", without_binaries)
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getPublishedBinaries".into()),
                mockito::Matcher::UrlEncoded("ws.size".into(), "1".into()),
            ]))
            .with_status(200)
            .with_body(serde_json::json!({"start": 0, "entries": []}).to_string())
            .create_async()
            .await;

        let client = LaunchpadClient::new(None).with_base_url(base.clone());
        assert!(
            source_has_active_binaries(&client, &format!("{base}{with_binaries}"))
                .await
                .unwrap()
        );
        assert!(
            !source_has_active_binaries(&client, &format!("{base}{without_binaries}"))
                .await
                .unwrap()
        );

        has_mock.assert_async().await;
        none_mock.assert_async().await;
    }

    #[test]
    fn parse_debian_changelog_handles_emacs_local_variables_on_trailerless_and_leading_blocks() {
        let raw = concat!(
            "Local variables:\n",
            "coding: utf-8\n",
            "End:\n",
            "\n",
            "hello (2.10-2) noble; urgency=medium\n",
            "\n",
            "  * Newer entry.\n",
            "\n",
            " -- Maint <m@ubuntu.com>  Wed, 14 Oct 2026 22:26:11 +0100\n",
            "\n",
            "hello (2.10-1) noble; urgency=low\n",
            "\n",
            "  * Initial entry without trailer.\n",
            "\n",
            "Local variables:\n",
            "mode: debian-changelog\n",
            "End:\n",
        );
        let entries = parse_debian_changelog(raw);
        assert_eq!(entries.len(), 2, "{entries:?}");
        assert_eq!(entries[0].version, "2.10-2");
        assert_eq!(entries[0].changes, "  * Newer entry.");
        assert_eq!(entries[1].version, "2.10-1");
        assert_eq!(entries[1].changes, "  * Initial entry without trailer.");

        let stanzas = parse_debian_changelog_stanzas(raw);
        assert_eq!(stanzas.len(), 2);
        let second_span = &raw[stanzas[1].1.clone()];
        assert!(second_span.contains("hello (2.10-1)"), "{second_span}");
        assert!(!second_span.contains("Local variables:"), "{second_span}");
        assert!(
            !second_span.contains("mode: debian-changelog"),
            "{second_span}"
        );

        let sliced_one = slice_debian_changelog(raw, 1);
        assert!(sliced_one.contains("hello (2.10-2)"), "{sliced_one}");
        assert!(!sliced_one.contains("hello (2.10-1)"), "{sliced_one}");
        assert!(
            !sliced_one.contains("mode: debian-changelog"),
            "{sliced_one}"
        );
    }
}
