use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension};

use super::images::{map_image_row, IMAGE_COLUMNS};
use super::models::{CollectionRecord, ImageRecord};
use super::DbError;

/// Every project's default collection carries this fixed description.
pub const DEFAULT_COLLECTION_DESCRIPTION: &str = "Default collection containing all images within the project";

/// The default collection's shortcut when it's first created — never reapplied on a later
/// resync, so a user free to change it afterwards without it silently reverting. See
/// `ensure_default_collection`.
pub const DEFAULT_COLLECTION_SHORTCUT: &str = "d";

/// Finds the default collection — identified structurally as the lowest-id row in
/// `collections`, since nothing else in this feature creates a `collections` row —
/// creating it if absent, and re-syncing its name/description either way (so a later
/// project rename doesn't leave a stale-named duplicate behind). Returns its id.
fn ensure_default_collection(conn: &Connection, name: &str, description: &str) -> Result<i64, DbError> {
    let existing: Option<i64> = conn
        .query_row("SELECT id FROM collections ORDER BY id LIMIT 1", [], |row| row.get(0))
        .optional()
        .map_err(DbError::Sqlite)?;

    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE collections SET name = ?2, description = ?3 WHERE id = ?1",
                rusqlite::params![id, name, description],
            )
            .map_err(DbError::Sqlite)?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO collections (name, description, shortcut) VALUES (?1, ?2, ?3)",
                rusqlite::params![name, description, DEFAULT_COLLECTION_SHORTCUT],
            )
            .map_err(DbError::Sqlite)?;
            Ok(conn.last_insert_rowid())
        }
    }
}

/// Whether a default collection has ever been created — backs the Generate/Update
/// MetaData button's label.
pub fn default_collection_exists(conn: &Connection) -> Result<bool, DbError> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM collections)", [], |row| row.get(0)).map_err(DbError::Sqlite)
}

/// Ensures the default collection exists (creating or renaming it via
/// `ensure_default_collection`), then links every current `images` row into it —
/// `INSERT OR IGNORE` makes this idempotent, so re-running after a rescan only adds
/// links for the images that are new since the last run. One transaction, so a
/// failure never leaves the collection created without its links (or vice versa).
/// Backs the Generate MetaData / Update MetaData button.
pub fn sync_default_collection(conn: &mut Connection, name: &str, description: &str) -> Result<i64, DbError> {
    let tx = conn.transaction().map_err(DbError::Sqlite)?;
    let collection_id = ensure_default_collection(&tx, name, description)?;
    tx.execute(
        "INSERT OR IGNORE INTO collection_images (collection_id, image_key) SELECT ?1, key FROM images",
        rusqlite::params![collection_id],
    )
    .map_err(DbError::Sqlite)?;
    tx.commit().map_err(DbError::Sqlite)?;
    Ok(collection_id)
}

/// Creates a new, user-defined collection — backs the "Add Collection..." dialog's Accept.
/// Returns its new id.
pub fn create_collection(conn: &Connection, name: &str, description: &str, shortcut: &str) -> Result<i64, DbError> {
    conn.execute(
        "INSERT INTO collections (name, description, shortcut) VALUES (?1, ?2, ?3)",
        rusqlite::params![name, description, shortcut],
    )
    .map_err(DbError::Sqlite)?;
    Ok(conn.last_insert_rowid())
}

/// Overwrites an existing collection's name/description/shortcut — backs the "Edit
/// Collection..." dialog's Accept.
pub fn update_collection(conn: &Connection, id: i64, name: &str, description: &str, shortcut: &str) -> Result<(), DbError> {
    conn.execute(
        "UPDATE collections SET name = ?2, description = ?3, shortcut = ?4 WHERE id = ?1",
        rusqlite::params![id, name, description, shortcut],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Deletes a collection and every `collection_images` row that referenced it, in one
/// transaction — backs the tree view's Delete context menu item.
pub fn delete_collection(conn: &mut Connection, id: i64) -> Result<(), DbError> {
    let tx = conn.transaction().map_err(DbError::Sqlite)?;
    tx.execute("DELETE FROM collection_images WHERE collection_id = ?1", rusqlite::params![id]).map_err(DbError::Sqlite)?;
    tx.execute("DELETE FROM collections WHERE id = ?1", rusqlite::params![id]).map_err(DbError::Sqlite)?;
    tx.commit().map_err(DbError::Sqlite)
}

/// Every collection, ordered by id ascending — backs the Left Navigation tree's Collections
/// node and the Image Viewer's per-collection toggle buttons.
pub fn list_collections(conn: &Connection) -> Result<Vec<CollectionRecord>, DbError> {
    let mut stmt = conn
        .prepare("SELECT id, name, description, shortcut FROM collections ORDER BY id ASC")
        .map_err(DbError::Sqlite)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CollectionRecord { id: row.get(0)?, name: row.get(1)?, description: row.get(2)?, shortcut: row.get(3)? })
        })
        .map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

