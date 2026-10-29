pub mod scan;
pub mod telemetry;

use scan::Item;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};

/// A deletion running on a background thread, so the UI never freezes on big folders.
pub struct Deletion {
    pub total: usize,
    pub done: Arc<AtomicUsize>,
    rx: Receiver<(Vec<Item>, u64)>,
}

impl Deletion {
    /// Takes the selected items out of `items` and deletes them in the background: moved to the OS trash,
    /// or removed for good when `permanent` (needed on servers, where trash frees no space).
    /// Items already inside the OS trash are always removed for good. `on_done` runs when finished.
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
            let mut freed = 0;
            let mut failed = Vec::new();
            for item in selected {
                let result = if permanent || scan::is_in_trash(&item.path) {
                    remove(&item.path)
                } else {
                    trash::delete(&item.path).map_err(|e| std::io::Error::other(e.to_string()))
                };
                match result {
                    Ok(()) => freed += item.size,
                    Err(_) => failed.push(item),
                }
                counter.fetch_add(1, Ordering::Relaxed);
            }
            let _ = tx.send((failed, freed));
            on_done();
        });
        Self { total, done, rx }
    }

    /// Once finished: the items that could not be deleted and the bytes freed.
    pub fn poll(&self) -> Option<(Vec<Item>, u64)> {
        self.rx.try_recv().ok()
    }
}

fn remove(path: &std::path::Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
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
    root: &std::path::Path,
    progress: &AtomicUsize,
    on_partial: impl FnOnce(Vec<Item>),
) -> Vec<Item> {
    let places = scan::Places::system();
    let mut roots = vec![root];
    roots.extend(places.downloads.as_deref());
    scan::scan(&roots, &places, progress, on_partial)
}
