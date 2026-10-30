//! App lock (macOS and Windows): opening a locked app needs the deskMonk password.
//! A privacy lock while deskMonk runs, not a security boundary: quitting deskMonk lifts it.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// File format: line 1 salt, line 2 password hash, then one locked .app path per line.
#[derive(Default)]
pub struct Config {
    salt: String,
    hash: String,
    pub apps: Vec<PathBuf>,
}

fn config_path() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("applock"))
}

/// Slow on purpose (iterated SHA-256), so a copied config file is costly to brute force.
fn hash(salt: &str, password: &str) -> String {
    let mut digest = Sha256::digest(format!("{salt}:{password}"));
    for _ in 0..100_000 {
        digest = Sha256::digest(digest);
    }
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

impl Config {
    pub fn load() -> Self {
        let text = config_path().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let mut lines = text.lines();
        Self {
            salt: lines.next().unwrap_or_default().into(),
            hash: lines.next().unwrap_or_default().into(),
            apps: lines.filter(|l| !l.is_empty()).map(PathBuf::from).collect(),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let path = config_path().ok_or_else(|| io::Error::other("no config folder"))?;
        fs::create_dir_all(path.parent().unwrap())?;
        let mut text = format!("{}\n{}\n", self.salt, self.hash);
        for app in &self.apps {
            text += &format!("{}\n", app.display());
        }
        fs::write(&path, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn has_password(&self) -> bool {
        !self.hash.is_empty()
    }

    pub fn set_password(&mut self, password: &str) {
        self.salt = uuid::Uuid::new_v4().to_string();
        self.hash = hash(&self.salt, password);
    }

    pub fn check(&self, password: &str) -> bool {
        let given = hash(&self.salt, password);
        self.has_password()
            && given.len() == self.hash.len()
            && given.bytes().zip(self.hash.bytes()).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0
    }
}

/// Apps that must never be locked: killing them breaks the desktop or relaunches in a loop.
#[cfg(not(windows))]
pub fn can_lock(app: &Path) -> bool {
    app.extension().is_some_and(|e| e == "app")
        && !app.starts_with("/System/Library/CoreServices")
        && std::env::current_exe().map_or(true, |me| !me.starts_with(app))
}

/// Windows: anything under %SystemRoot% runs the desktop itself, and an uninstaller is not an app.
#[cfg(windows)]
pub fn can_lock(app: &Path) -> bool {
    // Windows paths are case-insensitive, and %SystemRoot% is spelled "C:\WINDOWS" on many machines,
    // so Path::starts_with (which compares components byte for byte) would miss it.
    let key = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\");
    let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let path = key(app);
    let stem = app.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
    app.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        && !path.starts_with(&format!("{}\\", key(&system).trim_end_matches('\\')))
        && !stem.starts_with("unins")
        && std::env::current_exe().map_or(true, |me| key(&me) != path)
}

/// Lockable .app bundles in the usual app folders (and one folder deeper, e.g. /Applications/Adobe X/), sorted by name.
#[cfg(not(windows))]
pub fn installed_apps() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities"]
        .map(PathBuf::from)
        .into();
    dirs.extend(dirs::home_dir().map(|h| h.join("Applications")));
    let mut apps = Vec::new();
    for dir in dirs {
        apps_in(&dir, 1, &mut apps);
    }
    sorted(apps)
}

#[cfg(not(windows))]
fn apps_in(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    for path in fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        if path.extension().is_some_and(|e| e == "app") {
            out.push(path);
        } else if depth > 0 && path.is_dir() && path.file_name() != Some("Utilities".as_ref()) {
            apps_in(&path, depth - 1, out);
        }
    }
}

/// Windows has no app bundles, so the program folders are walked instead, see [`apps_in`].
#[cfg(windows)]
pub fn installed_apps() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    // Per-user installs (Chrome, VS Code, Spotify, most Electron apps) live here, not in Program Files.
    dirs.extend(dirs::data_local_dir().map(|d| d.join("Programs")));
    let mut apps = Vec::new();
    for dir in distinct(dirs) {
        apps_in(&dir, 2, &mut apps);
    }
    sorted(apps)
}

/// The program folders, each once. For a 64-bit process `ProgramFiles` and `ProgramW6432` are the
/// same folder, and walking both lists every app twice. [`sorted`] cannot clean that up afterwards:
/// it orders by name, which leaves the two copies of a name separated by the 32-bit one.
#[cfg(windows)]
fn distinct(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let key = |d: &PathBuf| d.to_string_lossy().to_lowercase().trim_end_matches('\\').to_owned();
    dirs.sort_by_key(key);
    dirs.dedup_by_key(|d| key(d));
    dirs
}

/// Windows marks GUI programs in the PE header. Console tools (bash, gh, 7za, updaters) share the
/// program folders but are not apps anyone locks, and they crowded the real ones out of the list.
/// Anything unreadable counts as an app, so a failure here can never hide one.
#[cfg(windows)]
fn is_gui_app(exe: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let subsystem = |mut f: fs::File| -> io::Result<u16> {
        let mut at = [0u8; 4];
        f.seek(SeekFrom::Start(0x3C))?; // e_lfanew: where the PE header starts
        f.read_exact(&mut at)?;
        let pe = u32::from_le_bytes(at) as u64;
        // Check the signature first: without it any file long enough to reach the offset below
        // would be read as a subsystem of 0 and dropped as "not an app".
        let mut sig = [0u8; 4];
        f.seek(SeekFrom::Start(pe))?;
        f.read_exact(&mut sig)?;
        if &sig != b"PE\0\0" {
            return Err(io::Error::other("not a PE file"));
        }
        // Signature (4) + COFF header (20), then Subsystem 68 bytes into the optional header —
        // the same offset for PE32 and PE32+, which differ only after it.
        f.seek(SeekFrom::Start(pe + 4 + 20 + 68))?;
        let mut sub = [0u8; 2];
        f.read_exact(&mut sub)?;
        Ok(u16::from_le_bytes(sub))
    };
    // 2 is IMAGE_SUBSYSTEM_WINDOWS_GUI.
    fs::File::open(exe).and_then(subsystem).map_or(true, |s| s == 2)
}

/// One app per folder: the .exe named after its folder, else the biggest one. An app folder also
/// holds updaters, crash handlers and helper processes, and listing all of them is unusable.
#[cfg(windows)]
fn apps_in(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let folder = dir.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    let mut best: Option<(bool, u64, PathBuf)> = None;
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.path();
        if meta.is_dir() {
            if depth > 0 {
                apps_in(&path, depth - 1, out);
            }
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) && is_gui_app(&path) {
            let named = path.file_stem().unwrap_or_default().to_string_lossy().to_lowercase() == folder;
            let candidate = (named, meta.len(), path);
            if best.as_ref().is_none_or(|b| (b.0, b.1) < (candidate.0, candidate.1)) {
                best = Some(candidate);
            }
        }
    }
    out.extend(best.map(|(_, _, path)| path));
}

