use crate::config::GlobalConfig;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Deploy event mirrored to the Aoraki console (`aoraki` app in the tree
/// monorepo). Field names align with the box agent contract (box_id,
/// build_ref, environment) so v2 agent-reported state correlates cleanly.
#[derive(Serialize, Deserialize)]
pub struct DeployEvent {
    pub app: String,
    pub environment: String,
    pub box_id: String,
    pub namespace: String,
    pub build_ref: String,
    pub ref_name: String,
    pub status: String,
    pub started_at: String,
    pub duration_secs: u64,
    pub actor: String,
    pub cli_version: String,
}

/// Who a CLI token belongs to, from GET /cli/me.
#[derive(Deserialize)]
pub struct Identity {
    pub user: Option<String>,
    pub org: String,
    pub token_name: String,
    pub expires_at: String,
}

/// Best-effort, never blocks or fails a deploy: on error the event is queued
/// locally (per remote) and retried the next time a deploy reports there.
pub fn report(global: &GlobalConfig, remote_pref: Option<&str>, event: &DeployEvent) {
    let (name, cfg) = match global.resolve_remote(remote_pref) {
        Ok(found) => found,
        Err(err) => {
            eprintln!("note: deploy event not reported — {err}");
            return;
        }
    };
    let Some(token) = &cfg.token else {
        eprintln!("note: remote '{name}' has no token — run `aoraki login {name}`");
        return;
    };
    flush_pending(name, &cfg.api_url, token);
    match post(&cfg.api_url, token, event) {
        Ok(()) => println!("→ deploy event reported to aoraki ({name})"),
        Err(err) => {
            eprintln!("warning: could not report deploy to '{name}' ({err}); queued for retry");
            queue(name, event);
        }
    }
}

/// Validate a token and name its owner. Any call counts as usage on the
/// server, restarting the 90-day idle-expiry clock.
pub fn whoami(api_url: &str, token: &str) -> Result<Identity> {
    let response = ureq::get(&format!("{}/cli/me", api_url.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call();
    match response {
        Ok(res) => {
            #[derive(Deserialize)]
            struct Envelope {
                data: Identity,
            }
            Ok(res.into_json::<Envelope>()?.data)
        }
        Err(ureq::Error::Status(401 | 403, _)) => {
            bail!("token rejected — it may be revoked or expired (90 days unused)")
        }
        Err(err) => bail!("could not reach aoraki: {err}"),
    }
}

fn post(api_url: &str, token: &str, event: &DeployEvent) -> Result<()> {
    ureq::post(&format!("{}/cli/deploys", api_url.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(5))
        .send_json(event)?;
    Ok(())
}

/// Queued events live under a per-remote directory so an event meant for
/// one console is never replayed into another.
fn pending_dir(remote: &str) -> PathBuf {
    crate::config::config_home()
        .join("pending-events")
        .join(remote)
}

fn queue(remote: &str, event: &DeployEvent) {
    let dir = pending_dir(remote);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let name = format!(
        "{}-{}.json",
        chrono::Utc::now().timestamp_millis(),
        event.build_ref
    );
    if let Ok(body) = serde_json::to_vec_pretty(event) {
        let _ = std::fs::write(dir.join(name), body);
    }
}

fn flush_pending(remote: &str, api_url: &str, token: &str) {
    let dir = pending_dir(remote);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<DeployEvent>(&text) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        if post(api_url, token, &event).is_ok() {
            let _ = std::fs::remove_file(&path);
        } else {
            break; // aoraki still unreachable; try again next time
        }
    }
}
