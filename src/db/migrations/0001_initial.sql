CREATE TABLE project_settings (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    project_name TEXT NOT NULL DEFAULT '',
    date_begin   TEXT,                 -- ISO-8601 date (YYYY-MM-DD), nullable
    date_end     TEXT,                 -- ISO-8601 date (YYYY-MM-DD), nullable
    author       TEXT NOT NULL DEFAULT '',
    last_scan    TEXT                  -- RFC3339 datetime (UTC), nullable until first scan
);
INSERT INTO project_settings (id) VALUES (1);

CREATE TABLE images (
    key        TEXT PRIMARY KEY,       -- xxh3 hex hash of `path`
    path       TEXT NOT NULL UNIQUE,   -- relative to source_directory, POSIX ('/') separators
    image_type TEXT NOT NULL           -- lowercase extension without dot: jpg, cr2, gif
);

CREATE TABLE directories (
    dir_name    TEXT PRIMARY KEY,      -- relative path from source_directory, POSIX separators
    author      TEXT NOT NULL DEFAULT '',
    camera_type TEXT NOT NULL DEFAULT ''
);