fn sorted(mut apps: Vec<PathBuf>) -> Vec<PathBuf> {
    apps.retain(|a| can_lock(a));
    apps.sort_by_key(|a| a.file_stem().unwrap_or_default().to_string_lossy().to_lowercase());
    apps.dedup();
    apps
}

#[derive(Default)]
struct State {
    apps: Vec<PathBuf>,
    /// Unlocked apps: when they were unlocked, or None once seen running. They lock again once they quit.
    allowed: HashMap<PathBuf, Option<Instant>>,
    /// Apps with a password prompt on screen; further launches are stopped without a new prompt.
    pending: HashSet<PathBuf>,
}

/// Grace period for an unlocked app to start before "not running" means it quit.
const START_GRACE: Duration = Duration::from_secs(15);

impl State {
    /// One pass over the running processes: returns the pids to stop and the apps to ask a password for.
    fn check(&mut self, procs: &[(i32, PathBuf)], now: Instant) -> (Vec<i32>, Vec<PathBuf>) {
        let (mut kill, mut ask) = (Vec::new(), Vec::new());
        for app in &self.apps {
            let pids: Vec<i32> = procs.iter().filter(|(_, p)| runs_from(p, app)).map(|(pid, _)| *pid).collect();
            if pids.is_empty() {
                // Quit after running, or never started within the grace period: locked again.
                if self.allowed.get(app).is_some_and(|t| t.is_none_or(|t| now - t > START_GRACE)) {
                    self.allowed.remove(app);
                }
                continue;
            }
            if let Some(t) = self.allowed.get_mut(app) {
                // Running: unlocked for as long as it runs.
                *t = None;
                continue;
            }
            kill.extend(pids);
            if self.pending.insert(app.clone()) {
                ask.push(app.clone());
            }
        }
        (kill, ask)
    }

