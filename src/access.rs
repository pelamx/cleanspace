//! Folder access in the macOS sandbox (Mac App Store build). The sandbox only lets the app into folders the
//! user picked; a security-scoped bookmark per folder keeps that permission across launches.

use objc2_foundation::{NSData, NSString, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn dir() -> Option<PathBuf> {
    Some(deskmonk::config_dir()?.join("access"))
}

/// Re-opens every folder the user granted before; returns them.
pub fn restore() -> Vec<PathBuf> {
    let files = dir().and_then(|d| std::fs::read_dir(d).ok()).into_iter().flatten().flatten();
    files
        .filter_map(|f| {
            let bytes = std::fs::read(f.path()).ok()?;
            let data = NSData::with_bytes(&bytes);
            let mut stale = objc2::runtime::Bool::NO;
            let url = unsafe {
                NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                    &data,
                    NSURLBookmarkResolutionOptions::WithSecurityScope,
                    None,
                    &mut stale,
                )
            }
            .ok()?;
            let path = PathBuf::from(url.path()?.to_string());
            unsafe { url.startAccessingSecurityScopedResource() }.then_some(path)
        })
        .collect()
}

/// Remembers a folder the user just picked (an open panel or a drop granted it for this run).
pub fn remember(folder: &Path) {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&folder.to_string_lossy()));
    let data = url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(NSURLBookmarkCreationOptions::WithSecurityScope, None, None);
    let (Ok(data), Some(dir)) = (data, dir()) else { return };
    let name: String = Sha256::digest(folder.as_os_str().as_encoded_bytes())[..8].iter().map(|b| format!("{b:02x}")).collect();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(name), data.to_vec());
}
