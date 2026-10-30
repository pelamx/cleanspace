//! App lock against real processes: a locked program is stopped and reported, the password opens it,
//! and once it quits it is locked again.
#![cfg(target_os = "linux")]

use deskmonk::applock::Watcher;
use std::process::{Child, Command};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

/// Waits up to 5 s for `done`.
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn exited(child: &mut Child) -> bool {
    child.try_wait().unwrap().is_some()
}

/// Our own copy of the program runs, so the lock can't touch the user's processes.
fn running_copies(exe: &std::path::Path) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_str()?.parse().ok()?, std::fs::read_link(e.path().join("exe")).ok()?)))
        .filter(|(_, p): &(u32, std::path::PathBuf)| p == exe)
        .map(|(pid, _)| pid)
        .collect()
}

#[test]
fn locks_unlocks_and_relocks_a_real_program() {
    let dir = std::env::temp_dir().join(format!("deskmonk-lock-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("locked-sleep");
    std::fs::copy("/bin/sleep", &exe).unwrap();
    let exe = std::fs::canonicalize(exe).unwrap();
    let app = dir.join("locked-sleep.desktop");
    std::fs::write(&app, format!("[Desktop Entry]\nType=Application\nName=Sleepy\nExec=\"{}\" 30 %U\n", exe.display())).unwrap();

    let (tx, rx) = channel();
    let watcher = Watcher::start(vec![app.clone()], move |a| tx.send(a).unwrap());

    // Launched: stopped, and the password is asked for this app.
    let mut first = Command::new(&exe).arg("30").spawn().unwrap();
    assert!(eventually(|| exited(&mut first)), "locked program kept running");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), app);

    // Right password: deskMonk opens it (field code dropped), and it keeps running.
    watcher.unlock(&app);
    assert!(eventually(|| running_copies(&exe).len() == 1), "unlocked program did not open");
    std::thread::sleep(Duration::from_secs(1));
    let opened = running_copies(&exe);
    assert_eq!(opened.len(), 1, "unlocked program was stopped");

    // Quit, then launched again: locked again.
    unsafe { libc::kill(opened[0] as i32, libc::SIGTERM) };
    assert!(eventually(|| running_copies(&exe).is_empty()));
    std::thread::sleep(Duration::from_millis(700));
    let mut again = Command::new(&exe).arg("30").spawn().unwrap();
    assert!(eventually(|| exited(&mut again)), "program was not locked again after quitting");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), app);

    std::fs::remove_dir_all(&dir).unwrap();
}
