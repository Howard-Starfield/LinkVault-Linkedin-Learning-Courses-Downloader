//! Background enrichment for legacy newspaper page geometry.
//!
//! Reader requests must never wait for this work. Current producer paths persist
//! dimensions directly; this module only repairs older rows that predate that
//! contract.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use rusqlite::params;

const DIMENSION_BACKFILL_BATCH_SIZE: usize = 16;

pub(super) fn schedule(db_path: PathBuf, running: Arc<AtomicBool>) {
    if running.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let lookup_path = db_path.clone();
        let candidates =
            tauri::async_runtime::spawn_blocking(move || missing_candidates(&lookup_path)).await;
        if let Ok(Ok(candidates)) = candidates {
            for chunk in candidates.chunks(DIMENSION_BACKFILL_BATCH_SIZE) {
                let batch_path = db_path.clone();
                let batch = chunk.to_vec();
                let _ = tauri::async_runtime::spawn_blocking(move || backfill(&batch_path, &batch))
                    .await;
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
        running.store(false, Ordering::SeqCst);
    });
}

pub(super) fn missing_candidates(db_path: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let candidates = connection
        .prepare(
            "SELECT id, COALESCE(optimized_path, original_path)
             FROM newspaper_pages
             WHERE status = 'completed'
               AND (pixel_width IS NULL OR pixel_height IS NULL)
               AND COALESCE(optimized_path, original_path) IS NOT NULL
             ORDER BY updated_at DESC, id",
        )
        .map_err(|error| error.to_string())?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                PathBuf::from(row.get::<_, String>(1)?),
            ))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(candidates)
}

pub(super) fn backfill(db_path: &Path, candidates: &[(String, PathBuf)]) -> Result<usize, String> {
    let mut connection = crate::cache::open_runtime(db_path).map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let mut updated = 0;
    for (page_id, path) in candidates {
        if let Ok((width, height)) = dimensions_without_full_decode(path) {
            updated += transaction
                .execute(
                    "UPDATE newspaper_pages
                     SET pixel_width = ?2, pixel_height = ?3
                     WHERE id = ?1
                       AND (pixel_width IS NULL OR pixel_height IS NULL)",
                    params![page_id, width, height],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(updated)
}

fn dimensions_without_full_decode(path: &Path) -> Result<(u32, u32), String> {
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    let mut header = [0_u8; 30];
    let mut header_len = 0;
    while header_len < header.len() {
        let bytes_read = file
            .read(&mut header[header_len..])
            .map_err(|error| error.to_string())?;
        if bytes_read == 0 {
            break;
        }
        header_len += bytes_read;
    }
    if let Some(dimensions) = webp_dimensions_from_header(&header[..header_len]) {
        return Ok(dimensions);
    }
    image::image_dimensions(path).map_err(|error| error.to_string())
}

fn webp_dimensions_from_header(header: &[u8]) -> Option<(u32, u32)> {
    if header.len() < 25 || &header[0..4] != b"RIFF" || &header[8..12] != b"WEBP" {
        return None;
    }

    match &header[12..16] {
        b"VP8 " if header.len() >= 30 && &header[23..26] == b"\x9d\x01\x2a" => {
            let width = u16::from_le_bytes([header[26], header[27]]) & 0x3fff;
            let height = u16::from_le_bytes([header[28], header[29]]) & 0x3fff;
            (width > 0 && height > 0).then_some((u32::from(width), u32::from(height)))
        }
        b"VP8L" if header[20] == 0x2f => {
            let bits = u32::from_le_bytes([header[21], header[22], header[23], header[24]]);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8X" if header.len() >= 30 => {
            let width = 1 + u32::from_le_bytes([header[24], header[25], header[26], 0]);
            let height = 1 + u32::from_le_bytes([header[27], header[28], header[29], 0]);
            Some((width, height))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_webp_canvas_dimensions_from_the_header_only() {
        let mut lossy = [0_u8; 30];
        lossy[0..4].copy_from_slice(b"RIFF");
        lossy[8..12].copy_from_slice(b"WEBP");
        lossy[12..16].copy_from_slice(b"VP8 ");
        lossy[23..26].copy_from_slice(b"\x9d\x01\x2a");
        lossy[26..28].copy_from_slice(&2500_u16.to_le_bytes());
        lossy[28..30].copy_from_slice(&4384_u16.to_le_bytes());
        assert_eq!(webp_dimensions_from_header(&lossy), Some((2500, 4384)));

        let mut lossless = [0_u8; 30];
        lossless[0..4].copy_from_slice(b"RIFF");
        lossless[8..12].copy_from_slice(b"WEBP");
        lossless[12..16].copy_from_slice(b"VP8L");
        lossless[20] = 0x2f;
        let lossless_bits = (2499_u32 & 0x3fff) | ((4383_u32 & 0x3fff) << 14);
        lossless[21..25].copy_from_slice(&lossless_bits.to_le_bytes());
        assert_eq!(webp_dimensions_from_header(&lossless), Some((2500, 4384)));

        let mut extended = [0_u8; 30];
        extended[0..4].copy_from_slice(b"RIFF");
        extended[8..12].copy_from_slice(b"WEBP");
        extended[12..16].copy_from_slice(b"VP8X");
        extended[24..27].copy_from_slice(&2499_u32.to_le_bytes()[..3]);
        extended[27..30].copy_from_slice(&4383_u32.to_le_bytes()[..3]);
        assert_eq!(webp_dimensions_from_header(&extended), Some((2500, 4384)));
    }
}
