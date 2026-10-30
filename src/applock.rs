//! App lock (macOS, Linux): opening a locked app needs the deskMonk password.
//! A privacy lock while deskMonk runs, not a security boundary: quitting deskMonk lifts it.
//! An app is a .app bundle on macOS and a .desktop launcher on Linux.

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// File format: line 1 salt, line 2 password hash, then one locked app path per line.
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

/// What an app is on this platform: a bundle folder on macOS, a launcher file on Linux.
pub const EXT: &str = if cfg!(target_os = "linux") { "desktop" } else { "app" };

/// Apps that must never be locked: killing them breaks the desktop or relaunches in a loop.
#[cfg(not(target_os = "linux"))]
pub fn can_lock(app: &Path) -> bool {
    app.extension().is_some_and(|e| e == EXT)
        && !app.starts_with("/System/Library/CoreServices")
        && std::env::current_exe().map_or(true, |me| !me.starts_with(app))
}

/// Launchers the menu shows, not deskMonk itself. Terminal apps are left out: deskMonk could not reopen them in a terminal.
#[cfg(target_os = "linux")]
pub fn can_lock(app: &Path) -> bool {
    let Ok(text) = fs::read_to_string(app) else { return false };
    let key = |k| desktop_key(&text, k);
    app.extension().is_some_and(|e| e == EXT)
        && key("Type") == Some("Application")
        && key("Exec").is_some()
        && key("NoDisplay") != Some("true")
        && key("Hidden") != Some("true")
        && key("Terminal") != Some("true")
        && app.file_stem() != Some("deskmonk".as_ref())
        && std::env::current_exe().map_or(true, |me| exe_of(&text) != Some(me))
}

/// Shown name: the bundle name on macOS, the launcher's Name on Linux.
pub fn name(app: &Path) -> String {
    let stem = || app.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    #[cfg(target_os = "linux")]
    {
        // Read once: the app list asks for every name on every frame.
        static NAMES: std::sync::LazyLock<Mutex<HashMap<PathBuf, String>>> = std::sync::LazyLock::new(Default::default);
        let mut names = NAMES.lock().unwrap();
        names
            .entry(app.to_path_buf())
            .or_insert_with(|| fs::read_to_string(app).ok().and_then(|t| desktop_key(&t, "Name").map(String::from)).unwrap_or_else(stem))
            .clone()
    }
    #[cfg(not(target_os = "linux"))]
    stem()
}

/// Folders apps are installed in, most specific first.
fn app_dirs() -> Vec<PathBuf> {
    if cfg!(target_os = "linux") {
        // XDG data dirs, plus Flatpak and Snap exports in case the session did not add them.
        let system = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or("/usr/local/share:/usr/share".into());
        let mut dirs: Vec<PathBuf> = dirs::data_dir().into_iter().collect();
        dirs.extend(dirs::data_dir().map(|d| d.join("flatpak/exports/share")));
        dirs.extend(system.split(':').map(PathBuf::from));
        dirs.extend(["/var/lib/flatpak/exports/share", "/var/lib/snapd/desktop"].map(PathBuf::from));
        dirs.into_iter().map(|d| d.join("applications")).collect()
    } else {
        let mut dirs: Vec<PathBuf> = ["/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities"]
            .map(PathBuf::from)
            .into();
        dirs.extend(dirs::home_dir().map(|h| h.join("Applications")));
        dirs
    }
}

/// Lockable apps in the usual app folders (and one folder deeper, e.g. /Applications/Adobe X/), sorted by name.
/// A launcher in a more specific folder hides one with the same file name (XDG rule: ~/.local overrides /usr).
pub fn installed_apps() -> Vec<PathBuf> {
    let mut apps = Vec::new();
    for dir in app_dirs() {
        apps_in(&dir, 1, &mut apps);
    }
    let mut seen = HashSet::new();
    apps.retain(|a| seen.insert(a.file_name().map(|n| n.to_os_string())) && can_lock(a));
    apps.sort_by_cached_key(|a| name(a).to_lowercase());
    apps
}