    /// Locked apps already running are left alone (they may hold unsaved work) until they quit.
    fn allow_running(&mut self, procs: &[(i32, PathBuf)], now: Instant) {
        for app in &self.apps {
            if procs.iter().any(|(_, p)| runs_from(p, app)) {
                self.allowed.insert(app.clone(), Some(now));
            }
        }
    }
}

/// Matches by bundle name anywhere in the path, so a copy of the app (or macOS's App Translocation
/// running it from a random folder) is locked too. Helpers inside the bundle match as well.
fn runs_from(process: &Path, app: &Path) -> bool {
    let Some(name) = app.file_name() else { return false };
    // Windows filenames are case-insensitive, and the name the process list reports need not be
    // spelled like the one on disk, so an exact compare would silently stop locking the app.
    process.components().any(|c| {
        let part = c.as_os_str();
        if cfg!(windows) { part.eq_ignore_ascii_case(name) } else { part == name }
    })
}

/// Background watcher; clones share one state.
#[derive(Clone)]
pub struct Watcher(Arc<Mutex<State>>);

impl Watcher {
    /// `on_blocked` runs (on the watcher thread) when a locked app was stopped and needs the password.
    pub fn start(apps: Vec<PathBuf>, on_blocked: impl Fn(PathBuf) + Send + 'static) -> Self {
        let watcher = Watcher(Arc::new(Mutex::new(State::default())));
        watcher.set_apps(apps);
        let state = watcher.0.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(300));
                let procs = processes();
                let (kill, ask) = state.lock().unwrap().check(&procs, Instant::now());
                for pid in kill {
                    // Just launched, so nothing unsaved is lost.
                    stop(pid);
                }
                ask.into_iter().for_each(&on_blocked);
            }
        });
        watcher
    }

    pub fn set_apps(&self, apps: Vec<PathBuf>) {
        let mut state = self.0.lock().unwrap();
        state.apps = apps;
        state.allow_running(&processes(), Instant::now());
    }

    /// Right password: opens the app and keeps it unlocked until it quits.
    pub fn unlock(&self, app: &Path) {
        let mut state = self.0.lock().unwrap();
        state.allowed.insert(app.to_path_buf(), Some(Instant::now()));
        state.pending.remove(app);
        // macOS opens the bundle; on Windows the path is the executable itself.
        let mut command = if cfg!(target_os = "macos") {
            let mut open = std::process::Command::new("open");
            open.arg(app);
            open
        } else {
            let mut exe = std::process::Command::new(app);
            // Without this the app inherits deskMonk's folder as its working directory.
            if let Some(dir) = app.parent() {
                exe.current_dir(dir);
            }
            exe
        };
        let _ = command.spawn();
    }

    /// Prompt closed without the password: the next launch asks again.
    pub fn dismiss(&self, app: &Path) {
        self.0.lock().unwrap().pending.remove(app);
    }
}

