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

/// Clears `event_images` and `events`, then re-clusters `images` at all three tiers
/// (Tight Burst, Session, Multi-hour) and writes the results back — all in one
/// transaction, so a failure never leaves a partially rebuilt table. A tier only
/// produces an `events` row for a group of 2 or more photos; a photo with no neighbor
/// within a tier's threshold simply gets no row for that tier.
pub fn regenerate(conn: &mut Connection, images: &[ImageRecord], thresholds: &EventThresholds) -> Result<(), DbError> {
    let tx = conn.transaction().map_err(DbError::Sqlite)?;
    tx.execute("DELETE FROM event_images", []).map_err(DbError::Sqlite)?;
    tx.execute("DELETE FROM events", []).map_err(DbError::Sqlite)?;

    {
        let mut insert_event = tx
            .prepare("INSERT INTO events (event_type, notes, title) VALUES (?1, '', ?2)")
            .map_err(DbError::Sqlite)?;
        let mut insert_link = tx
            .prepare("INSERT INTO event_images (event_id, image_key) VALUES (?1, ?2)")
            .map_err(DbError::Sqlite)?;

        for (event_type, gap, max_distance_km) in [
            (EventType::TightBurst, thresholds.burst, None),
            (EventType::Session, thresholds.session, Some(thresholds.session_max_distance_km)),
            (EventType::MultiHour, thresholds.multi_hour, Some(thresholds.multi_hour_max_distance_km)),
        ] {
            for group in cluster_by_time_gap(images, gap, max_distance_km) {
                if group.len() < 2 {
                    continue;
                }
                insert_event
                    .execute(rusqlite::params![event_type.to_string(), default_title(group.len())])
                    .map_err(DbError::Sqlite)?;
                let event_id = tx.last_insert_rowid();
                for image in &group {
                    insert_link.execute(rusqlite::params![event_id, image.key]).map_err(DbError::Sqlite)?;
                }
            }
        }
    }

    tx.commit().map_err(DbError::Sqlite)
}

/// Removes one image from every event it belongs to — backs the Image Viewer's Delete button.
/// Any `events` row left with zero remaining `event_images` rows is left as an orphan rather
/// than cleaned up here: `regenerate` is the only thing that rebuilds `events` from scratch,
/// and `list_events`'s inner join already excludes a zero-photo event from the views that
/// matter. The `images` row itself is left untouched, so a later Generate Events run
/// re-clusters the image if it's still eligible.
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

/// Whether any event currently holds a user edit — a title that no longer matches its
/// default (the photo count `regenerate` gave it) or non-empty notes. `regenerate`
/// unconditionally deletes and rebuilds every event, so this backs the confirmation
/// prompt Generate Events shows before it would silently discard those edits.
pub fn has_edited_events(conn: &Connection) -> Result<bool, DbError> {
    conn.query_row(
        "SELECT EXISTS ( \
             SELECT 1 FROM events e \
             JOIN (SELECT event_id, COUNT(*) c FROM event_images GROUP BY event_id) counts \
                 ON counts.event_id = e.id \
             WHERE e.notes != '' OR e.title != CAST(counts.c AS TEXT) \
         )",
        [],
        |row| row.get::<_, bool>(0),
    )
    .map_err(DbError::Sqlite)
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

    #[test]
    fn has_edited_events_is_false_right_after_a_regenerate() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30)];
        seed_images(&mut conn, &images);
        regenerate(&mut conn, &images, &thresholds()).unwrap();

        assert!(!has_edited_events(&conn).unwrap());
    }

    #[test]
    fn has_edited_events_is_true_once_a_title_or_notes_has_been_edited() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();

        let images = vec![image_at("a", 10, 0), image_at("b", 10, 30)];
        seed_images(&mut conn, &images);
        regenerate(&mut conn, &images, &thresholds()).unwrap();
        let event_id = list_events(&conn).unwrap()[0].id;

        update_event(&conn, event_id, "Birthday Party", "").unwrap();
        assert!(has_edited_events(&conn).unwrap());

        update_event(&conn, event_id, "2", "Great weather").unwrap();
        assert!(has_edited_events(&conn).unwrap());
    }
}
