//! Remembers recently opened repositories in `%APPDATA%\Rustle\recent.txt` (one path per line).

use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 15;

pub struct Recent {
    paths: Vec<PathBuf>,
    file: Option<PathBuf>,
}

fn key(p: &Path) -> String {
    p.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// Whether two paths point to the same location (case-insensitive, slash-agnostic).
pub fn same_path(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}

fn default_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(base).join("Rustle").join("recent.txt"))
}

impl Recent {
    /// Loads the list from the default location; paths that no longer exist are dropped.
    pub fn load() -> Self {
        Self::with_file(default_file())
    }

    fn with_file(file: Option<PathBuf>) -> Self {
        let paths = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| {
                s.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from)
                    .filter(|p| p.exists())
                    .take(MAX_RECENT)
                    .collect()
            })
            .unwrap_or_default();
        Self { paths, file }
    }

    /// Most recent first.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Moves (or inserts) `path` at the front and saves the list.
    pub fn add(&mut self, path: &Path) {
        let k = key(path);
        self.paths.retain(|p| key(p) != k);
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(MAX_RECENT);
        self.save();
    }

    fn save(&self) {
        let Some(f) = &self.file else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text: String = self.paths.iter().map(|p| format!("{}\n", p.display())).collect();
        let _ = std::fs::write(f, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_dedupes_orders_and_persists() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = (root.path().join("a"), root.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let file = root.path().join("cfg").join("recent.txt");

        let mut r = Recent::with_file(Some(file.clone()));
        assert!(r.paths().is_empty());
        r.add(&a);
        r.add(&b);
        r.add(&a);
        assert_eq!(r.paths(), [a.clone(), b.clone()]);

        let again = Recent::with_file(Some(file));
        assert_eq!(again.paths(), [a, b]);
    }

    #[test]
    fn missing_paths_are_dropped_on_load() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("recent.txt");
        std::fs::write(&file, format!("{}\nC:/does/not/exist\n", root.path().display())).unwrap();
        let r = Recent::with_file(Some(file));
        assert_eq!(r.paths().len(), 1);
    }

    #[test]
    fn list_is_capped() {
        let root = tempfile::tempdir().unwrap();
        let mut r = Recent::with_file(None);
        for i in 0..30 {
            r.add(&root.path().join(format!("r{i}")));
        }
        assert_eq!(r.paths().len(), MAX_RECENT);
    }
}
