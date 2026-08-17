use crate::db::models::ImageRecord;

/// Column headers, in display order, paired with a pixel width for `ListView::insert_column`.
pub const COLUMNS: [(&str, i32); 10] = [
    ("Path", 320),
    ("Date taken", 150),
    ("Corrected date taken", 150),
    ("Width", 70),
    ("Height", 70),
    ("Focal length", 100),
    ("ISO", 70),
    ("Exposure time", 110),
    ("Location", 150),
    ("Altitude", 80),
];

/// The Context Window tab's display name/lookup key for an "Image List" action:
/// the directory name alone, or `"dir/type"` when a File Type leaf was clicked.
pub fn tab_title(dir: &str, image_type: Option<&str>) -> String {
    match image_type {
        Some(ext) => format!("{dir}/{ext}"),
        None => dir.to_string(),
    }
}

/// The Context Window tab's display name/lookup key for an event tab. A stable,
/// id-based identifier rather than the event's (editable) title: `nwg::Tab::set_text`
/// underflows for the first tab in the strip (see `App::build_context_tab_entry`'s doc
/// comment), so an event tab's header can never be renamed after creation — the live
/// title is shown in the tab's title input and the tree instead.
pub fn event_tab_title(event_id: i64) -> String {
    format!("Event #{event_id}")
}

/// One `ImageRecord` as the 10 display strings, in `COLUMNS` order. `None` fields render
/// as an empty string (blank cell) rather than a placeholder like "N/A".
pub fn image_row(record: &ImageRecord) -> [String; 10] {
    [
        record.path.clone(),
        record.date_taken.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
        record.corrected_date_taken.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
        record.width.map(|w| w.to_string()).unwrap_or_default(),
        record.height.map(|h| h.to_string()).unwrap_or_default(),
        record.focal_length.map(|f| format!("{f:.1}mm")).unwrap_or_default(),
        record.iso.map(|i| i.to_string()).unwrap_or_default(),
        format_exposure_time(record.exposure_time),
        format_gps_coordinates(record.gps_latitude, record.gps_longitude),
        format_gps_altitude(record.gps_altitude),
    ]
}

/// Sub-second exposures render as a fraction (e.g. `1/125s`, matching how shutter speeds
/// are conventionally shown); one-second-or-longer exposures render as decimal seconds
/// (e.g. `2.0s`). `None` or a non-positive value (defensive, shouldn't occur) is blank.
/// `pub(crate)` so `image_viewer`'s metadata dialog can format the same field identically.
pub(crate) fn format_exposure_time(seconds: Option<f64>) -> String {
    match seconds {
        Some(s) if s > 0.0 && s < 1.0 => format!("1/{}s", (1.0 / s).round() as i64),
        Some(s) if s > 0.0 => format!("{s:.1}s"),
        _ => String::new(),
    }
}

/// `"{lat}, {lon}"` to 5 decimal places (~1.1m precision) when both are present; blank
/// otherwise — a coordinate needs both halves to mean anything.
pub(crate) fn format_gps_coordinates(lat: Option<f64>, lon: Option<f64>) -> String {
    match (lat, lon) {
        (Some(lat), Some(lon)) => format!("{lat:.5}, {lon:.5}"),
        _ => String::new(),
    }
}

pub(crate) fn format_gps_altitude(meters: Option<f64>) -> String {
    meters.map(|m| format!("{m:.0}m")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn tab_title_for_directory_only() {
        assert_eq!(tab_title("50D", None), "50D");
    }

    #[test]
    fn tab_title_for_directory_and_type() {
        assert_eq!(tab_title("50D", Some("jpg")), "50D/jpg");
    }

    #[test]
    fn event_tab_title_is_stable_and_id_based() {
        assert_eq!(event_tab_title(7), "Event #7");
    }

    #[test]
    fn image_row_formats_all_present_fields() {
        let record = ImageRecord {
            key: "k".to_string(),
            path: "50D/a.jpg".to_string(),
            image_type: "jpg".to_string(),
            toplevel_dir: Some("50D".to_string()),
            date_taken: NaiveDate::from_ymd_opt(2024, 3, 5).unwrap().and_hms_opt(14, 30, 0),
            corrected_date_taken: NaiveDate::from_ymd_opt(2024, 3, 6).unwrap().and_hms_opt(9, 15, 0),
            width: Some(6000),
            height: Some(4000),
            exposure_time: Some(0.008),
            iso: Some(400),
            focal_length: Some(50.0),
            gps_latitude: Some(40.446_33),
            gps_longitude: Some(-79.982),
            gps_altitude: Some(123.4),
            metadata_read_at: None,
            linked_key: None,
            rotation: None,
            color_black_r: None,
            color_black_g: None,
            color_black_b: None,
            color_white_r: None,
            color_white_g: None,
            color_white_b: None,
        };
        assert_eq!(
            image_row(&record),
            [
                "50D/a.jpg".to_string(),
                "2024-03-05 14:30:00".to_string(),
                "2024-03-06 09:15:00".to_string(),
                "6000".to_string(),
                "4000".to_string(),
                "50.0mm".to_string(),
                "400".to_string(),
                "1/125s".to_string(),
                "40.44633, -79.98200".to_string(),
                "123m".to_string(),
            ]
        );
    }

    #[test]
    fn image_row_blanks_all_absent_fields() {
        let record = ImageRecord {
            key: "k".to_string(),
            path: "50D/a.jpg".to_string(),
            image_type: "jpg".to_string(),
            toplevel_dir: Some("50D".to_string()),
            date_taken: None,
            corrected_date_taken: None,
            width: None,
            height: None,
            exposure_time: None,
            iso: None,
            focal_length: None,
            gps_latitude: None,
            gps_longitude: None,
            gps_altitude: None,
            metadata_read_at: None,
            linked_key: None,
            rotation: None,
            color_black_r: None,
            color_black_g: None,
            color_black_b: None,
            color_white_r: None,
            color_white_g: None,
            color_white_b: None,
        };
        assert_eq!(
            image_row(&record),
            [
                "50D/a.jpg".to_string(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ]
        );
    }

    #[test]
    fn image_row_formats_sub_second_exposure_as_a_fraction() {
        assert_eq!(format_exposure_time(Some(0.01)), "1/100s");
    }

    #[test]
    fn image_row_formats_one_second_or_longer_exposure_as_decimal_seconds() {
        assert_eq!(format_exposure_time(Some(2.0)), "2.0s");
    }

    #[test]
    fn format_gps_coordinates_formats_both_present_to_five_decimals() {
        assert_eq!(format_gps_coordinates(Some(40.446_33), Some(-79.982)), "40.44633, -79.98200");
    }

    #[test]
    fn format_gps_coordinates_blanks_when_either_half_is_missing() {
        assert_eq!(format_gps_coordinates(Some(40.446_33), None), "");
        assert_eq!(format_gps_coordinates(None, Some(-79.982)), "");
        assert_eq!(format_gps_coordinates(None, None), "");
    }

    #[test]
    fn format_gps_altitude_formats_meters_rounded_to_whole_numbers() {
        assert_eq!(format_gps_altitude(Some(123.4)), "123m");
        assert_eq!(format_gps_altitude(Some(-5.6)), "-6m");
    }

    #[test]
    fn format_gps_altitude_blanks_when_absent() {
        assert_eq!(format_gps_altitude(None), "");
    }
}
