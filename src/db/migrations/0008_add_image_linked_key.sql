-- Self-referential, nullable link to another `images` row: when a RAW (`cr2`) image is
-- paired with its compressed sibling (same directory, same filename stem), both rows'
-- `linked_key` point at each other. NULL for every unlinked image. See `crate::raw_linking`
-- for the pairing algorithm and `db::images::relink_raw_images`/`clear_raw_links` for how
-- this column gets written.
ALTER TABLE images ADD COLUMN linked_key TEXT REFERENCES images(key);
