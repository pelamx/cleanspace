//! Sequential disk speed test: writes a temporary file, reads it back, always deletes it.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Bytes per second.
pub struct Speed {
    pub write: f64,
    pub read: f64,
}

const BLOCK: usize = 8 << 20;

/// Unbuffered Windows reads need the buffer, the offset and the length aligned to the volume's
/// sector size. 4 KiB covers every sector size in use.
const ALIGN: usize = 4096;

/// Removes the test file however the test ends.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Writes then reads `size` bytes in `dir`. `progress` counts bytes done, up to 2 × `size`.
/// Refuses to run when the disk does not have plenty of free space: users of a cleaner are often nearly full.
/// On Windows the read pass skips the last partial 4 KiB sector, so `size` should be a round number.
/// Slow (seconds on an HDD): call it off the UI thread.
pub fn run(dir: &Path, size: u64, progress: &AtomicU64) -> io::Result<Speed> {
    if let Some((_, free)) = crate::disk_space(dir)
        && free < size.saturating_mul(3)
    {
        return Err(io::Error::other(crate::i18n::t(
            "Test için yeterli boş alan yok",
            "Not enough free space for the test",
            "No hay suficiente espacio libre para la prueba",
        )));
    }
    let file = TempFile(dir.join(format!(".deskmonk-speedtest-{}", std::process::id())));
    // Random-looking data: some SSD controllers compress zeros and would report fake speeds.
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let block: Vec<u8> = (0..BLOCK)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();

    let mut out = File::create(&file.0)?;
    no_cache(&out);
    let start = Instant::now();
    let mut done = 0;
    while done < size {
        let n = BLOCK.min((size - done) as usize);
        out.write_all(&block[..n])?;
        done += n as u64;
        progress.store(done, Ordering::Relaxed);
    }
    out.sync_all()?;
    let write = size as f64 / start.elapsed().as_secs_f64();
    drop_cache(&out);
    drop(out);

    let mut input = open_read(&file.0)?;
    no_cache(&input);
    // Align into a slightly larger buffer rather than allocate by hand; see ALIGN.
    let mut raw = vec![0; BLOCK + ALIGN];
    let offset = raw.as_ptr().align_offset(ALIGN);
    let buf = &mut raw[offset..offset + BLOCK];
    let start = Instant::now();
    let mut done = 0;
    let readable = if cfg!(windows) { size & !(ALIGN as u64 - 1) } else { size };
    while done < readable {
        let n = input.read(&mut buf[..BLOCK.min((readable - done) as usize)])?;
        if n == 0 {
            break;
        }
        done += n as u64;
        progress.store(size + done, Ordering::Relaxed);
    }
    let read = done as f64 / start.elapsed().as_secs_f64();
    // The skipped tail must not leave the progress bar short of the end.
    progress.store(2 * size, Ordering::Relaxed);
    Ok(Speed { write, read })
}

/// Windows: read past the cache, so the read pass measures the disk and not RAM. Falls back to a
/// normal open on the filesystems that refuse unbuffered handles.
#[cfg(windows)]
fn open_read(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
    const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN)
        .open(path)
        .or_else(|_| File::open(path))
}

#[cfg(not(windows))]
fn open_read(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// macOS: bypass the page cache, so reads come from the disk and not from RAM.
#[cfg(target_os = "macos")]
fn no_cache(file: &File) {
    use std::os::fd::AsRawFd;
    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) };
}

#[cfg(not(target_os = "macos"))]
fn no_cache(_: &File) {}

/// Linux: evict the written file from the page cache before reading it back.
#[cfg(target_os = "linux")]
fn drop_cache(file: &File) {
    use std::os::fd::AsRawFd;
    unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
}

/// Windows needs nothing here: the read handle itself bypasses the cache, see [`open_read`].
#[cfg(not(target_os = "linux"))]
fn drop_cache(_: &File) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_and_cleans_up() {
        let dir = std::env::temp_dir();
        let progress = AtomicU64::new(0);
        let speed = run(&dir, 16 << 20, &progress).unwrap();
        assert!(speed.write > 0.0 && speed.read > 0.0);
        assert_eq!(progress.load(Ordering::Relaxed), 32 << 20);
        assert!(!dir.join(format!(".deskmonk-speedtest-{}", std::process::id())).exists());
        assert!(run(&dir, u64::MAX / 4, &progress).is_err(), "must refuse without free space");
    }
}
