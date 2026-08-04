use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// Resolves a POSIX-separated `directories.dir_name` (as stored in the database,
/// see `db::mod::normalize_path`) back to a native path under the project's
/// Source Directory.
pub fn resolve_path(source_dir: &Path, dir_name: &str) -> PathBuf {
    let mut path = source_dir.to_path_buf();
    for part in dir_name.split('/') {
        path.push(part);
    }
    path
}

/// Fire-and-forget launch of Windows Explorer at `path`.
pub fn open(path: &Path) -> std::io::Result<Child> {
    Command::new("explorer").arg(path).spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_path_joins_a_single_component_dir_name_onto_source_dir() {
        let resolved = resolve_path(Path::new(r"C:\Photos"), "Vacation2024");
        assert_eq!(resolved, Path::new(r"C:\Photos").join("Vacation2024"));
    }

    #[test]
    fn resolve_path_converts_posix_separators_to_native_path_components() {
        let resolved = resolve_path(Path::new(r"C:\Photos"), "a/b");
        assert_eq!(resolved, Path::new(r"C:\Photos").join("a").join("b"));
    }
}
