//! YouTube provider adapter for the internal V1 transient bridge.
//!
//! URL interpretation, immutable scan plans and helper argument construction
//! live here.  Scheduling, lifecycle state, cancellation and revisions remain
//! in `crate::workflow::transient`.

pub mod commands;
mod error;
mod executor;
mod helper;
pub mod manifest_contract;
pub mod models;
mod scan;
pub mod transcript_normalizer;
