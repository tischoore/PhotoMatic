use std::collections::BTreeMap;

use chrono::{Duration, NaiveDateTime};

use crate::db::models::ImageRecord;

/// One lane's photo list, as shown by the Set Time Correction dialog. `toplevel_dir: None`
/// is the project-root bucket (images with no subfolder, sitting directly in the Source
/// Directory) — a normal grouping value here, same as everywhere else `toplevel_dir` is used
/// (directory counts, the Left Nav tree).
#[derive(Debug, Clone, PartialEq)]
pub struct LaneImages {
    pub toplevel_dir: Option<String>,
    pub photos: Vec<ImageRecord>,
}

/// Groups `images` (expected to be `ProjectDb::list_images_for_event_generation`'s result —
/// already RAW/compressed-linking aware, since `linked_key` is only ever set while Link RAW+JPG
/// is on) into one `LaneImages` per distinct `toplevel_dir`, each internally sorted
/// `(corrected_date_taken, path)` — the same ordering `db::images::list_images_by_directory`
/// uses, with `None` sorting first to match SQLite's default NULLS-first ascending order.
/// Groups themselves come back sorted by photo count descending — directories with more photos
/// (more likely alignment references) surface first in the Set Time Correction dialog's card
/// grid — with ties broken by `toplevel_dir` ascending (`None`/root first, then alphabetical);
/// that tie-break is a stable sort over the initial `BTreeMap` order, so it agrees with
/// `compute_offsets`'s own independent baseline tie-break without the two sharing code. A
/// directory with no images in `images` (e.g. its only photo is a linked RAW with no independent
/// slide) simply never produces an entry, so it never gets a lane and never participates in
/// baseline selection.
pub fn group_lanes(images: Vec<ImageRecord>) -> Vec<LaneImages> {
    let mut groups: BTreeMap<Option<String>, Vec<ImageRecord>> = BTreeMap::new();
    for image in images {
        groups.entry(image.toplevel_dir.clone()).or_default().push(image);
    }

    let mut lanes: Vec<LaneImages> = groups
        .into_iter()
        .map(|(toplevel_dir, mut photos)| {
            photos.sort_by(|a, b| (a.corrected_date_taken, &a.path).cmp(&(b.corrected_date_taken, &b.path)));
            LaneImages { toplevel_dir, photos }
        })
        .collect();
    lanes.sort_by(|a, b| b.photos.len().cmp(&a.photos.len()));
    lanes
}

/// One lane's contribution to `compute_offsets`: which directory it is, how many photos it
/// holds (used to pick the baseline), and the `corrected_date_taken` of whichever photo the
/// user currently has that lane's Prev/Next stepped to.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneSelection {
    pub toplevel_dir: Option<String>,
    pub photo_count: usize,
    pub selected_corrected_date_taken: Option<NaiveDateTime>,
}

/// Computes each non-baseline lane's `corrected_date_taken` offset from the photo the user has
/// currently aligned it to, relative to the lane with the most photos (the baseline, implicitly
/// offset zero and excluded from the result). Ties on photo count are broken by `toplevel_dir`
/// ascending — `None` (the project-root bucket) wins a tie over any named directory, matching
/// `group_lanes`'s ordering.
///
/// Each returned offset is `baseline's selection − that lane's selection`, meant to be *added*
/// to `corrected_date_taken`.
///
/// Errors (the message names the offending lane, for display via `nwg::modal_error_message`
/// without closing the dialog) when: fewer than two lanes are given; the baseline lane's
/// current selection has no `corrected_date_taken` yet; or any other lane's current selection
/// doesn't either — in both of the latter cases, Generate MetaData hasn't run on that photo yet
/// and there's nothing to align against.
pub fn compute_offsets(selections: &[LaneSelection]) -> Result<Vec<(Option<String>, Duration)>, String> {
    if selections.len() < 2 {
        return Err("At least two folders with photos are needed to set a time correction.".to_string());
    }

    let baseline = &selections[baseline_index(selections)];

    let baseline_dt = baseline.selected_corrected_date_taken.ok_or_else(|| missing_date_message(&baseline.toplevel_dir))?;

    let mut offsets = Vec::new();
    for lane in selections {
        if lane.toplevel_dir == baseline.toplevel_dir {
            continue;
        }
        let dt = lane.selected_corrected_date_taken.ok_or_else(|| missing_date_message(&lane.toplevel_dir))?;
        offsets.push((lane.toplevel_dir.clone(), baseline_dt - dt));
    }

    Ok(offsets)
}

