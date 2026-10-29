pub mod scan;
pub mod telemetry;

use scan::Item;

/// Moves selected items to the OS trash, or removes them for good when `permanent`
/// (needed on servers, where trash frees no space). Returns (done, failed, freed bytes) and drops done items from `items`.
pub fn delete_selected(items: &mut Vec<Item>, permanent: bool) -> (usize, usize, u64) {
    let (mut ok, mut failed, mut freed) = (0, 0, 0);
    items.retain(|item| {
        if !item.selected {
            return true;
        }
        let result = if permanent {
            std::fs::remove_file(&item.path).map_err(|e| e.to_string())
        } else {
            trash::delete(&item.path).map_err(|e| e.to_string())
        };
        match result {
            Ok(()) => {
                ok += 1;
                freed += item.size;
            }
            Err(_) => failed += 1,
        }
        item.path.exists()
    });
    (ok, failed, freed)
}

/// Default scan roots: the given folder plus the Downloads folder (for "old downloads").
pub fn scan_default(
    root: &std::path::Path,
    progress: &std::sync::atomic::AtomicUsize,
) -> Vec<Item> {
    let downloads = dirs::download_dir();
    let mut roots = vec![root];
    roots.extend(downloads.as_deref());
    scan::scan(&roots, downloads.as_deref(), progress)
}
