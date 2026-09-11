//! Bounded dispatch for application schema migrations that span providers.

use rusqlite::{Connection, Result};

mod linkedin_path_library_v8;
mod newspaper_clipping_drafts;
mod workflow_v7;

pub fn migrate(connection: &Connection) -> Result<()> {
    newspaper_clipping_drafts::install_and_verify(connection)?;
    workflow_v7::install_and_verify(connection)?;
    linkedin_path_library_v8::install_and_verify(connection)
}
