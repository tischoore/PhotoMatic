mod directories;
mod events;
mod images;
mod migrations;
pub mod models;
mod project_settings;

pub use events::EventThresholds;

use std::fmt;
use std::path::Path;

use chrono::{NaiveDate, Utc};
use rusqlite::Connection;

use crate::exif;
use crate::scan;

pub struct ProjectDb {
    conn: Connection,
}

#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
    Migration(rusqlite_migration::Error),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DbError::Sqlite(err) => write!(f, "{err}"),
            DbError::Migration(err) => write!(f, "{err}"),
        }
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(err: rusqlite::Error) -> Self {
        DbError::Sqlite(err)
    }
}

impl From<rusqlite_migration::Error> for DbError {
    fn from(err: rusqlite_migration::Error) -> Self {
        DbError::Migration(err)
    }
}

impl ProjectDb {
    /// Opens (creating if needed) the SQLite database at `path` and migrates it to the
    /// latest schema version.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let mut conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        migrations::apply(&mut conn)?;
        Ok(ProjectDb { conn })
    }

    pub fn schema_version(&self) -> Result<i64, DbError> {
        self.conn.query_row("PRAGMA user_version", [], |row| row.get(0)).map_err(DbError::Sqlite)
    }

    /// Folds one discovered `ScanUnit` into the database as the scan walks the
    /// filesystem, so `images`/`directories` rows land while the scan is still
    /// running rather than only once the whole tree has been walked. For a
    /// top-level directory, its `directories` row is upserted before its images,
    /// so the images' `toplevel_dir` foreign key always has a target to point to.
    pub fn apply_scan_unit(&mut self, unit: &scan::ScanUnit) -> Result<(), DbError> {
        match unit {
            scan::ScanUnit::RootFiles(files) => {
                let images = to_image_records(files, None);
                images::upsert_images(&mut self.conn, &images)?;
            }
            scan::ScanUnit::ToplevelDir { name, files } => {
                directories::upsert_directories(&mut self.conn, std::slice::from_ref(name))?;
                let images = to_image_records(files, Some(name.clone()));
                images::upsert_images(&mut self.conn, &images)?;
            }
        }
        Ok(())
    }

    /// Called once after every `ScanUnit` from a scan has been applied: stamps
    /// `project_settings.last_scan`.
    pub fn finish_scan(&self) -> Result<(), DbError> {
        project_settings::update_last_scan(&self.conn, Utc::now())
    }

    // The methods below have no GUI caller yet — this phase is backend/schema
    // plumbing only (see add_db.md). They're built now so a later metadata-editing UI
    // phase only adds controls + wiring, not schema/DB code.
    pub fn project_settings(&self) -> Result<models::ProjectSettings, DbError> {
        project_settings::get(&self.conn)
    }

    #[allow(dead_code)]
    pub fn update_project_settings(
        &self,
        name: &str,
        date_begin: Option<NaiveDate>,
        date_end: Option<NaiveDate>,
        author: &str,
    ) -> Result<(), DbError> {
        project_settings::update(&self.conn, name, date_begin, date_end, author)
    }

    #[allow(dead_code)]
    pub fn list_directories(&self) -> Result<Vec<models::DirectoryRecord>, DbError> {
        directories::list_directories(&self.conn)
    }

    #[allow(dead_code)]
    pub fn update_directory_metadata(&self, dir_name: &str, author: &str, camera_type: &str) -> Result<(), DbError> {
        directories::update_directory_metadata(&self.conn, dir_name, author, camera_type)
    }

    #[allow(dead_code)]
    pub fn list_images(&self) -> Result<Vec<models::ImageRecord>, DbError> {
        images::list_images(&self.conn)
    }

    /// Images that have never had EXIF metadata extraction attempted — the work
    /// list Generate MetaData iterates.
    pub fn list_images_pending_metadata(&self) -> Result<Vec<models::ImageRecord>, DbError> {
        images::list_images_pending_metadata(&self.conn)
    }

    /// Writes extracted EXIF metadata for the image `key`, stamping `metadata_read_at`
    /// so it drops out of `list_images_pending_metadata` regardless of outcome.
    pub fn update_image_metadata(&self, key: &str, metadata: &exif::ImageMetadata) -> Result<(), DbError> {
        images::update_metadata(&self.conn, key, metadata)
    }

    /// Per-directory, per-extension image counts backing the Left Navigation tree.
    pub fn directory_type_counts(&self) -> Result<Vec<(Option<String>, String, i64)>, DbError> {
        images::count_by_directory_and_type(&self.conn)
    }

    /// Images under a top-level directory, optionally filtered to one File Type, for the
    /// Context Window's "Image List" tabs.
    pub fn list_images_by_directory(
        &self,
        toplevel_dir: &str,
        image_type: Option<&str>,
    ) -> Result<Vec<models::ImageRecord>, DbError> {
        images::list_images_by_directory(&self.conn, toplevel_dir, image_type)
    }

    /// Clears and rebuilds the `events`/`event_images` tables from `images`, clustered at
    /// all three tiers (Tight Burst, Session, Multi-hour) per `thresholds` — backs the
    /// "Generate Events" button.
    pub fn regenerate_events(&mut self, images: &[models::ImageRecord], thresholds: &EventThresholds) -> Result<(), DbError> {
        events::regenerate(&mut self.conn, images, thresholds)
    }
}

