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
    format!(
        "Progress: {}: 50% ({downloaded}/{total} bytes)",
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
            format!(
                "Error: {}: {}",
                sanitize_plain_fragment(filename),
                sanitize_plain_fragment(error)
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
            | '\u{2500}'..='\u{257F}'
            | '\u{2600}'..='\u{27BF}'
            | '\u{1F300}'..='\u{1FAFF}' => {}
            c if c.is_control() => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
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
    // Ensure the output directory exists.
    std::fs::create_dir_all(output_dir).map_err(|e| {
        LpError::Io(std::io::Error::new(
            e.kind(),
            format!(
                "Failed to create output directory '{}': {e}",
                output_dir.display()
            ),
        ))
    })?;

    if urls.is_empty() {
        return Ok(Vec::new());
    }

    let total_files = urls.len();
    if progress == ProgressMode::Plain {
        eprintln!("{}", format_plain_start(total_files, output_dir));
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
                    filename: url.clone(),
                    error: e.to_string(),
                };
                if progress == ProgressMode::Plain {
                    eprintln!("{}", format_plain_outcome(&outcome));
                }
                immediate_failures.push((idx, outcome));
                continue;
            }
        };

        // Create a progress bar for this file in rich mode.
        let pb = multi.as_ref().map(|m| {
            let bar = m.add(ProgressBar::new(0));
            bar.set_style(progress_style_bytes());
            bar.set_prefix(format!("[{}/{}]", idx + 1, total_files));
            bar.set_message(filename.clone());
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
                        eprintln!(
                            "{}",
                            format_plain_milestone(&filename, downloaded, total_size)
                        );
                    }
                })
                .await;

            match result {
                Ok(size) => {
                    let outcome = DownloadOutcome::Success(DownloadedFile { path: dest, size });
                    if let Some(pb) = pb {
                        pb.set_length(size);
                        pb.set_position(size);
                        pb.finish_with_message(format!("OK: {filename}"));
                    } else if progress == ProgressMode::Plain {
                        eprintln!("{}", format_plain_outcome(&outcome));
                    }
                    (idx, outcome)
                }
                Err(e) => {
                    // Clean up partial file if it was created.
                    let _ = std::fs::remove_file(&dest);
                    let outcome = DownloadOutcome::Failed {
                        filename: filename.clone(),
                        error: e.to_string(),
                    };
                    if let Some(pb) = pb {
                        pb.abandon_with_message(format!("Error: {filename}"));
                    } else if progress == ProgressMode::Plain {
                        eprintln!("{}", format_plain_outcome(&outcome));
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
                    eprintln!("{}", format_plain_outcome(&outcome));
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
        eprintln!("{}", format_plain_summary(&results));
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

/// Extract the filename from a URL (last path segment).
///
/// Uses percent-decoding (NOT form-urlencoded) so that `+` characters in
/// filenames (common in Debian versioning like `dfsg+24.04`) are preserved
/// literally rather than being misinterpreted as spaces.
fn extract_filename(url: &str) -> Result<String> {
    // URLs may have query strings; strip them first.
    let path_part = url.split('?').next().unwrap_or(url);
    let raw = path_part
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| LpError::Other(format!("Cannot extract filename from URL: {url}")))?;

    // Percent-decode only (preserves '+' literally).
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8_lossy()
        .into_owned();

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
}
