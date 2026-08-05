use crate::db::models::DirectoryRecord;

/// The fixed, ordered set of extensions every directory node in the Left
/// Navigation tree shows a count for, matching the File Types checkboxes.
const TYPES: [&str; 3] = ["jpg", "cr2", "gif"];

/// One top-level directory node in the Left Navigation tree, with its per-type
/// image counts in a fixed `TYPES` order (0 where a type has no images).
#[derive(Debug, Clone, PartialEq)]
pub struct NavDirNode {
    pub dir_name: String,
    pub type_counts: Vec<(&'static str, i64)>,
}

/// Builds one `NavDirNode` per row in `dirs`, pulling each of its `TYPES` counts
/// out of `counts` (as returned by `ProjectDb::directory_type_counts`). Pure and
/// GUI/DB-free so the "one node per directory, fixed three-type counts" shape is
/// unit-testable on its own.
pub fn build(dirs: &[DirectoryRecord], counts: &[(Option<String>, String, i64)]) -> Vec<NavDirNode> {
    dirs.iter()
        .map(|dir| {
            let type_counts = TYPES
                .iter()
                .map(|&ext| {
                    let count = counts
                        .iter()
                        .find(|(dir_name, image_type, _)| {
                            dir_name.as_deref() == Some(dir.dir_name.as_str()) && image_type == ext
                        })
                        .map(|(_, _, count)| *count)
                        .unwrap_or(0);
                    (ext, count)
                })
                .collect();
            NavDirNode { dir_name: dir.dir_name.clone(), type_counts }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> DirectoryRecord {
        DirectoryRecord { dir_name: name.to_string(), author: String::new(), camera_type: String::new() }
    }

    #[test]
    fn no_directories_produces_no_nodes() {
        assert_eq!(build(&[], &[]), vec![]);
    }

    #[test]
    fn directory_with_no_images_shows_all_three_types_at_zero() {
        let nodes = build(&[dir("sub")], &[]);
        assert_eq!(nodes, vec![NavDirNode { dir_name: "sub".to_string(), type_counts: vec![("jpg", 0), ("cr2", 0), ("gif", 0)] }]);
    }

    #[test]
    fn counts_land_on_the_matching_type_in_fixed_order() {
        let counts = vec![
            (Some("sub".to_string()), "gif".to_string(), 3),
            (Some("sub".to_string()), "jpg".to_string(), 12),
        ];
        let nodes = build(&[dir("sub")], &counts);
        assert_eq!(nodes[0].type_counts, vec![("jpg", 12), ("cr2", 0), ("gif", 3)]);
    }

    #[test]
    fn counts_do_not_leak_across_directories_or_from_root_level_files() {
        let counts = vec![
            (None, "jpg".to_string(), 5),                        // root-level files, no directory
            (Some("other".to_string()), "jpg".to_string(), 7),    // a different directory
        ];
        let nodes = build(&[dir("sub")], &counts);
        assert_eq!(nodes[0].type_counts, vec![("jpg", 0), ("cr2", 0), ("gif", 0)]);
    }
}
