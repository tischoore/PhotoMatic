use chrono::{Duration, NaiveDateTime};
use rusqlite::Connection;
use xxhash_rust::xxh3::xxh3_64;

use super::models::ImageRecord;
use super::DbError;
use crate::raw_linking::pair_raw_with_compressed;

/// Hashes `path` into a stable, non-cryptographic dedup key (xxh3, lowercase hex).
/// Same input always produces the same key; different paths produce different keys.
pub fn image_key(path: &str) -> String {
    format!("{:016x}", xxh3_64(path.as_bytes()))
}

/// Inserts or refreshes `images` rows. Unconditionally overwrites `path`/`image_type`/
/// `toplevel_dir` for a known key — there's no user-entered data on this table to protect.
pub fn upsert_images(conn: &mut Connection, images: &[ImageRecord]) -> Result<(), DbError> {
    let tx = conn.transaction().map_err(DbError::Sqlite)?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO images (key, path, image_type, toplevel_dir) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(key) DO UPDATE SET path = excluded.path, image_type = excluded.image_type,
                 toplevel_dir = excluded.toplevel_dir",
            )
            .map_err(DbError::Sqlite)?;
        for image in images {
            stmt.execute(rusqlite::params![image.key, image.path, image.image_type, image.toplevel_dir])
                .map_err(DbError::Sqlite)?;
        }
    }
    tx.commit().map_err(DbError::Sqlite)?;
    Ok(())
}

/// Visible to the rest of `db` (in particular `db::events`'s `event_images` query) so an
/// event's photo table can be built with the same column set/order as every other
/// `ImageRecord` query, without duplicating it.
pub(super) const IMAGE_COLUMNS: &str = "key, path, image_type, toplevel_dir, date_taken, corrected_date_taken, \
     width, height, exposure_time, iso, focal_length, gps_latitude, gps_longitude, gps_altitude, metadata_read_at, \
     linked_key, rotation, color_black_r, color_black_g, color_black_b, color_white_r, color_white_g, color_white_b, \
     color_blend";

pub(super) fn map_image_row(row: &rusqlite::Row) -> rusqlite::Result<ImageRecord> {
    Ok(ImageRecord {
        key: row.get(0)?,
        path: row.get(1)?,
        image_type: row.get(2)?,
        toplevel_dir: row.get(3)?,
        date_taken: row.get(4)?,
        corrected_date_taken: row.get(5)?,
        width: row.get(6)?,
        height: row.get(7)?,
        exposure_time: row.get(8)?,
        iso: row.get(9)?,
        focal_length: row.get(10)?,
        gps_latitude: row.get(11)?,
        gps_longitude: row.get(12)?,
        gps_altitude: row.get(13)?,
        metadata_read_at: row.get(14)?,
        linked_key: row.get(15)?,
        rotation: row.get(16)?,
        color_black_r: row.get(17)?,
        color_black_g: row.get(18)?,
        color_black_b: row.get(19)?,
        color_white_r: row.get(20)?,
        color_white_g: row.get(21)?,
        color_white_b: row.get(22)?,
        color_blend: row.get(23)?,
    })
}

