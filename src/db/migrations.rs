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
            // M::up(include_str!("migrations/0007_....sql")),  <- next migration goes here
        ])
    })
}

pub fn apply(conn: &mut rusqlite::Connection) -> Result<(), rusqlite_migration::Error> {
    migrations().to_latest(conn)
}
