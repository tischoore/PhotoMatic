ALTER TABLE images ADD COLUMN toplevel_dir TEXT REFERENCES directories(dir_name);