pub fn list_images(conn: &Connection) -> Result<Vec<ImageRecord>, DbError> {
    let mut stmt = conn
        .prepare(&format!("SELECT {IMAGE_COLUMNS} FROM images ORDER BY path"))
        .map_err(DbError::Sqlite)?;
    let rows = stmt.query_map([], map_image_row).map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

/// Images that have never had EXIF metadata extraction attempted
/// (`metadata_read_at IS NULL`) — the work list Generate MetaData iterates,
/// instead of every row, so a second run after an interrupted or partial
/// first run doesn't redo already-processed images.
pub fn list_images_pending_metadata(conn: &Connection) -> Result<Vec<ImageRecord>, DbError> {
    let mut stmt = conn
        .prepare(&format!("SELECT {IMAGE_COLUMNS} FROM images WHERE metadata_read_at IS NULL ORDER BY path"))
        .map_err(DbError::Sqlite)?;
    let rows = stmt.query_map([], map_image_row).map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

/// Writes extracted EXIF metadata for the image `key`, stamping
/// `metadata_read_at` to now regardless of whether any of `metadata`'s
/// fields came back `Some` — so the row drops out of
/// `list_images_pending_metadata` either way, and a file with no EXIF at
/// all is only ever opened and parsed once.
pub fn update_metadata(conn: &Connection, key: &str, metadata: &crate::exif::ImageMetadata) -> Result<(), DbError> {
    conn.execute(
        "UPDATE images SET date_taken = ?2, corrected_date_taken = ?2, width = ?3, height = ?4, exposure_time = ?5, \
         iso = ?6, focal_length = ?7, gps_latitude = ?8, gps_longitude = ?9, gps_altitude = ?10, \
         metadata_read_at = ?11 WHERE key = ?1",
        rusqlite::params![
            key,
            metadata.date_taken,
            metadata.width,
            metadata.height,
            metadata.exposure_time_seconds,
            metadata.iso,
            metadata.focal_length_mm,
            metadata.gps_latitude,
            metadata.gps_longitude,
            metadata.gps_altitude_m,
            chrono::Utc::now().naive_utc(),
        ],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Counts images grouped by top-level directory and extension, for the Left
/// Navigation tree. `toplevel_dir` is `None` for the group of images sitting
/// directly in the Source Directory.
pub fn count_by_directory_and_type(conn: &Connection) -> Result<Vec<(Option<String>, String, i64)>, DbError> {
    let mut stmt = conn
        .prepare("SELECT toplevel_dir, image_type, COUNT(*) FROM images GROUP BY toplevel_dir, image_type")
        .map_err(DbError::Sqlite)?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

/// Images under a specific top-level directory, optionally filtered to one File Type
/// extension, sorted chronologically by `corrected_date_taken` (falling back to `path` when
/// dates tie or are still unset, e.g. before Generate MetaData has run) — backs the Left
/// Navigation tree's "Image List" context menu action.
pub fn list_images_by_directory(
    conn: &Connection,
    toplevel_dir: &str,
    image_type: Option<&str>,
) -> Result<Vec<ImageRecord>, DbError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {IMAGE_COLUMNS} FROM images WHERE toplevel_dir = ?1 AND (?2 IS NULL OR image_type = ?2) \
             ORDER BY corrected_date_taken, path"
        ))
        .map_err(DbError::Sqlite)?;
    let rows = stmt
        .query_map(rusqlite::params![toplevel_dir, image_type], map_image_row)
        .map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

/// Re-derives every `linked_key` from scratch: clears all existing links, then pairs every
/// RAW image against its compressed sibling (via `raw_linking::pair_raw_with_compressed`,
/// run over the *entire* `images` table, not just the most recent scan batch, since a pairing
/// can span two separate scans) and writes both directions of each pair. One transaction, so
/// a failure never leaves a half-relinked table. Backs "Link RAW and compressed images" —
/// called at the end of every scan while the option is enabled, and once immediately when the
/// option is turned on for an already-scanned project.
pub fn relink_raw_images(conn: &mut Connection) -> Result<(), DbError> {
    let tx = conn.transaction().map_err(DbError::Sqlite)?;
    tx.execute("UPDATE images SET linked_key = NULL", []).map_err(DbError::Sqlite)?;

    let candidates: Vec<ImageRecord> = {
        let mut stmt = tx.prepare(&format!("SELECT {IMAGE_COLUMNS} FROM images")).map_err(DbError::Sqlite)?;
        let rows = stmt.query_map([], map_image_row).map_err(DbError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)?
    };

    {
        let mut stmt = tx.prepare("UPDATE images SET linked_key = ?2 WHERE key = ?1").map_err(DbError::Sqlite)?;
        for (raw_key, compressed_key) in pair_raw_with_compressed(&candidates) {
            stmt.execute(rusqlite::params![raw_key, compressed_key]).map_err(DbError::Sqlite)?;
            stmt.execute(rusqlite::params![compressed_key, raw_key]).map_err(DbError::Sqlite)?;
        }
    }

    tx.commit().map_err(DbError::Sqlite)
}

/// Clears every `linked_key` immediately — backs unchecking "Link RAW and compressed images".
pub fn clear_raw_links(conn: &Connection) -> Result<(), DbError> {
    conn.execute("UPDATE images SET linked_key = NULL", []).map_err(DbError::Sqlite)?;
    Ok(())
}

/// Writes the current photo's rotation (degrees clockwise, 0/90/180/270) — backs the Image
/// Viewer's Rotate button (Alt+R).
pub fn set_rotation(conn: &Connection, key: &str, rotation: i32) -> Result<(), DbError> {
    conn.execute("UPDATE images SET rotation = ?2 WHERE key = ?1", rusqlite::params![key, rotation])
        .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Writes the current photo's Simplest Color Balance correction (a low/high clip point per RGB
/// channel) — backs the Image Viewer's Auto Correct button.
pub fn set_color_correction(
    conn: &Connection,
    key: &str,
    params: &crate::color_correction::ColorCorrectionParams,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE images SET color_black_r = ?2, color_black_g = ?3, color_black_b = ?4, \
         color_white_r = ?5, color_white_g = ?6, color_white_b = ?7 WHERE key = ?1",
        rusqlite::params![
            key,
            params.black[0],
            params.black[1],
            params.black[2],
            params.white[0],
            params.white[1],
            params.white[2],
        ],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Nulls out the current photo's Simplest Color Balance correction — backs the Image Viewer's
/// Auto Correct checkbox being unchecked.
pub fn clear_color_correction(conn: &Connection, key: &str) -> Result<(), DbError> {
    conn.execute(
        "UPDATE images SET color_black_r = NULL, color_black_g = NULL, color_black_b = NULL, \
         color_white_r = NULL, color_white_g = NULL, color_white_b = NULL, color_blend = NULL WHERE key = ?1",
        rusqlite::params![key],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Writes the current photo's Auto Correct blend fraction (0.0 = original pixels, 1.0 = the
/// full correction) — backs the Image Viewer's Blend slider. A single-column update, unlike
/// `set_color_correction`: the clip points themselves aren't changing, so there's no need to
/// recompute them.
pub fn set_color_blend(conn: &Connection, key: &str, blend: f64) -> Result<(), DbError> {
    conn.execute("UPDATE images SET color_blend = ?2 WHERE key = ?1", rusqlite::params![key, blend])
        .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Resets one photo's `corrected_date_taken` back to its original `date_taken` — backs the
/// Image Viewer's Edit > Clear Time Correction, undoing any per-directory shift a previous
/// Set Time Correction run (`apply_time_corrections`) applied to it.
pub fn clear_time_correction(conn: &Connection, key: &str) -> Result<(), DbError> {
    conn.execute("UPDATE images SET corrected_date_taken = date_taken WHERE key = ?1", rusqlite::params![key])
        .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Resets every photo's `corrected_date_taken` back to its original `date_taken` in one
/// statement — backs the main window's Edit > Clear All Time Correction, effectively undoing
/// every Set Time Correction alignment in the project at once.
pub fn clear_all_time_corrections(conn: &Connection) -> Result<(), DbError> {
    conn.execute("UPDATE images SET corrected_date_taken = date_taken", []).map_err(DbError::Sqlite)?;
    Ok(())
}

/// Every image except a RAW image that's currently linked to a compressed sibling — the
/// event-eligible image list Generate Events clusters, so a linked RAW never forms or joins
/// its own event; only its compressed sibling participates. An unlinked RAW (no counterpart
/// found, or "Link RAW and compressed images" disabled) is included as normal.
pub fn list_event_eligible_images(conn: &Connection) -> Result<Vec<ImageRecord>, DbError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {IMAGE_COLUMNS} FROM images \
             WHERE NOT (image_type = 'cr2' AND linked_key IS NOT NULL) ORDER BY path"
        ))
        .map_err(DbError::Sqlite)?;
    let rows = stmt.query_map([], map_image_row).map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

/// Images matching any of `keys`, in no particular order — used by the Image Viewer to
/// batch-fetch every displayed photo's linked counterpart in a single query rather than one
/// query per photo. Returns an empty list without touching the database when `keys` is empty.
pub fn list_images_by_keys(conn: &Connection, keys: &[String]) -> Result<Vec<ImageRecord>, DbError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; keys.len()].join(",");
    let mut stmt = conn
        .prepare(&format!("SELECT {IMAGE_COLUMNS} FROM images WHERE key IN ({placeholders})"))
        .map_err(DbError::Sqlite)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(keys), map_image_row).map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

/// Adds `offset` to `corrected_date_taken` for every row under `toplevel_dir` (`None` = the
/// project-root bucket), for each `(toplevel_dir, offset)` pair in `corrections`, in one
/// transaction — backs the Set Time Correction dialog's Accept button. Applied against *every*
/// row under the directory, not `list_event_eligible_images`'s filtered view, so a linked RAW
/// picks up the same shift as the compressed sibling its lane displayed on its behalf. A row
/// whose `corrected_date_taken` is still NULL is left untouched rather than erred: it has
/// nothing to offset yet, and will be seeded already-corrected the next time Generate MetaData
/// runs on it (mirrors `update_metadata`'s "not yet processed" stance). The new value is
/// computed in Rust (`NaiveDateTime + Duration`) and written back directly, not via a SQL
/// `datetime()` modifier, so no sub-second precision is lost. Uses `toplevel_dir IS ?1`, not
/// `= ?1`, since `=` never matches NULL and the project-root bucket needs it to.
pub fn apply_time_corrections(conn: &mut Connection, corrections: &[(Option<String>, Duration)]) -> Result<(), DbError> {
    let tx = conn.transaction().map_err(DbError::Sqlite)?;
    {
        let mut select_stmt = tx
            .prepare("SELECT key, corrected_date_taken FROM images WHERE toplevel_dir IS ?1 AND corrected_date_taken IS NOT NULL")
            .map_err(DbError::Sqlite)?;
        let mut update_stmt = tx.prepare("UPDATE images SET corrected_date_taken = ?2 WHERE key = ?1").map_err(DbError::Sqlite)?;

        for (toplevel_dir, offset) in corrections {
            let rows: Vec<(String, NaiveDateTime)> = select_stmt
                .query_map(rusqlite::params![toplevel_dir], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(DbError::Sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(DbError::Sqlite)?;
            for (key, date_taken) in rows {
                update_stmt.execute(rusqlite::params![key, date_taken + *offset]).map_err(DbError::Sqlite)?;
            }
        }
    }
    tx.commit().map_err(DbError::Sqlite)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_path_always_hashes_to_the_same_key() {
        assert_eq!(image_key("sub/a.jpg"), image_key("sub/a.jpg"));
    }

    #[test]
    fn different_paths_hash_to_different_keys() {
        assert_ne!(image_key("sub/a.jpg"), image_key("sub/b.jpg"));
    }

    fn image(key: &str, path: &str, image_type: &str) -> ImageRecord {
        ImageRecord { key: key.to_string(), path: path.to_string(), image_type: image_type.to_string(), ..ImageRecord::default() }
    }

    fn migrated_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        conn
    }

    #[test]
    fn relink_raw_images_links_both_directions_of_a_pair() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("raw", "50D/a.cr2", "cr2"), image("jpg", "50D/a.jpg", "jpg")]).unwrap();

        relink_raw_images(&mut conn).unwrap();

        let images = list_images(&conn).unwrap();
        let raw = images.iter().find(|i| i.key == "raw").unwrap();
        let jpg = images.iter().find(|i| i.key == "jpg").unwrap();
        assert_eq!(raw.linked_key.as_deref(), Some("jpg"));
        assert_eq!(jpg.linked_key.as_deref(), Some("raw"));
    }

    #[test]
    fn relink_raw_images_leaves_unmatched_images_unlinked() {
        // Neither of these RAWs has a same-stem compressed sibling in its own directory —
        // `50D/b.jpg` doesn't match `a`'s stem, and `other/a.jpg` doesn't share `50D/a.cr2`'s
        // directory — so nothing here should end up linked.
        let mut conn = migrated_conn();
        upsert_images(
            &mut conn,
            &[image("raw", "50D/a.cr2", "cr2"), image("wrong-stem", "50D/b.jpg", "jpg"), image("wrong-dir", "other/a.jpg", "jpg")],
        )
        .unwrap();

        relink_raw_images(&mut conn).unwrap();

        let images = list_images(&conn).unwrap();
        assert!(images.iter().all(|i| i.linked_key.is_none()));
    }

    #[test]
    fn relink_raw_images_clears_stale_links_before_recomputing() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("raw", "50D/a.cr2", "cr2"), image("jpg", "50D/a.jpg", "jpg")]).unwrap();
        relink_raw_images(&mut conn).unwrap();

        // Simulates the jpg being renamed between scans, so it no longer shares a stem with
        // the RAW — a second relink must drop the stale link rather than leaving the RAW
        // pointing at a key whose path has since diverged.
        upsert_images(&mut conn, &[image("jpg", "50D/renamed.jpg", "jpg")]).unwrap();
        relink_raw_images(&mut conn).unwrap();

        let images = list_images(&conn).unwrap();
        assert!(images.iter().all(|i| i.linked_key.is_none()));
    }

    #[test]
    fn clear_raw_links_nulls_every_linked_key() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("raw", "50D/a.cr2", "cr2"), image("jpg", "50D/a.jpg", "jpg")]).unwrap();
        relink_raw_images(&mut conn).unwrap();

        clear_raw_links(&conn).unwrap();

        let images = list_images(&conn).unwrap();
        assert!(images.iter().all(|i| i.linked_key.is_none()));
    }

    #[test]
    fn list_event_eligible_images_excludes_a_linked_raw_but_keeps_its_compressed_counterpart() {
        let mut conn = migrated_conn();
        upsert_images(
            &mut conn,
            &[
                image("raw", "50D/a.cr2", "cr2"),
                image("jpg", "50D/a.jpg", "jpg"),
                image("lonely-raw", "50D/b.cr2", "cr2"),
            ],
        )
        .unwrap();
        relink_raw_images(&mut conn).unwrap();

        let eligible = list_event_eligible_images(&conn).unwrap();
        let keys: Vec<&str> = eligible.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"jpg"));
        assert!(!keys.contains(&"raw"));
        assert!(keys.contains(&"lonely-raw"));
    }

    #[test]
    fn list_images_by_keys_returns_only_the_requested_rows() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg", "jpg"), image("b", "b.jpg", "jpg"), image("c", "c.jpg", "jpg")])
            .unwrap();

        let result = list_images_by_keys(&conn, &["a".to_string(), "c".to_string()]).unwrap();

        let keys: Vec<&str> = result.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"a"));
        assert!(keys.contains(&"c"));
    }

    #[test]
    fn list_images_by_keys_returns_empty_for_empty_input() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(list_images_by_keys(&conn, &[]).unwrap().is_empty());
    }

    #[test]
    fn set_rotation_round_trips_through_list_images() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg", "jpg")]).unwrap();
        assert_eq!(list_images(&conn).unwrap()[0].rotation, None);

        set_rotation(&conn, "a", 90).unwrap();

        assert_eq!(list_images(&conn).unwrap()[0].rotation, Some(90));
    }

    #[test]
    fn set_color_correction_round_trips_through_list_images() {
        use crate::color_correction::ColorCorrectionParams;

        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg", "jpg")]).unwrap();
        assert_eq!(crate::color_correction::from_record(&list_images(&conn).unwrap()[0]), None);

        let params = ColorCorrectionParams { black: [1, 2, 3], white: [250, 251, 252], blend: 0.75 };
        set_color_correction(&conn, "a", &params).unwrap();
        set_color_blend(&conn, "a", params.blend).unwrap();

        assert_eq!(crate::color_correction::from_record(&list_images(&conn).unwrap()[0]), Some(params));
    }

    #[test]
    fn set_color_blend_round_trips_through_list_images() {
        use crate::color_correction::ColorCorrectionParams;

        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg", "jpg")]).unwrap();
        let params = ColorCorrectionParams { black: [1, 2, 3], white: [250, 251, 252], blend: 0.5 };
        set_color_correction(&conn, "a", &params).unwrap();
        set_color_blend(&conn, "a", 0.5).unwrap();
        assert_eq!(list_images(&conn).unwrap()[0].color_blend, Some(0.5));

        set_color_blend(&conn, "a", 0.25).unwrap();

        assert_eq!(list_images(&conn).unwrap()[0].color_blend, Some(0.25));
    }

    #[test]
    fn clear_color_correction_nulls_previously_set_columns() {
        use crate::color_correction::ColorCorrectionParams;

        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg", "jpg")]).unwrap();
        let params = ColorCorrectionParams { black: [1, 2, 3], white: [250, 251, 252], blend: 0.5 };
        set_color_correction(&conn, "a", &params).unwrap();
        set_color_blend(&conn, "a", params.blend).unwrap();
        assert_eq!(crate::color_correction::from_record(&list_images(&conn).unwrap()[0]), Some(params));

        clear_color_correction(&conn, "a").unwrap();

        assert_eq!(crate::color_correction::from_record(&list_images(&conn).unwrap()[0]), None);
    }

    #[test]
    fn clear_color_correction_also_nulls_color_blend() {
        use crate::color_correction::ColorCorrectionParams;

        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg", "jpg")]).unwrap();
        let params = ColorCorrectionParams { black: [1, 2, 3], white: [250, 251, 252], blend: 0.5 };
        set_color_correction(&conn, "a", &params).unwrap();
        set_color_blend(&conn, "a", params.blend).unwrap();

        clear_color_correction(&conn, "a").unwrap();

        assert_eq!(list_images(&conn).unwrap()[0].color_blend, None);
    }

    #[test]
    fn clear_time_correction_resets_corrected_date_taken_to_date_taken() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg", "jpg")]).unwrap();
        let date_taken = NaiveDateTime::parse_from_str("2024-01-01 10:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        conn.execute(
            "UPDATE images SET date_taken = ?2, corrected_date_taken = ?3 WHERE key = ?1",
            rusqlite::params![
                "a",
                date_taken,
                NaiveDateTime::parse_from_str("2024-01-01 11:30:00", "%Y-%m-%d %H:%M:%S").unwrap(),
            ],
        )
        .unwrap();

        clear_time_correction(&conn, "a").unwrap();

        assert_eq!(list_images(&conn).unwrap()[0].corrected_date_taken, Some(date_taken));
    }

    #[test]
    fn clear_all_time_corrections_resets_corrected_date_taken_for_every_image() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg", "jpg"), image("b", "b.jpg", "jpg")]).unwrap();
        let date_a = NaiveDateTime::parse_from_str("2024-01-01 10:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        let date_b = NaiveDateTime::parse_from_str("2024-02-02 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap();
        conn.execute(
            "UPDATE images SET date_taken = ?2, corrected_date_taken = ?3 WHERE key = ?1",
            rusqlite::params!["a", date_a, NaiveDateTime::parse_from_str("2024-01-01 11:30:00", "%Y-%m-%d %H:%M:%S").unwrap()],
        )
        .unwrap();
        conn.execute(
            "UPDATE images SET date_taken = ?2, corrected_date_taken = ?3 WHERE key = ?1",
            rusqlite::params!["b", date_b, NaiveDateTime::parse_from_str("2024-02-02 09:15:00", "%Y-%m-%d %H:%M:%S").unwrap()],
        )
        .unwrap();

        clear_all_time_corrections(&conn).unwrap();

        let images = list_images(&conn).unwrap();
        assert_eq!(images.iter().find(|i| i.key == "a").unwrap().corrected_date_taken, Some(date_a));
        assert_eq!(images.iter().find(|i| i.key == "b").unwrap().corrected_date_taken, Some(date_b));
    }

    fn image_with_dir(key: &str, path: &str, toplevel_dir: Option<&str>) -> ImageRecord {
        ImageRecord {
            key: key.to_string(),
            path: path.to_string(),
            image_type: "jpg".to_string(),
            toplevel_dir: toplevel_dir.map(|s| s.to_string()),
            ..ImageRecord::default()
        }
    }

    fn set_corrected_date_taken(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "UPDATE images SET corrected_date_taken = ?2 WHERE key = ?1",
            rusqlite::params![key, NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").unwrap()],
        )
        .unwrap();
    }

    #[test]
    fn apply_time_corrections_shifts_only_the_targeted_directorys_dated_rows() {
        let mut conn = migrated_conn();
        super::super::directories::upsert_directories(&mut conn, &["50D".to_string(), "40D".to_string()]).unwrap();
        upsert_images(
            &mut conn,
            &[
                image_with_dir("a", "50D/a.jpg", Some("50D")),
                image_with_dir("b", "50D/b.jpg", Some("50D")),
                image_with_dir("c", "40D/c.jpg", Some("40D")),
            ],
        )
        .unwrap();
        set_corrected_date_taken(&conn, "a", "2026-01-01 10:00:00");
        // "b" is left with no corrected_date_taken, as if Generate MetaData hasn't run on it yet.
        set_corrected_date_taken(&conn, "c", "2026-01-01 10:00:00");

        apply_time_corrections(&mut conn, &[(Some("50D".to_string()), Duration::minutes(30))]).unwrap();

        let images = list_images(&conn).unwrap();
        let a = images.iter().find(|i| i.key == "a").unwrap();
        let b = images.iter().find(|i| i.key == "b").unwrap();
        let c = images.iter().find(|i| i.key == "c").unwrap();
        assert_eq!(a.corrected_date_taken.unwrap().to_string(), "2026-01-01 10:30:00");
        assert_eq!(b.corrected_date_taken, None);
        assert_eq!(c.corrected_date_taken.unwrap().to_string(), "2026-01-01 10:00:00");
    }

    #[test]
    fn apply_time_corrections_matches_the_project_root_bucket_via_is_null() {
        let mut conn = migrated_conn();
        super::super::directories::upsert_directories(&mut conn, &["50D".to_string()]).unwrap();
        upsert_images(&mut conn, &[image_with_dir("root", "root.jpg", None), image_with_dir("dir", "50D/a.jpg", Some("50D"))])
            .unwrap();
        set_corrected_date_taken(&conn, "root", "2026-01-01 10:00:00");
        set_corrected_date_taken(&conn, "dir", "2026-01-01 10:00:00");

        apply_time_corrections(&mut conn, &[(None, Duration::hours(1))]).unwrap();

        let images = list_images(&conn).unwrap();
        let root = images.iter().find(|i| i.key == "root").unwrap();
        let dir = images.iter().find(|i| i.key == "dir").unwrap();
        assert_eq!(root.corrected_date_taken.unwrap().to_string(), "2026-01-01 11:00:00");
        assert_eq!(dir.corrected_date_taken.unwrap().to_string(), "2026-01-01 10:00:00");
    }
}