/// Normalizes a relative path to forward-slash (POSIX) separators, regardless of the
/// platform's native separator, so stored paths are portable across machines.
fn normalize_path(path: &Path) -> String {
    path.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Maps `ScannedFile`s to `ImageRecord`s: normalizes each path to POSIX separators,
/// hashes it into a stable image key, and stamps `toplevel_dir`.
fn to_image_records(files: &[scan::ScannedFile], toplevel_dir: Option<String>) -> Vec<models::ImageRecord> {
    files
        .iter()
        .map(|file| {
            let path = normalize_path(&file.relative_path);
            models::ImageRecord {
                key: images::image_key(&path),
                path,
                image_type: file.extension.clone(),
                toplevel_dir: toplevel_dir.clone(),
                ..Default::default()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{ScanUnit, ScannedFile};
    use std::path::PathBuf;

    fn temp_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("photomatic-test-db-{}-{}.sqlite3", std::process::id(), name))
    }

    /// Mirrors what `scan::scan_directory` would report for a root containing
    /// `a.jpg` directly, plus a `sub` directory containing `b.cr2`.
    fn sample_units() -> Vec<ScanUnit> {
        vec![
            ScanUnit::RootFiles(vec![ScannedFile {
                relative_path: PathBuf::from("a.jpg"),
                extension: "jpg".to_string(),
                toplevel_dir: None,
            }]),
            ScanUnit::ToplevelDir {
                name: "sub".to_string(),
                files: vec![ScannedFile {
                    relative_path: PathBuf::from("sub").join("b.cr2"),
                    extension: "cr2".to_string(),
                    toplevel_dir: Some("sub".to_string()),
                }],
            },
        ]
    }

    fn apply_units(db: &mut ProjectDb, units: &[ScanUnit]) {
        for unit in units {
            db.apply_scan_unit(unit).unwrap();
        }
    }

    #[test]
    fn open_on_nonexistent_path_creates_it_and_migrates_to_latest() {
        let path = temp_db_path("open-fresh");
        std::fs::remove_file(&path).ok();

        let db = ProjectDb::open(&path).unwrap();

        assert!(path.exists());
        assert_eq!(db.schema_version().unwrap(), 4);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn reopening_an_already_migrated_db_is_a_noop() {
        let path = temp_db_path("reopen");
        std::fs::remove_file(&path).ok();

        let db1 = ProjectDb::open(&path).unwrap();
        let version1 = db1.schema_version().unwrap();
        drop(db1);

        let db2 = ProjectDb::open(&path).unwrap();
        assert_eq!(db2.schema_version().unwrap(), version1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn apply_scan_unit_inserts_images_and_directories() {
        let path = temp_db_path("apply-scan-insert");
        std::fs::remove_file(&path).ok();
        let mut db = ProjectDb::open(&path).unwrap();

        apply_units(&mut db, &sample_units());

        let images = db.list_images().unwrap();
        assert_eq!(images.len(), 2);
        assert!(images.iter().any(|i| i.path == "a.jpg" && i.image_type == "jpg" && i.toplevel_dir.is_none()));
        assert!(images.iter().any(|i| {
            i.path == "sub/b.cr2" && i.image_type == "cr2" && i.toplevel_dir == Some("sub".to_string())
        }));

        let dirs = db.list_directories().unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].dir_name, "sub");
        assert_eq!(dirs[0].author, "");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn apply_scan_unit_twice_with_identical_units_does_not_duplicate_rows() {
        let path = temp_db_path("apply-scan-twice");
        std::fs::remove_file(&path).ok();
        let mut db = ProjectDb::open(&path).unwrap();

        apply_units(&mut db, &sample_units());
        apply_units(&mut db, &sample_units());

        assert_eq!(db.list_images().unwrap().len(), 2);
        assert_eq!(db.list_directories().unwrap().len(), 1);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rescan_does_not_clobber_user_entered_directory_metadata() {
        let path = temp_db_path("rescan-preserves-metadata");
        std::fs::remove_file(&path).ok();
        let mut db = ProjectDb::open(&path).unwrap();

        apply_units(&mut db, &sample_units());
        db.update_directory_metadata("sub", "Jane", "Canon").unwrap();

        let mut second_units = sample_units();
        second_units.push(ScanUnit::ToplevelDir { name: "new_dir".to_string(), files: vec![] });
        apply_units(&mut db, &second_units);

        let dirs = db.list_directories().unwrap();
        let sub = dirs.iter().find(|d| d.dir_name == "sub").unwrap();
        assert_eq!(sub.author, "Jane");
        assert_eq!(sub.camera_type, "Canon");

        let new_dir = dirs.iter().find(|d| d.dir_name == "new_dir").unwrap();
        assert_eq!(new_dir.author, "");
        assert_eq!(new_dir.camera_type, "");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn finish_scan_updates_last_scan_timestamp_but_apply_scan_unit_alone_does_not() {
        let path = temp_db_path("finish-scan-last-scan");
        std::fs::remove_file(&path).ok();
        let mut db = ProjectDb::open(&path).unwrap();

        assert!(db.project_settings().unwrap().last_scan.is_none());
        apply_units(&mut db, &sample_units());
        assert!(db.project_settings().unwrap().last_scan.is_none());

        db.finish_scan().unwrap();
        assert!(db.project_settings().unwrap().last_scan.is_some());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn directory_type_counts_groups_by_directory_and_extension() {
        let path = temp_db_path("directory-type-counts");
        std::fs::remove_file(&path).ok();
        let mut db = ProjectDb::open(&path).unwrap();

        apply_units(&mut db, &sample_units());

        let counts = db.directory_type_counts().unwrap();
        assert_eq!(counts.len(), 2);
        assert!(counts.contains(&(None, "jpg".to_string(), 1)));
        assert!(counts.contains(&(Some("sub".to_string()), "cr2".to_string(), 1)));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn list_images_by_directory_filters_by_directory_and_optional_type() {
        let path = temp_db_path("list-images-by-directory");
        std::fs::remove_file(&path).ok();
        let mut db = ProjectDb::open(&path).unwrap();

        let units = vec![
            ScanUnit::ToplevelDir {
                name: "50D".to_string(),
                files: vec![
                    ScannedFile {
                        relative_path: PathBuf::from("50D").join("a.jpg"),
                        extension: "jpg".to_string(),
                        toplevel_dir: Some("50D".to_string()),
                    },
                    ScannedFile {
                        relative_path: PathBuf::from("50D").join("b.jpg"),
                        extension: "jpg".to_string(),
                        toplevel_dir: Some("50D".to_string()),
                    },
                    ScannedFile {
                        relative_path: PathBuf::from("50D").join("c.cr2"),
                        extension: "cr2".to_string(),
                        toplevel_dir: Some("50D".to_string()),
                    },
                ],
            },
            ScanUnit::ToplevelDir {
                name: "other".to_string(),
                files: vec![ScannedFile {
                    relative_path: PathBuf::from("other").join("d.jpg"),
                    extension: "jpg".to_string(),
                    toplevel_dir: Some("other".to_string()),
                }],
            },
        ];
        apply_units(&mut db, &units);

        assert_eq!(db.list_images_by_directory("50D", None).unwrap().len(), 3);
        assert_eq!(
            db.list_images_by_directory("50D", Some("jpg"))
                .unwrap()
                .iter()
                .map(|i| i.path.as_str())
                .collect::<Vec<_>>(),
            vec!["50D/a.jpg", "50D/b.jpg"],
        );
        assert!(db.list_images_by_directory("nonexistent", None).unwrap().is_empty());

        std::fs::remove_file(&path).ok();
    }

    /// Mirrors `read_metadata`'s output for a fully-tagged JPEG.
    fn sample_metadata() -> exif::ImageMetadata {
        exif::ImageMetadata {
            date_taken: chrono::NaiveDate::from_ymd_opt(2024, 6, 15)
                .unwrap()
                .and_hms_opt(12, 30, 45),
            width: Some(800),
            height: Some(600),
            exposure_time_seconds: Some(1.0 / 500.0),
            iso: Some(200),
            focal_length_mm: Some(50.0),
        }
    }

    #[test]
    fn list_images_pending_metadata_excludes_images_already_processed() {
        let path = temp_db_path("pending-metadata-selection");
        std::fs::remove_file(&path).ok();
        let mut db = ProjectDb::open(&path).unwrap();

        apply_units(&mut db, &sample_units());
        let key = images::image_key("a.jpg");
        db.update_image_metadata(&key, &sample_metadata()).unwrap();

        let pending = db.list_images_pending_metadata().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path, "sub/b.cr2");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn update_image_metadata_round_trips_through_list_images() {
        let path = temp_db_path("update-metadata-roundtrip");
        std::fs::remove_file(&path).ok();
        let mut db = ProjectDb::open(&path).unwrap();

        apply_units(&mut db, &sample_units());
        let key = images::image_key("a.jpg");
        let metadata = sample_metadata();
        db.update_image_metadata(&key, &metadata).unwrap();

        let images = db.list_images().unwrap();
        let image = images.iter().find(|i| i.path == "a.jpg").unwrap();
        assert_eq!(image.date_taken, metadata.date_taken);
        assert_eq!(image.width, metadata.width);
        assert_eq!(image.height, metadata.height);
        assert_eq!(image.exposure_time, metadata.exposure_time_seconds);
        assert_eq!(image.iso, metadata.iso);
        assert_eq!(image.focal_length, metadata.focal_length_mm);
        assert!(image.metadata_read_at.is_some());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rescan_does_not_clobber_or_requeue_already_generated_metadata() {
        let path = temp_db_path("rescan-preserves-metadata-generation");
        std::fs::remove_file(&path).ok();
        let mut db = ProjectDb::open(&path).unwrap();

        apply_units(&mut db, &sample_units());
        let key = images::image_key("a.jpg");
        db.update_image_metadata(&key, &sample_metadata()).unwrap();

        apply_units(&mut db, &sample_units());

        let images = db.list_images().unwrap();
        let image = images.iter().find(|i| i.path == "a.jpg").unwrap();
        assert_eq!(image.width, Some(800));
        assert!(image.metadata_read_at.is_some());

        let pending = db.list_images_pending_metadata().unwrap();
        assert!(!pending.iter().any(|i| i.path == "a.jpg"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_settings_round_trips_through_update() {
        let path = temp_db_path("project-settings-roundtrip");
        std::fs::remove_file(&path).ok();
        let db = ProjectDb::open(&path).unwrap();

        let begin = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let end = chrono::NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        db.update_project_settings("My Project", Some(begin), Some(end), "Alice").unwrap();

        let settings = db.project_settings().unwrap();
        assert_eq!(settings.project_name, "My Project");
        assert_eq!(settings.date_begin, Some(begin));
        assert_eq!(settings.date_end, Some(end));
        assert_eq!(settings.author, "Alice");

        std::fs::remove_file(&path).ok();
    }
}
