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
}

/// A row in the `directories` table.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectoryRecord {
    pub dir_name: String,
    pub author: String,
    pub camera_type: String,
}

/// The clustering granularity an `events` row was generated at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// A row in the `events` table. No query returns this yet — reserved for the events-browsing
/// UI that will follow the "Generate Events" button, the same way `DirectoryRecord`'s
/// `author`/`camera_type` were added ahead of their editing UI.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct EventRecord {
    pub id: i64,
    pub event_type: EventType,
    pub notes: String,
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