#[cfg(target_os = "macos")]
fn processes() -> Vec<(i32, PathBuf)> {
    use std::os::unix::ffi::OsStrExt;
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let mut pids = vec![0i32; count.max(0) as usize + 64];
    let size = (pids.len() * std::mem::size_of::<i32>()) as i32;
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
    pids.truncate(count.max(0) as usize);
    let me = std::process::id() as i32;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    pids.into_iter()
        .filter(|&pid| pid != me)
        .filter_map(|pid| {
            let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
            (len > 0).then(|| (pid, PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len as usize]))))
        })
        .collect()
}

/// Windows: one snapshot gives every process's executable name. The name is all [`runs_from`] needs,
/// and opening a handle per process three times a second to read its full path is not worth it.
#[cfg(windows)]
fn processes() -> Vec<(i32, PathBuf)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let me = std::process::id();
    let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut procs = Vec::new();
    let mut more = unsafe { Process32FirstW(snapshot, &mut entry) };
    while more != 0 {
        let name = &entry.szExeFile[..entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)];
        if entry.th32ProcessID != me && !name.is_empty() {
            procs.push((entry.th32ProcessID as i32, PathBuf::from(String::from_utf16_lossy(name))));
        }
        more = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    unsafe { CloseHandle(snapshot) };
    procs
}

#[cfg(not(any(target_os = "macos", windows)))]
fn processes() -> Vec<(i32, PathBuf)> {
    Vec::new()
}

