//! Built-in content-provider adapters.
//!
//! Provider modules own discovery, authentication adapters, provider clients,
//! domain projections, and artifact behavior. Shared scheduling, retry,
//! cancellation, and recovery will migrate to `crate::workflow`.

pub mod coursera;
pub mod linkedin;
pub mod newspaper;
