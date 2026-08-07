-- GPS extraction (0005) was added after many databases already had every image's
-- metadata_read_at stamped from an earlier "Generate MetaData" run. Since that column
-- gates which rows list_images_pending_metadata() returns, those rows would otherwise
-- never be re-read and would never pick up GPS. Resetting it here requeues every image
-- for one more metadata pass.
UPDATE images SET metadata_read_at = NULL;