fn apps_in(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    for path in fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()) {
        if path.extension().is_some_and(|e| e == EXT) {
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

/// A running process: pid, executable path, and (Linux) its cgroup, which names the app it was launched as.
type Proc = (i32, PathBuf, String);

/// Grace period for an unlocked app to start before "not running" means it quit.
const START_GRACE: Duration = Duration::from_secs(15);

impl State {
    /// One pass over the running processes: returns the pids to stop and the apps to ask a password for.
    fn check(&mut self, procs: &[Proc], now: Instant) -> (Vec<i32>, Vec<PathBuf>) {
        let (mut kill, mut ask) = (Vec::new(), Vec::new());
        for app in &self.apps {
            let runs = matcher(app);
            let pids: Vec<i32> = procs.iter().filter(|p| runs(p)).map(|p| p.0).collect();
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
    fn allow_running(&mut self, procs: &[Proc], now: Instant) {
        for app in &self.apps {
            if procs.iter().any(matcher(app)) {
                self.allowed.insert(app.clone(), Some(now));
            }
        }
    }
}

/// Matches by bundle name anywhere in the path, so a copy of the app (or macOS's App Translocation
/// running it from a random folder) is locked too. Helpers inside the bundle match as well.
#[cfg(not(target_os = "linux"))]
fn matcher(app: &Path) -> impl Fn(&Proc) -> bool + use<> {
    let name = app.file_name().map(|n| n.to_os_string());
    move |(_, process, _)| name.as_ref().is_some_and(|name| process.components().any(|c| c.as_os_str() == name))
}

/// Matches the launcher's program, or a process the desktop started for this launcher: GNOME, KDE, uwsm,
/// D-Bus activation and Flatpak put each launched app in a systemd scope named after its .desktop id, Snap in snap.<name>.*.
/// The scope also catches apps started through a wrapper (flatpak run, a shell script) that the program path misses.
#[cfg(target_os = "linux")]
fn matcher(app: &Path) -> impl Fn(&Proc) -> bool + use<> {
    let text = fs::read_to_string(app).unwrap_or_default();
    let exes = exes_of(&text);
    // Escaped as GNOME, KDE and Flatpak name the unit: bytes other than [A-Za-z0-9:_.] become \xNN.
    let id: String = app.file_stem().unwrap_or_default().to_string_lossy().bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b":_.".contains(&b) { (b as char).to_string() } else { format!("\\x{b:02x}") })
        .collect();
    let snap = desktop_key(&text, "X-SnapInstanceName").map(|s| format!("snap.{s}."));
    move |(_, process, cgroup)| {
        let unit = cgroup.lines().filter_map(|l| l.rsplit('/').next()).collect::<Vec<_>>();
        exes.contains(process)
            || unit.iter().any(|u| {
                u.starts_with("app-") && [format!("-{id}-"), format!("-{id}@"), format!("-{id}.scope")].iter().any(|p| u.contains(p.as_str()))
                    || snap.as_ref().is_some_and(|s| u.starts_with(s.as_str()))
            })
    }
}

/// A value of the [Desktop Entry] group (unlocalized).
#[cfg(target_os = "linux")]
fn desktop_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some((k, v)) = line.split_once('=') && k.trim() == key {
            return Some(v.trim());
        }
    }
    None
}

/// The Exec line as arguments: quotes and escapes resolved, field codes (%U, %f…) and Flatpak's @@ markers dropped.
#[cfg(target_os = "linux")]
fn exec_args(text: &str) -> Vec<String> {
    let (mut args, mut arg, mut quoted, mut chars) = (Vec::new(), None::<String>, false, desktop_key(text, "Exec").unwrap_or("").chars());
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                arg.get_or_insert_default();
            }
            '\\' if quoted => arg.get_or_insert_default().extend(chars.next()),
            c if c.is_whitespace() && !quoted => args.extend(arg.take()),
            c => arg.get_or_insert_default().push(c),
        }
    }
    args.extend(arg);
    args.retain(|a| !(a.len() == 2 && a.starts_with('%') && a != "%%") && !a.starts_with("@@"));
    args.iter_mut().for_each(|a| *a = a.replace("%%", "%"));
    args
}

