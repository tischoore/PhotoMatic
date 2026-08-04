use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::project::FileExtensions;

/// A single file found during a scan, relative to the scan root.
#[derive(Debug, Clone, PartialEq)]
pub struct ScannedFile {
    pub relative_path: PathBuf,
    /// Lowercase, no leading dot: "jpg", "cr2", "gif".
    pub extension: String,
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
    /// Every subdirectory under the scan root, relative paths (root itself excluded).
    pub dirs: Vec<PathBuf>,
    pub elapsed: Duration,
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

/// Recursively walks `root`, collecting files whose extension matches one of the
/// extensions enabled in `extensions` (case-insensitive), plus every subdirectory
/// found along the way. Extensions that are disabled are not collected at all, per
/// the File Types setting. Directories that can't be read (permissions, races) are
/// silently skipped.
pub fn scan_directory(root: &Path, extensions: &FileExtensions) -> ScanResult {
    let start = Instant::now();
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    visit(root, root, extensions, &mut files, &mut dirs);
    ScanResult { files, dirs, elapsed: start.elapsed() }
}

fn visit(
    root: &Path,
    dir: &Path,
    extensions: &FileExtensions,
    files: &mut Vec<ScannedFile>,
    dirs: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Ok(relative) = path.strip_prefix(root) {
                dirs.push(relative.to_path_buf());
            }
            visit(root, &path, extensions, files, dirs);
            continue;
        }

        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let ext = ext.to_lowercase();

        let enabled = match ext.as_str() {
            "jpg" => extensions.jpg,
            "cr2" => extensions.cr2,
            "gif" => extensions.gif,
            _ => false,
        };
        if !enabled {
            continue;
        }

        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        files.push(ScannedFile { relative_path: relative.to_path_buf(), extension: ext });
    }
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

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: true });

        assert_eq!(counts(&result.files), ScanCounts { jpg: 2, cr2: 1, gif: 1 });

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn disabled_extension_is_not_counted() {
        let root = temp_dir("disabled-extension");
        std::fs::write(root.join("a.jpg"), b"").unwrap();
        std::fs::write(root.join("b.gif"), b"").unwrap();

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: false });

        assert_eq!(counts(&result.files), ScanCounts { jpg: 1, cr2: 0, gif: 0 });

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nested_and_empty_subdirectories_are_all_collected() {
        let root = temp_dir("nested-dirs");
        let sub = root.join("sub");
        let empty_sub = root.join("empty_sub");
        let nested = sub.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&empty_sub).unwrap();
        std::fs::write(nested.join("a.jpg"), b"").unwrap();

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: true });

        let mut dirs = result.dirs.clone();
        dirs.sort();
        let mut expected = vec![
            PathBuf::from("sub"),
            PathBuf::from("empty_sub"),
            PathBuf::from("sub").join("nested"),
        ];
        expected.sort();
        assert_eq!(dirs, expected);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn extensions_are_lowercased_in_scanned_files() {
        let root = temp_dir("lowercase-extensions");
        std::fs::write(root.join("a.CR2"), b"").unwrap();

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: true });

        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].extension, "cr2");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn format_summary_omits_disabled_extensions_and_reports_elapsed_time() {
        let result = ScanResult {
            files: vec![
                ScannedFile { relative_path: PathBuf::from("a.jpg"), extension: "jpg".to_string() },
                ScannedFile { relative_path: PathBuf::from("b.jpg"), extension: "jpg".to_string() },
                ScannedFile { relative_path: PathBuf::from("c.jpg"), extension: "jpg".to_string() },
                ScannedFile { relative_path: PathBuf::from("d.gif"), extension: "gif".to_string() },
                ScannedFile { relative_path: PathBuf::from("e.gif"), extension: "gif".to_string() },
                ScannedFile { relative_path: PathBuf::from("f.gif"), extension: "gif".to_string() },
                ScannedFile { relative_path: PathBuf::from("g.gif"), extension: "gif".to_string() },
                ScannedFile { relative_path: PathBuf::from("h.gif"), extension: "gif".to_string() },
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
