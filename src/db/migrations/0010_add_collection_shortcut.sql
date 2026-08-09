ALTER TABLE collections ADD COLUMN shortcut TEXT NOT NULL DEFAULT '';

UPDATE collections SET shortcut = 'd' WHERE id = (SELECT MIN(id) FROM collections) AND shortcut = '';

CREATE UNIQUE INDEX idx_collections_shortcut ON collections(shortcut);