/// A lane's display-only offset from the baseline, as shown live in the Set Time Correction
/// dialog's card headers (see `lane_offsets`) — unlike `compute_offsets`, this never errors out
/// the whole batch over one lane's missing date, since it must still render a header for every
/// other lane while the user works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneOffset {
    /// The lane with the most photos — always displayed as `0`, never computed against itself.
    Baseline,
    /// `baseline's selection − this lane's selection`, meant to be *added* to `corrected_date_taken`.
    Offset(Duration),
    /// Either the baseline's or this lane's current selection has no `corrected_date_taken` yet
    /// (Generate MetaData hasn't run on it), so no offset can be computed.
    Unknown,
}

/// The index into `selections` of the baseline lane: the one with the most photos, ties broken by
/// `toplevel_dir` ascending (`None`/root wins a tie over any named directory) — shared by
/// `compute_offsets` and `lane_offsets` so both agree on which lane is the baseline.
fn baseline_index(selections: &[LaneSelection]) -> usize {
    let mut ordered: Vec<usize> = (0..selections.len()).collect();
    ordered.sort_by(|&a, &b| {
        selections[b].photo_count.cmp(&selections[a].photo_count).then_with(|| selections[a].toplevel_dir.cmp(&selections[b].toplevel_dir))
    });
    ordered[0]
}

/// One `LaneOffset` per lane in `selections`, same order, for the Set Time Correction dialog's
/// card headers — recomputed on every thumbnail click (including in the baseline lane itself,
/// which shifts every other lane's offset) so the headers always reflect the current selections.
pub fn lane_offsets(selections: &[LaneSelection]) -> Vec<LaneOffset> {
    if selections.is_empty() {
        return Vec::new();
    }

    let baseline_idx = baseline_index(selections);
    let baseline_dt = selections[baseline_idx].selected_corrected_date_taken;

    selections
        .iter()
        .enumerate()
        .map(|(index, lane)| {
            if index == baseline_idx {
                return LaneOffset::Baseline;
            }
            match (baseline_dt, lane.selected_corrected_date_taken) {
                (Some(baseline_dt), Some(dt)) => LaneOffset::Offset(baseline_dt - dt),
                _ => LaneOffset::Unknown,
            }
        })
        .collect()
}

/// Renders a `LaneOffset` for display in a card header: `"0"` for the baseline, a signed
/// `HH:MM:SS` for a computed offset, or a call-out matching `lane_status_text`'s wording when the
/// offset can't be computed yet.
pub fn format_lane_offset(offset: LaneOffset) -> String {
    match offset {
        LaneOffset::Baseline => "0".to_string(),
        LaneOffset::Offset(duration) => format_offset(duration),
        LaneOffset::Unknown => "no date yet".to_string(),
    }
}

/// Formats a `Duration` as a signed `HH:MM:SS`, e.g. `+01:23:45` or `-00:05:00` — hours are not
/// capped at 24, so a multi-day offset still renders as a single (larger) hour count rather than
/// wrapping.
fn format_offset(duration: Duration) -> String {
    let total_seconds = duration.num_seconds();
    let sign = if total_seconds < 0 { '-' } else { '+' };
    let magnitude = total_seconds.abs();
    format!("{sign}{:02}:{:02}:{:02}", magnitude / 3600, (magnitude % 3600) / 60, magnitude % 60)
}

fn lane_label(toplevel_dir: &Option<String>) -> &str {
    toplevel_dir.as_deref().unwrap_or("(root)")
}

