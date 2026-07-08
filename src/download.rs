//! Common file download utilities for Launchpad packages.
//!
//! This module provides shared download functionality used by both the
//! `queue` and `packages` modules to download source files from Launchpad.
//!
//! Downloaded files are written to a user-specified output directory
//! (defaulting to the current working directory).

use std::path::{Path, PathBuf};

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

/// Download a list of files from the given URLs into `output_dir`.
///
/// Each URL is fetched (with OAuth authentication when credentials are
/// present on the client) and saved under `output_dir` using the filename
/// from the URL's last path segment.
///
/// Unlike an all-or-nothing approach, this function attempts every download
/// and reports per-file success or failure via [`DownloadOutcome`].  This
/// allows the CLI to show progress for each file individually.
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

    let mut results = Vec::with_capacity(urls.len());

    for url in urls {
        let filename = match extract_filename(url) {
            Ok(f) => f,
            Err(e) => {
                results.push(DownloadOutcome::Failed {
                    filename: url.clone(),
                    error: e.to_string(),
                });
                continue;
            }
        };
        let dest = output_dir.join(&filename);
        match client.download_to_file(url, &dest).await {
            Ok(size) => {
                results.push(DownloadOutcome::Success(DownloadedFile {
                    path: dest,
                    size,
                }));
            }
            Err(e) => {
                // Clean up partial file if it was created.
                let _ = std::fs::remove_file(&dest);
                results.push(DownloadOutcome::Failed {
                    filename,
                    error: e.to_string(),
                });
            }
        }
    }

    Ok(results)
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
