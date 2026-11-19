//! Common file download utilities for Launchpad packages.
//!
//! This module provides shared download functionality used by both the
//! `queue` and `packages` modules to download source files from Launchpad.
//!
//! Downloaded files are written to a user-specified output directory
//! (defaulting to the current working directory). Progress reporting on stderr
//! can use animated bars ([`ProgressMode::Rich`]), non-redrawing lines for
//! screen readers and non-terminal streams ([`ProgressMode::Plain`]), or be
//! disabled ([`ProgressMode::None`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

use crate::client::LaunchpadClient;
use crate::error::{LpError, Result};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Metadata about a successfully downloaded file.
#[derive(Debug, Clone)]
pub struct DownloadedFile {
    /// The local path where the file was saved.
    pub path: PathBuf,
    /// The size in bytes of the downloaded file.
    pub size: u64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// The outcome of attempting to download a single file.
#[derive(Debug, Clone)]
pub enum DownloadOutcome {
    /// The file was downloaded successfully.
    Success(DownloadedFile),
    /// The download failed with an error message.
    Failed {
        /// The filename that was being downloaded.
        filename: String,
        /// A human-readable error description.
        error: String,
    },
}

/// How [`download_files_with_progress`] reports progress on stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressMode {
    /// Animated progress bars via `indicatif`.
    #[default]
    Rich,
    /// Start line, occasional milestones, and a summary on stderr, with no redrawing.
    Plain,
    /// Do not report progress.
    None,
}

/// Format the initial stderr line emitted before downloads begin in
/// [`ProgressMode::Plain`].
pub fn format_plain_start(total_files: usize, output_dir: &Path) -> String {
    format!(
        "Downloading {total_files} file(s) to '{}'...",
        sanitize_plain_fragment(&output_dir.display().to_string())
    )
}

/// Format an intermediate transfer milestone line emitted on stderr in
/// [`ProgressMode::Plain`].
pub fn format_plain_milestone(filename: &str, downloaded: u64, total: u64) -> String {
    let percent = if total == 0 {
        0
    } else {
        ((u128::from(downloaded) * 100) / u128::from(total)).min(100)
    };
    format!(
        "Progress: {}: {percent}% ({downloaded}/{total} bytes)",
        sanitize_plain_fragment(filename)
    )
}

/// Format a per-file completion or failure line emitted on stderr in
/// [`ProgressMode::Plain`].
pub fn format_plain_outcome(outcome: &DownloadOutcome) -> String {
    match outcome {
        DownloadOutcome::Success(file) => {
            let filename = file.path.file_name().map_or_else(
                || file.path.to_string_lossy().into_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            format!(
                "OK: {} ({} bytes)",
                sanitize_plain_fragment(&filename),
                file.size
            )
        }
        DownloadOutcome::Failed { filename, error } => {
            let filename = crate::error::redact_secrets(filename);
            let error = crate::error::redact_secrets(error);
            format!(
                "Error: {}: {}",
                sanitize_plain_fragment(&filename),
                sanitize_plain_fragment(&error)
            )
        }
    }
}

/// Format the final summary line emitted after downloads finish in
/// [`ProgressMode::Plain`].
pub fn format_plain_summary(outcomes: &[DownloadOutcome]) -> String {
    let failed = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, DownloadOutcome::Failed { .. }))
        .count();
    let downloaded = outcomes.len() - failed;
    if failed == 0 {
        format!("Download complete: {downloaded} file(s) downloaded.")
    } else {
        format!("Download complete: {downloaded} file(s) downloaded, {failed} failed.")
    }
}

