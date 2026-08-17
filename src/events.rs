use chrono::Duration;

use crate::db::models::ImageRecord;

/// Great-circle distance in kilometers between two WGS84 decimal-degree coordinates.
pub fn haversine_distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_KM: f64 = 6371.0;

    let (lat1_rad, lat2_rad) = (lat1.to_radians(), lat2.to_radians());
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();

    let a = (dlat / 2.0).sin().powi(2) + lat1_rad.cos() * lat2_rad.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    EARTH_RADIUS_KM * c
}

/// Groups `images` by `corrected_date_taken`: images with no `corrected_date_taken` are
/// excluded entirely, the rest are sorted ascending, then split into groups wherever the gap
/// since the previous photo exceeds `gap`, or — when `max_distance_km` is `Some` and both
/// photos have GPS coordinates — the great-circle distance since the previous photo exceeds
/// it. A photo missing GPS data simply skips the distance check against its neighbor, falling
/// back to time-gap-only behavior for that pair. Each returned group is sorted ascending and
/// non-empty; groups of size 1 are still returned here — deciding whether a singleton group
/// counts as a real "event" is left to the caller.
pub fn cluster_by_time_gap(images: &[ImageRecord], gap: Duration, max_distance_km: Option<f64>) -> Vec<Vec<ImageRecord>> {
    let mut dated: Vec<ImageRecord> = images.iter().filter(|i| i.corrected_date_taken.is_some()).cloned().collect();
    dated.sort_by_key(|i| i.corrected_date_taken.unwrap());

    let mut groups: Vec<Vec<ImageRecord>> = Vec::new();
    for image in dated {
        let starts_new_group = match groups.last().and_then(|group| group.last()) {
            Some(prev) => {
                let time_exceeded = image.corrected_date_taken.unwrap() - prev.corrected_date_taken.unwrap() > gap;
                let distance_exceeded = max_distance_km.is_some_and(|max_km| {
                    match (prev.gps_latitude, prev.gps_longitude, image.gps_latitude, image.gps_longitude) {
                        (Some(lat1), Some(lon1), Some(lat2), Some(lon2)) => {
                            haversine_distance_km(lat1, lon1, lat2, lon2) > max_km
                        }
                        _ => false,
                    }
                });
                time_exceeded || distance_exceeded
            }
            None => true,
        };
        if starts_new_group {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(image);
    }
    groups
}

/// The title an `events` row is given when first created — the photo count, as a plain
/// number. Kept as its own function (rather than inlined at the `INSERT` call site in
/// `db::events::regenerate`) so it's unit-testable without a database.
pub fn default_title(image_count: usize) -> String {
    image_count.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn image_at(key: &str, y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> ImageRecord {
        let when = NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, s);
        ImageRecord {
            key: key.to_string(),
            path: format!("{key}.jpg"),
            image_type: "jpg".to_string(),
            date_taken: when,
            corrected_date_taken: when,
            ..ImageRecord::default()
        }
    }

    fn undated(key: &str) -> ImageRecord {
        ImageRecord { key: key.to_string(), path: format!("{key}.jpg"), image_type: "jpg".to_string(), ..ImageRecord::default() }
    }

    fn with_gps(image: ImageRecord, lat: f64, lon: f64) -> ImageRecord {
        ImageRecord { gps_latitude: Some(lat), gps_longitude: Some(lon), ..image }
    }

    #[test]
    fn empty_input_produces_no_groups() {
        assert_eq!(cluster_by_time_gap(&[], Duration::hours(1), None), Vec::<Vec<ImageRecord>>::new());
    }

    #[test]
    fn single_dated_image_is_its_own_group() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        assert_eq!(cluster_by_time_gap(&[a.clone()], Duration::hours(1), None), vec![vec![a]]);
    }

    #[test]
    fn images_within_the_gap_merge_into_one_group() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        let b = image_at("b", 2026, 7, 15, 10, 5, 0);
        let groups = cluster_by_time_gap(&[a.clone(), b.clone()], Duration::minutes(10), None);
        assert_eq!(groups, vec![vec![a, b]]);
    }

    #[test]
    fn images_past_the_gap_split_into_two_groups() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        let b = image_at("b", 2026, 7, 15, 12, 0, 0);
        let groups = cluster_by_time_gap(&[a.clone(), b.clone()], Duration::minutes(10), None);
        assert_eq!(groups, vec![vec![a], vec![b]]);
    }

    #[test]
    fn unsorted_input_still_clusters_correctly() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        let b = image_at("b", 2026, 7, 15, 10, 5, 0);
        let c = image_at("c", 2026, 7, 15, 12, 0, 0);
        let groups = cluster_by_time_gap(&[c.clone(), a.clone(), b.clone()], Duration::minutes(10), None);
        assert_eq!(groups, vec![vec![a, b], vec![c]]);
    }

    #[test]
    fn undated_images_are_excluded_from_every_group() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        let groups = cluster_by_time_gap(&[a.clone(), undated("u")], Duration::hours(1), None);
        assert_eq!(groups, vec![vec![a]]);
    }

    #[test]
    fn haversine_distance_is_zero_for_the_same_point() {
        assert_eq!(haversine_distance_km(55.6761, 12.5683, 55.6761, 12.5683), 0.0);
    }

    #[test]
    fn haversine_distance_matches_known_city_distance() {
        // Copenhagen to Aarhus, Denmark: ~157 km great-circle (the ~187km/2h drive distance
        // follows roads/ferries, not the straight-line distance this function computes).
        let km = haversine_distance_km(55.6761, 12.5683, 56.1629, 10.2039);
        assert!((km - 157.0).abs() < 2.0, "expected ~157km, got {km}");
    }

    #[test]
    fn images_within_the_gap_but_beyond_the_distance_cap_split_into_two_groups() {
        let a = with_gps(image_at("a", 2026, 7, 15, 10, 0, 0), 55.6761, 12.5683); // Copenhagen
        let b = with_gps(image_at("b", 2026, 7, 15, 10, 30, 0), 56.1629, 10.2039); // Aarhus, ~157km away
        let groups = cluster_by_time_gap(&[a.clone(), b.clone()], Duration::hours(1), Some(50.0));
        assert_eq!(groups, vec![vec![a], vec![b]]);
    }

    #[test]
    fn images_within_the_gap_and_distance_cap_still_merge() {
        let a = with_gps(image_at("a", 2026, 7, 15, 10, 0, 0), 55.6761, 12.5683);
        let b = with_gps(image_at("b", 2026, 7, 15, 10, 30, 0), 55.6800, 12.5700);
        let groups = cluster_by_time_gap(&[a.clone(), b.clone()], Duration::hours(1), Some(50.0));
        assert_eq!(groups, vec![vec![a, b]]);
    }

    #[test]
    fn missing_gps_on_either_side_falls_back_to_time_only() {
        let a = with_gps(image_at("a", 2026, 7, 15, 10, 0, 0), 55.6761, 12.5683);
        let b = image_at("b", 2026, 7, 15, 10, 30, 0); // no GPS
        let groups = cluster_by_time_gap(&[a.clone(), b.clone()], Duration::hours(1), Some(50.0));
        assert_eq!(groups, vec![vec![a, b]]);
    }

    #[test]
    fn default_title_is_the_photo_count_as_a_plain_number() {
        assert_eq!(default_title(12), "12");
        assert_eq!(default_title(2), "2");
    }
}
