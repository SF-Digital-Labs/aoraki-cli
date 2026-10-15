//! The once-a-day "a newer aoraki is out" line.
//!
//! At most once a day, a background thread asks GitHub for the latest
//! release and caches it in `update-check.json` next to the config. The
//! command itself never waits on that request; when it ends, if the
//! cached release is newer, one line on stderr says so (at most once a
//! day). Off when stderr isn't a terminal (scripts, CI), when `CI` is set,
//! or with `AORAKI_NO_UPDATE_CHECK=1`.

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::commands::update::{is_newer, latest_version};

const DAY: u64 = 24 * 60 * 60;
/// How long the end of a command may wait for an in-flight check.
const GRACE: Duration = Duration::from_millis(1500);

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    /// Unix seconds of the last GitHub check.
    #[serde(default)]
    checked_at: u64,
    #[serde(default)]
    latest: Option<String>,
    /// Unix seconds the notice was last shown.
    #[serde(default)]
    notified_at: u64,
}

pub struct Notice {
    refresh: Option<JoinHandle<()>>,
}

/// Start a background check if the cache is a day old. None = notices off.
pub fn start() -> Option<Notice> {
    use std::io::IsTerminal;
    if !std::io::stderr().is_terminal()
        || std::env::var_os("CI").is_some()
        || std::env::var_os("AORAKI_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty() && v != "0")
    {
        return None;
    }
    let refresh = (now().saturating_sub(load().checked_at) >= DAY).then(|| {
        std::thread::spawn(|| {
            // Recorded before asking, so a blocked or slow GitHub means a
            // retry tomorrow, not a wait on every command.
            let mut cache = load();
            cache.checked_at = now();
            save(&cache);
            if let Ok(v) = latest_version(Duration::from_secs(5)) {
                // Re-read: finish() may have saved since.
                let mut cache = load();
                cache.latest = Some(v);
                save(&cache);
            }
        })
    });
    Some(Notice { refresh })
}

impl Notice {
    /// After the command: show the line if a newer release is known.
    pub fn finish(self) {
        if let Some(handle) = self.refresh {
            let until = Instant::now() + GRACE;
            while !handle.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let mut cache = load();
        let current = env!("CARGO_PKG_VERSION");
        let Some(latest) = cache.latest.clone() else {
            return;
        };
        if let Some(line) = message(&latest, current, now(), cache.notified_at) {
            eprintln!("\n{line}");
            cache.notified_at = now();
            save(&cache);
        }
    }
}

/// The notice to show, if any: a newer release, not already shown today.
fn message(latest: &str, current: &str, now: u64, notified_at: u64) -> Option<String> {
    (is_newer(latest, current) && now.saturating_sub(notified_at) >= DAY)
        .then(|| format!("aoraki {latest} is available (you have {current}) — run `aoraki update`"))
}

fn path() -> PathBuf {
    crate::config::config_home().join("update-check.json")
}

fn load() -> Cache {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Best effort: a read-only config dir just means no caching.
fn save(cache: &Cache) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(cache) {
        // Write-then-rename so a concurrent run never reads half a file;
        // pid + counter keeps this process's two writers apart too.
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = p.with_extension(format!("json.{}.{n}", std::process::id()));
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &p).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_so_once_a_day_when_newer() {
        let m = message("0.6.0", "0.5.1", 10 * DAY, 0).unwrap();
        assert!(m.contains("0.6.0") && m.contains("aoraki update"), "{m}");
        assert!(message("0.6.0", "0.5.1", 10 * DAY, 10 * DAY - 60).is_none());
        assert!(message("0.6.0", "0.5.1", 10 * DAY, 9 * DAY).is_some());
    }

    #[test]
    fn quiet_when_up_to_date() {
        assert!(message("0.5.1", "0.5.1", 10 * DAY, 0).is_none());
        assert!(message("0.5.0", "0.5.1", 10 * DAY, 0).is_none());
    }
}
