//! Disk map: folder sizes as a tree, and a squarified treemap layout to draw it.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Node {
    pub name: String,
    pub size: u64,
    pub dir: bool,
    /// Biggest first. Small entries are merged into one "small items" node, so a home folder with
    /// millions of files stays a few MB in memory and the map only holds boxes big enough to see.
    pub children: Vec<Node>,
}

/// Children kept per folder before the rest is merged.
const KEEP: usize = 40;

/// Walks `root` without following symlinks or entering other volumes; counts files in `files`.
/// Slow on big folders: call it off the UI thread.
pub fn build(root: &Path, files: &AtomicUsize) -> Node {
    let dev = fs::symlink_metadata(root).ok().and_then(|m| device(&m));
    let name = root.display().to_string();
    walk(root, name, dev, files)
}

#[cfg(unix)]
fn device(meta: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.dev())
}

/// Windows needs no device id: it mounts other volumes as junctions and mount points, which carry
/// the reparse tags std reports as symlinks, so the `is_dir` check above has already skipped them.
/// Testing the reparse attribute instead would also skip cloud-backed folders (OneDrive on demand).
#[cfg(not(unix))]
fn device(_: &fs::Metadata) -> Option<u64> {
    None
}

fn walk(path: &Path, name: String, dev: Option<u64>, files: &AtomicUsize) -> Node {
    let mut children = Vec::new();
    // Unreadable folders count as empty instead of failing the whole map.
    for entry in fs::read_dir(path).into_iter().flatten().flatten() {
        // DirEntry::metadata does not follow symlinks.
        let Ok(meta) = entry.metadata() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if meta.is_dir() {
            // Mounted volumes (and macOS firmlinks into the data volume) would be counted twice.
            if device(&meta) == dev {
                children.push(walk(&entry.path(), name, dev, files));
            }
        } else {
            files.fetch_add(1, Ordering::Relaxed);
            children.push(Node { name, size: meta.len(), dir: false, children: Vec::new() });
        }
    }
    children.sort_by_key(|c| std::cmp::Reverse(c.size));
    if children.len() > KEEP {
        let rest = children.split_off(KEEP - 1);
        children.push(Node {
            name: crate::tf!("{} küçük öğe", "{} small items", "{} elementos pequeños", rest.len()),
            size: rest.iter().map(|c| c.size).sum(),
            dir: false,
            children: Vec::new(),
        });
    }
    let size = children.iter().map(|c| c.size).sum();
    Node { name, size, dir: true, children }
}

/// Squarified treemap (Bruls et al.): splits `rect` ([x, y, w, h]) into one box per size, areas proportional
/// to sizes, as close to square as possible. `sizes` must be biggest first; zero sizes get an empty box.
pub fn treemap(sizes: &[u64], rect: [f32; 4]) -> Vec<[f32; 4]> {
    let mut out = vec![[rect[0], rect[1], 0.0, 0.0]; sizes.len()];
    let total: u64 = sizes.iter().sum();
    let [mut x, mut y, mut w, mut h] = rect.map(f64::from);
    if total == 0 || w <= 0.0 || h <= 0.0 {
        return out;
    }
    let scale = w * h / total as f64;
    let areas: Vec<f64> = sizes.iter().take_while(|&&s| s > 0).map(|&s| s as f64 * scale).collect();
    // Worst aspect ratio of a row of boxes laid along a side of length `side`.
    let worst = |row: &[f64], side: f64| {
        let sum: f64 = row.iter().sum();
        let (max, min) = (row[0], row[row.len() - 1]);
        (side * side * max / (sum * sum)).max(sum * sum / (side * side * min))
    };
    let mut i = 0;
    while i < areas.len() {
        let side = w.min(h);
        let mut j = i + 1;
        while j < areas.len() && worst(&areas[i..=j], side) <= worst(&areas[i..j], side) {
            j += 1;
        }
        let thick = areas[i..j].iter().sum::<f64>() / side;
        let mut offset = 0.0;
        for (k, area) in areas[i..j].iter().enumerate() {
            let len = area / thick;
            let r = if w >= h { [x, y + offset, thick, len] } else { [x + offset, y, len, thick] };
            out[i + k] = r.map(|v| v as f32);
            offset += len;
        }
        if w >= h {
            x += thick;
            w -= thick;
        } else {
            y += thick;
            h -= thick;
        }
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treemap_areas_match_sizes_and_fit() {
        let sizes = [600, 300, 60, 30, 10, 0];
        let rects = treemap(&sizes, [10.0, 20.0, 400.0, 250.0]);
        for (r, &s) in rects.iter().zip(&sizes) {
            let expected = s as f32 / 1000.0 * 400.0 * 250.0;
            assert!((r[2] * r[3] - expected).abs() < 0.5, "{r:?} for {s}");
            assert!(r[0] >= 10.0 - 1e-3 && r[1] >= 20.0 - 1e-3);
            assert!(r[0] + r[2] <= 410.0 + 1e-3 && r[1] + r[3] <= 270.0 + 1e-3);
        }
        assert!(treemap(&[0, 0], [0.0, 0.0, 10.0, 10.0]).iter().all(|r| r[2] == 0.0));
    }

    #[test]
    fn builds_tree_and_merges_small_files() {
        let dir = std::env::temp_dir().join(format!("deskmonk-map-{}", std::process::id()));
        fs::create_dir_all(dir.join("big")).unwrap();
        fs::write(dir.join("big/a"), vec![0; 5000]).unwrap();
        for i in 0..50 {
            fs::write(dir.join(format!("f{i}")), vec![0; 10]).unwrap();
        }
        let files = AtomicUsize::new(0);
        let tree = build(&dir, &files);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(files.load(Ordering::Relaxed), 51);
        assert_eq!(tree.size, 5500);
        assert_eq!(tree.children.len(), KEEP);
        assert_eq!((tree.children[0].name.as_str(), tree.children[0].size), ("big", 5000));
        assert_eq!(tree.children[KEEP - 1].name, "12 small items");
    }
}
