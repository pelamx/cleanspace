use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

pub const LARGE_FILE: u64 = 500 * 1024 * 1024;
pub const OLD_DOWNLOAD: Duration = Duration::from_secs(90 * 24 * 3600);
/// Smaller duplicates free almost nothing and are mostly app internals.
const DUP_MIN: u64 = 32 * 1024;
/// Never descended into: deleting anything inside breaks repos / projects, and they are huge to walk.
const SKIP_DIRS: &[&str] = &[".git", "node_modules"];

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

#[derive(Clone)]
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

/// Folders whose whole content is safe to delete: caches regenerate, the trash is already "deleted".
/// Each first-level entry inside one becomes a single Junk item (e.g. ~/.cache/google-chrome).
pub fn junk_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let home = dirs::home_dir();
    if cfg!(windows) {
        // dirs::cache_dir() is all of %LOCALAPPDATA% on Windows (real app data), so only %TEMP%.
        roots.push(std::env::temp_dir());
    } else {
        roots.extend(dirs::cache_dir()); // ~/.cache, ~/Library/Caches
    }
    if cfg!(target_os = "macos") {
        roots.extend(home.as_ref().map(|h| h.join(".Trash")));
    } else if !cfg!(windows) {
        roots.extend(dirs::data_dir().map(|d| d.join("Trash")));
    }
    roots.extend(home.map(|h| h.join(".npm").join("_cacache")));
    roots
}

/// Well-known folders the classification depends on.
pub struct Places {
    pub downloads: Option<PathBuf>,
    /// See [`junk_roots`].
    pub junk: Vec<PathBuf>,
    /// The user's own document/media folders.
    pub user: Vec<PathBuf>,
}

impl Places {
    pub fn system() -> Self {
        let user = [
            dirs::download_dir(),
            dirs::document_dir(),
            dirs::desktop_dir(),
            dirs::picture_dir(),
            dirs::audio_dir(),
            dirs::video_dir(),
        ];
        Self {
            downloads: dirs::download_dir(),
            junk: junk_roots(),
            user: user.into_iter().flatten().collect(),
        }
    }
}

/// The OS trash: its items must be removed for good, moving them "to the trash" again is meaningless.
pub fn is_in_trash(path: &Path) -> bool {
    let trash = if cfg!(target_os = "macos") {
        dirs::home_dir().map(|h| h.join(".Trash"))
    } else {
        dirs::data_dir().map(|d| d.join("Trash"))
    };
    trash.is_some_and(|t| path.starts_with(t))
}

/// Hash of the whole file, or only its first `limit` bytes.
fn hash_file(path: &Path, limit: Option<u64>) -> io::Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    io::copy(
        &mut io::Read::take(File::open(path)?, limit.unwrap_or(u64::MAX)),
        &mut hasher,
    )?;
    Ok(hasher.finalize().into())
}

/// Splits `paths` into groups with equal hash, dropping singletons.
fn same_hash(paths: Vec<PathBuf>, limit: Option<u64>) -> Vec<Vec<PathBuf>> {
    let mut by_hash: HashMap<[u8; 32], Vec<PathBuf>> = HashMap::new();
    for path in paths {
        if let Ok(h) = hash_file(&path, limit) {
            by_hash.entry(h).or_default().push(path);
        }
    }
    by_hash.into_values().filter(|g| g.len() > 1).collect()
}

/// Scans `roots` (symlinks not followed). Each file lands in at most one category so sizes are never double-counted.
/// Content of `places.junk` folders found under a root is reported per first-level entry instead of per file.
/// `on_partial` gets everything except duplicates as soon as the walk is done, because comparing contents
/// for duplicates is by far the slowest step; the returned list is the final one and replaces it.
pub fn scan(
    roots: &[&Path],
    places: &Places,
    progress: &AtomicUsize,
    on_partial: impl FnOnce(Vec<Item>),
) -> Vec<Item> {
    // path -> (size, modified, protected = inside a hidden folder or a git repo: app/project internals)
    let mut files: HashMap<PathBuf, (u64, SystemTime, bool)> = HashMap::new();
    let mut junk_dirs: HashMap<PathBuf, u64> = HashMap::new();
    let mut repos: HashSet<PathBuf> = HashSet::new();
    for root in roots {
        let walk = WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| !SKIP_DIRS.contains(&e.file_name().to_str().unwrap_or("")));
        for entry in walk.filter_map(Result::ok) {
            // Directories come before their content, so a repo is known before its files.
            if entry.file_type().is_dir() && entry.path().join(".git").exists() {
                repos.insert(entry.path().to_path_buf());
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            progress.fetch_add(1, Ordering::Relaxed);
            let path = entry.into_path();
            if let Some((jroot, rel)) = places
                .junk
                .iter()
                .find_map(|j| Some((j, path.strip_prefix(j).ok()?)))
            {
                let key = jroot.join(rel.components().next().unwrap());
                *junk_dirs.entry(key).or_default() += meta.len();
                continue;
            }
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let hidden = rel.parent().is_some_and(|p| {
                p.components()
                    .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
            });
            let protected = hidden || path.ancestors().any(|a| repos.contains(a));
            files.insert(
                path,
                (
                    meta.len(),
                    meta.modified().unwrap_or(SystemTime::now()),
                    protected,
                ),
            );
        }
    }

    let mut junk_items: Vec<_> = junk_dirs.into_iter().collect();
    junk_items.sort_by(|a, b| b.1.cmp(&a.1));
    let mut items: Vec<Item> = junk_items
        .into_iter()
        .map(|(path, size)| Item {
            path,
            size,
            category: Category::Junk,
            group: 0,
            selected: true,
        })
        .collect();
    on_partial(
        items
            .iter()
            .cloned()
            .chain(classify(&files, places))
            .collect(),
    );

    // Duplicates: same size, then same first 64 KB, then same full content. Only user files, not app internals.
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    for (path, (size, _, protected)) in &files {
        if *size >= DUP_MIN && !protected {
            by_size.entry(*size).or_default().push(path.clone());
        }
    }
    let mut groups: Vec<Vec<PathBuf>> = Vec::new();
    for paths in by_size.into_values().filter(|p| p.len() > 1) {
        for candidates in same_hash(paths, Some(64 * 1024)) {
            groups.extend(same_hash(candidates, None));
        }
    }
    // Sort members first so each group's kept copy and the group order are deterministic.
    groups.iter_mut().for_each(|g| g.sort());
    groups.sort();
    for (i, group) in groups.into_iter().enumerate() {
        for (j, path) in group.into_iter().enumerate() {
            let (size, ..) = files.remove(&path).unwrap();
            // The first copy is always kept. Extra copies are pre-selected only in the user's own folders;
            // elsewhere they may be files an installed app needs.
            let selected = j > 0 && places.user.iter().any(|u| path.starts_with(u));
            items.push(Item {
                path,
                size,
                category: Category::Duplicate,
                group: i + 1,
                selected,
            });
        }
    }

    items.extend(classify(&files, places));
    items
}

