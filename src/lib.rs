pub mod scan;
pub mod telemetry;

use scan::Item;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};

/// A deletion running on a background thread, so the UI never freezes on big folders.
pub struct Deletion {
    pub total: usize,
    pub done: Arc<AtomicUsize>,
    rx: Receiver<Finished>,
}

/// Result of a [`Deletion`].
pub struct Finished {
    /// Items that could not be deleted (completely); they stay selected.
    pub failed: Vec<Item>,
    pub freed: u64,
    /// Reason of the first failure, for the status line.
    pub first_error: Option<String>,
}

/// Whether deleting this item removes it for good. Junk (caches, the trash, temp files) is removed for good:
/// moving it to the trash frees no space and caches regenerate anyway. The user's own files go to the trash.
pub fn is_permanent(item: &Item, permanent: bool) -> bool {
    permanent || item.category == scan::Category::Junk || scan::is_in_trash(&item.path)
}

impl Deletion {
    /// Takes the selected items out of `items` and deletes them in the background, see [`is_permanent`].
    /// `permanent` removes everything for good (needed on servers, where trash frees no space).
    /// `on_done` runs when finished.
    pub fn start(
        items: &mut Vec<Item>,
        permanent: bool,
        on_done: impl FnOnce() + Send + 'static,
    ) -> Self {
        let (selected, rest): (Vec<Item>, Vec<Item>) =
            std::mem::take(items).into_iter().partition(|i| i.selected);
        *items = rest;
        let done = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        let (total, counter) = (selected.len(), done.clone());
        std::thread::spawn(move || {
            let mut result = Finished {
                failed: Vec::new(),
                freed: 0,
                first_error: None,
            };
            for item in selected {
                let outcome = if is_permanent(&item, permanent) {
                    remove(&item.path)
                } else {
                    trash::delete(&item.path).map_err(|e| std::io::Error::other(e.to_string()))
                };
                match outcome {
                    Ok(()) => result.freed += item.size,
                    Err(e) => {
                        result
                            .first_error
                            .get_or_insert_with(|| format!("{}: {e}", item.path.display()));
                        result.failed.push(item);
                    }
                }
                counter.fetch_add(1, Ordering::Relaxed);
            }
            let _ = tx.send(result);
            on_done();
        });
        Self { total, done, rx }
    }

    /// The result, once finished.
    pub fn poll(&self) -> Option<Finished> {
        self.rx.try_recv().ok()
    }
}

/// Removes a file or folder tree for good, best effort: read-only folders (left by package builds, Go, …) are
/// made writable first, and a failing entry does not stop the rest of the tree from being freed.
/// The OS trash folder itself is emptied but kept, since the OS expects it to exist.
fn remove(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return fs::remove_file(path).or_else(|e| {
            // Windows refuses to delete read-only files; on Unix the parent folder's permission is what counts.
            if cfg!(windows) && make_writable(path, &meta).is_ok() {
                fs::remove_file(path)
            } else {
                Err(e)
            }
        });
    }
    let _ = make_writable(path, &meta);
    let mut first_err = None;
    for entry in fs::read_dir(path)? {
        if let Err(e) = entry.and_then(|e| remove(&e.path())) {
            first_err.get_or_insert(e);
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    if scan::trash_dir().as_deref() == Some(path) {
        if !cfg!(target_os = "macos") {
            // freedesktop trash layout
            fs::create_dir_all(path.join("files"))?;
            fs::create_dir_all(path.join("info"))?;
        }
        return Ok(());
    }
    fs::remove_dir(path)
}

fn make_writable(path: &Path, meta: &fs::Metadata) -> io::Result<()> {
    let mut perms = meta.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(perms.mode() | 0o700);
    }
    #[cfg(not(unix))]
    perms.set_readonly(false);
    fs::set_permissions(path, perms)
}

/// Carries the user's ticks from `old` (e.g. a partial scan list) over to the same items in `new`.
pub fn keep_selection(old: &[Item], new: &mut [Item]) {
    let before: std::collections::HashMap<_, _> = old
        .iter()
        .map(|i| (&i.path, (i.category, i.selected)))
        .collect();
    for item in new {
        if let Some(&(cat, selected)) = before.get(&item.path)
            && cat == item.category
        {
            item.selected = selected;
        }
    }
}

/// Default scan roots: the given folder plus the Downloads folder (for "old downloads").
/// See [`scan::scan`] for `on_partial`.
pub fn scan_default(
    root: &Path,
    progress: &AtomicUsize,
    mut on_partial: impl FnMut(Vec<Item>, scan::Phase),
) -> Vec<Item> {
    let places = scan::Places::system();
    let mut roots = vec![root];
    roots.extend(places.downloads.as_deref());
    scan::scan(&roots, &places, progress, &mut on_partial)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn removes_read_only_folders() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cleanspace-rm-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("pkg/sub")).unwrap();
        std::fs::write(dir.join("pkg/sub/file"), "x").unwrap();
        for d in ["pkg", "pkg/sub"] {
            std::fs::set_permissions(dir.join(d), std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        super::remove(&dir).unwrap();
        assert!(!dir.exists());
    }
}
