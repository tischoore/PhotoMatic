-- Plain INTEGER PRIMARY KEY (not AUTOINCREMENT): ids are free to be reused after a
-- "Generate Events" regeneration clears the table, since event_images is always
-- regenerated together with it in the same transaction and nothing else references
-- an events.id across regenerations.
CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    event_type TEXT NOT NULL,      -- 'Tight Burst' | 'Session' | 'Multi-hour'
    notes TEXT NOT NULL DEFAULT '' -- reserved for a later manual-editing UI
);

CREATE TABLE event_images (
    event_id INTEGER NOT NULL REFERENCES events(id),
    image_key TEXT NOT NULL REFERENCES images(key),
    PRIMARY KEY (event_id, image_key)
);
CREATE INDEX idx_event_images_image_key ON event_images(image_key);
