ALTER TABLE images ADD COLUMN date_taken TEXT;         -- ISO-8601 datetime, nullable until metadata is generated
ALTER TABLE images ADD COLUMN width INTEGER;           -- pixels, nullable
ALTER TABLE images ADD COLUMN height INTEGER;          -- pixels, nullable
ALTER TABLE images ADD COLUMN exposure_time REAL;      -- seconds, nullable
ALTER TABLE images ADD COLUMN iso INTEGER;             -- nullable
ALTER TABLE images ADD COLUMN focal_length REAL;       -- millimeters, nullable
ALTER TABLE images ADD COLUMN metadata_read_at TEXT;   -- RFC3339 datetime metadata extraction was last attempted, nullable until Generate MetaData has processed this row
