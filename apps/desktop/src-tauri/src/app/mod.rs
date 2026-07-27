//! Application-level Tauri services.
//!
//! This module owns desktop lifecycle concerns that are not specific to a
//! content provider. Workflow execution will live in `crate::workflow`, while
//! provider discovery and download behavior lives in `crate::providers`.

pub mod database;
pub mod database_diagnostics;
pub mod database_writer;
pub mod security;
pub mod storage;
pub(crate) mod updates;
