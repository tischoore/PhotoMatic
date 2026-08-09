use std::collections::HashMap;

use crate::db::models::ImageRecord;

/// The RAW file extension this app currently recognizes for pairing. `images.image_type`
/// stores extensions lowercase without a leading dot (see `db::images::upsert_images`).
const RAW_TYPE: &str = "cr2";

/// Pairs each RAW image in `images` with its compressed sibling: same directory and filename
/// stem (case-insensitive), matched strictly 1:1. Returns `(raw_key, compressed_key)` for
/// every unambiguous pairing found.
///
/// A directory+stem group pairs only when it holds exactly one RAW image and exactly one
/// non-RAW image — any other shape (two RAWs sharing a stem, two non-RAW candidates sharing a
/// stem, or a RAW with no sibling at all) is left out rather than guessed at. "Compressed
/// sibling" means any non-RAW type, not just `jpg`: the ambiguity rule above already protects
/// against a `.jpg` and a `.gif` sharing a stem being paired at random.
pub fn pair_raw_with_compressed(images: &[ImageRecord]) -> Vec<(String, String)> {
    let mut groups: HashMap<(String, String), Vec<&ImageRecord>> = HashMap::new();
    for image in images {
        let (dir, stem) = split_dir_and_stem(&image.path);
        groups.entry((dir.to_lowercase(), stem.to_lowercase())).or_default().push(image);
    }

    groups
        .values()
        .filter_map(|group| {
            let raws: Vec<&&ImageRecord> = group.iter().filter(|i| i.image_type == RAW_TYPE).collect();
            let compressed: Vec<&&ImageRecord> = group.iter().filter(|i| i.image_type != RAW_TYPE).collect();
            match (raws.as_slice(), compressed.as_slice()) {
                ([raw], [comp]) => Some((raw.key.clone(), comp.key.clone())),
                _ => None,
            }
        })
        .collect()
}

/// Splits a normalized POSIX-style `images.path` into its directory (empty string for a
/// root-level file) and filename stem (the filename without its final extension).
fn split_dir_and_stem(path: &str) -> (&str, &str) {
    let (dir, filename) = match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    };
    let stem = filename.rfind('.').map_or(filename, |i| &filename[..i]);
    (dir, stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(key: &str, path: &str, image_type: &str) -> ImageRecord {
        ImageRecord { key: key.to_string(), path: path.to_string(), image_type: image_type.to_string(), ..ImageRecord::default() }
    }

    #[test]
    fn pairs_a_raw_with_its_single_compressed_sibling_in_the_same_directory() {
        let images = vec![image("raw", "50D/IMG_0001.cr2", "cr2"), image("jpg", "50D/IMG_0001.jpg", "jpg")];

        let pairs = pair_raw_with_compressed(&images);

        assert_eq!(pairs, vec![("raw".to_string(), "jpg".to_string())]);
    }

    #[test]
    fn matches_stems_case_insensitively() {
        let images = vec![image("raw", "50D/IMG_0001.CR2", "cr2"), image("jpg", "50D/img_0001.JPG", "jpg")];

        let pairs = pair_raw_with_compressed(&images);

        assert_eq!(pairs, vec![("raw".to_string(), "jpg".to_string())]);
    }

    #[test]
    fn does_not_pair_across_different_directories() {
        let images = vec![image("raw", "50D/IMG_0001.cr2", "cr2"), image("jpg", "other/IMG_0001.jpg", "jpg")];

        assert!(pair_raw_with_compressed(&images).is_empty());
    }

    #[test]
    fn leaves_unpaired_when_two_non_raw_candidates_share_a_stem() {
        let images = vec![
            image("raw", "50D/IMG_0001.cr2", "cr2"),
            image("jpg", "50D/IMG_0001.jpg", "jpg"),
            image("gif", "50D/IMG_0001.gif", "gif"),
        ];

        assert!(pair_raw_with_compressed(&images).is_empty());
    }

    #[test]
    fn leaves_unpaired_when_two_raw_candidates_share_a_stem() {
        // Two RAW rows landing on the very same path can't happen from a real scan (`path` is
        // unique in the database), but the pure pairing function only sees directory+stem, so
        // this still exercises "more than one RAW candidate in a group" defensively.
        let images = vec![
            image("raw-a", "50D/IMG_0001.cr2", "cr2"),
            image("raw-b", "50D/IMG_0001.cr2", "cr2"),
            image("jpg", "50D/IMG_0001.jpg", "jpg"),
        ];

        assert!(pair_raw_with_compressed(&images).is_empty());
    }

    #[test]
    fn ignores_images_with_no_sibling_at_all() {
        let images = vec![image("raw", "50D/IMG_0001.cr2", "cr2"), image("jpg", "50D/IMG_0002.jpg", "jpg")];

        assert!(pair_raw_with_compressed(&images).is_empty());
    }
}
