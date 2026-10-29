//! Opt-in anonymous usage stats. Never sends file names or paths, only counts and byte totals.
//! Disabled entirely unless built with `CLEANSPACE_TELEMETRY_URL=https://host/api/events`.

use crate::scan::{Category, Item};
use std::fs;
use std::path::PathBuf;

pub const URL: Option<&str> = option_env!("CLEANSPACE_TELEMETRY_URL");

pub struct Telemetry {
    pub install_id: String,
    /// None = user has not been asked yet.
    pub consent: Option<bool>,
}

fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("cleanspace").join("telemetry"))
}

impl Telemetry {
    /// File format: line 1 install id, line 2 "yes"/"no" (absent = not asked).
    pub fn load() -> Self {
        let text = config_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .unwrap_or_default();
        let mut lines = text.lines();
        let install_id = lines.next().filter(|l| !l.is_empty()).map(str::to_owned);
        let consent = lines.next().map(|l| l == "yes");
        Self {
            install_id: install_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            consent,
        }
    }

    pub fn set_consent(&mut self, yes: bool) {
        self.consent = Some(yes);
        if let Some(path) = config_path() {
            let _ = fs::create_dir_all(path.parent().unwrap());
            let _ = fs::write(
                path,
                format!("{}\n{}\n", self.install_id, if yes { "yes" } else { "no" }),
            );
        }
    }

    /// True when the app should ask for consent.
    pub fn needs_prompt(&self) -> bool {
        URL.is_some() && self.consent.is_none()
    }

    pub fn scan_done(&self, client: &str, files_scanned: usize, items: &[Item]) {
        let bytes = |c: Category| {
            items
                .iter()
                .filter(|i| i.category == c)
                .map(|i| i.size)
                .sum::<u64>()
        };
        self.send(serde_json::json!({
            "kind": "scan",
            "client": client,
            "files": files_scanned,
            "bytes": items.iter().map(|i| i.size).sum::<u64>(),
            "duplicate_bytes": bytes(Category::Duplicate),
            "junk_bytes": bytes(Category::Junk),
            "old_download_bytes": bytes(Category::OldDownload),
            "large_bytes": bytes(Category::Large),
        }));
    }

    pub fn clean_done(&self, client: &str, files: usize, bytes: u64) {
        self.send(serde_json::json!({ "kind": "clean", "client": client, "files": files, "bytes": bytes }));
    }

    fn send(&self, mut event: serde_json::Value) {
        let (Some(url), Some(true)) = (URL, self.consent) else {
            return;
        };
        event["install_id"] = self.install_id.clone().into();
        event["version"] = env!("CARGO_PKG_VERSION").into();
        event["os"] = std::env::consts::OS.into();
        // Fire and forget: stats must never block or break cleaning.
        std::thread::spawn(move || {
            let _ = ureq::post(url)
                .header("Content-Type", "application/json")
                .send(event.to_string());
        });
    }
}