/// Kills a process that was just launched, so nothing unsaved is lost.
#[cfg(unix)]
fn stop(pid: i32) {
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

#[cfg(windows)]
fn stop(pid: i32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // Fails for processes of other users and for elevated ones, which this lock does not reach anyway.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid as u32) };
    if !handle.is_null() {
        unsafe {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
}

#[cfg(not(any(unix, windows)))]
fn stop(_: i32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn finds_apps_one_folder_deep() {
        let dir = std::env::temp_dir().join(format!("deskmonk-apps-{}", std::process::id()));
        for app in ["A.app/Contents", "Suite/B.app", "Suite/Deep/C.app", "Utilities/D.app"] {
            fs::create_dir_all(dir.join(app)).unwrap();
        }
        let mut found = Vec::new();
        apps_in(&dir, 1, &mut found);
        found.sort();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, vec![dir.join("A.app"), dir.join("Suite/B.app")]);
    }

    /// One exe per folder: named after the folder wins, otherwise the biggest.
    #[cfg(windows)]
    #[test]
    fn picks_one_exe_per_program_folder() {
        let dir = std::env::temp_dir().join(format!("deskmonk-apps-{}", std::process::id()));
        for (file, size) in [
            ("Vendor/Editor/Editor.exe", 10),      // named after its folder, though not the biggest
            ("Vendor/Editor/crashpad.exe", 900),
            ("Vendor/Editor/readme.txt", 999),
            ("Player/big.exe", 500),               // no name match, so the biggest wins
            ("Player/small.exe", 20),
            ("Deep/Down/Here/hidden.exe", 10),     // past the depth limit
            ("Setup/unins000.exe", 400),           // an uninstaller is not an app
        ] {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, vec![0; size]).unwrap();
        }
        let mut found = Vec::new();
        apps_in(&dir, 2, &mut found);
        let found = sorted(found);
        fs::remove_dir_all(&dir).unwrap();
        // sorted() orders by name: "big" before "Editor".
        assert_eq!(found, vec![dir.join("Player/big.exe"), dir.join("Vendor/Editor/Editor.exe")]);
        // %SystemRoot% is "C:\WINDOWS" on most machines: the check must not care about case.
        assert!(!can_lock(Path::new(r"C:\Windows\System32\winlogon.exe")), "the desktop must stay unlockable");
        assert!(!can_lock(Path::new(r"c:\windows\explorer.exe")), "the desktop must stay unlockable");
        assert!(can_lock(Path::new(r"C:\Windows Apps\App\app.exe")), "a folder that merely starts the same way is fine");
        assert!(!can_lock(Path::new(r"C:\Program Files\App\notes.txt")));
        assert!(can_lock(Path::new(r"C:\Program Files\App\app.exe")));

        // Real binaries, so the PE parsing is checked against what Windows actually ships.
        let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        assert!(is_gui_app(&system.join("explorer.exe")), "explorer is a GUI app");
        assert!(!is_gui_app(&system.join(r"System32\cmd.exe")), "cmd is a console tool, not an app");
        assert!(is_gui_app(Path::new("deskmonk-no-such-file.exe")), "unreadable must not hide an app");
    }

    /// Program Files is handed to us twice under two names, so every app appeared twice.
    #[cfg(windows)]
    #[test]
    fn walks_each_program_folder_once() {
        let dirs = |v: &[&str]| distinct(v.iter().map(PathBuf::from).collect()).len();
        // ProgramFiles and ProgramW6432, as Windows actually sets them.
        assert_eq!(dirs(&[r"C:\Program Files", r"C:\Program Files (x86)", r"C:\Program Files"]), 2);
        assert_eq!(dirs(&[r"C:\Program Files", r"c:\program files\"]), 1, "case and a trailing slash are the same folder");
        assert_eq!(dirs(&[r"C:\Program Files", r"C:\Program Files (x86)"]), 2, "these are genuinely different");
    }

    /// Windows reports process names in whatever case it likes; matching must not care.
    #[cfg(windows)]
    #[test]
    fn matches_process_names_whatever_the_case() {
        let app = Path::new(r"C:\Program Files\Thing\Thing.exe");
        assert!(runs_from(Path::new("Thing.exe"), app));
        assert!(runs_from(Path::new("THING.EXE"), app));
        assert!(runs_from(Path::new("thing.exe"), app));
        assert!(!runs_from(Path::new("other.exe"), app));
    }

    #[test]
    fn password_round_trip() {
        let mut config = Config::default();
        assert!(!config.check(""));
        config.set_password("gizli123");
        assert!(config.check("gizli123") && !config.check("gizli124") && !config.check(""));
    }

    #[test]
    fn stops_new_launches_until_unlocked_and_relocks_after_quit() {
        let app = PathBuf::from("/Applications/Notes.app");
        let running = vec![(42, app.join("Contents/MacOS/Notes"))];
        let translocated = vec![(43, PathBuf::from("/private/var/folders/x/AppTranslocation/y/d/Notes.app/Contents/MacOS/Notes"))];
        let t0 = Instant::now();
        let mut state = State { apps: vec![app.clone()], ..Default::default() };

        // Fresh launch: stopped, one prompt, no second prompt while it is open.
        assert_eq!(state.check(&running, t0), (vec![42], vec![app.clone()]));
        assert_eq!(state.check(&translocated, t0), (vec![43], vec![]));
        // Unlocked: left alone while running, locked again after quitting.
        state.pending.clear();
        state.allowed.insert(app.clone(), Some(t0));
        assert_eq!(state.check(&[], t0 + Duration::from_secs(5)), (vec![], vec![]));
        assert_eq!(state.check(&running, t0 + Duration::from_secs(60)), (vec![], vec![]));
        // Quit, then reopened at once: asks again (no grace after quitting).
        assert_eq!(state.check(&[], t0 + Duration::from_secs(61)), (vec![], vec![]));
        assert_eq!(state.check(&running, t0 + Duration::from_secs(62)).0, vec![42]);
        // Unlocked but never started within the grace period: locked again.
        let mut state = State { apps: vec![app.clone()], ..Default::default() };
        state.allowed.insert(app.clone(), Some(t0));
        assert_eq!(state.check(&[], t0 + Duration::from_secs(16)), (vec![], vec![]));
        assert_eq!(state.check(&running, t0 + Duration::from_secs(17)).0, vec![42]);

        // Already running when locked: never killed.
        let mut state = State { apps: vec![app.clone()], ..Default::default() };
        state.allow_running(&running, t0);
        assert_eq!(state.check(&running, t0), (vec![], vec![]));
        assert!(!runs_from(Path::new("/Applications/Notes Helper.app/x"), &app));
    }
}
