use std::sync::OnceLock;

use rusqlite_migration::{Migrations, M};

fn migrations() -> &'static Migrations<'static> {
    static MIGRATIONS: OnceLock<Migrations> = OnceLock::new();
    MIGRATIONS.get_or_init(|| {
        Migrations::new(vec![
            M::up(include_str!("migrations/0001_initial.sql")),
            M::up(include_str!("migrations/0002_add_image_toplevel_dir.sql")),
            // M::up(include_str!("migrations/0003_....sql")),  <- next migration goes here
        ])
    })
}

pub fn apply(conn: &mut rusqlite::Connection) -> Result<(), rusqlite_migration::Error> {
    migrations().to_latest(conn)
}
