//! `aoraki whoami [remote]` — which account each stored key maps to.
//! Also a liveness probe: a passing check counts as usage and extends the
//! key's 90-day idle window.

use crate::{aoraki, config};
use anyhow::Result;

pub fn run(remote: Option<String>) -> Result<()> {
    let global = config::load_global()?;
    if global.remotes.is_empty() {
        println!("no remotes configured — run `aoraki login --url <aoraki-api-url>`");
        return Ok(());
    }

    let selected: Vec<(&str, &config::RemoteConfig)> = match &remote {
        Some(name) => vec![global.resolve_remote(Some(name))?],
        None => global.remotes.iter().map(|(k, v)| (k.as_str(), v)).collect(),
    };

    let mut failed = false;
    for (name, cfg) in selected {
        match &cfg.token {
            None => println!("{name}: {} — not logged in", cfg.api_url),
            Some(token) => match aoraki::whoami(&cfg.api_url, token) {
                Ok(id) => {
                    let expires = id.expires_at.split('T').next().unwrap_or_default();
                    println!(
                        "{name}: {} — {} (org: {}, key: {}, extended to {})",
                        cfg.api_url,
                        id.user.as_deref().unwrap_or("you"),
                        id.org,
                        id.token_name,
                        expires,
                    );
                }
                Err(err) => {
                    println!("{name}: {} — ✗ {err}", cfg.api_url);
                    failed = true;
                }
            },
        }
    }
    if failed {
        anyhow::bail!("some remotes failed — log in again where needed");
    }
    Ok(())
}
