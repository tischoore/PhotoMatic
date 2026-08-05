use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::project::FileExtensions;

/// A single file found during a scan, relative to the scan root.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedFile {
    pub relative_path: PathBuf,
    /// Lowercase, no leading dot: "jpg", "cr2", "gif".
    pub extension: String,
    /// Name of the top-level directory (one level below the scan root) this file
    /// lives under. `None` if the file sits directly in the scan root.
    pub toplevel_dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    pub files: Vec<ScannedFile>,
    /// Every top-level directory under the scan root (one level deep, root itself
    /// excluded) — not every nested subdirectory.
    pub dirs: Vec<PathBuf>,
    pub elapsed: Duration,
}

/// One unit of work discovered while walking the scan root: either the batch of
/// files sitting directly in the root, or one top-level directory together with
/// every matching file found anywhere beneath it. Fired via the `on_unit` callback
/// as each is discovered, so a caller (e.g. the database) can persist it while the
/// scan is still running rather than waiting for the whole tree to be walked.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanUnit {
    RootFiles(Vec<ScannedFile>),
    ToplevelDir { name: String, files: Vec<ScannedFile> },
}

/// Walks `root` one top-level entry at a time: files directly in `root` are
/// collected first and reported as a single `ScanUnit::RootFiles`, then each
/// top-level subdirectory is walked to completion (recursively, to any depth) and
/// reported as one `ScanUnit::ToplevelDir` before moving on to the next. Extensions
/// that are disabled are not collected at all, per the File Types setting.
/// Directories that can't be read (permissions, races) are silently skipped.
pub fn scan_directory(root: &Path, extensions: &FileExtensions, mut on_unit: impl FnMut(&ScanUnit)) -> ScanResult {
    let start = Instant::now();
    let mut files = Vec::new();
    let mut dirs = Vec::new();

    let Ok(entries) = std::fs::read_dir(root) else {
        return ScanResult { files, dirs, elapsed: start.elapsed() };
    };

    let mut root_files = Vec::new();
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
            continue;
        }
        if let Some(file) = scanned_file(root, &path, extensions, None) {
            root_files.push(file);
        }
    }

    if !root_files.is_empty() {
        files.extend(root_files.clone());
        on_unit(&ScanUnit::RootFiles(root_files));
    }

    for dir_path in subdirs {
        let Ok(relative) = dir_path.strip_prefix(root) else {
            continue;
        };
        let name = relative.to_string_lossy().into_owned();
        dirs.push(relative.to_path_buf());

        let mut nested = Vec::new();
        visit(root, &dir_path, extensions, &name, &mut nested);
        files.extend(nested.clone());
        on_unit(&ScanUnit::ToplevelDir { name, files: nested });
    }

    ScanResult { files, dirs, elapsed: start.elapsed() }
}

/// Recursively collects matching files under `dir`, to any depth, tagging each with
/// the top-level directory `toplevel` they were found under.
fn visit(root: &Path, dir: &Path, extensions: &FileExtensions, toplevel: &str, files: &mut Vec<ScannedFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(root, &path, extensions, toplevel, files);
            continue;
        }
        if let Some(file) = scanned_file(root, &path, extensions, Some(toplevel)) {
            files.push(file);
        }
    }
}

