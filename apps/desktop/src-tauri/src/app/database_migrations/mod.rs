//! Bounded dispatch for application schema migrations that span providers.

use rusqlite::{Connection, Result};

pub(crate) mod linkedin_course_placement_v9;
pub(crate) mod linkedin_path_library_v8;
mod linkedin_query_indexes;
mod newspaper_clipping_drafts;
mod workflow_v7;

pub fn migrate(connection: &Connection) -> Result<()> {
    newspaper_clipping_drafts::install_and_verify(connection)?;
    workflow_v7::install_and_verify(connection)?;
    linkedin_path_library_v8::install_and_verify(connection)?;
    linkedin_course_placement_v9::install_and_verify(connection)
}

/// Query indexes are installed after `migrate`, not inside it.
///
/// `app::database::initialize` still has to rebuild `artifacts` for older
/// installations whose CHECK constraint predates the `quiz` and `study_guide`
/// types, and that rebuild drops the table together with its indexes. Running
/// this pass last guarantees the indexes exist on the table that survives.
pub fn install_query_indexes(connection: &Connection) -> Result<()> {
    linkedin_query_indexes::install_and_verify(connection)
}