/// The program the launcher runs, resolved through PATH and symlinks; None for a generic launcher
/// (flatpak, snap, a shell or interpreter), whose path every other app started that way shares.
#[cfg(target_os = "linux")]
fn exe_of(text: &str) -> Option<PathBuf> {
    let args = exec_args(text);
    let program = args.iter().find(|a| !(*a == "env" || a.ends_with("/env") || (a.contains('=') && !a.starts_with('/'))))?;
    let path = if program.contains('/') {
        PathBuf::from(program)
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(program)).find(|p| p.is_file())?
    };
    let path = fs::canonicalize(path).ok()?;
    (!is_generic(&path)).then_some(path)
}

/// A program many apps run through: matching its path would stop all of them.
#[cfg(target_os = "linux")]
fn is_generic(path: &Path) -> bool {
    let base = path.file_name().unwrap_or_default().to_string_lossy();
    let generic = ["flatpak", "snap", "sh", "bash", "dash", "zsh", "env", "java", "perl", "node", "wine", "gio", "xdg-open", "sudo", "pkexec", "dotnet", "mono", "ruby", "php"];
    generic.contains(&base.as_ref()) || base.starts_with("python") || base.starts_with("electron")
}

/// The launcher's program, and what it turns into if it is a script: many launchers run a wrapper that
/// execs the real program (Brave, Chrome, Signal, Spotify), so the wrapper's path is never seen running.
#[cfg(target_os = "linux")]
fn exes_of(text: &str) -> Vec<PathBuf> {
    let mut exes: Vec<PathBuf> = exe_of(text).into_iter().collect();
    let mut next = 0;
    // Chrome goes through two scripts; the limit is only there to end a loop of scripts.
    while next < exes.len() && next < 8 {
        for target in exec_targets(&exes[next]) {
            if !exes.contains(&target) {
                exes.push(target);
            }
        }
        next += 1;
    }
    exes
}

/// The programs a script starts: `exec /path` and `$DIR/name` beside the script, with or without exec (Brave Origin).
/// A plain `/path` line is not followed: it may be a helper like notify-send, which other programs run too.
/// shortcut: `exec electron app.asar` (Obsidian) is skipped, telling it apart needs the process's arguments.
#[cfg(target_os = "linux")]
fn exec_targets(script: &Path) -> Vec<PathBuf> {
    use std::io::Read;
    let mut head = Vec::new();
    // Read only the start: this runs on every watcher pass, and the program may be a huge binary.
    if fs::File::open(script).and_then(|f| f.take(64 << 10).read_to_end(&mut head)).is_err() || !head.starts_with(b"#!") {
        return Vec::new();
    }
    let target = |line: &str| {
        let line = line.trim_start();
        let exec = line.strip_prefix("exec ");
        let mut words = exec.unwrap_or(line).split_whitespace();
        let mut word = words.next()?;
        while word.starts_with('-') {
            if word == "-a" {
                words.next();
            }
            word = words.next()?;
        }
        let word = word.trim_matches(['"', '\'']);
        let path = match word.chars().next()? {
            '/' if exec.is_some() => PathBuf::from(word),
            '$' => script.parent()?.join(word.split_once('/')?.1),
            _ => return None,
        };
        let path = fs::canonicalize(path).ok()?;
        (path.is_file() && !is_generic(&path)).then_some(path)
    };
    String::from_utf8_lossy(&head).lines().filter_map(target).collect()
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
        open(app);
    }

    /// Prompt closed without the password: the next launch asks again.
    pub fn dismiss(&self, app: &Path) {
        self.0.lock().unwrap().pending.remove(app);
    }
}

#[cfg(target_os = "macos")]
fn processes() -> Vec<Proc> {
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
            (len > 0).then(|| (pid, PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len as usize])), String::new()))
        })
        .collect()
}

/// The user's own processes only: someone else's (or root's) are never stopped.
#[cfg(target_os = "linux")]
fn processes() -> Vec<Proc> {
    use std::os::unix::fs::MetadataExt;
    let (me, uid) = (std::process::id() as i32, unsafe { libc::getuid() });
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let pid: i32 = e.file_name().to_str()?.parse().ok()?;
            if pid == me || e.metadata().ok()?.uid() != uid {
                return None;
            }
            // A sandboxed app's program may be unreadable; its cgroup still names it. Updated programs read "… (deleted)".
            let exe = fs::read_link(e.path().join("exe")).map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)"))).unwrap_or_default();
            Some((pid, exe, fs::read_to_string(e.path().join("cgroup")).unwrap_or_default()))
        })
        .collect()
}

