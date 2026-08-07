use chrono::Duration;

use crate::db::models::ImageRecord;

/// Groups `images` by `date_taken`: images with no `date_taken` are excluded entirely, the
/// rest are sorted ascending, then split into groups wherever the gap since the previous
/// photo exceeds `gap`. Each returned group is sorted ascending and non-empty; groups of size
/// 1 are still returned here — deciding whether a singleton group counts as a real "event" is
/// left to the caller.
pub fn cluster_by_time_gap(images: &[ImageRecord], gap: Duration) -> Vec<Vec<ImageRecord>> {
    let mut dated: Vec<ImageRecord> = images.iter().filter(|i| i.date_taken.is_some()).cloned().collect();
    dated.sort_by_key(|i| i.date_taken.unwrap());

    let mut groups: Vec<Vec<ImageRecord>> = Vec::new();
    for image in dated {
        let starts_new_group = match groups.last().and_then(|group| group.last()) {
            Some(prev) => image.date_taken.unwrap() - prev.date_taken.unwrap() > gap,
            None => true,
        };
        if starts_new_group {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(image);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn image_at(key: &str, y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> ImageRecord {
        ImageRecord {
            key: key.to_string(),
            path: format!("{key}.jpg"),
            image_type: "jpg".to_string(),
            date_taken: NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, s),
            ..ImageRecord::default()
        }
    }

    fn undated(key: &str) -> ImageRecord {
        ImageRecord { key: key.to_string(), path: format!("{key}.jpg"), image_type: "jpg".to_string(), ..ImageRecord::default() }
    }

    #[test]
    fn empty_input_produces_no_groups() {
        assert_eq!(cluster_by_time_gap(&[], Duration::hours(1)), Vec::<Vec<ImageRecord>>::new());
    }

    #[test]
    fn single_dated_image_is_its_own_group() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        assert_eq!(cluster_by_time_gap(&[a.clone()], Duration::hours(1)), vec![vec![a]]);
    }

    #[test]
    fn images_within_the_gap_merge_into_one_group() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        let b = image_at("b", 2026, 7, 15, 10, 5, 0);
        let groups = cluster_by_time_gap(&[a.clone(), b.clone()], Duration::minutes(10));
        assert_eq!(groups, vec![vec![a, b]]);
    }

    #[test]
    fn images_past_the_gap_split_into_two_groups() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        let b = image_at("b", 2026, 7, 15, 12, 0, 0);
        let groups = cluster_by_time_gap(&[a.clone(), b.clone()], Duration::minutes(10));
        assert_eq!(groups, vec![vec![a], vec![b]]);
    }

    #[test]
    fn unsorted_input_still_clusters_correctly() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        let b = image_at("b", 2026, 7, 15, 10, 5, 0);
        let c = image_at("c", 2026, 7, 15, 12, 0, 0);
        let groups = cluster_by_time_gap(&[c.clone(), a.clone(), b.clone()], Duration::minutes(10));
        assert_eq!(groups, vec![vec![a, b], vec![c]]);
    }

    #[test]
    fn undated_images_are_excluded_from_every_group() {
        let a = image_at("a", 2026, 7, 15, 10, 0, 0);
        let groups = cluster_by_time_gap(&[a.clone(), undated("u")], Duration::hours(1));
        assert_eq!(groups, vec![vec![a]]);
    }
}
