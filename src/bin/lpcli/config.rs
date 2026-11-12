//! User configuration for the `lpcli` command-line tool.
//!
//! Configuration is read from `$XDG_CONFIG_HOME/lpcli/config.toml` (or the
//! platform equivalent).  A missing file is not an error; it yields the
//! default configuration.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use lpcli::error::{LpError, Result};

/// Top-level configuration for the `lpcli` command-line tool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    /// Display-related settings.
    pub display: DisplayConfig,
}

/// Display-related settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    /// Terminal width used when formatting output, in columns.
    ///
    /// `None` means "auto-detect" (the default).
    pub width: Option<u16>,
}

/// Returns the path to the lpcli configuration file.
///
/// Defaults to `~/.config/lpcli/config.toml` on Linux.
pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| LpError::Config("Cannot determine config directory".to_string()))?;
    Ok(base.join("lpcli").join("config.toml"))
}

/// Load the user configuration.
///
/// A missing configuration file is not an error; the default configuration is
/// returned instead.
pub fn load() -> Result<AppConfig> {
    load_from(&config_path()?)
}

/// Load the user configuration from `path`.
///
/// A missing file yields [`AppConfig::default`].  Malformed TOML, unknown
/// fields, wrong value types, and out-of-range widths are reported as
/// [`LpError::Config`] with a message that identifies the problem.
pub fn load_from(path: &Path) -> Result<AppConfig> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AppConfig::default());
        }
        Err(err) => {
            return Err(LpError::Config(format!(
                "Failed to read config file {}: {err}",
                path.display()
            )));
        }
    };

    let config: AppConfig = toml::from_str(&raw).map_err(|err| {
        LpError::Config(format!(
            "Failed to parse config file {}: {err}",
            path.display()
        ))
    })?;
    if config.display.width == Some(0) {
        return Err(LpError::Config(format!(
            "Invalid display.width in {}: must be between 1 and 65535 (got 0)",
            path.display()
        )));
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Write `contents` to a temporary `config.toml` and return the directory
    /// (which must stay alive) and the file path.
    fn write_config(contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, contents).unwrap();
        (dir, path)
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");

        assert_eq!(load_from(&path).unwrap(), AppConfig::default());
    }

    #[test]
    fn valid_config_loads_width() {
        let (_dir, path) = write_config("[display]\nwidth = 150\n");

        let config = load_from(&path).unwrap();

        assert_eq!(config.display.width, Some(150));
    }

    #[test]
    fn boundary_widths_are_valid() {
        for width in [1, u16::MAX] {
            let (_dir, path) = write_config(&format!("[display]\nwidth = {width}\n"));
            assert_eq!(load_from(&path).unwrap().display.width, Some(width));
        }
    }

    #[test]
    fn omitted_width_is_none() {
        let (_dir, path) = write_config("[display]\n");

        let config = load_from(&path).unwrap();

        assert_eq!(config.display.width, None);
    }

    #[test]
    fn omitted_display_table_is_default() {
        let (_dir, path) = write_config("");

        let config = load_from(&path).unwrap();

        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn malformed_toml_is_config_error() {
        let (_dir, path) = write_config("[display\nwidth = 150\n");

        let err = load_from(&path).unwrap_err();

        assert!(matches!(err, LpError::Config(_)));
        assert!(err.to_string().contains("Failed to parse config"));
    }

    #[test]
    fn zero_width_is_config_error() {
        let (_dir, path) = write_config("[display]\nwidth = 0\n");

        let err = load_from(&path).unwrap_err();

        assert!(matches!(err, LpError::Config(_)));
        assert!(err.to_string().contains("between 1 and 65535"));
        assert!(err.to_string().contains("got 0"));
    }

    #[test]
    fn out_of_range_width_is_config_error() {
        let (_dir, path) = write_config("[display]\nwidth = 65536\n");

        let err = load_from(&path).unwrap_err();

        assert!(matches!(err, LpError::Config(_)));
        assert!(err.to_string().contains("65536"));
        assert!(err.to_string().contains("width"));
    }

    #[test]
    fn negative_width_is_config_error() {
        let (_dir, path) = write_config("[display]\nwidth = -1\n");

        let err = load_from(&path).unwrap_err();

        assert!(matches!(err, LpError::Config(_)));
        assert!(err.to_string().contains("-1"));
        assert!(err.to_string().contains("width"));
    }

    #[test]
    fn wrong_type_width_is_config_error() {
        let (_dir, path) = write_config("[display]\nwidth = \"wide\"\n");

        let err = load_from(&path).unwrap_err();

        assert!(matches!(err, LpError::Config(_)));
        assert!(err.to_string().contains("width"));
        assert!(err.to_string().contains("wide"));
    }

    #[test]
    fn unknown_field_is_config_error() {
        let (_dir, path) = write_config("[display]\nwidth = 150\ncolour = true\n");

        let err = load_from(&path).unwrap_err();

        assert!(matches!(err, LpError::Config(_)));
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn unknown_top_level_field_is_config_error() {
        let (_dir, path) = write_config("displai = true\n");
        let err = load_from(&path).unwrap_err();
        assert!(matches!(err, LpError::Config(_)));
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn config_path_ends_with_lpcli_config_toml() {
        if dirs::config_dir().is_some() {
            let path = config_path().unwrap();
            assert!(path.ends_with(Path::new("lpcli").join("config.toml")));
        }
    }
}