// shortcut: no app lock on Windows; it would need its own process listing.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn processes() -> Vec<Proc> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn open(app: &Path) {
    let _ = std::process::Command::new("open").arg(app).spawn();
}

/// Runs the launcher's command; the child is waited for so it leaves no zombie behind.
#[cfg(target_os = "linux")]
fn open(app: &Path) {
    let args = fs::read_to_string(app).map(|t| exec_args(&t)).unwrap_or_default();
    let Some((program, rest)) = args.split_first() else { return };
    match std::process::Command::new(program).args(rest).current_dir(dirs::home_dir().unwrap_or("/".into())).spawn() {
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => eprintln!("could not open {}: {e}", app.display()),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn open(_: &Path) {}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_apps_one_folder_deep() {
        let dir = std::env::temp_dir().join(format!("deskmonk-apps-{}", std::process::id()));
        for app in ["A.{EXT}/Contents", "Suite/B.{EXT}", "Suite/Deep/C.{EXT}", "Utilities/D.{EXT}"] {
            fs::create_dir_all(dir.join(app.replace("{EXT}", EXT))).unwrap();
        }
        let mut found = Vec::new();
        apps_in(&dir, 1, &mut found);
        found.sort();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, vec![dir.join(format!("A.{EXT}")), dir.join(format!("Suite/B.{EXT}"))]);
    }

    #[test]
    fn password_round_trip() {
        let mut config = Config::default();
        assert!(!config.check(""));
        config.set_password("gizli123");
        assert!(config.check("gizli123") && !config.check("gizli124") && !config.check(""));
    }

    /// The app under test and two of its processes: a plain launch and one the matcher must also catch
    /// (macOS: a translocated copy; Linux: started through a wrapper, recognized by its systemd scope).
    #[cfg(not(target_os = "linux"))]
    fn notes() -> (PathBuf, Vec<Proc>, Vec<Proc>) {
        let app = PathBuf::from("/Applications/Notes.app");
        let running = vec![(42, app.join("Contents/MacOS/Notes"), String::new())];
        let other = vec![(43, PathBuf::from("/private/var/folders/x/AppTranslocation/y/d/Notes.app/Contents/MacOS/Notes"), String::new())];
        assert!(!matcher(&app)(&(1, PathBuf::from("/Applications/Notes Helper.app/x"), String::new())));
        (app, running, other)
    }

    #[cfg(target_os = "linux")]
    fn notes() -> (PathBuf, Vec<Proc>, Vec<Proc>) {
        let dir = std::env::temp_dir().join(format!("deskmonk-notes-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("notes");
        fs::write(&exe, "").unwrap();
        let app = dir.join("org.example.my-notes.desktop");
        fs::write(&app, format!("[Desktop Entry]\nType=Application\nName=My Notes\nExec=env A=1 \"{}\" %U\n", exe.display())).unwrap();
        let exe = fs::canonicalize(exe).unwrap();
        let scope = |unit: &str| (43, PathBuf::from("/usr/bin/bash"), format!("0::/user.slice/user-1000.slice/user@1000.service/app.slice/{unit}\n"));
        let runs = matcher(&app);
        assert!(runs(&scope("app-gnome-org.example.my\\x2dnotes-43.scope")));
        assert!(runs(&scope("app-Hyprland-org.example.my\\x2dnotes@abc.service")));
        assert!(!runs(&scope("app-gnome-org.example.my\\x2dnotes\\x2dbeta-43.scope")));
        assert!(!runs(&scope("app-gnome-org.example.other-43.scope")));
        // KDE: random suffix after the id.
        assert!(runs(&scope("app-org.example.my\\x2dnotes-0c8e2f1a9b7d4e6f8a1b2c3d4e5f6a7b.scope")));
        assert_eq!(name(&app), "My Notes");
        (app, vec![(42, exe, String::new())], vec![scope("app-flatpak-org.example.my\\x2dnotes-43.scope")])
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_launchers() {
        let entry = "[Desktop Entry]\nName=Files\nExec=flatpak run --branch=stable org.x.Files @@u %U @@ \"a b\" 100%%\n[Desktop Action new]\nName=New\n";
        assert_eq!(desktop_key(entry, "Name"), Some("Files"));
        assert_eq!(exec_args(entry), ["flatpak", "run", "--branch=stable", "org.x.Files", "a b", "100%"]);
        // A generic launcher's path would match every app started through it.
        assert_eq!(exe_of(entry), None);
        assert_eq!(exe_of("[Desktop Entry]\nExec=sh -c 'x'\n"), None);
        assert_eq!(desktop_key("[Desktop Action x]\nName=No\n", "Name"), None);
        // Escaped like systemd unit names, so ids with '+' or '@' still match their scope.
        let dir = std::env::temp_dir().join(format!("deskmonk-plus-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let app = dir.join("org.x.C++ IDE.desktop");
        fs::write(&app, "[Desktop Entry]\nExec=sh\n").unwrap();
        assert!(matcher(&app)(&(1, PathBuf::new(), "0::/app.slice/app-gnome-org.x.C\\x2b\\x2b\\x20IDE-9.scope\n".into())));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn follows_wrapper_scripts_but_not_into_shared_programs() {
        let dir = std::env::temp_dir().join(format!("deskmonk-wrap-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let dir = fs::canonicalize(&dir).unwrap();
        let write = |name: &str, text: &str| {
            let path = dir.join(name);
            fs::write(&path, text).unwrap();
            path
        };
        let launcher = |script: &Path| exes_of(&format!("[Desktop Entry]\nExec=\"{}\" %U\n", script.display()));

        let real = write("real", "\x7fELF");
        let electron = write("electron43", "\x7fELF");
        // Like Brave: an absolute path, after other shell lines.
        let brave = write("brave", &format!("#!/bin/bash\nexport A=1\nexec {} \"$@\"\n", real.display()));
        assert_eq!(launcher(&brave), [brave.clone(), real.clone()]);
        // Like Chrome: script to script, with redirects, `-a` and a path beside the script.
        let inner = write("inner", "#!/bin/bash\nexec < /dev/null\nexec > >(exec cat)\nexec 2> >(exec cat >&2)\nexec -a \"$0\" \"$HERE/real\" \"$@\"\n");
        let outer = write("outer", &format!("#!/bin/bash\nexec {} $FLAGS \"$@\"\n", inner.display()));
        assert_eq!(launcher(&outer), [outer.clone(), inner.clone(), real.clone()]);
        // Like Brave Origin: no exec, the script stays around as the program's parent.
        let plain = write("plain", "#!/bin/bash\nHERE=\"`dirname \"$0\"`\"\n[ -f \"$HERE/real\" ] || touch \"$HERE/x\"\n\"$HERE/real\" \"$@\" || true\n");
        assert_eq!(launcher(&plain), [plain.clone(), real.clone()]);
        // Stopping a shared program would stop every app that uses it, and a plain /path line may be such a helper.
        for line in [format!("exec {} app.asar", electron.display()), format!("{} --notify", real.display()), "exec electron43 app.asar".into(), "exec /bin/sh -c x".into(), "exec $GDB \"$dir/real\"".into(), "exec /no/such/file".into()] {
            let script = write("shared", &format!("#!/bin/sh\n{line}\n"));
            assert_eq!(launcher(&script), [script.clone()], "{line}");
        }
        // Not a script: its text is not looked at. Scripts that exec themselves still end.
        let binary = write("binary", &format!("\x7fELF\nexec {}\n", real.display()));
        assert_eq!(launcher(&binary), [binary.clone()]);
        let looping = dir.join("looping");
        fs::write(&looping, format!("#!/bin/sh\nexec {}\n", looping.display())).unwrap();
        assert_eq!(launcher(&looping), [looping.clone()]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stops_new_launches_until_unlocked_and_relocks_after_quit() {
        let (app, running, other) = notes();
        let t0 = Instant::now();
        let mut state = State { apps: vec![app.clone()], ..Default::default() };

        // Fresh launch: stopped, one prompt, no second prompt while it is open.
        assert_eq!(state.check(&running, t0), (vec![42], vec![app.clone()]));
        assert_eq!(state.check(&other, t0), (vec![43], vec![]));
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
    }
}
