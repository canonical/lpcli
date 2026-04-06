//! # lpcli – Launchpad CLI library
//!
//! `lpcli` is an async Rust library that provides idiomatic, type-safe wrappers
//! around the [Launchpad.net REST API](https://api.launchpad.net/devel.html).
//!
//! ## Modules
//!
//! | Module | Description |
//! |--------|-------------|
//! | [`auth`] | OAuth 1.0a authentication and credential management |
//! | [`client`] | Low-level HTTP client for the Launchpad API |
//! | [`bugs`] | Bug tracking operations |
//! | [`people`] | Person and team queries |
//! | [`packages`] | Source packages and distribution series |
//! | [`projects`] | Project and milestone queries |
//! | [`error`] | Error types (`LpError`, `Result`) |
//!
//! ## Quick start
//!
//! ```no_run
//! use lpcli::{auth, client::LaunchpadClient, bugs};
//!
//! #[tokio::main]
//! async fn main() -> lpcli::error::Result<()> {
//!     let creds = auth::load_credentials()?;
//!     let lp = LaunchpadClient::new(Some(creds));
//!     let bug = bugs::get_bug(&lp, 1).await?;
//!     println!("Bug #{}: {}", bug.id, bug.title);
//!     Ok(())
//! }
//! ```

pub mod access_tokens;
pub mod auth;
pub mod bugs;
pub mod client;
pub mod cves;
pub mod error;
pub mod git;
pub mod packages;
pub mod people;
pub mod projects;
pub mod questions;
pub mod snaps;
pub mod specifications;
pub mod status;
pub mod translations;
pub mod webhooks;
