CREATE TABLE collections (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL
);

CREATE TABLE collection_images (
    collection_id INTEGER NOT NULL REFERENCES collections(id),
    image_key     TEXT NOT NULL REFERENCES images(key),
    PRIMARY KEY (collection_id, image_key)
);
CREATE INDEX idx_collection_images_image_key ON collection_images(image_key);
