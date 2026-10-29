//! Regression test: the GUI starts, scans and keeps running, also when its config files are damaged.
//! It runs the real binary in a throwaway HOME. Needs a display, so it is skipped on Linux without one
//! (headless CI); the damaged-config case runs only where HOME moves the config folder (macOS, Linux).

#![cfg(feature = "gui")]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn has_display() -> bool {
    !cfg!(target_os = "linux") || std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn temp_home(name: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("deskmonk-start-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("Downloads")).unwrap();
    std::fs::write(home.join("Downloads/a.txt"), "same").unwrap();
    std::fs::write(home.join("Downloads/b.txt"), "same").unwrap();
    home
}

/// Runs the app for a few seconds with HOME at `home`; fails if it exits early or panics.
fn assert_starts(home: &Path) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_deskmonk"))
        // A folder argument scans it right away, so the scan runs during startup too.
        .arg(home)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("deskmonk binary runs");
    std::thread::sleep(Duration::from_secs(4));
    let exited = child.try_wait().unwrap();
    let _ = child.kill();
    let _ = child.wait();
    let mut stderr = String::new();
    let _ = child.stderr.take().unwrap().read_to_string(&mut stderr);
    assert!(exited.is_none(), "deskmonk exited during startup ({exited:?}):\n{stderr}");
    assert!(!stderr.contains("panicked"), "deskmonk panicked:\n{stderr}");
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn starts() {
    if has_display() {
        assert_starts(&temp_home("clean"));
    }
}

#[cfg(unix)]
#[test]
fn starts_with_damaged_config() {
    if !has_display() {
        return;
    }
    let home = temp_home("damaged");
    let config = if cfg!(target_os = "macos") { home.join("Library/Application Support/deskmonk") } else { home.join(".config/deskmonk") };
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("lang"), "zz-unknown").unwrap();
    std::fs::write(config.join("telemetry"), "\n\nmaybe\n").unwrap();
    std::fs::write(config.join("plan"), b"DM-????\nnot-a-number\n\xff\xfe\n-5\n").unwrap();
    // Two lines only: lines after the second are locked apps, which the watcher would act on.
    std::fs::write(config.join("applock"), "salt-without-hash\n").unwrap();
    assert_starts(&home);
}