fn missing_date_message(toplevel_dir: &Option<String>) -> String {
    format!("\"{}\" has no corrected date yet on the selected photo — run Generate MetaData first.", lane_label(toplevel_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(path: &str, toplevel_dir: Option<&str>, corrected_date_taken: Option<&str>) -> ImageRecord {
        ImageRecord {
            path: path.to_string(),
            toplevel_dir: toplevel_dir.map(|s| s.to_string()),
            corrected_date_taken: corrected_date_taken.map(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()),
            ..ImageRecord::default()
        }
    }

    #[test]
    fn group_lanes_breaks_a_photo_count_tie_by_directory_name_with_root_first() {
        let images = vec![
            image("50D/b.jpg", Some("50D"), None),
            image("root.jpg", None, None),
            image("40D/a.jpg", Some("40D"), None),
        ];

        let lanes = group_lanes(images);

        // All three directories are tied at 1 photo each, so the tie-break (name ascending,
        // root first) decides the order.
        let dirs: Vec<Option<String>> = lanes.iter().map(|l| l.toplevel_dir.clone()).collect();
        assert_eq!(dirs, vec![None, Some("40D".to_string()), Some("50D".to_string())]);
    }

    #[test]
    fn group_lanes_orders_by_photo_count_descending_overriding_directory_name_order() {
        let images = vec![
            image("40D/a.jpg", Some("40D"), None),
            image("50D/a.jpg", Some("50D"), None),
            image("50D/b.jpg", Some("50D"), None),
        ];

        let lanes = group_lanes(images);

        // 50D has more photos than 40D, so it comes first despite sorting after alphabetically.
        let dirs: Vec<Option<String>> = lanes.iter().map(|l| l.toplevel_dir.clone()).collect();
        assert_eq!(dirs, vec![Some("50D".to_string()), Some("40D".to_string())]);
    }

    #[test]
    fn group_lanes_sorts_each_lanes_photos_by_corrected_date_taken_then_path() {
        let images = vec![
            image("50D/b.jpg", Some("50D"), Some("2026-01-01 10:00:00")),
            image("50D/a.jpg", Some("50D"), Some("2026-01-01 09:00:00")),
            image("50D/c.jpg", Some("50D"), None),
        ];

        let lanes = group_lanes(images);

        let paths: Vec<&str> = lanes[0].photos.iter().map(|p| p.path.as_str()).collect();
        // NULL corrected_date_taken sorts first, matching SQLite's ORDER BY default.
        assert_eq!(paths, vec!["50D/c.jpg", "50D/a.jpg", "50D/b.jpg"]);
    }

    fn selection(toplevel_dir: Option<&str>, photo_count: usize, selected: Option<&str>) -> LaneSelection {
        LaneSelection {
            toplevel_dir: toplevel_dir.map(|s| s.to_string()),
            photo_count,
            selected_corrected_date_taken: selected.map(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()),
        }
    }

    #[test]
    fn compute_offsets_errors_with_fewer_than_two_lanes() {
        let selections = vec![selection(Some("50D"), 10, Some("2026-01-01 10:00:00"))];
        assert!(compute_offsets(&selections).is_err());
    }

    #[test]
    fn compute_offsets_picks_the_lane_with_the_most_photos_as_baseline() {
        let selections = vec![
            selection(Some("40D"), 5, Some("2026-01-01 10:00:00")),
            selection(Some("50D"), 10, Some("2026-01-01 10:05:00")),
        ];

        let offsets = compute_offsets(&selections).unwrap();

        // 50D (10 photos) is the baseline, so only 40D gets an offset.
        assert_eq!(offsets.len(), 1);
        assert_eq!(offsets[0].0, Some("40D".to_string()));
        assert_eq!(offsets[0].1, Duration::minutes(5));
    }

    #[test]
    fn compute_offsets_breaks_a_photo_count_tie_by_directory_name_with_root_winning() {
        let selections = vec![
            selection(Some("50D"), 10, Some("2026-01-01 10:00:00")),
            selection(None, 10, Some("2026-01-01 09:00:00")),
        ];

        let offsets = compute_offsets(&selections).unwrap();

        // Root (None) wins the tie, so 50D is the one corrected, by -1 hour (root - 50D).
        assert_eq!(offsets.len(), 1);
        assert_eq!(offsets[0].0, Some("50D".to_string()));
        assert_eq!(offsets[0].1, Duration::hours(-1));
    }

    #[test]
    fn compute_offsets_computes_baseline_minus_lane_for_every_non_baseline_lane() {
        let selections = vec![
            selection(Some("50D"), 10, Some("2026-01-01 12:00:00")),
            selection(Some("40D"), 3, Some("2026-01-01 11:30:00")),
            selection(Some("60D"), 2, Some("2026-01-01 12:10:00")),
        ];

        let offsets = compute_offsets(&selections).unwrap();

        assert_eq!(offsets.len(), 2);
        let forty_d = offsets.iter().find(|(dir, _)| dir.as_deref() == Some("40D")).unwrap();
        assert_eq!(forty_d.1, Duration::minutes(30));
        let sixty_d = offsets.iter().find(|(dir, _)| dir.as_deref() == Some("60D")).unwrap();
        assert_eq!(sixty_d.1, Duration::minutes(-10));
    }

    #[test]
    fn compute_offsets_errors_naming_the_baseline_when_its_selection_has_no_corrected_date() {
        let selections = vec![selection(Some("50D"), 10, None), selection(Some("40D"), 5, Some("2026-01-01 10:00:00"))];

        let err = compute_offsets(&selections).unwrap_err();

        assert!(err.contains("50D"));
    }

    #[test]
    fn compute_offsets_errors_naming_a_non_baseline_lane_when_its_selection_has_no_corrected_date() {
        let selections = vec![selection(Some("50D"), 10, Some("2026-01-01 10:00:00")), selection(Some("40D"), 5, None)];

        let err = compute_offsets(&selections).unwrap_err();

        assert!(err.contains("40D"));
    }

    #[test]
    fn lane_offsets_marks_the_largest_lane_baseline_and_computes_the_rest() {
        let selections = vec![
            selection(Some("40D"), 5, Some("2026-01-01 10:00:00")),
            selection(Some("50D"), 10, Some("2026-01-01 10:05:00")),
        ];

        let offsets = lane_offsets(&selections);

        assert_eq!(offsets[1], LaneOffset::Baseline);
        assert_eq!(offsets[0], LaneOffset::Offset(Duration::minutes(5)));
    }

    #[test]
    fn lane_offsets_breaks_a_photo_count_tie_the_same_way_compute_offsets_does() {
        let selections = vec![selection(Some("50D"), 10, Some("2026-01-01 10:00:00")), selection(None, 10, Some("2026-01-01 09:00:00"))];

        let offsets = lane_offsets(&selections);

        // Root (None) wins the tie, matching compute_offsets_breaks_a_photo_count_tie_with_root_winning.
        assert_eq!(offsets[0], LaneOffset::Offset(Duration::hours(-1)));
        assert_eq!(offsets[1], LaneOffset::Baseline);
    }

    #[test]
    fn lane_offsets_is_unknown_when_the_baseline_has_no_selection() {
        let selections = vec![selection(Some("50D"), 10, None), selection(Some("40D"), 5, Some("2026-01-01 10:00:00"))];

        let offsets = lane_offsets(&selections);

        assert_eq!(offsets[0], LaneOffset::Baseline);
        assert_eq!(offsets[1], LaneOffset::Unknown);
    }

    #[test]
    fn lane_offsets_is_unknown_when_a_non_baseline_lane_has_no_selection() {
        let selections = vec![selection(Some("50D"), 10, Some("2026-01-01 10:00:00")), selection(Some("40D"), 5, None)];

        let offsets = lane_offsets(&selections);

        assert_eq!(offsets[1], LaneOffset::Unknown);
    }

    #[test]
    fn lane_offsets_on_a_single_lane_is_just_the_baseline() {
        let selections = vec![selection(Some("50D"), 10, Some("2026-01-01 10:00:00"))];

        assert_eq!(lane_offsets(&selections), vec![LaneOffset::Baseline]);
    }

    #[test]
    fn lane_offsets_on_an_empty_slice_is_empty() {
        assert_eq!(lane_offsets(&[]), Vec::new());
    }

    #[test]
    fn format_lane_offset_renders_each_variant() {
        assert_eq!(format_lane_offset(LaneOffset::Baseline), "0");
        assert_eq!(format_lane_offset(LaneOffset::Offset(Duration::minutes(5))), "+00:05:00");
        assert_eq!(format_lane_offset(LaneOffset::Offset(Duration::hours(-1))), "-01:00:00");
        assert_eq!(format_lane_offset(LaneOffset::Unknown), "no date yet");
    }

    #[test]
    fn format_offset_pads_and_signs_hh_mm_ss() {
        assert_eq!(format_offset(Duration::zero()), "+00:00:00");
        assert_eq!(format_offset(Duration::seconds(3665)), "+01:01:05");
        assert_eq!(format_offset(Duration::seconds(-3665)), "-01:01:05");
    }

    #[test]
    fn format_offset_does_not_cap_hours_at_24() {
        assert_eq!(format_offset(Duration::hours(30)), "+30:00:00");
    }
}