/// Builds a `ScannedFile` for `path` if its extension is enabled in `extensions`,
/// tagging it with `toplevel` (the top-level directory name, or `None` for files
/// directly in the scan root).
fn scanned_file(root: &Path, path: &Path, extensions: &FileExtensions, toplevel: Option<&str>) -> Option<ScannedFile> {
    let ext = path.extension().and_then(|e| e.to_str())?.to_lowercase();

    let enabled = match ext.as_str() {
        "jpg" => extensions.jpg,
        "cr2" => extensions.cr2,
        "gif" => extensions.gif,
        _ => false,
    };
    if !enabled {
        return None;
    }

    let relative = path.strip_prefix(root).ok()?;
    Some(ScannedFile {
        relative_path: relative.to_path_buf(),
        extension: ext,
        toplevel_dir: toplevel.map(|s| s.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("photomatic-test-scan-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn dirs_only_contains_toplevel_directories() {
        let root = temp_dir("toplevel-dirs-only");
        let sub = root.join("sub");
        let empty_sub = root.join("empty_sub");
        let nested = sub.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&empty_sub).unwrap();
        std::fs::write(nested.join("a.jpg"), b"").unwrap();

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: true }, |_| {});

        let mut dirs = result.dirs.clone();
        dirs.sort();
        let mut expected = vec![PathBuf::from("sub"), PathBuf::from("empty_sub")];
        expected.sort();
        assert_eq!(dirs, expected);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn extensions_are_lowercased_in_scanned_files() {
        let root = temp_dir("lowercase-extensions");
        std::fs::write(root.join("a.CR2"), b"").unwrap();

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: true }, |_| {});

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].extension, "cr2");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn files_are_tagged_with_their_toplevel_directory_or_none_for_root() {
        let root = temp_dir("toplevel-tagging");
        let sub = root.join("sub");
        let nested = sub.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("root.jpg"), b"").unwrap();
        std::fs::write(sub.join("direct.jpg"), b"").unwrap();
        std::fs::write(nested.join("deep.jpg"), b"").unwrap();

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: true }, |_| {});

        let root_file = result.files.iter().find(|f| f.relative_path == PathBuf::from("root.jpg")).unwrap();
        assert_eq!(root_file.toplevel_dir, None);

        let direct_file =
            result.files.iter().find(|f| f.relative_path == PathBuf::from("sub").join("direct.jpg")).unwrap();
        assert_eq!(direct_file.toplevel_dir, Some("sub".to_string()));

        let deep_file = result
            .files
            .iter()
            .find(|f| f.relative_path == PathBuf::from("sub").join("nested").join("deep.jpg"))
            .unwrap();
        assert_eq!(deep_file.toplevel_dir, Some("sub".to_string()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn on_unit_fires_once_per_root_file_batch_and_once_per_toplevel_directory() {
        let root = temp_dir("scan-units");
        let sub_a = root.join("sub_a");
        let sub_b = root.join("sub_b");
        std::fs::create_dir_all(&sub_a).unwrap();
        std::fs::create_dir_all(&sub_b).unwrap();
        std::fs::write(root.join("root.jpg"), b"").unwrap();
        std::fs::write(sub_a.join("a.jpg"), b"").unwrap();
        std::fs::write(sub_b.join("b.jpg"), b"").unwrap();

        let mut units = Vec::new();
        scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: true }, |unit| {
            units.push(unit.clone());
        });

        let root_units: Vec<_> = units
            .iter()
            .filter_map(|u| match u {
                ScanUnit::RootFiles(files) => Some(files.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(root_units.len(), 1);
        assert_eq!(root_units[0].len(), 1);
        assert_eq!(root_units[0][0].relative_path, PathBuf::from("root.jpg"));

        let mut toplevel_names: Vec<String> = units
            .iter()
            .filter_map(|u| match u {
                ScanUnit::ToplevelDir { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        toplevel_names.sort();
        assert_eq!(toplevel_names, vec!["sub_a".to_string(), "sub_b".to_string()]);

        for unit in &units {
            if let ScanUnit::ToplevelDir { name, files } = unit {
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].toplevel_dir, Some(name.clone()));
            }
        }

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn on_unit_does_not_fire_for_root_files_when_root_has_none() {
        let root = temp_dir("no-root-files");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("a.jpg"), b"").unwrap();

        let mut units = Vec::new();
        scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: true }, |unit| {
            units.push(unit.clone());
        });

        assert!(!units.iter().any(|u| matches!(u, ScanUnit::RootFiles(_))));

        std::fs::remove_dir_all(&root).ok();
    }

}
