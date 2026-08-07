use crate::db::models::DirectoryRecord;
use crate::project::FileExtensions;

/// The fixed ordering of known extensions, each paired with the `FileExtensions`
/// flag that says whether it's currently selected for the project. A directory
/// node only gets a count child for the ones that are enabled.
const TYPES: [(&str, fn(&FileExtensions) -> bool); 3] =
    [("jpg", |e| e.jpg), ("cr2", |e| e.cr2), ("gif", |e| e.gif)];

/// One top-level directory node in the Left Navigation tree, with a per-type
/// image count (in `TYPES` order) for each currently-enabled File Type only —
/// a disabled type has no entry at all, not just a zero count.
#[derive(Debug, Clone, PartialEq)]
pub struct NavDirNode {
    pub dir_name: String,
    pub type_counts: Vec<(&'static str, i64)>,
}

/// Builds one `NavDirNode` per row in `dirs`, pulling each enabled `TYPES` count
/// out of `counts` (as returned by `ProjectDb::directory_type_counts`); types not
/// selected in `extensions` are omitted entirely. Pure and GUI/DB-free so this
/// shape is unit-testable on its own.
pub fn build(
    dirs: &[DirectoryRecord],
    counts: &[(Option<String>, String, i64)],
    extensions: &FileExtensions,
) -> Vec<NavDirNode> {
    dirs.iter()
        .map(|dir| {
            let type_counts = TYPES
                .iter()
                .filter(|(_, enabled)| enabled(extensions))
                .map(|&(ext, _)| {
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
        assert_eq!(build(&[], &[], &FileExtensions::default()), vec![]);
    }

    #[test]
    fn directory_with_no_images_shows_all_enabled_types_at_zero() {
        let nodes = build(&[dir("sub")], &[], &FileExtensions::default());
        assert_eq!(nodes, vec![NavDirNode { dir_name: "sub".to_string(), type_counts: vec![("jpg", 0), ("cr2", 0), ("gif", 0)] }]);
    }

    #[test]
    fn counts_land_on_the_matching_type_in_fixed_order() {
        let counts = vec![
            (Some("sub".to_string()), "gif".to_string(), 3),
            (Some("sub".to_string()), "jpg".to_string(), 12),
        ];
        let nodes = build(&[dir("sub")], &counts, &FileExtensions::default());
        assert_eq!(nodes[0].type_counts, vec![("jpg", 12), ("cr2", 0), ("gif", 3)]);
    }

    #[test]
    fn counts_do_not_leak_across_directories_or_from_root_level_files() {
        let counts = vec![
            (None, "jpg".to_string(), 5),                        // root-level files, no directory
            (Some("other".to_string()), "jpg".to_string(), 7),    // a different directory
        ];
        let nodes = build(&[dir("sub")], &counts, &FileExtensions::default());
        assert_eq!(nodes[0].type_counts, vec![("jpg", 0), ("cr2", 0), ("gif", 0)]);
    }

    #[test]
    fn disabled_type_is_omitted_even_when_it_has_counts() {
        let counts = vec![(Some("sub".to_string()), "gif".to_string(), 9)];
        let extensions = FileExtensions { jpg: true, cr2: true, gif: false };
        let nodes = build(&[dir("sub")], &counts, &extensions);
        assert_eq!(nodes[0].type_counts, vec![("jpg", 0), ("cr2", 0)]);
    }

    #[test]
    fn all_types_disabled_produces_empty_type_counts_but_keeps_the_directory() {
        let extensions = FileExtensions { jpg: false, cr2: false, gif: false };
        let nodes = build(&[dir("sub")], &[], &extensions);
        assert_eq!(nodes, vec![NavDirNode { dir_name: "sub".to_string(), type_counts: vec![] }]);
    }
}
