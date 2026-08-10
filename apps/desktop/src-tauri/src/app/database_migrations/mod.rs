//! Bounded dispatch for application schema migrations that span providers.

use rusqlite::{Connection, Result};

mod newspaper_clipping_drafts;

pub fn migrate(connection: &Connection) -> Result<()> {
    newspaper_clipping_drafts::install_and_verify(connection)
}
