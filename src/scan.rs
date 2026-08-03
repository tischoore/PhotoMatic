use std::path::Path;
use std::time::{Duration, Instant};

use crate::project::FileExtensions;

/// Per-extension file counts from a directory scan.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScanCounts {
    pub jpg: u64,
    pub cr2: u64,
    pub gif: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanResult {
    pub counts: ScanCounts,
    pub elapsed: Duration,
}

/// Recursively walks `root`, counting files whose extension matches one of the
/// extensions enabled in `extensions` (case-insensitive). Extensions that are
/// disabled are not counted at all, per the File Types setting. Directories that
/// can't be read (permissions, races) are silently skipped.
pub fn scan_directory(root: &Path, extensions: &FileExtensions) -> ScanResult {
    let start = Instant::now();
    let mut counts = ScanCounts::default();
    visit(root, extensions, &mut counts);
    ScanResult { counts, elapsed: start.elapsed() }
}

fn visit(dir: &Path, extensions: &FileExtensions, counts: &mut ScanCounts) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, extensions, counts);
            continue;
        }

        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        match ext.to_lowercase().as_str() {
            "jpg" if extensions.jpg => counts.jpg += 1,
            "cr2" if extensions.cr2 => counts.cr2 += 1,
            "gif" if extensions.gif => counts.gif += 1,
            _ => {}
        }
    }
}

/// Builds the scan-log lines for a finished scan: one line per *enabled* extension
/// with its count, followed by one line reporting elapsed time. Disabled extensions
/// are omitted entirely, since they weren't scanned.
pub fn format_summary(result: &ScanResult, extensions: &FileExtensions) -> Vec<String> {
    let mut lines = Vec::new();

    if extensions.jpg {
        lines.push(format!("Found {} *.jpg file(s)", result.counts.jpg));
    }
    if extensions.cr2 {
        lines.push(format!("Found {} *.CR2 file(s)", result.counts.cr2));
    }
    if extensions.gif {
        lines.push(format!("Found {} *.gif file(s)", result.counts.gif));
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

        assert_eq!(result.counts, ScanCounts { jpg: 2, cr2: 1, gif: 1 });

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn disabled_extension_is_not_counted() {
        let root = temp_dir("disabled-extension");
        std::fs::write(root.join("a.jpg"), b"").unwrap();
        std::fs::write(root.join("b.gif"), b"").unwrap();

        let result = scan_directory(&root, &FileExtensions { jpg: true, cr2: true, gif: false });

        assert_eq!(result.counts, ScanCounts { jpg: 1, cr2: 0, gif: 0 });

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn format_summary_omits_disabled_extensions_and_reports_elapsed_time() {
        let result = ScanResult {
            counts: ScanCounts { jpg: 3, cr2: 0, gif: 5 },
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
