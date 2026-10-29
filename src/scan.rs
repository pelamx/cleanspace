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

/// Hashes at most `limit` bytes from the start of the file.
fn hash_file(path: &Path, limit: u64) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    io::copy(&mut io::Read::take(File::open(path)?, limit), &mut hasher)?;
    Ok(hasher.finalize().into())
}

const PREFIX: u64 = 64 * 1024;

/// Live scan counters for the UI: files found, then content hashes done out of those needed.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicUsize,
    pub hashed: AtomicUsize,
    pub to_hash: AtomicUsize,
}

/// Runs `f` over `items` on all cores, keeping order.
pub(crate) fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut out: Vec<(usize, R)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break done };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().unwrap())
            .collect()
    });
    out.sort_unstable_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

/// Splits every group by hash of its first `limit` bytes, keeping sub-groups with 2+ members.
fn split_by_hash(groups: Vec<Vec<PathBuf>>, limit: u64, progress: &Progress) -> Vec<Vec<PathBuf>> {
    let paths: Vec<&PathBuf> = groups.iter().flatten().collect();
    progress.to_hash.fetch_add(paths.len(), Ordering::Relaxed);
    let mut hashes = par_map(&paths, |path| {
        let hash = hash_file(path, limit).ok();
        progress.hashed.fetch_add(1, Ordering::Relaxed);
        hash
    })
    .into_iter();
    let mut out = Vec::new();
    for group in groups {
        let mut by_hash: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
        for path in group {
            if let Some(hash) = hashes.next().flatten() {
                by_hash.entry(hash).or_default().push(path);
            }
        }
        out.extend(by_hash.into_values().filter(|g| g.len() > 1));
    }
    out
}

/// Hidden dirs (.git, .cargo, .npm…), build/dependency dirs and ~/Library hold app and project
/// files whose "duplicates" must not be deleted. Read-only dirs (e.g. Go's module cache) can't be cleaned anyway;
/// unix only, since Windows marks customized folders like Pictures read-only without blocking writes.
fn skip_dir(entry: &walkdir::DirEntry, library: Option<&Path>) -> bool {
    let name = entry.file_name().to_str().unwrap_or("");
    entry.depth() > 0
        && entry.file_type().is_dir()
        && (name.starts_with('.')
            || matches!(name, "node_modules" | "target")
            || library == Some(entry.path())
            || (cfg!(unix) && entry.metadata().is_ok_and(|m| m.permissions().readonly())))
}

/// Scans `roots` (symlinks not followed). Each file lands in at most one category so sizes are never double-counted.
pub fn scan(roots: &[&Path], downloads: Option<&Path>, progress: &Progress) -> Vec<Item> {
    let mut files: HashMap<PathBuf, (u64, SystemTime)> = HashMap::new();
    let library = dirs::home_dir().map(|h| h.join("Library"));
    for root in roots {
        let walk = WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| !skip_dir(e, library.as_deref()));
        for entry in walk.filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                let modified = meta.modified().unwrap_or(SystemTime::now());
                files.insert(entry.into_path(), (meta.len(), modified));
                progress.files.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    let mut items = Vec::new();

    // Duplicates: same size, then same first 64 KB (cheap filter), then same full-content hash.
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for (path, (size, _)) in &files {
        if *size > 0 {
            by_size.entry(*size).or_default().push(path.clone());
        }
    }
    let same_size: Vec<_> = by_size.into_values().filter(|p| p.len() > 1).collect();
    // Files up to PREFIX bytes were hashed whole already; only bigger ones need a full pass.
    let (mut groups, big): (Vec<_>, Vec<_>) = split_by_hash(same_size, PREFIX, progress)
        .into_iter()
        .partition(|g| files[&g[0]].0 <= PREFIX);
    groups.extend(split_by_hash(big, u64::MAX, progress));
    // Sort members first so each group's kept copy and the group order are deterministic.
    groups.iter_mut().for_each(|g| g.sort());
    groups.sort();
    for (i, group) in groups.into_iter().enumerate() {
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
        // Copies inside skipped dirs are ignored.
        for skipped in [".git", "node_modules", "readonly"] {
            // "readonly" is only skipped on unix; elsewhere its copy is a third duplicate.
            fs::create_dir_all(dir.join(skipped)).unwrap();
            fs::write(dir.join(skipped).join("a.txt"), "same content").unwrap();
        }
        let mut perms = fs::metadata(dir.join("readonly")).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(dir.join("readonly"), perms.clone()).unwrap();
        fs::write(dir.join("c.txt"), "same lengthX").unwrap(); // same size, different content
        fs::write(dir.join("x.tmp"), "junk").unwrap();
        fs::write(dir.join(".DS_Store"), "j").unwrap();
        // Same size and same first 64 KB, different tail: not duplicates.
        let mut big = vec![7u8; 70_000];
        fs::write(dir.join("big1.bin"), &big).unwrap();
        big[69_999] = 8;
        fs::write(dir.join("big2.bin"), &big).unwrap();

        let items = scan(&[&dir], None, &Progress::default());
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(dir.join("readonly"), perms).unwrap();
        fs::remove_dir_all(&dir).unwrap();

        let dups: Vec<_> = items
            .iter()
            .filter(|i| i.category == Category::Duplicate)
            .collect();
        assert_eq!(dups.len(), if cfg!(unix) { 2 } else { 3 });
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
