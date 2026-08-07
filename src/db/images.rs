use rusqlite::Connection;
use xxhash_rust::xxh3::xxh3_64;

use super::models::ImageRecord;
use super::DbError;

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

const IMAGE_COLUMNS: &str = "key, path, image_type, toplevel_dir, date_taken, width, height, \
     exposure_time, iso, focal_length, gps_latitude, gps_longitude, gps_altitude, metadata_read_at";

fn map_image_row(row: &rusqlite::Row) -> rusqlite::Result<ImageRecord> {
    Ok(ImageRecord {
        key: row.get(0)?,
        path: row.get(1)?,
        image_type: row.get(2)?,
        toplevel_dir: row.get(3)?,
        date_taken: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        exposure_time: row.get(7)?,
        iso: row.get(8)?,
        focal_length: row.get(9)?,
        gps_latitude: row.get(10)?,
        gps_longitude: row.get(11)?,
        gps_altitude: row.get(12)?,
        metadata_read_at: row.get(13)?,
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
        "UPDATE images SET date_taken = ?2, width = ?3, height = ?4, exposure_time = ?5, iso = ?6, \
         focal_length = ?7, gps_latitude = ?8, gps_longitude = ?9, gps_altitude = ?10, \
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
/// extension, sorted by path — backs the Left Navigation tree's "Image List" context
/// menu action.
pub fn list_images_by_directory(
    conn: &Connection,
    toplevel_dir: &str,
    image_type: Option<&str>,
) -> Result<Vec<ImageRecord>, DbError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {IMAGE_COLUMNS} FROM images WHERE toplevel_dir = ?1 AND (?2 IS NULL OR image_type = ?2) ORDER BY path"
        ))
        .map_err(DbError::Sqlite)?;
    let rows = stmt
        .query_map(rusqlite::params![toplevel_dir, image_type], map_image_row)
        .map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
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
}