fn is_unsafe_unicode_format(c: char) -> bool {
    matches!(
        c,
        '\u{2028}' | '\u{2029}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

fn sanitize_plain_fragment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\r' => {}
            '\n' | '\t' => out.push(' '),
            '—' | '–' => out.push('-'),
            '✓' => out.push_str("OK"),
            '✗' => out.push_str("Error"),
            '●'
            | 'ℹ'
            | '\u{FE0E}'
            | '\u{FE0F}'
            | '\u{200D}'
            | '\u{2500}'..='\u{257F}'
            | '\u{2600}'..='\u{27BF}'
            | '\u{1F300}'..='\u{1FAFF}' => {}
            c if c.is_control() || is_unsafe_unicode_format(c) => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

fn sanitize_terminal_fragment(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\r' | '\n' | '\t' => out.push(' '),
            c if c.is_control() || is_unsafe_unicode_format(c) => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

fn write_stderr_line(line: &str) {
    use std::io::Write;

    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(line.as_bytes());
    let _ = stderr.write_all(b"\n");
}

/// Download a list of files from the given URLs into `output_dir` in parallel
/// using animated terminal progress bars ([`ProgressMode::Rich`]).
///
/// Each URL is fetched concurrently (with OAuth authentication when
/// credentials are present on the client) and saved under `output_dir`
/// using the filename from the URL's last path segment.
///
/// Unlike an all-or-nothing approach, this function attempts every download
/// and reports per-file success or failure via [`DownloadOutcome`].
///
/// # Errors
///
/// Returns a hard error only if the output directory cannot be created.
/// Individual file download failures are captured in the returned outcomes.
pub async fn download_files(
    client: &LaunchpadClient,
    urls: &[String],
    output_dir: &Path,
) -> Result<Vec<DownloadOutcome>> {
    download_files_with_progress(client, urls, output_dir, ProgressMode::Rich).await
}

/// Download a list of files from the given URLs into `output_dir` in parallel,
/// reporting progress according to `progress`.
///
/// # Errors
///
/// Returns a hard error only if the output directory cannot be created.
/// Individual file download failures are captured in the returned outcomes.
pub async fn download_files_with_progress(
    client: &LaunchpadClient,
    urls: &[String],
    output_dir: &Path,
    progress: ProgressMode,
) -> Result<Vec<DownloadOutcome>> {
    if urls.is_empty() {
        return Ok(Vec::new());
    }

    // Ensure the output directory exists only when there is work to write.
    tokio::fs::create_dir_all(output_dir).await.map_err(|e| {
        LpError::Io(std::io::Error::new(
            e.kind(),
            format!(
                "Failed to create output directory '{}': {e}",
                output_dir.display()
            ),
        ))
    })?;

    let total_files = urls.len();
    if progress == ProgressMode::Plain {
        write_stderr_line(&format_plain_start(total_files, output_dir));
    }

    let multi = (progress == ProgressMode::Rich).then(MultiProgress::new);

    // Prepare tasks: extract filenames upfront so we can report errors
    // immediately for malformed URLs without spawning a task.
    let mut tasks = Vec::with_capacity(total_files);
    let mut immediate_failures = Vec::new();

    for (idx, url) in urls.iter().enumerate() {
        let filename = match extract_filename(url) {
            Ok(f) => f,
            Err(e) => {
                let outcome = DownloadOutcome::Failed {
                    filename: redact_url_for_display(url),
                    error: crate::error::redact_secrets(&e.to_string()),
                };
                if progress == ProgressMode::Plain {
                    write_stderr_line(&format_plain_outcome(&outcome));
                }
                immediate_failures.push((idx, outcome));
                continue;
            }
        };

        let safe_display_name = sanitize_terminal_fragment(&filename);

        // Create a progress bar for this file in rich mode.
        let pb = multi.as_ref().map(|m| {
            let bar = m.add(ProgressBar::new(0));
            bar.set_style(progress_style_bytes());
            bar.set_prefix(format!("[{}/{}]", idx + 1, total_files));
            bar.set_message(safe_display_name.clone());
            bar
        });

        let dest = output_dir.join(&filename);
        let client_clone = client.clone();
        let url_clone = url.clone();

        let handle = tokio::spawn(async move {
            let milestone_reported = AtomicBool::new(false);
            let result = client_clone
                .download_to_file_with_progress(&url_clone, &dest, |downloaded, total| {
                    if let Some(pb) = &pb {
                        if let Some(total_size) = total
                            && pb.length() != Some(total_size)
                        {
                            pb.set_length(total_size);
                        }
                        pb.set_position(downloaded);
                    }
                    if progress == ProgressMode::Plain
                        && let Some(total_size) = total
                        && total_size > 0
                        && downloaded < total_size
                        && downloaded.saturating_mul(2) >= total_size
                        && !milestone_reported.swap(true, Ordering::Relaxed)
                    {
                        write_stderr_line(&format_plain_milestone(
                            &filename, downloaded, total_size,
                        ));
                    }
                })
                .await;

            match result {
                Ok(size) => {
                    let outcome = DownloadOutcome::Success(DownloadedFile { path: dest, size });
                    if let Some(pb) = pb {
                        pb.set_length(size);
                        pb.set_position(size);
                        pb.finish_with_message(format!("OK: {safe_display_name}"));
                    } else if progress == ProgressMode::Plain {
                        write_stderr_line(&format_plain_outcome(&outcome));
                    }
                    (idx, outcome)
                }
                Err(e) => {
                    // Clean up partial file if it was created.
                    let _ = tokio::fs::remove_file(&dest).await;
                    let outcome = DownloadOutcome::Failed {
                        filename: filename.clone(),
                        error: crate::error::redact_secrets(&e.to_string()),
                    };
                    if let Some(pb) = pb {
                        pb.abandon_with_message(format!("Error: {safe_display_name}"));
                    } else if progress == ProgressMode::Plain {
                        write_stderr_line(&format_plain_outcome(&outcome));
                    }
                    (idx, outcome)
                }
            }
        });

        tasks.push(handle);
    }

    // Await all download tasks concurrently.
    let task_results = futures_util::future::join_all(tasks).await;

    // Collect results in original URL order.
    let mut indexed_results: Vec<(usize, DownloadOutcome)> = Vec::with_capacity(total_files);

    for join_result in task_results {
        match join_result {
            Ok(outcome) => indexed_results.push(outcome),
            Err(e) => {
                // A JoinError means the task panicked — should not happen,
                // but handle gracefully.
                let outcome = DownloadOutcome::Failed {
                    filename: "<unknown>".to_string(),
                    error: format!("Download task failed: {e}"),
                };
                if progress == ProgressMode::Plain {
                    write_stderr_line(&format_plain_outcome(&outcome));
                }
                indexed_results.push((usize::MAX, outcome));
            }
        }
    }

    // Merge immediate failures with task results, then sort by index.
    indexed_results.extend(immediate_failures);
    indexed_results.sort_by_key(|(idx, _)| *idx);

    let results: Vec<DownloadOutcome> = indexed_results
        .into_iter()
        .map(|(_, outcome)| outcome)
        .collect();
    if progress == ProgressMode::Plain && !results.is_empty() {
        write_stderr_line(&format_plain_summary(&results));
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Progress bar styling
// ---------------------------------------------------------------------------

/// Build a progress bar style for byte-based file downloads.
///
/// Shows: `[current/total] filename [=====>  ] bytes/total (speed, eta)`
fn progress_style_bytes() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template("{prefix} {msg}\n     [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
        .expect("progress bar template is valid")
        .progress_chars("━╸─")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Strip query strings, fragments, and URL passwords and redact any credentials for display.
fn redact_url_for_display(url: &str) -> String {
    if let Ok(mut parsed) = url::Url::parse(url) {
        parsed.set_query(None);
        parsed.set_fragment(None);
        if parsed.password().is_some() {
            let _ = parsed.set_password(Some("[redacted]"));
        }
        return crate::error::redact_secrets(&sanitize_terminal_fragment(parsed.as_str()));
    }
    let without_fragment = url.split('#').next().unwrap_or(url);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    crate::error::redact_secrets(&sanitize_terminal_fragment(without_query))
}

fn is_safe_filename(name: &str) -> bool {
    if name.is_empty() || matches!(name, "." | "..") {
        return false;
    }
    if name.contains(['/', '\\', '\0']) {
        return false;
    }
    if name
        .chars()
        .any(|c| c.is_control() || is_unsafe_unicode_format(c))
    {
        return false;
    }
    let mut components = Path::new(name).components();
    matches!(
        components.next(),
        Some(std::path::Component::Normal(part)) if part == std::ffi::OsStr::new(name)
    ) && components.next().is_none()
}

/// Extract the filename from a URL (last path segment).
///
/// Uses percent-decoding (NOT form-urlencoded) so that `+` characters in
/// filenames (common in Debian versioning like `dfsg+24.04`) are preserved
/// literally rather than being misinterpreted as spaces. Rejects path
/// traversal, directory separators, and control or bidi-formatting characters.
fn extract_filename(url: &str) -> Result<String> {
    let parsed = url::Url::parse(url).map_err(|e| {
        LpError::Other(format!(
            "Invalid download URL '{}': {e}",
            redact_url_for_display(url)
        ))
    })?;
    let raw = parsed
        .path()
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            LpError::Other(format!(
                "Cannot extract filename from URL: {}",
                redact_url_for_display(url)
            ))
        })?;

    // Percent-decode only (preserves '+' literally).
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .into_owned();

    if !is_safe_filename(&decoded) {
        return Err(LpError::Other(format!(
            "Unsafe filename '{}' in URL: {}",
            sanitize_plain_fragment(&decoded),
            redact_url_for_display(url)
        )));
    }

    Ok(decoded)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_filename_simple() {
        let url = "https://launchpad.net/ubuntu/+archive/primary/+files/hello_2.10-3.dsc";
        let name = extract_filename(url).unwrap();
        assert_eq!(name, "hello_2.10-3.dsc");
    }

    #[test]
    fn extract_filename_with_query() {
        let url = "https://example.com/files/pkg_1.0.orig.tar.gz?token=abc123";
        let name = extract_filename(url).unwrap();
        assert_eq!(name, "pkg_1.0.orig.tar.gz");
    }

    #[test]
    fn extract_filename_empty_url() {
        let result = extract_filename("https://example.com/");
        assert!(result.is_err());
    }

    #[test]
    fn extract_filename_preserves_plus() {
        let url = "https://launchpad.net/+files/rustc-1.91_1.91.1+dfsg~24.04.orig.tar.xz";
        let name = extract_filename(url).unwrap();
        assert_eq!(name, "rustc-1.91_1.91.1+dfsg~24.04.orig.tar.xz");
    }

    #[test]
    fn extract_filename_percent_encoded() {
        let url = "https://example.com/files/hello%20world_1.0.dsc";
        let name = extract_filename(url).unwrap();
        assert_eq!(name, "hello world_1.0.dsc");
    }

    #[test]
    fn extract_filename_strips_fragment_and_rejects_traversal_and_controls() {
        assert_eq!(
            extract_filename("https://example.com/files/pkg_1.0.tar.gz#section").unwrap(),
            "pkg_1.0.tar.gz"
        );
        for bad in [
            "https://example.com/files/..%2F..%2F.bashrc",
            "https://example.com/files/%2Ftmp%2Fpwned",
            "https://example.com/files/%2e%2e",
            "https://example.com/files/%2e",
            "https://example.com/files/a%5Cb",
            "https://example.com/files/a%00b",
            "https://example.com/files/a%1b%5B31mb",
            "https://example.com/files/a%E2%80%AEb",
            "https://example.com",
            "https://example.com/dir/",
            "not a url",
        ] {
            let err = extract_filename(bad).unwrap_err();
            let msg = err.to_string();
            assert!(
                !msg.contains('\u{1b}') && !msg.contains('\u{202e}'),
                "unsafe control in error for {bad:?}: {msg:?}"
            );
        }
        let err = extract_filename("https://user:topsecret@example.com/?oauth_token=leak#frag")
            .unwrap_err()
            .to_string();
        assert!(!err.contains("topsecret"), "{err}");
        assert!(!err.contains("leak"), "{err}");
        assert!(!err.contains("frag"), "{err}");
    }

    #[test]
    fn format_plain_milestone_computes_accurate_percentage_without_overflow() {
        assert_eq!(
            format_plain_milestone("a.tar.gz", 512, 1024),
            "Progress: a.tar.gz: 50% (512/1024 bytes)"
        );
        assert_eq!(
            format_plain_milestone("a.tar.gz", 900, 1000),
            "Progress: a.tar.gz: 90% (900/1000 bytes)"
        );
        assert_eq!(
            format_plain_milestone("a.tar.gz", 10, 0),
            "Progress: a.tar.gz: 0% (10/0 bytes)"
        );
        assert_eq!(
            format_plain_milestone("a.tar.gz", 2000, 1000),
            "Progress: a.tar.gz: 100% (2000/1000 bytes)"
        );
        assert_eq!(
            format_plain_milestone("huge.iso", u64::MAX / 2, u64::MAX),
            format!(
                "Progress: huge.iso: 49% ({}/{} bytes)",
                u64::MAX / 2,
                u64::MAX
            )
        );
    }

    #[test]
    fn format_plain_outcome_and_terminal_sanitization_redact_secrets_and_bidi() {
        let outcome = DownloadOutcome::Failed {
            filename: "pkg_\u{202E}infoℹ️.dsc".into(),
            error: "oauth_token_secret=topsecret 'access_token': 'tok2'\u{2028}Authorization: Bearer hdr".into(),
        };
        let rendered = format_plain_outcome(&outcome);
        for leaked in ["topsecret", "tok2", "hdr"] {
            assert!(!rendered.contains(leaked), "{leaked} leaked: {rendered}");
        }
        for bad in ['\u{202E}', '\u{2028}', '\u{FE0F}', 'ℹ'] {
            assert!(
                !rendered.contains(bad),
                "unsafe char {bad:?} in {rendered:?}"
            );
        }

        let term = sanitize_terminal_fragment("hi\x1b[31m\r\n\t\u{202E}end");
        assert_eq!(term, "hi\u{FFFD}[31m   \u{FFFD}end");
    }

    #[tokio::test]
    async fn download_files_with_progress_skips_mkdir_on_empty_and_redacts_failures() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("never-created");
        let client = LaunchpadClient::new(None);
        let empty = download_files_with_progress(&client, &[], &target, ProgressMode::Plain)
            .await
            .unwrap();
        assert!(empty.is_empty());
        assert!(
            !target.exists(),
            "empty URL list must not create output directory"
        );

        let urls = vec![
            "https://user:pwsecret@example.com/files/..%2Fbad?oauth_token=toksecret#frag".into(),
        ];
        let outcomes = download_files_with_progress(&client, &urls, &target, ProgressMode::None)
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0] {
            DownloadOutcome::Failed { filename, error } => {
                assert!(!filename.contains("pwsecret"), "{filename}");
                assert!(!filename.contains("toksecret"), "{filename}");
                assert!(!filename.contains("frag"), "{filename}");
                assert!(!error.contains("pwsecret"), "{error}");
                assert!(!error.contains("toksecret"), "{error}");
            }
            DownloadOutcome::Success(_) => panic!("expected traversal URL to fail"),
        }
    }
}
