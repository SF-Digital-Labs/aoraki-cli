//! `aoraki whoami [remote] [--default <remote>]` — the one view of "who is
//! this CLI": every remote, the account behind its key, and → marking the
//! default (the console commands use when an environment doesn't pin one).
//! Also a liveness probe: a passing check counts as usage and extends the
//! key's 90-day idle window. `--default` re-points [defaults].remote.

use crate::{aoraki, config};
use anyhow::{bail, Result};

pub fn run(remote: Option<String>, set_default: Option<String>) -> Result<()> {
    let mut global = config::load_global()?;
    if global.remotes.is_empty() {
        println!("no remotes configured — run `aoraki login`");
        return Ok(());
    }

    if let Some(name) = &set_default {
        if !global.remotes.contains_key(name) {
            bail!(
                "unknown remote '{}' (configured: {})",
                name,
                global
                    .remotes
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        config::write_default_remote(name)?;
        global.defaults.remote = Some(name.clone());
        println!("✓ default remote is now '{name}'");
    }

    let default = global.defaults.remote.as_deref();
    let selected: Vec<(&str, &config::RemoteConfig)> = match &remote {
        Some(name) => vec![global.resolve_remote(Some(name))?],
        None => global.remotes.iter().map(|(k, v)| (k.as_str(), v)).collect(),
    };

    let mut failed = false;
    // Self-healing: record each remote's org so committed aoraki.toml
    // files can pin the org instead of a machine-local name.
    let mut org_updates: Vec<(String, String, String, String, Option<String>)> = Vec::new();
    for (name, cfg) in selected {
        let marker = if Some(name) == default { "→" } else { " " };
        match &cfg.token {
            None => println!("{marker} {name}: {} — not logged in", cfg.api_url),
            Some(token) => match aoraki::whoami(&cfg.api_url, token) {
                Ok(id) => {
                    let expires = id.expires_at.split('T').next().unwrap_or_default();
                    println!(
                        "{marker} {name}: {} — {} (org: {}, key: {}, extended to {})",
                        cfg.api_url,
                        id.user.as_deref().unwrap_or("you"),
                        id.org,
                        id.token_name,
                        expires,
                    );
                    if cfg.org.as_deref() != Some(id.org.as_str())
                        || (id.org_hex.is_some() && cfg.org_id != id.org_hex)
                    {
                        org_updates.push((
                            name.to_string(),
                            cfg.api_url.clone(),
                            token.clone(),
                            id.org.clone(),
                            id.org_hex.clone(),
                        ));
                    }
                }
                Err(err) => {
                    println!("{marker} {name}: {} — ✗ {err}", cfg.api_url);
                    failed = true;
                }
            },
        }
    }
    for (name, api_url, token, org, org_id) in org_updates {
        let _ = config::write_remote(&name, &api_url, Some(&token), Some(&org), org_id.as_deref());
    }
    if remote.is_none() && default.is_none() && global.remotes.len() > 1 {
        println!("\nno default set — `aoraki whoami --default <remote>` to pick one");
    }
    if failed {
        bail!("some remotes failed — log in again where needed");
    }
    Ok(())
}