/// A single collection by id, for prefilling the "Edit Collection..." dialog.
pub fn get_collection(conn: &Connection, id: i64) -> Result<Option<CollectionRecord>, DbError> {
    conn.query_row("SELECT id, name, description, shortcut FROM collections WHERE id = ?1", rusqlite::params![id], |row| {
        Ok(CollectionRecord { id: row.get(0)?, name: row.get(1)?, description: row.get(2)?, shortcut: row.get(3)? })
    })
    .optional()
    .map_err(DbError::Sqlite)
}

/// The id of the default collection — structurally, whichever `collections` row currently
/// has the lowest id (see `ensure_default_collection`) — or `None` before one has ever been
/// created. Lets the GUI grey out Delete for it, since deleting it would silently hand
/// "default" status to a different collection on the next Generate/Update MetaData run.
pub fn default_collection_id(conn: &Connection) -> Result<Option<i64>, DbError> {
    conn.query_row("SELECT id FROM collections ORDER BY id LIMIT 1", [], |row| row.get(0)).optional().map_err(DbError::Sqlite)
}

/// The photos belonging to one collection, ordered chronologically — backs the collection's
/// View context menu item.
pub fn collection_images(conn: &Connection, collection_id: i64) -> Result<Vec<ImageRecord>, DbError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {IMAGE_COLUMNS} FROM images \
             JOIN collection_images ci ON ci.image_key = images.key \
             WHERE ci.collection_id = ?1 ORDER BY images.date_taken"
        ))
        .map_err(DbError::Sqlite)?;
    let rows = stmt.query_map(rusqlite::params![collection_id], map_image_row).map_err(DbError::Sqlite)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::Sqlite)
}

/// How many photos belong to a collection — backs the "inactive if no images exist yet"
/// rule on the View context menu item.
pub fn collection_image_count(conn: &Connection, collection_id: i64) -> Result<i64, DbError> {
    conn.query_row(
        "SELECT COUNT(*) FROM collection_images WHERE collection_id = ?1",
        rusqlite::params![collection_id],
        |row| row.get(0),
    )
    .map_err(DbError::Sqlite)
}

/// Every collection each of `keys` currently belongs to, batched into one query — preloads
/// the Image Viewer's per-collection toggle button state the same way
/// `images::list_images_by_keys` batches the RAW/compressed counterpart lookup. Returns an
/// empty map without touching the database when `keys` is empty.
pub fn collections_for_images(conn: &Connection, keys: &[String]) -> Result<HashMap<String, HashSet<i64>>, DbError> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let placeholders = vec!["?"; keys.len()].join(",");
    let mut stmt = conn
        .prepare(&format!("SELECT image_key, collection_id FROM collection_images WHERE image_key IN ({placeholders})"))
        .map_err(DbError::Sqlite)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(keys), |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))
        .map_err(DbError::Sqlite)?;

    let mut membership: HashMap<String, HashSet<i64>> = HashMap::new();
    for row in rows {
        let (image_key, collection_id) = row.map_err(DbError::Sqlite)?;
        membership.entry(image_key).or_default().insert(collection_id);
    }
    Ok(membership)
}

