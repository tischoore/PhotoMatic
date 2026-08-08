-- The events-browsing UI's title field, editable per-event. Defaults to '' here so
-- existing rows (from a database that predates this column) fall back cleanly;
-- `regenerate()` always supplies a real default (the photo count) for new rows.
ALTER TABLE events ADD COLUMN title TEXT NOT NULL DEFAULT '';
