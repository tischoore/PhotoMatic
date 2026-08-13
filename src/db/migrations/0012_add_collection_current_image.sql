ALTER TABLE collections ADD COLUMN current_img TEXT REFERENCES images(key);
