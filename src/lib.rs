pub mod scan;
pub mod telemetry;

use scan::Item;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Moves selected items to the OS trash, or removes them for good when `permanent`
/// (needed on servers, where trash frees no space). Returns (done, failed, freed bytes) and drops done items from `items`.
/// Works on all cores; `progress` counts files handled.
pub fn delete_selected(
    items: &mut Vec<Item>,
    permanent: bool,
    progress: &AtomicUsize,
) -> (usize, usize, u64) {
    let paths: Vec<&std::path::PathBuf> = items
        .iter()
        .filter(|i| i.selected)
        .map(|i| &i.path)
        .collect();
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut ctx = trash::TrashContext::default();
    // Finder asks for a password on every file it can't move and costs a round-trip per file; NSFileManager just fails fast.
    #[cfg(target_os = "macos")]
    trash::macos::TrashContextExtMacos::set_delete_method(
        &mut ctx,
        trash::macos::DeleteMethod::NsFileManager,
    );
    scan::par_map(&paths, |path| {
        let _ = if permanent {
            std::fs::remove_file(path).map_err(|e| e.to_string())
        } else {
            ctx.delete(path).map_err(|e| e.to_string())
        };
        progress.fetch_add(1, Ordering::Relaxed);
    });
    let (mut ok, mut failed, mut freed) = (0, 0, 0);
    items.retain(|item| {
        if !item.selected {
            return true;
        }
        let gone = !item.path.exists();
        if gone {
            ok += 1;
            freed += item.size;
        } else {
            failed += 1;
        }
        !gone
    });
    (ok, failed, freed)
}

/// Default scan roots: the given folder plus the Downloads folder (for "old downloads").
pub fn scan_default(root: &std::path::Path, progress: &scan::Progress) -> Vec<Item> {
    let downloads = dirs::download_dir();
    let mut roots = vec![root];
    roots.extend(downloads.as_deref());
    scan::scan(&roots, downloads.as_deref(), progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scan::Category;

    #[test]
    fn deletes_only_selected() {
        let dir = std::env::temp_dir().join(format!("cleanspace-del-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut items: Vec<Item> = ["keep", "drop"]
            .iter()
            .map(|name| {
                std::fs::write(dir.join(name), "x").unwrap();
                Item {
                    path: dir.join(name),
                    size: 1,
                    category: Category::Junk,
                    group: 0,
                    selected: *name == "drop",
                }
            })
            .collect();
        items.push(Item {
            path: dir.join("missing-dir/x"),
            size: 5,
            category: Category::Junk,
            group: 0,
            selected: true,
        });
        let progress = AtomicUsize::new(0);
        let result = delete_selected(&mut items, true, &progress);
        let keep_exists = dir.join("keep").exists();
        std::fs::remove_dir_all(&dir).unwrap();
        // A path that never existed counts as gone too, matching "nothing left to free".
        assert_eq!(result, (2, 0, 6));
        assert!(keep_exists && items.len() == 1);
        assert_eq!(progress.load(Ordering::Relaxed), 2);
    }
}
