use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

pub const LARGE_FILE: u64 = 500 * 1024 * 1024;
pub const OLD_DOWNLOAD: Duration = Duration::from_secs(90 * 24 * 3600);

const JUNK_NAMES: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];
const JUNK_EXT: &[&str] = &[
    "tmp",
    "temp",
    "bak",
    "old",
    "log",
    "crdownload",
    "part",
    "dmp",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Duplicate,
    Junk,
    OldDownload,
    Large,
}

impl Category {
    pub const ALL: [Category; 4] = [Self::Duplicate, Self::Junk, Self::OldDownload, Self::Large];

    pub fn label(self) -> &'static str {
        match self {
            Self::Duplicate => "Kopyalar",
            Self::Junk => "Gereksiz dosyalar",
            Self::OldDownload => "Eski indirmeler",
            Self::Large => "Büyük dosyalar",
        }
    }
}

pub struct Item {
    pub path: PathBuf,
    pub size: u64,
    pub category: Category,
    /// Duplicate group number; 0 for other categories.
    pub group: usize,
    pub selected: bool,
}

fn is_junk(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    JUNK_NAMES.contains(&name) || name.ends_with('~') || JUNK_EXT.contains(&ext.as_str())
}

fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(path)?, &mut hasher)?;
    Ok(hasher.finalize().into())
}

/// Scans `roots` (symlinks not followed). Each file lands in at most one category so sizes are never double-counted.
pub fn scan(roots: &[&Path], downloads: Option<&Path>, progress: &AtomicUsize) -> Vec<Item> {
    let mut files: HashMap<PathBuf, (u64, SystemTime)> = HashMap::new();
    for root in roots {
        for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let modified = meta.modified().unwrap_or(SystemTime::now());
                files.insert(entry.into_path(), (meta.len(), modified));
                progress.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    let mut items = Vec::new();

    // Duplicates: same size first, then same full-content hash.
    let mut by_size: HashMap<u64, Vec<&PathBuf>> = HashMap::new();
    for (path, (size, _)) in &files {
        if *size > 0 {
            by_size.entry(*size).or_default().push(path);
        }
    }
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for paths in by_size.values().filter(|p| p.len() > 1) {
        let mut by_hash: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
        for path in paths {
            if let Ok(h) = hash_file(path) {
                by_hash.entry(h).or_default().push((*path).clone());
            }
        }
        groups.extend(by_hash.into_values().filter(|g| g.len() > 1));
    }
    groups.sort();
    for (i, mut group) in groups.into_iter().enumerate() {
        group.sort();
        for (j, path) in group.into_iter().enumerate() {
            let (size, _) = files.remove(&path).unwrap();
            // Keep the first copy of every group unselected.
            items.push(Item {
                path,
                size,
                category: Category::Duplicate,
                group: i + 1,
                selected: j > 0,
            });
        }
    }

    let now = SystemTime::now();
    let mut rest: Vec<_> = files.into_iter().collect();
    rest.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    for (path, (size, modified)) in rest {
        let old = now.duration_since(modified).unwrap_or_default() > OLD_DOWNLOAD;
        let (category, selected) = if is_junk(&path) {
            (Category::Junk, true)
        } else if old && downloads.is_some_and(|d| path.starts_with(d)) {
            (Category::OldDownload, false)
        } else if size >= LARGE_FILE {
            (Category::Large, false)
        } else {
            continue;
        };
        items.push(Item {
            path,
            size,
            category,
            group: 0,
            selected,
        });
    }
    items
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_duplicates_and_junk() {
        let dir = std::env::temp_dir().join(format!("cleanspace-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.txt"), "same content").unwrap();
        fs::write(dir.join("sub/b.txt"), "same content").unwrap();
        fs::write(dir.join("c.txt"), "same lengthX").unwrap(); // same size, different content
        fs::write(dir.join("x.tmp"), "junk").unwrap();
        fs::write(dir.join(".DS_Store"), "j").unwrap();

        let items = scan(&[&dir], None, &AtomicUsize::new(0));
        fs::remove_dir_all(&dir).unwrap();

        let dups: Vec<_> = items
            .iter()
            .filter(|i| i.category == Category::Duplicate)
            .collect();
        assert_eq!(dups.len(), 2);
        assert!(dups[0].path.ends_with("a.txt") && !dups[0].selected);
        assert!(dups[1].selected);
        assert_eq!(
            items
                .iter()
                .filter(|i| i.category == Category::Junk)
                .count(),
            2
        );
        assert_eq!(items.len(), 4);
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
