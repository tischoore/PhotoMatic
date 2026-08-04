use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The PhotoMatic project file format (`.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {
    /// The folder photos are imported/read from. `None` until the user sets one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_directory: Option<PathBuf>,

    /// Which file extensions are included when reading `source_directory`.
    #[serde(default, skip_serializing_if = "FileExtensions::is_default")]
    pub file_extensions: FileExtensions,

    /// Path to this project's SQLite database, relative to the project .json file's own
    /// directory (keeps the project folder portable if moved/copied together). `None`
    /// until the project has been saved for the first time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_path: Option<PathBuf>,

    /// Cached copy of the database's PRAGMA user_version as of the last time this app
    /// opened/migrated it. Informational only — the database is authoritative; refreshed
    /// every time the DB is opened or migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_schema_version: Option<i64>,

    /// When the database was last written to (e.g. after a scan committed rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_last_modified: Option<chrono::DateTime<chrono::Utc>>,
}

/// Which file extensions to include from the Source Directory. Defaults to all included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileExtensions {
    #[serde(default = "FileExtensions::default_true")]
    pub jpg: bool,
    #[serde(default = "FileExtensions::default_true")]
    pub cr2: bool,
    #[serde(default = "FileExtensions::default_true")]
    pub gif: bool,
}

impl FileExtensions {
    fn default_true() -> bool {
        true
    }

    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for FileExtensions {
    fn default() -> Self {
        FileExtensions { jpg: true, cr2: true, gif: true }
    }
}

#[derive(Debug)]
pub enum ProjectError {
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::Io(err) => write!(f, "{err}"),
            ProjectError::Json(err) => write!(f, "{err}"),
        }
    }
}

impl From<std::io::Error> for ProjectError {
    fn from(err: std::io::Error) -> Self {
        ProjectError::Io(err)
    }
}

impl From<serde_json::Error> for ProjectError {
    fn from(err: serde_json::Error) -> Self {
        ProjectError::Json(err)
    }
}

pub fn load(path: &Path) -> Result<ProjectFile, ProjectError> {
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

pub fn save(path: &Path, project: &ProjectFile) -> Result<(), ProjectError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(project)?;
    std::fs::write(path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("photomatic-test-{}-{}", std::process::id(), name))
    }

    #[test]
    fn round_trips_through_json() {
        let path = temp_path("project-roundtrip.json");
        let project = ProjectFile::default();

        save(&path, &project).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded, project);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn new_project_saves_as_empty_json_object() {
        let path = temp_path("project-empty.json");

        save(&path, &ProjectFile::default()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();

        assert_eq!(text.trim(), "{}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn round_trips_source_directory_through_json() {
        let path = temp_path("project-source-directory.json");
        let project = ProjectFile {
            source_directory: Some(std::path::PathBuf::from(r"C:\photos\import")),
            ..ProjectFile::default()
        };

        save(&path, &project).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded, project);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn file_extensions_default_to_all_included() {
        assert_eq!(FileExtensions::default(), FileExtensions { jpg: true, cr2: true, gif: true });
    }

    #[test]
    fn round_trips_file_extensions_through_json() {
        let path = temp_path("project-file-extensions.json");
        let project = ProjectFile {
            source_directory: None,
            file_extensions: FileExtensions { jpg: true, cr2: false, gif: false },
            ..ProjectFile::default()
        };

        save(&path, &project).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded, project);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn round_trips_database_fields_through_json_and_omits_them_when_none() {
        let path = temp_path("project-database-fields.json");
        let project = ProjectFile {
            database_path: Some(PathBuf::from("myproject.sqlite3")),
            database_schema_version: Some(1),
            database_last_modified: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z").unwrap().with_timezone(&chrono::Utc),
            ),
            ..ProjectFile::default()
        };

        save(&path, &project).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, project);

        save(&path, &ProjectFile::default()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.trim(), "{}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_reports_an_error_for_malformed_json() {
        let path = temp_path("project-malformed.json");
        std::fs::write(&path, "not valid json").unwrap();

        let result = load(&path);

        assert!(result.is_err());
        std::fs::remove_file(&path).ok();
    }
}
