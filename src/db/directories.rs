use rusqlite::Connection;

use super::models::DirectoryRecord;
use super::DbError;

/// Inserts new `directories` rows. Existing rows are left untouched — this is the
/// whole mechanism behind "a rescan must not clobber user-entered Author/CameraType".
pub fn upsert_directories(conn: &mut Connection, dir_names: &[String]) -> Result<(), DbError> {
    let tx = conn.transaction().map_err(DbError::Sqlite)?;
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO directories (dir_name, author, camera_type) VALUES (?1, '', '')
                 ON CONFLICT(dir_name) DO NOTHING",
            )
            .map_err(DbError::Sqlite)?;
        for dir_name in dir_names {
            stmt.execute(rusqlite::params![dir_name]).map_err(DbError::Sqlite)?;
        }
    }
    tx.commit().map_err(DbError::Sqlite)?;
    Ok(())
}

pub fn list_directories(conn: &Connection) -> Result<Vec<DirectoryRecord>, DbError> {
    let mut stmt = conn
        .prepare("SELECT dir_name, author, camera_type FROM directories ORDER BY dir_name")
        .map_err(DbError::Sqlite)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(DirectoryRecord { dir_name: row.get(0)?, author: row.get(1)?, camera_type: row.get(2)? })
        })
        .map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

pub fn update_directory_metadata(
    conn: &Connection,
    dir_name: &str,
    author: &str,
    camera_type: &str,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE directories SET author = ?2, camera_type = ?3 WHERE dir_name = ?1",
        rusqlite::params![dir_name, author, camera_type],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}
