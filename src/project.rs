use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The PhotoMatic project file format (`.json`). Empty for now — paths and other
/// project data will be added here as features are built.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectFile {}

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
    fn load_reports_an_error_for_malformed_json() {
        let path = temp_path("project-malformed.json");
        std::fs::write(&path, "not valid json").unwrap();

        let result = load(&path);

        assert!(result.is_err());
        std::fs::remove_file(&path).ok();
    }
}
