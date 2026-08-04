use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::Connection;

use super::models::ProjectSettings;
use super::DbError;

pub fn get(conn: &Connection) -> Result<ProjectSettings, DbError> {
    conn.query_row(
        "SELECT project_name, date_begin, date_end, author, last_scan FROM project_settings WHERE id = 1",
        [],
        |row| {
            Ok(ProjectSettings {
                project_name: row.get(0)?,
                date_begin: row.get(1)?,
                date_end: row.get(2)?,
                author: row.get(3)?,
                last_scan: row.get(4)?,
            })
        },
    )
    .map_err(DbError::Sqlite)
}

pub fn update(
    conn: &Connection,
    name: &str,
    date_begin: Option<NaiveDate>,
    date_end: Option<NaiveDate>,
    author: &str,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE project_settings SET project_name = ?1, date_begin = ?2, date_end = ?3, author = ?4 WHERE id = 1",
        rusqlite::params![name, date_begin, date_end, author],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

pub fn update_last_scan(conn: &Connection, last_scan: DateTime<Utc>) -> Result<(), DbError> {
    conn.execute(
        "UPDATE project_settings SET last_scan = ?1 WHERE id = 1",
        rusqlite::params![last_scan],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}
