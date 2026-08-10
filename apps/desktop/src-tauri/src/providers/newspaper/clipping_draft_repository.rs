//! SQLite ownership for recovery-only newspaper clipping note checkpoints.

use rusqlite::{params, Connection, OptionalExtension, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftCheckpoint {
    pub clipping_id: String,
    pub base_revision: u64,
    pub writer_session_id: String,
    pub writer_sequence: u64,
    pub title: String,
    pub markdown: String,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftCheckpointAck {
    pub writer_session_id: String,
    pub writer_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftLoadOutcome {
    NotFound,
    NotEditable,
    Empty {
        canonical_revision: u64,
    },
    Found {
        canonical_revision: u64,
        checkpoint: DraftCheckpoint,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftWriteOutcome {
    Stored,
    Idempotent,
    StaleSequence,
    WriterConflict,
    NotFound,
    NotEditable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftIdentityOutcome {
    Applied,
    StaleSequence,
    WriterConflict,
    NotFound,
}

pub fn load(connection: &Connection, clipping_id: &str) -> Result<DraftLoadOutcome> {
    let canonical = connection
        .query_row(
            "SELECT asset_state, revision FROM newspaper_clippings WHERE id = ?1",
            [clipping_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )
        .optional()?;
    let Some((state, canonical_revision)) = canonical else {
        return Ok(DraftLoadOutcome::NotFound);
    };
    if !matches!(state.as_str(), "ready" | "missing") {
        return Ok(DraftLoadOutcome::NotEditable);
    }
    let checkpoint = connection
        .query_row(
            "SELECT clipping_id, base_revision, writer_session_id, writer_sequence,
                    draft_title, draft_markdown, updated_at
             FROM newspaper_clipping_note_drafts WHERE clipping_id = ?1",
            [clipping_id],
            |row| {
                Ok(DraftCheckpoint {
                    clipping_id: row.get(0)?,
                    base_revision: row.get(1)?,
                    writer_session_id: row.get(2)?,
                    writer_sequence: row.get(3)?,
                    title: row.get(4)?,
                    markdown: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()?;
    Ok(match checkpoint {
        Some(checkpoint) => DraftLoadOutcome::Found {
            canonical_revision,
            checkpoint,
        },
        None => DraftLoadOutcome::Empty { canonical_revision },
    })
}

pub fn checkpoint(connection: &Connection, draft: &DraftCheckpoint) -> Result<DraftWriteOutcome> {
    match load(connection, &draft.clipping_id)? {
        DraftLoadOutcome::NotFound => return Ok(DraftWriteOutcome::NotFound),
        DraftLoadOutcome::NotEditable => return Ok(DraftWriteOutcome::NotEditable),
        _ => {}
    }
    let changed = connection.execute(
        "INSERT INTO newspaper_clipping_note_drafts (
            clipping_id, base_revision, writer_session_id, writer_sequence,
            draft_title, draft_markdown, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(clipping_id) DO UPDATE SET
            base_revision = excluded.base_revision,
            writer_sequence = excluded.writer_sequence,
            draft_title = excluded.draft_title,
            draft_markdown = excluded.draft_markdown,
            updated_at = excluded.updated_at
         WHERE newspaper_clipping_note_drafts.writer_session_id = excluded.writer_session_id
           AND newspaper_clipping_note_drafts.writer_sequence < excluded.writer_sequence",
        params![
            draft.clipping_id,
            draft.base_revision,
            draft.writer_session_id,
            draft.writer_sequence,
            draft.title,
            draft.markdown,
            draft.updated_at,
        ],
    )?;
    if changed == 1 {
        return Ok(DraftWriteOutcome::Stored);
    }
    let DraftLoadOutcome::Found { checkpoint, .. } = load(connection, &draft.clipping_id)? else {
        return Ok(DraftWriteOutcome::NotFound);
    };
    if checkpoint.writer_session_id != draft.writer_session_id {
        return Ok(DraftWriteOutcome::WriterConflict);
    }
    if checkpoint.writer_sequence == draft.writer_sequence
        && checkpoint.base_revision == draft.base_revision
        && checkpoint.title == draft.title
        && checkpoint.markdown == draft.markdown
    {
        Ok(DraftWriteOutcome::Idempotent)
    } else {
        Ok(DraftWriteOutcome::StaleSequence)
    }
}

pub fn claim(
    connection: &Connection,
    clipping_id: &str,
    prior: &DraftCheckpointAck,
    writer_session_id: &str,
) -> Result<DraftIdentityOutcome> {
    let changed = connection.execute(
        "UPDATE newspaper_clipping_note_drafts
         SET writer_session_id = ?4, writer_sequence = 1
         WHERE clipping_id = ?1 AND writer_session_id = ?2 AND writer_sequence = ?3",
        params![
            clipping_id,
            prior.writer_session_id,
            prior.writer_sequence,
            writer_session_id,
        ],
    )?;
    if changed == 1 {
        return Ok(DraftIdentityOutcome::Applied);
    }
    classify_identity_miss(connection, clipping_id, prior)
}

pub fn discard(
    connection: &Connection,
    clipping_id: &str,
    identity: &DraftCheckpointAck,
) -> Result<DraftIdentityOutcome> {
    let changed = connection.execute(
        "DELETE FROM newspaper_clipping_note_drafts
         WHERE clipping_id = ?1 AND writer_session_id = ?2 AND writer_sequence = ?3",
        params![
            clipping_id,
            identity.writer_session_id,
            identity.writer_sequence
        ],
    )?;
    if changed == 1 {
        return Ok(DraftIdentityOutcome::Applied);
    }
    classify_identity_miss(connection, clipping_id, identity)
}

fn classify_identity_miss(
    connection: &Connection,
    clipping_id: &str,
    identity: &DraftCheckpointAck,
) -> Result<DraftIdentityOutcome> {
    let stored = connection
        .query_row(
            "SELECT writer_session_id, writer_sequence
             FROM newspaper_clipping_note_drafts WHERE clipping_id = ?1",
            [clipping_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )
        .optional()?;
    Ok(match stored {
        None => DraftIdentityOutcome::NotFound,
        Some((session, _)) if session != identity.writer_session_id => {
            DraftIdentityOutcome::WriterConflict
        }
        Some(_) => DraftIdentityOutcome::StaleSequence,
    })
}

pub fn clear_acknowledged(
    connection: &Connection,
    clipping_id: &str,
    identity: &DraftCheckpointAck,
    exact_content: Option<(&str, &str)>,
) -> Result<usize> {
    match exact_content {
        Some((title, markdown)) => connection.execute(
            "DELETE FROM newspaper_clipping_note_drafts
             WHERE clipping_id = ?1 AND writer_session_id = ?2 AND writer_sequence <= ?3
               AND draft_title = ?4 AND draft_markdown = ?5",
            params![
                clipping_id,
                identity.writer_session_id,
                identity.writer_sequence,
                title,
                markdown
            ],
        ),
        None => connection.execute(
            "DELETE FROM newspaper_clipping_note_drafts
             WHERE clipping_id = ?1 AND writer_session_id = ?2 AND writer_sequence <= ?3",
            params![
                clipping_id,
                identity.writer_session_id,
                identity.writer_sequence
            ],
        ),
    }
}
