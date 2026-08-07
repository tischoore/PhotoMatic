use chrono::Duration;
use rusqlite::Connection;

use super::models::{EventType, ImageRecord};
use super::DbError;
use crate::events::cluster_by_time_gap;

/// The per-tier gap thresholds a "Generate Events" run clusters with.
pub struct EventThresholds {
    pub burst: Duration,
    pub session: Duration,
    pub multi_hour: Duration,
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
        let mut insert_event =
            tx.prepare("INSERT INTO events (event_type, notes) VALUES (?1, '')").map_err(DbError::Sqlite)?;
        let mut insert_link = tx
            .prepare("INSERT INTO event_images (event_id, image_key) VALUES (?1, ?2)")
            .map_err(DbError::Sqlite)?;

        for (event_type, gap) in [
            (EventType::TightBurst, thresholds.burst),
            (EventType::Session, thresholds.session),
            (EventType::MultiHour, thresholds.multi_hour),
        ] {
            for group in cluster_by_time_gap(images, gap) {
                if group.len() < 2 {
                    continue;
                }
                insert_event.execute(rusqlite::params![event_type.to_string()]).map_err(DbError::Sqlite)?;
                let event_id = tx.last_insert_rowid();
                for image in &group {
                    insert_link.execute(rusqlite::params![event_id, image.key]).map_err(DbError::Sqlite)?;
                }
            }
        }
    }

    tx.commit().map_err(DbError::Sqlite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn image_at(key: &str, h: u32, min: u32) -> ImageRecord {
        ImageRecord {
            key: key.to_string(),
            path: format!("{key}.jpg"),
            image_type: "jpg".to_string(),
            date_taken: NaiveDate::from_ymd_opt(2026, 7, 15).unwrap().and_hms_opt(h, min, 0),
            ..ImageRecord::default()
        }
    }

    fn thresholds() -> EventThresholds {
        EventThresholds { burst: Duration::seconds(10), session: Duration::minutes(60), multi_hour: Duration::hours(8) }
    }

    /// `event_images.image_key` has a foreign key into `images(key)`, so every image
    /// clustered in a test must exist there first — mirrors how `regenerate` is always
    /// called with images that already came from `db.list_images()` in production.
    fn seed_images(conn: &mut Connection, images: &[ImageRecord]) {
        crate::db::images::upsert_images(conn, images).unwrap();
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
        let a = ImageRecord {
            date_taken: NaiveDate::from_ymd_opt(2026, 7, 15).unwrap().and_hms_opt(10, 0, 0),
            ..image_at("a", 10, 0)
        };
        let b = ImageRecord {
            date_taken: NaiveDate::from_ymd_opt(2026, 7, 15).unwrap().and_hms_opt(10, 0, 5),
            ..image_at("b", 10, 0)
        };
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
}
