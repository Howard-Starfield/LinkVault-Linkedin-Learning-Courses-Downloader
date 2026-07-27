//! LinkedIn Learning provider.
//!
//! The crate-root re-exports in `lib.rs` are a temporary compatibility facade.
//! New code should use this module's owned paths or shared workflow ports.

pub mod artifact_downloader;
pub mod auth;
pub mod browser_cookies;
pub(crate) mod commands;
pub mod course;
pub mod download_orchestrator;
pub mod exercise_archive;
pub(crate) mod linkedin;
pub mod live_clients;
pub mod quality;
pub mod quiz_hints;
pub mod token_store;
