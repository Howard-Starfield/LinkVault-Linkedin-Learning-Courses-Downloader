//! Coursera provider adapter.
//!
//! The crate-root `coursera` export is a compatibility facade used while the
//! shared workflow kernel is introduced provider by provider.
//!
//! See:
//! - `docs/architecture/README.md`
//! - `docs/learning/agent-harness-coursera/README.md`
//! - `docs/learning/agent-harness-coursera/ISOLATION_RULES.md`

pub mod auth;
pub mod client;
pub mod commands;
pub mod config;
pub mod coursera_token_store;
pub mod define;
pub mod downloader;
pub mod error;
pub mod executor;
pub mod extractors;
pub mod filter;
pub mod format;
pub mod job;
pub mod orchestrator;
pub mod projection;
pub mod syllabus;
pub mod utils;
pub mod workflow_compat;

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
mod boundary_smoke {
    /// The provider compiles as one owned module and its submodules resolve.
    #[test]
    fn module_compiles_and_submodules_resolve() {
        // Module compilation is the assertion.
    }
}
