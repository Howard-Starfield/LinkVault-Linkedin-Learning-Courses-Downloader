//! YouTube provider adapter.
//!
//! URL interpretation, immutable scan plans, helper argument construction,
//! transcript normalization and artifact verification live here. Durable run
//! persistence uses `crate::workflow::WorkflowRuntime`. Process containment
//! uses `crate::app::managed_process`.

pub mod commands;
mod download_progress;
mod error;
mod executor;
mod helper;
pub(crate) mod kernel;
pub(crate) mod live;
pub mod manifest_contract;
pub mod media_protocol;
pub mod media_verifier;
pub mod models;
pub mod playback;
mod scan;
pub(crate) mod transcript_inspection;
pub mod transcript_normalizer;
