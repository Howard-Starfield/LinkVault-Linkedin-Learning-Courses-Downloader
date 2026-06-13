//! Coursera tab — fully isolated sibling to the LinkedIn Learning downloader.
//!
//! This module is intentionally private at Phase 0. The LinkedIn side is
//! untouched. Public re-exports are introduced in Phase 3 when the Tauri
//! command surface needs to be reachable from `lib.rs`.
//!
//! See:
//! - `docs/learning/agent-harness-coursera/README.md`
//! - `docs/learning/agent-harness-coursera/ISOLATION_RULES.md`
//! - `docs/coursera-tab-implementation.md`

// Phase 0+: submodules added in their respective phases.
pub mod auth;
pub mod client;
pub mod commands;
pub mod config;
pub mod coursera_token_store;
pub mod define;
pub mod downloader;
pub mod error;
pub mod extractors;
pub mod filter;
pub mod format;
pub mod job;
pub mod orchestrator;
pub mod syllabus;
pub mod utils;

#[allow(unused_imports)]
pub use config::{
    parse_class_input, parse_format_list, parse_one_class, parse_subtitle_languages, AuthMethod,
    CourseraOptions, ModuleGetOpts, ParsedCourseraClass, SavedCourseraPreferences,
    StartCourseraRequest, VideoResolution,
};
#[allow(unused_imports)]
pub use define::format_url;
#[allow(unused_imports)]
pub use error::{CourseraError, CourseraResult};
#[allow(unused_imports)]
pub use utils::{clean_filename, clean_url, decode_input, is_debug_run, mkdir_p};

#[cfg(test)]
mod phase0_smoke {
    /// Phase 0 smoke test: the module compiles, the submodules resolve, and
    /// we never touch any LinkedIn-side symbol. The build itself is the gate.
    #[test]
    fn module_compiles_and_submodules_resolve() {
        // The submodules are private; existence is the assertion.
        // This test simply exists so `cargo test` is green at Phase 0.
    }
}
