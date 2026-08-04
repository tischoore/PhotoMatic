use chrono::{DateTime, NaiveDate, Utc};

/// The singleton `project_settings` row.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSettings {
    pub project_name: String,
    pub date_begin: Option<NaiveDate>,
    pub date_end: Option<NaiveDate>,
    pub author: String,
    pub last_scan: Option<DateTime<Utc>>,
}

/// A row in the `images` table.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageRecord {
    pub key: String,
    pub path: String,
    pub image_type: String,
    /// The top-level directory (`directories.dir_name`) this image lives under.
    /// `None` if the image sits directly in the project root.
    pub toplevel_dir: Option<String>,
}

/// A row in the `directories` table.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectoryRecord {
    pub dir_name: String,
    pub author: String,
    pub camera_type: String,
}
