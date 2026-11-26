//! # lpcli – Launchpad CLI library
//!
//! `lpcli` is an async Rust library that provides idiomatic, type-safe wrappers
//! around the [Launchpad.net REST API](https://api.launchpad.net/devel.html).
//!
//! ## Modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`client`] | Low-level HTTP client for the Launchpad API |
//! | [`bugs`] | Bug tracking operations |
//! | [`people`] | Person and team queries |
//! | [`packages`] | Source packages and distribution series |
//! | [`projects`] | Project and milestone queries |
//! | [`error`] | Error types (`LpError`, `Result`) |
//!
//! The default `auth` feature provides the `auth`, `hardware_tokens`, and
//! `hardware_key` modules and enables the CLI. Library users who only access
//! public Launchpad data can disable default features to skip the OAuth,
//! keyring, and hardware-token dependencies.
//!
//! ## Quick start
//!
//! ```no_run
//! use lpcli::{bugs, client::LaunchpadClient};
//!
//! #[tokio::main]
//! async fn main() -> lpcli::error::Result<()> {
//!     let lp = LaunchpadClient::anonymous();
//!     let bug = bugs::get_bug(&lp, 1).await?;
//!     println!("Bug #{}: {}", bug.id, bug.title);
//!     Ok(())
//! }
//! ```

pub mod access_tokens;
#[cfg(feature = "auth")]
pub mod auth;
pub mod bugs;
pub mod client;
pub mod cves;
pub mod download;
pub mod error;
pub mod git;
#[cfg(feature = "auth")]
pub mod hardware_key;
#[cfg(feature = "auth")]
pub mod hardware_tokens;
pub mod packages;
pub mod people;
pub mod projects;
pub mod questions;
pub mod queue;
pub mod snaps;
pub mod specifications;
pub mod status;
pub mod time;
pub mod translations;
pub mod webhooks;
