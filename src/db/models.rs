use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageRecord {
    pub key: String,
    pub path: String,
    pub image_type: String,
    /// The top-level directory (`directories.dir_name`) this image lives under.
    /// `None` if the image sits directly in the project root.
    pub toplevel_dir: Option<String>,
    pub date_taken: Option<NaiveDateTime>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub exposure_time: Option<f64>,
    pub iso: Option<u32>,
    pub focal_length: Option<f64>,
    /// When EXIF metadata extraction was last attempted for this row (regardless of
    /// whether any tag was found). `None` means Generate MetaData hasn't processed
    /// it yet — this is the marker `list_images_pending_metadata` filters on.
    pub metadata_read_at: Option<NaiveDateTime>,
}

/// A row in the `directories` table.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectoryRecord {
    pub dir_name: String,
    pub author: String,
    pub camera_type: String,
}
