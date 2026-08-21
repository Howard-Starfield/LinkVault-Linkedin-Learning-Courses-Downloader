//! Application-level Tauri services.
//!
//! This module owns desktop lifecycle concerns that are not specific to a
//! content provider. Workflow execution will live in `crate::workflow`, while
//! provider discovery and download behavior lives in `crate::providers`.

pub mod cooperative_exit;
pub mod database;
pub mod database_diagnostics;
pub mod database_migrations;
pub mod database_writer;
#[cfg(feature = "crop-baseline")]
pub mod newspaper_clipping_crop_baseline;
#[cfg(feature = "durability-baseline")]
pub mod newspaper_clipping_note_durability_baseline;
pub mod safe_output_filesystem;
pub mod security;
pub mod storage;
pub(crate) mod updates;
pub(crate) mod window_activation;
