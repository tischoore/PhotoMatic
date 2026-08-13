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
    /// Decimal degrees (WGS84), signed — north/east positive.
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    /// Meters, signed — below sea level negative.
    pub gps_altitude: Option<f64>,
    /// When EXIF metadata extraction was last attempted for this row (regardless of
    /// whether any tag was found). `None` means Generate MetaData hasn't processed
    /// it yet — this is the marker `list_images_pending_metadata` filters on.
    pub metadata_read_at: Option<NaiveDateTime>,
    /// The `key` of this image's linked RAW/compressed sibling (same directory, same
    /// filename stem), when "Link RAW and compressed images" has paired it — set on both
    /// sides of a pair. `None` when unlinked. See `crate::raw_linking::pair_raw_with_compressed`.
    pub linked_key: Option<String>,
    /// Degrees clockwise (0/90/180/270) the Image Viewer's Rotate button has rotated this
    /// photo, wrapping back to 0 past 270. `None` means no rotation has ever been applied.
    pub rotation: Option<i32>,
    /// Simplest Color Balance low/high clip points (0-255) the Image Viewer's Auto Correct
    /// button computed for this photo, one pair per RGB channel. `None` (on any of the six)
    /// means Auto Correct has never been run — see `color_correction::from_record`.
    pub color_black_r: Option<u8>,
    pub color_black_g: Option<u8>,
    pub color_black_b: Option<u8>,
    pub color_white_r: Option<u8>,
    pub color_white_g: Option<u8>,
    pub color_white_b: Option<u8>,
}

/// A row in the `directories` table.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectoryRecord {
    pub dir_name: String,
    pub author: String,
    pub camera_type: String,
}

/// The clustering granularity an `events` row was generated at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    TightBurst,
    Session,
    MultiHour,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            EventType::TightBurst => "Tight Burst",
            EventType::Session => "Session",
            EventType::MultiHour => "Multi-hour",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseEventTypeError(String);

impl std::fmt::Display for ParseEventTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not a valid EventType: {:?}", self.0)
    }
}

impl std::str::FromStr for EventType {
    type Err = ParseEventTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Tight Burst" => Ok(EventType::TightBurst),
            "Session" => Ok(EventType::Session),
            "Multi-hour" => Ok(EventType::MultiHour),
            other => Err(ParseEventTypeError(other.to_string())),
        }
    }
}

/// A row in the `events` table, as loaded for the events-browsing UI (an event tab's
/// title input, notes box, and photo table).
#[derive(Debug, Clone, PartialEq)]
pub struct EventRecord {
    pub id: i64,
    pub event_type: EventType,
    pub title: String,
    pub notes: String,
}

/// One `events` row plus its photo count, as listed for the Left Navigation tree's
/// Events node — grouping into per-type tree nodes happens in `crate::event_tree`,
/// not here.
#[derive(Debug, Clone, PartialEq)]
pub struct EventSummary {
    pub id: i64,
    pub event_type: EventType,
    pub title: String,
    pub image_count: i64,
}

/// A row in the `collections` table, as listed for the Left Navigation tree's
/// Collections node and the Image Viewer's per-collection toggle buttons.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionRecord {
    pub id: i64,
    pub name: String,
    pub description: String,
    /// The single-character key assigned to this collection (e.g. `"d"` for the
    /// default collection), shown as an Image Viewer button's label.
    pub shortcut: String,
    /// The `key` of the photo the Image Viewer should resume on when this collection is
    /// next opened. `None` means either nothing has been viewed yet, or the last-viewed
    /// photo was the first one in the (date-sorted) list — both cases start at index 0, so
    /// there's no need to distinguish them. See `image_viewer::start_index_for_current_img`.
    pub current_img: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_round_trips_through_its_string_form() {
        for event_type in [EventType::TightBurst, EventType::Session, EventType::MultiHour] {
            assert_eq!(event_type.to_string().parse::<EventType>().unwrap(), event_type);
        }
    }

    #[test]
    fn event_type_rejects_an_unknown_string() {
        assert!("Nonsense".parse::<EventType>().is_err());
    }
}
