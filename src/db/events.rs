use std::collections::{BTreeSet, HashSet};

use chrono::Duration;
use rusqlite::{Connection, OptionalExtension};

use super::images::{map_image_row, IMAGE_COLUMNS};
use super::models::{EventRecord, EventSummary, EventType, ImageRecord};
use super::DbError;
use crate::events::{cluster_by_time_gap, default_title};

/// The per-tier gap thresholds a "Generate Events" run clusters with. `session_max_distance_km`
/// and `multi_hour_max_distance_km` additionally split a group wherever the GPS distance since
/// the previous photo exceeds them (when both photos have GPS) — Tight Burst has no distance
/// threshold, since at that time scale GPS noise isn't a meaningful signal.
pub struct EventThresholds {
    pub burst: Duration,
    pub session: Duration,
    pub multi_hour: Duration,
    pub session_max_distance_km: f64,
    pub multi_hour_max_distance_km: f64,
}

/// One `events` row as it exists in the database right now, together with the full set of
/// image keys currently linked to it — the "before" state `plan` diffs a fresh clustering
/// against, so an unchanged group can be left alone instead of deleted and reinserted.
struct ExistingEvent {
    id: i64,
    event_type: EventType,
    title: String,
    notes: String,
    image_keys: BTreeSet<String>,
}

impl ExistingEvent {
    /// Whether a user has actually typed something into this event: the title no longer
    /// matches the plain photo-count default every event starts with, or notes is non-empty.
    /// Backs `MergePlan.discarded_edited_titles`, the per-event replacement for the old
    /// project-wide `has_edited_events` check.
    fn is_edited(&self) -> bool {
        !self.notes.is_empty() || self.title != default_title(self.image_keys.len())
    }
}

/// What bringing `events`/`event_images` in line with a fresh clustering actually requires —
/// computed by `plan` (pure, no database access) so the app layer can inspect
/// `discarded_edited_titles` and ask for confirmation *before* `apply_plan` writes anything.
#[derive(Default)]
pub struct MergePlan {
    inserts: Vec<(EventType, BTreeSet<String>)>,
    /// `(event_id, new full image-key set, new title if the old one was still the plain
    /// default)` — covers both a group simply gaining members and a merge, where the
    /// lowest-id existing event survives and absorbs every merged group's photos.
    extends: Vec<(i64, BTreeSet<String>, Option<String>)>,
    removes: Vec<i64>,
    /// Titles of events this plan deletes (merged away or no longer produced by any tier)
    /// that had a user edit worth warning about before applying.
    pub discarded_edited_titles: Vec<String>,
}

impl MergePlan {
    /// Whether applying this plan would discard any user-entered title/notes — the app layer's
    /// cue to confirm before calling `apply_plan`.
    pub fn is_destructive(&self) -> bool {
        !self.discarded_edited_titles.is_empty()
    }
}

/// A plain count of what a `regenerate` run actually did — enough for its own direct callers
/// (mainly this module's tests) without needing the full `MergePlan`.
#[derive(Debug, Default, PartialEq)]
pub struct MergeSummary {
    pub inserted: usize,
    pub extended: usize,
    pub removed: usize,
    pub discarded_edited_titles: Vec<String>,
}

/// Reads every current `events` row together with its linked image keys, grouped in Rust
/// (rows arrive already contiguous per event since the query orders by `e.id`).
fn load_existing(conn: &Connection) -> Result<Vec<ExistingEvent>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT e.id, e.event_type, e.title, e.notes, ei.image_key \
             FROM events e JOIN event_images ei ON ei.event_id = e.id \
             ORDER BY e.id",
        )
        .map_err(DbError::Sqlite)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?))
        })
        .map_err(DbError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(DbError::Sqlite)?;

    let mut existing: Vec<ExistingEvent> = Vec::new();
    for (id, event_type, title, notes, image_key) in rows {
        match existing.last_mut() {
            Some(last) if last.id == id => {
                last.image_keys.insert(image_key);
            }
            _ => {
                let mut image_keys = BTreeSet::new();
                image_keys.insert(image_key);
                existing.push(ExistingEvent { id, event_type: parse_event_type(&event_type), title, notes, image_keys });
            }
        }
    }
    Ok(existing)
}