/// Adds one image to a collection — the Image Viewer's per-collection toggle button turning
/// on. `INSERT OR IGNORE` since the button's own in-memory state already prevents a
/// double-add, but this keeps a stale click harmless either way.
pub fn add_image_to_collection(conn: &Connection, collection_id: i64, image_key: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR IGNORE INTO collection_images (collection_id, image_key) VALUES (?1, ?2)",
        rusqlite::params![collection_id, image_key],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Removes one image from a collection — the Image Viewer's per-collection toggle button
/// turning off.
pub fn remove_image_from_collection(conn: &Connection, collection_id: i64, image_key: &str) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM collection_images WHERE collection_id = ?1 AND image_key = ?2",
        rusqlite::params![collection_id, image_key],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Removes one image from every collection it belongs to, default collection included — backs
/// the Image Viewer's Delete button. Unlike `remove_linked_raw_images_from_collections`, this
/// targets one explicit image key rather than the RAW/linked-key rule, and isn't restricted to
/// RAW images. The `images` row itself is left untouched, so a later Generate/Update MetaData
/// run's `INSERT OR IGNORE` (`sync_default_collection`) naturally re-adds the image to the
/// default collection.
pub fn remove_image_from_all_collections(conn: &Connection, image_key: &str) -> Result<(), DbError> {
    conn.execute("DELETE FROM collection_images WHERE image_key = ?1", rusqlite::params![image_key])
        .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Removes every RAW image that currently has a linked compressed counterpart from every
/// collection it belongs to (not just the default one) — backs checking "Link RAW and
/// compressed images", so a RAW image whose compressed twin already represents it doesn't
/// also sit in a collection. Mirrors the exact `image_type = 'cr2' AND linked_key IS NOT
/// NULL` rule `images::list_event_eligible_images` already uses to exclude a linked RAW from
/// event generation. An unlinked RAW (no compressed sibling found) is left untouched, since
/// nothing else represents it in a collection.
pub fn remove_linked_raw_images_from_collections(conn: &Connection) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM collection_images WHERE image_key IN ( \
             SELECT key FROM images WHERE image_type = 'cr2' AND linked_key IS NOT NULL \
         )",
        [],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

/// Re-adds every image (RAW included) to the default collection — backs unchecking "Link RAW
/// and compressed images", the reverse of `remove_linked_raw_images_from_collections`. A
/// no-op if the default collection doesn't exist yet (Generate/Update MetaData hasn't run) —
/// nothing to restore into.
pub fn restore_all_images_to_default_collection(conn: &Connection) -> Result<(), DbError> {
    let Some(default_id) = default_collection_id(conn)? else { return Ok(()) };
    conn.execute(
        "INSERT OR IGNORE INTO collection_images (collection_id, image_key) SELECT ?1, key FROM images",
        rusqlite::params![default_id],
    )
    .map_err(DbError::Sqlite)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::images::upsert_images;
    use crate::db::models::ImageRecord;

    fn migrated_conn() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::apply(&mut conn).unwrap();
        conn
    }

    fn image(key: &str, path: &str) -> ImageRecord {
        ImageRecord { key: key.to_string(), path: path.to_string(), image_type: "jpg".to_string(), ..ImageRecord::default() }
    }

    fn cr2_image(key: &str, path: &str) -> ImageRecord {
        ImageRecord { key: key.to_string(), path: path.to_string(), image_type: "cr2".to_string(), ..ImageRecord::default() }
    }

    /// Sets `images.linked_key` directly — `upsert_images` alone only stores
    /// key/path/image_type/toplevel_dir, so this mirrors how `db::events::tests::seed_images`
    /// backfills `date_taken` for fields a real scan would only fill in later.
    fn link(conn: &Connection, key: &str, linked_key: &str) {
        conn.execute("UPDATE images SET linked_key = ?2 WHERE key = ?1", rusqlite::params![key, linked_key]).unwrap();
    }

    fn collection_image_keys(conn: &Connection, collection_id: i64) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT image_key FROM collection_images WHERE collection_id = ?1 ORDER BY image_key")
            .unwrap();
        stmt.query_map(rusqlite::params![collection_id], |row| row.get(0)).unwrap().map(|r| r.unwrap()).collect()
    }

    #[test]
    fn sync_default_collection_creates_the_collection_and_links_every_image() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg"), image("b", "b.jpg")]).unwrap();

        let collection_id = sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();

        let (name, description): (String, String) = conn
            .query_row("SELECT name, description FROM collections WHERE id = ?1", rusqlite::params![collection_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(name, "MyTrip");
        assert_eq!(description, DEFAULT_COLLECTION_DESCRIPTION);
        assert_eq!(collection_image_keys(&conn, collection_id), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn sync_default_collection_is_idempotent_and_picks_up_new_images_on_rescan() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg")]).unwrap();
        let first_id = sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();

        upsert_images(&mut conn, &[image("b", "b.jpg")]).unwrap();
        let second_id = sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();

        assert_eq!(first_id, second_id);
        assert_eq!(collection_image_keys(&conn, second_id), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn sync_default_collection_resyncs_name_on_rename() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg")]).unwrap();

        let first_id = sync_default_collection(&mut conn, "OldName", DEFAULT_COLLECTION_DESCRIPTION).unwrap();
        let second_id = sync_default_collection(&mut conn, "NewName", DEFAULT_COLLECTION_DESCRIPTION).unwrap();

        assert_eq!(first_id, second_id);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM collections", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
        let name: String =
            conn.query_row("SELECT name FROM collections WHERE id = ?1", rusqlite::params![second_id], |row| row.get(0)).unwrap();
        assert_eq!(name, "NewName");
    }

    #[test]
    fn default_collection_exists_is_false_until_first_sync() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg")]).unwrap();

        assert!(!default_collection_exists(&conn).unwrap());
        sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();
        assert!(default_collection_exists(&conn).unwrap());
    }

    #[test]
    fn ensure_default_collection_sets_shortcut_d_only_when_first_created() {
        let mut conn = migrated_conn();

        let id = sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();
        assert_eq!(get_collection(&conn, id).unwrap().unwrap().shortcut, "d");
    }

    #[test]
    fn resyncing_the_default_collection_does_not_reset_a_customized_shortcut() {
        let mut conn = migrated_conn();
        let id = sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();
        update_collection(&conn, id, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION, "x").unwrap();

        sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();

        assert_eq!(get_collection(&conn, id).unwrap().unwrap().shortcut, "x");
    }

    #[test]
    fn create_collection_round_trips_through_list_collections_ordered_by_id() {
        let conn = migrated_conn();

        let first = create_collection(&conn, "Trip A", "First", "a").unwrap();
        let second = create_collection(&conn, "Trip B", "Second", "b").unwrap();

        let collections = list_collections(&conn).unwrap();
        assert_eq!(
            collections,
            vec![
                CollectionRecord { id: first, name: "Trip A".to_string(), description: "First".to_string(), shortcut: "a".to_string() },
                CollectionRecord { id: second, name: "Trip B".to_string(), description: "Second".to_string(), shortcut: "b".to_string() },
            ]
        );
    }

    #[test]
    fn update_collection_overwrites_name_description_and_shortcut() {
        let conn = migrated_conn();
        let id = create_collection(&conn, "Trip A", "First", "a").unwrap();

        update_collection(&conn, id, "Renamed", "Updated", "z").unwrap();

        let record = get_collection(&conn, id).unwrap().unwrap();
        assert_eq!(record.name, "Renamed");
        assert_eq!(record.description, "Updated");
        assert_eq!(record.shortcut, "z");
    }

    #[test]
    fn delete_collection_removes_it_and_its_collection_images_rows() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg")]).unwrap();
        let id = create_collection(&conn, "Trip A", "First", "a").unwrap();
        add_image_to_collection(&conn, id, "a").unwrap();

        delete_collection(&mut conn, id).unwrap();

        assert!(get_collection(&conn, id).unwrap().is_none());
        let link_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM collection_images WHERE collection_id = ?1", rusqlite::params![id], |row| row.get(0)).unwrap();
        assert_eq!(link_count, 0);
    }

    #[test]
    fn default_collection_id_is_the_lowest_id_collection() {
        let conn = migrated_conn();
        let first = create_collection(&conn, "Trip A", "First", "a").unwrap();
        create_collection(&conn, "Trip B", "Second", "b").unwrap();

        assert_eq!(default_collection_id(&conn).unwrap(), Some(first));
    }

    #[test]
    fn default_collection_id_is_none_before_any_collection_exists() {
        let conn = migrated_conn();
        assert_eq!(default_collection_id(&conn).unwrap(), None);
    }

    #[test]
    fn collection_images_orders_by_date_taken() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg"), image("b", "b.jpg")]).unwrap();
        conn.execute("UPDATE images SET date_taken = '2026-01-02 00:00:00' WHERE key = 'a'", []).unwrap();
        conn.execute("UPDATE images SET date_taken = '2026-01-01 00:00:00' WHERE key = 'b'", []).unwrap();
        let id = create_collection(&conn, "Trip A", "First", "a").unwrap();
        add_image_to_collection(&conn, id, "a").unwrap();
        add_image_to_collection(&conn, id, "b").unwrap();

        let images = collection_images(&conn, id).unwrap();

        assert_eq!(images.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
    }

    #[test]
    fn collection_image_count_reflects_membership() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg")]).unwrap();
        let id = create_collection(&conn, "Trip A", "First", "a").unwrap();
        assert_eq!(collection_image_count(&conn, id).unwrap(), 0);

        add_image_to_collection(&conn, id, "a").unwrap();
        assert_eq!(collection_image_count(&conn, id).unwrap(), 1);
    }

    #[test]
    fn collections_for_images_batches_membership_by_image_key() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg"), image("b", "b.jpg")]).unwrap();
        let first = create_collection(&conn, "Trip A", "First", "a").unwrap();
        let second = create_collection(&conn, "Trip B", "Second", "b").unwrap();
        add_image_to_collection(&conn, first, "a").unwrap();
        add_image_to_collection(&conn, second, "a").unwrap();
        add_image_to_collection(&conn, second, "b").unwrap();

        let membership = collections_for_images(&conn, &["a".to_string(), "b".to_string()]).unwrap();

        assert_eq!(membership.get("a").unwrap(), &std::collections::HashSet::from([first, second]));
        assert_eq!(membership.get("b").unwrap(), &std::collections::HashSet::from([second]));
    }

    #[test]
    fn collections_for_images_is_empty_for_empty_input() {
        let conn = migrated_conn();
        assert!(collections_for_images(&conn, &[]).unwrap().is_empty());
    }

    #[test]
    fn add_and_remove_image_from_collection_round_trip() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg")]).unwrap();
        let id = create_collection(&conn, "Trip A", "First", "a").unwrap();

        add_image_to_collection(&conn, id, "a").unwrap();
        assert_eq!(collection_image_keys(&conn, id), vec!["a".to_string()]);

        remove_image_from_collection(&conn, id, "a").unwrap();
        assert!(collection_image_keys(&conn, id).is_empty());
    }

    #[test]
    fn remove_image_from_all_collections_removes_it_from_every_collection_including_default() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg")]).unwrap();
        let default_id = sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();
        let custom_id = create_collection(&conn, "Custom", "", "c").unwrap();
        add_image_to_collection(&conn, custom_id, "a").unwrap();

        remove_image_from_all_collections(&conn, "a").unwrap();

        assert!(collection_image_keys(&conn, default_id).is_empty());
        assert!(collection_image_keys(&conn, custom_id).is_empty());
    }

    #[test]
    fn remove_image_from_all_collections_leaves_other_images_untouched() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("a", "a.jpg"), image("b", "b.jpg")]).unwrap();
        let id = create_collection(&conn, "Trip A", "First", "a").unwrap();
        add_image_to_collection(&conn, id, "a").unwrap();
        add_image_to_collection(&conn, id, "b").unwrap();

        remove_image_from_all_collections(&conn, "a").unwrap();

        assert_eq!(collection_image_keys(&conn, id), vec!["b".to_string()]);
    }

    #[test]
    fn create_collection_rejects_a_duplicate_shortcut() {
        let conn = migrated_conn();
        create_collection(&conn, "Trip A", "First", "a").unwrap();

        assert!(create_collection(&conn, "Trip B", "Second", "a").is_err());
    }

    #[test]
    fn remove_linked_raw_images_from_collections_removes_a_linked_raw_from_every_collection_it_is_in() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[cr2_image("raw", "50D/a.cr2"), image("jpg", "50D/a.jpg")]).unwrap();
        link(&conn, "raw", "jpg");
        link(&conn, "jpg", "raw");
        let default_id = sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();
        let custom_id = create_collection(&conn, "Custom", "", "c").unwrap();
        add_image_to_collection(&conn, custom_id, "raw").unwrap();

        remove_linked_raw_images_from_collections(&conn).unwrap();

        assert!(collection_image_keys(&conn, default_id) == vec!["jpg".to_string()]);
        assert!(collection_image_keys(&conn, custom_id).is_empty());
    }

    #[test]
    fn remove_linked_raw_images_from_collections_leaves_an_unlinked_raw_untouched() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[cr2_image("lonely-raw", "50D/b.cr2"), image("jpg", "50D/a.jpg")]).unwrap();
        let default_id = sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();

        remove_linked_raw_images_from_collections(&conn).unwrap();

        assert_eq!(collection_image_keys(&conn, default_id), vec!["jpg".to_string(), "lonely-raw".to_string()]);
    }

    #[test]
    fn restore_all_images_to_default_collection_re_adds_every_image_including_raw() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[cr2_image("raw", "50D/a.cr2"), image("jpg", "50D/a.jpg")]).unwrap();
        link(&conn, "raw", "jpg");
        link(&conn, "jpg", "raw");
        let default_id = sync_default_collection(&mut conn, "MyTrip", DEFAULT_COLLECTION_DESCRIPTION).unwrap();
        remove_linked_raw_images_from_collections(&conn).unwrap();
        assert_eq!(collection_image_keys(&conn, default_id), vec!["jpg".to_string()]);

        restore_all_images_to_default_collection(&conn).unwrap();

        assert_eq!(collection_image_keys(&conn, default_id), vec!["jpg".to_string(), "raw".to_string()]);
    }

    #[test]
    fn restore_all_images_to_default_collection_is_a_noop_when_no_default_collection_exists_yet() {
        let mut conn = migrated_conn();
        upsert_images(&mut conn, &[image("jpg", "50D/a.jpg")]).unwrap();

        restore_all_images_to_default_collection(&conn).unwrap();

        assert!(!default_collection_exists(&conn).unwrap());
    }
}
