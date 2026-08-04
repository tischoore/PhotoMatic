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

pub fn list_images(conn: &Connection) -> Result<Vec<ImageRecord>, DbError> {
    let mut stmt = conn
        .prepare("SELECT key, path, image_type, toplevel_dir FROM images ORDER BY path")
        .map_err(DbError::Sqlite)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ImageRecord {
                key: row.get(0)?,
                path: row.get(1)?,
                image_type: row.get(2)?,
                toplevel_dir: row.get(3)?,
            })
        })
        .map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
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
