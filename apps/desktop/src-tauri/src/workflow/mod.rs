//! Shared durable workflow boundary.
//!
//! Persistence and a synthetic supervisor live here. Provider executors
//! register during later strangler cutovers; domain extractors stay provider-owned.

pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod ports;

pub use application::runtime::WorkflowRuntime;
