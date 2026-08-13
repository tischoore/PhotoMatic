use std::sync::OnceLock;

use rusqlite_migration::{Migrations, M};

fn migrations() -> &'static Migrations<'static> {
    static MIGRATIONS: OnceLock<Migrations> = OnceLock::new();
    MIGRATIONS.get_or_init(|| {
        Migrations::new(vec![
            M::up(include_str!("migrations/0001_initial.sql")),
            M::up(include_str!("migrations/0002_add_image_toplevel_dir.sql")),
            M::up(include_str!("migrations/0003_add_image_exif_metadata.sql")),
            M::up(include_str!("migrations/0004_add_events.sql")),
            M::up(include_str!("migrations/0005_add_image_gps.sql")),
            M::up(include_str!("migrations/0006_reset_metadata_read_at_for_gps_backfill.sql")),
            M::up(include_str!("migrations/0007_add_event_title.sql")),
            M::up(include_str!("migrations/0008_add_image_linked_key.sql")),
            M::up(include_str!("migrations/0009_add_collections.sql")),
            M::up(include_str!("migrations/0010_add_collection_shortcut.sql")),
            M::up(include_str!("migrations/0011_add_image_rotation.sql")),
            // M::up(include_str!("migrations/0012_....sql")),  <- next migration goes here
        ])
    })
}

pub fn apply(conn: &mut rusqlite::Connection) -> Result<(), rusqlite_migration::Error> {
    migrations().to_latest(conn)
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    /// Migrations through 0009 only — a real database that already existed before the
    /// `shortcut` column (migration 0010) was added, i.e. one that's sitting at schema
    /// version 9 with a `collections` row (the default collection) that predates
    /// `shortcut` entirely. This is not hypothetical: a live project database was found in
    /// exactly this state (`user_version = 9`, no `shortcut` column) after `shortcut` was
    /// first added by editing migration 0009 in place instead of via a new migration —
    /// `rusqlite_migration` tracks progress by `PRAGMA user_version`, so a database already
    /// at 9 never re-runs 0009's (changed) SQL. These tests guard the real upgrade path.
    fn migrations_through_0009() -> Migrations<'static> {
        Migrations::new(vec![
            M::up(include_str!("migrations/0001_initial.sql")),
            M::up(include_str!("migrations/0002_add_image_toplevel_dir.sql")),
            M::up(include_str!("migrations/0003_add_image_exif_metadata.sql")),
            M::up(include_str!("migrations/0004_add_events.sql")),
            M::up(include_str!("migrations/0005_add_image_gps.sql")),
            M::up(include_str!("migrations/0006_reset_metadata_read_at_for_gps_backfill.sql")),
            M::up(include_str!("migrations/0007_add_event_title.sql")),
            M::up(include_str!("migrations/0008_add_image_linked_key.sql")),
            M::up(include_str!("migrations/0009_add_collections.sql")),
        ])
    }

    #[test]
    fn upgrading_a_pre_shortcut_database_backfills_the_default_collections_shortcut_to_d() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations_through_0009().to_latest(&mut conn).unwrap();
        conn.execute("INSERT INTO collections (name, description) VALUES ('MyTrip', 'desc')", []).unwrap();

        apply(&mut conn).unwrap();

        let shortcut: String =
            conn.query_row("SELECT shortcut FROM collections WHERE name = 'MyTrip'", [], |row| row.get(0)).unwrap();
        assert_eq!(shortcut, "d");
    }

    #[test]
    fn upgrading_a_pre_shortcut_database_with_no_collections_yet_is_a_harmless_noop() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations_through_0009().to_latest(&mut conn).unwrap();

        apply(&mut conn).unwrap();

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM collections", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn the_shortcut_column_is_unique_after_migrating_a_fresh_database() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply(&mut conn).unwrap();
        conn.execute("INSERT INTO collections (name, description, shortcut) VALUES ('A', '', 'x')", []).unwrap();

        let result = conn.execute("INSERT INTO collections (name, description, shortcut) VALUES ('B', '', 'x')", []);

        assert!(result.is_err());
    }
}