/// Junk-by-name, old downloads and large files among `files`, biggest first.
fn classify(files: &HashMap<PathBuf, (u64, SystemTime, bool)>, places: &Places) -> Vec<Item> {
    let now = SystemTime::now();
    let mut items = Vec::new();
    for (path, &(size, modified, _)) in files {
        let old = now.duration_since(modified).unwrap_or_default() > OLD_DOWNLOAD;
        let (category, selected) = if is_junk(path) {
            (Category::Junk, true)
        } else if old
            && places
                .downloads
                .as_ref()
                .is_some_and(|d| path.starts_with(d))
        {
            (Category::OldDownload, false)
        } else if size >= LARGE_FILE {
            (Category::Large, false)
        } else {
            continue;
        };
        items.push(Item {
            path: path.clone(),
            size,
            category,
            group: 0,
            selected,
        });
    }
    items.sort_by(|a, b| b.size.cmp(&a.size));
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
    fn classifies_files() {
        let dir = std::env::temp_dir().join(format!("cleanspace-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for d in [
            "sub",
            ".config",
            "node_modules/pkg",
            "cache/app/deep",
            "repo/.git",
            "repo/src",
        ] {
            fs::create_dir_all(dir.join(d)).unwrap();
        }
        let big = vec![7u8; 40_000];
        let mut other = big.clone();
        other[39_999] = 8; // same size and same first 64 KB check passes only on full hash
        fs::write(dir.join("a.bin"), &big).unwrap();
        fs::write(dir.join("sub/b.bin"), &big).unwrap();
        fs::write(dir.join("c.bin"), &other).unwrap();
        fs::write(dir.join(".config/d.bin"), &big).unwrap(); // hidden: not a duplicate candidate
        fs::write(dir.join("node_modules/pkg/e.bin"), &big).unwrap(); // skipped entirely
        fs::write(dir.join("repo/src/f.bin"), &big).unwrap(); // inside a git repo: not a candidate
        fs::write(dir.join("x.tmp"), "junk").unwrap();
        fs::write(dir.join(".DS_Store"), "j").unwrap();
        fs::write(dir.join("cache/app/1"), "aaaa").unwrap();
        fs::write(dir.join("cache/app/deep/2"), "bb").unwrap();

        let progress = AtomicUsize::new(0);
        let places = Places {
            downloads: None,
            junk: vec![dir.join("cache")],
            user: vec![dir.join("sub")],
        };
        let mut partial = Vec::new();
        let items = scan(&[&dir], &places, &progress, |p| partial = p);
        assert_eq!(
            partial
                .iter()
                .filter(|i| i.category == Category::Junk)
                .count(),
            3
        );
        assert!(partial.iter().all(|i| i.category != Category::Duplicate));
        fs::remove_dir_all(&dir).unwrap();

        let of = |c: Category| items.iter().filter(|i| i.category == c).collect::<Vec<_>>();
        let dups = of(Category::Duplicate);
        assert_eq!(dups.len(), 2);
        assert!(dups[0].path.ends_with("a.bin") && !dups[0].selected);
        assert!(dups[1].path.ends_with("sub/b.bin") && dups[1].selected);
        let junk = of(Category::Junk);
        assert_eq!(junk.len(), 3);
        assert!(junk[0].path.ends_with("cache/app") && junk[0].size == 6); // whole cache folder as one item
        assert_eq!(progress.load(Ordering::Relaxed), 9); // node_modules and .git not walked
        assert_eq!(items.len(), 5);
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