/// Diffs a fresh clustering of `images` at all three tiers (Tight Burst, Session, Multi-hour)
/// against `existing` and returns what needs to change — never what's already correct. For
/// each tier's groups of 2+ photos:
/// - No existing event overlaps it → a brand new event.
/// - Exactly one existing event overlaps and its membership is unchanged → left alone entirely.
/// - Exactly one existing event overlaps and the group only grew (clustering by time gap can
///   only ever grow or merge a group as more images are added, never shrink or split one) →
///   that event keeps its id/notes, its `event_images` extend to the new membership, and its
///   title updates to the new count only if it was still the plain default (never overwriting
///   a user's own title).
/// - More than one existing event overlaps (two previously separate groups got bridged by a
///   new photo) → the lowest-id event survives and absorbs every photo, the rest are deleted.
///
/// Any existing event never produced by any tier at all (can happen when a time correction
/// reshuffles `corrected_date_taken`, not just when photos are added) is deleted as an orphan.
/// A deleted event that `is_edited()` contributes its title to `discarded_edited_titles`.
fn plan(existing: Vec<ExistingEvent>, images: &[ImageRecord], thresholds: &EventThresholds) -> MergePlan {
    let mut merge_plan = MergePlan::default();
    let mut matched: HashSet<i64> = HashSet::new();
    let mut removed: HashSet<i64> = HashSet::new();

    for (event_type, gap, max_distance_km) in [
        (EventType::TightBurst, thresholds.burst, None),
        (EventType::Session, thresholds.session, Some(thresholds.session_max_distance_km)),
        (EventType::MultiHour, thresholds.multi_hour, Some(thresholds.multi_hour_max_distance_km)),
    ] {
        for group in cluster_by_time_gap(images, gap, max_distance_km) {
            if group.len() < 2 {
                continue;
            }
            let new_keys: BTreeSet<String> = group.iter().map(|i| i.key.clone()).collect();

            let overlapping: Vec<&ExistingEvent> =
                existing.iter().filter(|e| e.event_type == event_type && !e.image_keys.is_disjoint(&new_keys)).collect();

            match overlapping.as_slice() {
                [] => merge_plan.inserts.push((event_type, new_keys)),
                [one] if one.image_keys == new_keys => {
                    matched.insert(one.id);
                }
                _ => {
                    let survivor = overlapping.iter().min_by_key(|e| e.id).unwrap();
                    matched.insert(survivor.id);
                    let new_title =
                        (survivor.title == default_title(survivor.image_keys.len())).then(|| default_title(new_keys.len()));
                    merge_plan.extends.push((survivor.id, new_keys, new_title));

                    for other in overlapping.iter().filter(|e| e.id != survivor.id) {
                        if removed.insert(other.id) {
                            merge_plan.removes.push(other.id);
                            if other.is_edited() {
                                merge_plan.discarded_edited_titles.push(other.title.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    for event in &existing {
        if !matched.contains(&event.id) && removed.insert(event.id) {
            merge_plan.removes.push(event.id);
            if event.is_edited() {
                merge_plan.discarded_edited_titles.push(event.title.clone());
            }
        }
    }

    merge_plan
}

/// Executes a `MergePlan`: inserts new events, extends existing ones' `event_images` to their
/// new membership (updating title only where the plan says to), and deletes merged-away or
/// orphaned events together with their `event_images` rows (no `ON DELETE CASCADE` on that
/// foreign key, so this is done explicitly). All in one transaction, so a failure never leaves
/// things half-applied.
fn apply(conn: &mut Connection, merge_plan: MergePlan) -> Result<(), DbError> {
    let tx = conn.transaction().map_err(DbError::Sqlite)?;

    for id in &merge_plan.removes {
        tx.execute("DELETE FROM event_images WHERE event_id = ?1", rusqlite::params![id]).map_err(DbError::Sqlite)?;
        tx.execute("DELETE FROM events WHERE id = ?1", rusqlite::params![id]).map_err(DbError::Sqlite)?;
    }

    {
        let mut insert_event =
            tx.prepare("INSERT INTO events (event_type, notes, title) VALUES (?1, '', ?2)").map_err(DbError::Sqlite)?;
        let mut insert_link =
            tx.prepare("INSERT INTO event_images (event_id, image_key) VALUES (?1, ?2)").map_err(DbError::Sqlite)?;
        for (event_type, keys) in &merge_plan.inserts {
            insert_event.execute(rusqlite::params![event_type.to_string(), default_title(keys.len())]).map_err(DbError::Sqlite)?;
            let event_id = tx.last_insert_rowid();
            for key in keys {
                insert_link.execute(rusqlite::params![event_id, key]).map_err(DbError::Sqlite)?;
            }
        }
    }

    {
        let mut delete_links = tx.prepare("DELETE FROM event_images WHERE event_id = ?1").map_err(DbError::Sqlite)?;
        let mut insert_link =
            tx.prepare("INSERT INTO event_images (event_id, image_key) VALUES (?1, ?2)").map_err(DbError::Sqlite)?;
        let mut update_title = tx.prepare("UPDATE events SET title = ?2 WHERE id = ?1").map_err(DbError::Sqlite)?;
        for (id, keys, new_title) in &merge_plan.extends {
            delete_links.execute(rusqlite::params![id]).map_err(DbError::Sqlite)?;
            for key in keys {
                insert_link.execute(rusqlite::params![id, key]).map_err(DbError::Sqlite)?;
            }
            if let Some(title) = new_title {
                update_title.execute(rusqlite::params![title, id]).map_err(DbError::Sqlite)?;
            }
        }
    }

    tx.commit().map_err(DbError::Sqlite)
}

/// Reads the database's current events and diffs a fresh clustering of `images` against them
/// (see `plan`) — the read half of a regeneration, split out so the app layer can inspect
/// `MergePlan.discarded_edited_titles` and confirm before `apply_plan` writes anything.
pub fn plan_regeneration(conn: &Connection, images: &[ImageRecord], thresholds: &EventThresholds) -> Result<MergePlan, DbError> {
    let existing = load_existing(conn)?;
    Ok(plan(existing, images, thresholds))
}

/// Writes a previously computed `MergePlan` to the database.
pub fn apply_plan(conn: &mut Connection, merge_plan: MergePlan) -> Result<(), DbError> {
    apply(conn, merge_plan)
}

/// Convenience wrapper for callers that don't need to confirm anything before writing (this
/// module's own tests, mainly): plans and immediately applies a regeneration in one call.
pub fn regenerate(conn: &mut Connection, images: &[ImageRecord], thresholds: &EventThresholds) -> Result<MergeSummary, DbError> {
    let existing = load_existing(conn)?;
    let merge_plan = plan(existing, images, thresholds);
    let summary = MergeSummary {
        inserted: merge_plan.inserts.len(),
        extended: merge_plan.extends.len(),
        removed: merge_plan.removes.len(),
        discarded_edited_titles: merge_plan.discarded_edited_titles.clone(),
    };
    apply(conn, merge_plan)?;
    Ok(summary)
}

/// Removes one image from every event it belongs to — backs the Image Viewer's Delete button.
/// Any `events` row left with zero remaining `event_images` rows is left as an orphan rather
/// than cleaned up here: `plan`'s orphan-removal pass deletes it on the next regeneration
/// (it can no longer overlap any freshly clustered group), and `list_events`'s inner join
/// already excludes a zero-photo event from the views that matter in the meantime. The
/// `images` row itself is left untouched, so a later Generate Events run re-clusters the
/// image if it's still eligible.
pub fn remove_image_from_all_events(conn: &Connection, image_key: &str) -> Result<(), DbError> {
    conn.execute("DELETE FROM event_images WHERE image_key = ?1", rusqlite::params![image_key])
        .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Every `events` row with its photo count, ordered chronologically by each event's
/// earliest photo — backs the Left Navigation tree's Events node. Grouping these into
/// per-type tree nodes is `event_tree::build`'s job, not this query's; an event with no
/// linked photos (shouldn't occur — `regenerate` only ever inserts groups of 2+) is
/// simply excluded by the inner joins rather than surfaced as a zero-count row.
pub fn list_events(conn: &Connection) -> Result<Vec<EventSummary>, DbError> {
    let mut stmt = conn
        .prepare(
            "SELECT e.id, e.event_type, e.title, COUNT(ei.image_key) \
             FROM events e \
             JOIN event_images ei ON ei.event_id = e.id \
             JOIN images img ON img.key = ei.image_key \
             GROUP BY e.id \
             ORDER BY MIN(img.corrected_date_taken)",
        )
        .map_err(DbError::Sqlite)?;
    let rows = stmt
        .query_map([], |row| {
            let event_type: String = row.get(1)?;
            Ok((row.get::<_, i64>(0)?, event_type, row.get::<_, String>(2)?, row.get::<_, i64>(3)?))
        })
        .map_err(DbError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(DbError::Sqlite)?;

    Ok(rows
        .into_iter()
        .map(|(id, event_type, title, image_count)| EventSummary { id, event_type: parse_event_type(&event_type), title, image_count })
        .collect())
}

/// The photos belonging to one event, ordered chronologically — the event tab's photo table.
pub fn event_images(conn: &Connection, event_id: i64) -> Result<Vec<ImageRecord>, DbError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {IMAGE_COLUMNS} FROM images \
             JOIN event_images ei ON ei.image_key = images.key \
             WHERE ei.event_id = ?1 ORDER BY images.corrected_date_taken"
        ))
        .map_err(DbError::Sqlite)?;
    let rows = stmt.query_map(rusqlite::params![event_id], map_image_row).map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

/// A single event's id/type/title/notes, for populating a newly opened event tab.
pub fn get_event(conn: &Connection, event_id: i64) -> Result<Option<EventRecord>, DbError> {
    conn.query_row("SELECT id, event_type, title, notes FROM events WHERE id = ?1", rusqlite::params![event_id], |row| {
        let event_type: String = row.get(1)?;
        Ok((row.get::<_, i64>(0)?, event_type, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
    })
    .optional()
    .map_err(DbError::Sqlite)
    .map(|row| row.map(|(id, event_type, title, notes)| EventRecord { id, event_type: parse_event_type(&event_type), title, notes }))
}

/// `events.event_type` only ever holds a value this crate itself wrote via
/// `EventType::to_string()` (`regenerate`'s insert), so a row that fails to parse back
/// indicates a corrupted database rather than a case calling code should recover from.
fn parse_event_type(value: &str) -> EventType {
    value.parse().expect("events.event_type should always round-trip through EventType::to_string()")
}

/// Writes a user-edited title/notes back to one `events` row — the event tab's title
/// input and notes box both call this on every keystroke (see `App`'s per-tab
/// `OnTextInput` handling), since these are cheap local SQLite writes.
pub fn update_event(conn: &Connection, event_id: i64, title: &str, notes: &str) -> Result<(), DbError> {
    conn.execute(
        "UPDATE events SET title = ?2, notes = ?3 WHERE id = ?1",
        rusqlite::params![event_id, title, notes],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn image_at(key: &str, h: u32, min: u32) -> ImageRecord {
        let when = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap().and_hms_opt(h, min, 0);
        ImageRecord {
            key: key.to_string(),
            path: format!("{key}.jpg"),
            image_type: "jpg".to_string(),
            date_taken: when,
            corrected_date_taken: when,
            ..ImageRecord::default()
        }
    }

    fn thresholds() -> EventThresholds {
        EventThresholds {
            burst: Duration::seconds(10),
            session: Duration::minutes(60),
            multi_hour: Duration::hours(8),
            session_max_distance_km: 1000.0,
            multi_hour_max_distance_km: 1000.0,
        }
    }

    fn with_gps(image: ImageRecord, lat: f64, lon: f64) -> ImageRecord {
        ImageRecord { gps_latitude: Some(lat), gps_longitude: Some(lon), ..image }
    }

    /// `event_images.image_key` has a foreign key into `images(key)`, so every image
    /// clustered in a test must exist there first — mirrors how `regenerate` is always
    /// called with images that already came from `db.list_images()` in production.
    /// Also writes `date_taken`/`corrected_date_taken` back to the row: `upsert_images` alone
    /// only stores key/path/image_type/toplevel_dir (both normally arrive later via
    /// `update_metadata`), but `list_events`/`event_images` join against the `images` table's
    /// `corrected_date_taken`, not the in-memory `ImageRecord`s `regenerate` clusters —
    /// mirroring how, in production, `db.list_images()` (regenerate's actual image source)
    /// only ever has one once metadata extraction already wrote it.
    fn seed_images(conn: &mut Connection, images: &[ImageRecord]) {
        crate::db::images::upsert_images(conn, images).unwrap();
        for image in images {
            conn.execute(
                "UPDATE images SET date_taken = ?2, corrected_date_taken = ?3 WHERE key = ?1",
                rusqlite::params![image.key, image.date_taken, image.corrected_date_taken],
            )
            .unwrap();
        }
    }

    fn events_and_links(conn: &Connection) -> (Vec<(i64, String)>, Vec<(i64, String)>) {
        let mut event_stmt = conn.prepare("SELECT id, event_type FROM events ORDER BY id").unwrap();
        let events: Vec<(i64, String)> =
            event_stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().map(|r| r.unwrap()).collect();

        let mut link_stmt = conn.prepare("SELECT event_id, image_key FROM event_images ORDER BY event_id, image_key").unwrap();
        let links: Vec<(i64, String)> =
            link_stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().map(|r| r.unwrap()).collect();

        (events, links)
    }

    #[test]
    fn regenerate_creates_a_session_event_for_two_images_within_an_hour() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30)];
        seed_images(&mut conn, &images);
        regenerate(&mut conn, &images, &thresholds()).unwrap();

        // 30 minutes apart clears the Session tier (<=60min) and, being nested, the
        // coarser Multi-hour tier (<=8h) too — but not the 10-second Tight Burst tier.
        let (events, links) = events_and_links(&conn);
        assert_eq!(events, vec![(1, "Session".to_string()), (2, "Multi-hour".to_string())]);
        assert_eq!(
            links,
            vec![(1, "a".to_string()), (1, "b".to_string()), (2, "a".to_string()), (2, "b".to_string())]
        );
    }

    #[test]
    fn remove_image_from_all_events_removes_only_that_images_links() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30)];
        seed_images(&mut conn, &images);
        regenerate(&mut conn, &images, &thresholds()).unwrap();

        remove_image_from_all_events(&conn, "a").unwrap();

        let (events, links) = events_and_links(&conn);
        assert_eq!(events, vec![(1, "Session".to_string()), (2, "Multi-hour".to_string())]);
        assert_eq!(links, vec![(1, "b".to_string()), (2, "b".to_string())]);
    }

    #[test]
    fn a_photo_with_no_neighbor_in_any_tier_gets_no_event_rows() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("lonely", 3, 0)];
        seed_images(&mut conn, &images);
        regenerate(&mut conn, &images, &thresholds()).unwrap();

        let (events, links) = events_and_links(&conn);
        assert!(events.is_empty());
        assert!(links.is_empty());
    }

    #[test]
    fn images_can_belong_to_up_to_three_tiers_at_once() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        // Two shots 5 seconds apart: a Tight Burst, a Session, and a Multi-hour group all at once.
        let a_when = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap().and_hms_opt(10, 0, 0);
        let a = ImageRecord { date_taken: a_when, corrected_date_taken: a_when, ..image_at("a", 10, 0) };
        let b_when = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap().and_hms_opt(10, 0, 5);
        let b = ImageRecord { date_taken: b_when, corrected_date_taken: b_when, ..image_at("b", 10, 0) };
        seed_images(&mut conn, &[a.clone(), b.clone()]);
        regenerate(&mut conn, &[a, b], &thresholds()).unwrap();

        let (events, links) = events_and_links(&conn);
        assert_eq!(events.len(), 3);
        assert_eq!(events.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(), vec!["Tight Burst", "Session", "Multi-hour"]);
        assert_eq!(links.len(), 6); // 2 images x 3 tiers
    }

    #[test]
    fn regenerating_replaces_rather_than_duplicates_prior_results() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30)];
        seed_images(&mut conn, &images);
        regenerate(&mut conn, &images, &thresholds()).unwrap();
        regenerate(&mut conn, &images, &thresholds()).unwrap();

        let (events, links) = events_and_links(&conn);
        assert_eq!(events, vec![(1, "Session".to_string()), (2, "Multi-hour".to_string())]);
        assert_eq!(
            links,
            vec![(1, "a".to_string()), (1, "b".to_string()), (2, "a".to_string()), (2, "b".to_string())]
        );
    }

    #[test]
    fn a_distant_session_pair_splits_by_distance_while_its_tight_burst_pair_does_not() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        // 5 seconds apart clears Tight Burst too, so the same pair is clustered at all three
        // tiers, but Copenhagen -> Aarhus (~157km) exceeds the Session/Multi-hour distance cap.
        let a_when = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap().and_hms_opt(10, 0, 0);
        let a = with_gps(
            ImageRecord { date_taken: a_when, corrected_date_taken: a_when, ..image_at("a", 10, 0) },
            55.6761,
            12.5683,
        );
        let b_when = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap().and_hms_opt(10, 0, 5);
        let b = with_gps(
            ImageRecord { date_taken: b_when, corrected_date_taken: b_when, ..image_at("b", 10, 0) },
            56.1629,
            10.2039,
        );
        seed_images(&mut conn, &[a.clone(), b.clone()]);

        let mut thresholds = thresholds();
        thresholds.session_max_distance_km = 50.0;
        thresholds.multi_hour_max_distance_km = 50.0;
        regenerate(&mut conn, &[a, b], &thresholds).unwrap();

        let (events, links) = events_and_links(&conn);
        // Tight Burst still forms one group (no distance check); Session/Multi-hour each split
        // into two singleton groups, which aren't persisted as events (need >=2 photos).
        assert_eq!(events, vec![(1, "Tight Burst".to_string())]);
        assert_eq!(links, vec![(1, "a".to_string()), (1, "b".to_string())]);
    }

    #[test]
    fn regenerate_sets_each_events_default_title_to_its_photo_count() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30)];
        seed_images(&mut conn, &images);
        regenerate(&mut conn, &images, &thresholds()).unwrap();

        let titles: Vec<String> = conn
            .prepare("SELECT title FROM events ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(titles, vec!["2".to_string(), "2".to_string()]);
    }

    #[test]
    fn list_events_orders_chronologically_rather_than_by_insertion_order() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        // X (08:00/10:00, 2h apart) clears only the Multi-hour tier; Y (20:00/20:30, 30min
        // apart) clears both Session and Multi-hour. Tiers are processed Tight Burst ->
        // Session -> Multi-hour, so Y's Session event is inserted (id 1) before X's
        // Multi-hour event (id 2) — even though X's photos are chronologically earlier.
        // list_events must still list X's event first.
        let x = vec![image_at("x-a", 8, 0), image_at("x-b", 10, 0)];
        let y = vec![image_at("y-a", 20, 0), image_at("y-b", 20, 30)];
        let all: Vec<ImageRecord> = x.iter().chain(y.iter()).cloned().collect();
        seed_images(&mut conn, &all);
        regenerate(&mut conn, &all, &thresholds()).unwrap();

        // Confirm event id 2 really is X's Multi-hour event before relying on that id below.
        let x_multi_hour_photos = event_images(&conn, 2).unwrap();
        assert_eq!(x_multi_hour_photos.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), vec!["x-a", "x-b"]);

        let events = list_events(&conn).unwrap();
        let position_of = |id: i64| events.iter().position(|e| e.id == id).unwrap();
        assert!(position_of(2) < position_of(1), "X's earlier photos should list before Y's Session event despite the higher id");
    }

    #[test]
    fn event_images_returns_only_that_events_photos_in_chronological_order() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("b", 10, 30), image_at("a", 10, 0), image_at("lonely", 3, 0)];
        seed_images(&mut conn, &images);
        regenerate(&mut conn, &images, &thresholds()).unwrap();

        let session_event = list_events(&conn).unwrap().into_iter().find(|e| e.event_type == EventType::Session).unwrap();
        let photos = event_images(&conn, session_event.id).unwrap();
        assert_eq!(photos.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn get_event_and_update_event_round_trip_title_and_notes() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30)];
        seed_images(&mut conn, &images);
        regenerate(&mut conn, &images, &thresholds()).unwrap();
        let event_id = list_events(&conn).unwrap()[0].id;

        update_event(&conn, event_id, "Birthday Party", "Great weather").unwrap();

        let event = get_event(&conn, event_id).unwrap().unwrap();
        assert_eq!(event.title, "Birthday Party");
        assert_eq!(event.notes, "Great weather");
    }

    #[test]
    fn get_event_returns_none_for_an_unknown_id() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        assert!(get_event(&conn, 999).unwrap().is_none());
    }

    /// Builds an `ExistingEvent` directly (no database needed) — `plan` is pure, so its tests
    /// don't need to seed a real `events` table first.
    fn existing_event(id: i64, event_type: EventType, title: &str, keys: &[&str]) -> ExistingEvent {
        ExistingEvent { id, event_type, title: title.to_string(), notes: String::new(), image_keys: keys.iter().map(|k| k.to_string()).collect() }
    }

    #[test]
    fn plan_leaves_an_unchanged_group_completely_alone() {
        let existing = vec![existing_event(1, EventType::Session, "2", &["a", "b"])];
        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30)];

        let plan = plan(existing, &images, &thresholds());

        // Only the Session tier reproduces {a, b} unchanged; Multi-hour reproduces it too but
        // as a brand new event, since no existing Multi-hour event was seeded.
        assert!(plan.extends.is_empty());
        assert!(plan.removes.is_empty());
        assert!(plan.discarded_edited_titles.is_empty());
        assert_eq!(plan.inserts.len(), 1, "the Multi-hour tier still needs a new event");
    }

    #[test]
    fn plan_extends_a_group_that_gained_a_photo_and_updates_its_default_title() {
        let existing = vec![existing_event(1, EventType::Session, "2", &["a", "b"])];
        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30), image_at("c", 10, 45)];

        let plan = plan(existing, &images, &thresholds());

        let extend = plan.extends.iter().find(|(id, ..)| *id == 1).expect("event 1 should extend");
        assert_eq!(extend.1, BTreeSet::from(["a".to_string(), "b".to_string(), "c".to_string()]));
        assert_eq!(extend.2, Some("3".to_string()));
        assert!(plan.discarded_edited_titles.is_empty());
    }

    #[test]
    fn plan_extending_a_group_with_an_edited_title_preserves_it() {
        let existing = vec![existing_event(1, EventType::Session, "Birthday Party", &["a", "b"])];
        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30), image_at("c", 10, 45)];

        let plan = plan(existing, &images, &thresholds());

        let extend = plan.extends.iter().find(|(id, ..)| *id == 1).expect("event 1 should extend");
        assert_eq!(extend.2, None, "an edited title should never be overwritten");
    }

    #[test]
    fn plan_merges_two_bridged_events_keeping_the_lower_id_and_reporting_the_others_edited_title() {
        let existing = vec![
            existing_event(1, EventType::Session, "2", &["a", "b"]),
            existing_event(5, EventType::Session, "Vacation", &["d", "e"]),
        ];
        // c bridges a/b (10:00-10:30) and d/e (11:00-11:30) into one Session group.
        let images = vec![
            image_at("a", 10, 0),
            image_at("b", 10, 30),
            image_at("c", 10, 45),
            image_at("d", 11, 0),
            image_at("e", 11, 30),
        ];

        let plan = plan(existing, &images, &thresholds());

        let extend = plan.extends.iter().find(|(id, ..)| *id == 1).expect("event 1 should survive as the merge target");
        assert_eq!(extend.1, ["a", "b", "c", "d", "e"].map(String::from).into_iter().collect::<BTreeSet<_>>());
        assert_eq!(plan.removes, vec![5]);
        assert_eq!(plan.discarded_edited_titles, vec!["Vacation".to_string()]);
    }

    #[test]
    fn plan_deletes_an_orphaned_event_no_tier_reproduces_and_reports_its_edited_title() {
        // "a" and "b" used to cluster together, but corrected_date_taken has since moved them
        // 8 hours apart — no tier reclusters them, so the old event is a pure orphan now.
        let existing = vec![existing_event(1, EventType::Session, "Renamed", &["a", "b"])];
        let images = vec![image_at("a", 0, 0), image_at("b", 8, 0)];

        let plan = plan(existing, &images, &thresholds());

        assert_eq!(plan.removes, vec![1]);
        assert_eq!(plan.discarded_edited_titles, vec!["Renamed".to_string()]);
    }

    #[test]
    fn regenerate_via_plan_and_apply_matches_the_original_wipe_and_rebuild_behavior() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30)];
        seed_images(&mut conn, &images);
        let first = regenerate(&mut conn, &images, &thresholds()).unwrap();
        assert_eq!(first, MergeSummary { inserted: 2, extended: 0, removed: 0, discarded_edited_titles: vec![] });

        // Regenerating again with the same images changes nothing — every group is unchanged.
        let second = regenerate(&mut conn, &images, &thresholds()).unwrap();
        assert_eq!(second, MergeSummary { inserted: 0, extended: 0, removed: 0, discarded_edited_titles: vec![] });
    }
}
