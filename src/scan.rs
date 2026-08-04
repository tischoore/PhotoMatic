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

/// Per-extension file counts from a directory scan. Derived from `ScanResult::files`
/// via `counts()` — not stored independently, so there's exactly one source of truth.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScanCounts {
    pub jpg: u64,
    pub cr2: u64,
    pub gif: u64,
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

/// Tallies `files` by extension.
pub fn counts(files: &[ScannedFile]) -> ScanCounts {
    let mut counts = ScanCounts::default();
    for file in files {
        match file.extension.as_str() {
            "jpg" => counts.jpg += 1,
            "cr2" => counts.cr2 += 1,
            "gif" => counts.gif += 1,
            _ => {}
        }
    }
    counts
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

/// Builds the scan-log lines for a finished scan: one line per *enabled* extension
/// with its count, followed by one line reporting elapsed time. Disabled extensions
/// are omitted entirely, since they weren't scanned.
pub fn format_summary(result: &ScanResult, extensions: &FileExtensions) -> Vec<String> {
    let mut lines = Vec::new();
    let counts = counts(&result.files);

    if extensions.jpg {
        lines.push(format!("Found {} *.jpg file(s)", counts.jpg));
    }
    if extensions.cr2 {
        lines.push(format!("Found {} *.CR2 file(s)", counts.cr2));
    }
    if extensions.gif {
        lines.push(format!("Found {} *.gif file(s)", counts.gif));
    }
    lines.push(format!("Scan completed in {:.2}s", result.elapsed.as_secs_f64()));

    lines
}

/// Appends `new_lines` to `log`, keeping only the most recent `cap` entries.
pub fn append_capped(log: &mut Vec<String>, new_lines: impl IntoIterator<Item = String>, cap: usize) {
    log.extend(new_lines);
    if log.len() > cap {
        log.drain(0..log.len() - cap);
    }
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
    fn counts_enabled_extensions_recursively_and_case_insensitively() {
        let root = temp_dir("counts-recursive");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();

        std::fs::write(root.join("a.jpg"), b"").unwrap();
        std::fs::write(root.join("b.JPG"), b"").unwrap();
        std::fs::write(sub.join("c.CR2"), b"").unwrap();
        std::fs::write(sub.join("d.gif"), b"").unwrap();
        std::fs::write(root.join("e.txt"), b"").unwrap();

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: true }, |_| {});

        assert_eq!(counts(&result.files), ScanCounts { jpg: 2, cr2: 1, gif: 1 });

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn disabled_extension_is_not_counted() {
        let root = temp_dir("disabled-extension");
        std::fs::write(root.join("a.jpg"), b"").unwrap();
        std::fs::write(root.join("b.gif"), b"").unwrap();

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: false }, |_| {});

        assert_eq!(counts(&result.files), ScanCounts { jpg: 1, cr2: 0, gif: 0 });

        std::fs::remove_dir_all(&root).ok();
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

    #[test]
    fn format_summary_omits_disabled_extensions_and_reports_elapsed_time() {
        let result = ScanResult {
            files: vec![
                ScannedFile { relative_path: PathBuf::from("a.jpg"), extension: "jpg".to_string(), toplevel_dir: None },
                ScannedFile { relative_path: PathBuf::from("b.jpg"), extension: "jpg".to_string(), toplevel_dir: None },
                ScannedFile { relative_path: PathBuf::from("c.jpg"), extension: "jpg".to_string(), toplevel_dir: None },
                ScannedFile { relative_path: PathBuf::from("d.gif"), extension: "gif".to_string(), toplevel_dir: None },
                ScannedFile { relative_path: PathBuf::from("e.gif"), extension: "gif".to_string(), toplevel_dir: None },
                ScannedFile { relative_path: PathBuf::from("f.gif"), extension: "gif".to_string(), toplevel_dir: None },
                ScannedFile { relative_path: PathBuf::from("g.gif"), extension: "gif".to_string(), toplevel_dir: None },
                ScannedFile { relative_path: PathBuf::from("h.gif"), extension: "gif".to_string(), toplevel_dir: None },
            ],
            dirs: vec![],
            elapsed: Duration::from_millis(1500),
        };
        let extensions = FileExtensions { jpg: true, cr2: false, gif: true };

        let lines = format_summary(&result, &extensions);

        assert_eq!(
            lines,
            vec![
                "Found 3 *.jpg file(s)".to_string(),
                "Found 5 *.gif file(s)".to_string(),
                "Scan completed in 1.50s".to_string(),
            ]
        );
    }

    #[test]
    fn append_capped_keeps_only_the_most_recent_entries() {
        let mut log: Vec<String> = (1..=8).map(|n| format!("line {n}")).collect();

        append_capped(&mut log, vec!["line 9".to_string(), "line 10".to_string(), "line 11".to_string()], 10);

        assert_eq!(log.len(), 10);
        assert_eq!(log.first().unwrap(), "line 2");
        assert_eq!(log.last().unwrap(), "line 11");
    }
}
