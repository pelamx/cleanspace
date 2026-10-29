//! App lock (macOS): opening a locked app needs the deskMonk password.
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
pub fn can_lock(app: &Path) -> bool {
    app.extension().is_some_and(|e| e == "app")
        && !app.starts_with("/System/Library/CoreServices")
        && std::env::current_exe().map_or(true, |me| !me.starts_with(app))
}

/// Lockable .app bundles in the usual app folders (and one folder deeper, e.g. /Applications/Adobe X/), sorted by name.
pub fn installed_apps() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities"]
        .map(PathBuf::from)
        .into();
    dirs.extend(dirs::home_dir().map(|h| h.join("Applications")));
    let mut apps = Vec::new();
    for dir in dirs {
        apps_in(&dir, 1, &mut apps);
    }
    apps.retain(|a| can_lock(a));
    apps.sort_by_key(|a| a.file_stem().unwrap_or_default().to_string_lossy().to_lowercase());
    apps.dedup();
    apps
}

fn apps_in(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    for path in fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        if path.extension().is_some_and(|e| e == "app") {
            out.push(path);
        } else if depth > 0 && path.is_dir() && path.file_name() != Some("Utilities".as_ref()) {
            apps_in(&path, depth - 1, out);
        }
    }
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
    app.file_name().is_some_and(|name| process.components().any(|c| c.as_os_str() == name))
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
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
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
        let _ = std::process::Command::new("open").arg(app).spawn();
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

// shortcut: app lock is macOS only; Windows/Linux would need their own process listing.
#[cfg(not(target_os = "macos"))]
fn processes() -> Vec<(i32, PathBuf)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

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
