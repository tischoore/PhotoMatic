ALTER TABLE images ADD COLUMN corrected_date_taken TEXT; -- ISO-8601 datetime; seeded from date_taken by Generate MetaData, independently correctable later
UPDATE images SET corrected_date_taken = date_taken;
