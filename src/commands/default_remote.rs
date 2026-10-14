//! `aoraki default [remote]` — which console commands target by default.
//! With a name: re-point [defaults].remote (same as `whoami --default`).
//! Without: show the remotes and → the current default (no network).

use crate::config;
use anyhow::{bail, Result};

pub fn run(remote: Option<String>) -> Result<()> {
    let global = config::load_global()?;
    if global.remotes.is_empty() {
        println!("no remotes configured — run `aoraki login`");
        return Ok(());
    }
    let names: Vec<&str> = global.remotes.keys().map(String::as_str).collect();
    let Some(name) = remote else {
        let current = global.defaults.remote.as_deref();
        for (n, cfg) in &global.remotes {
            let marker = if Some(n.as_str()) == current {
                "→"
            } else {
                " "
            };
            let org = cfg
                .org
                .as_deref()
                .map(|o| format!(" (org: {o})"))
                .unwrap_or_default();
            println!("{marker} {n}: {}{org}", cfg.api_url);
        }
        if current.is_none() {
            println!("\nno default set — `aoraki default <remote>` to pick one");
        }
        return Ok(());
    };
    if !global.remotes.contains_key(&name) {
        bail!("unknown remote '{name}' (configured: {})", names.join(", "));
    }
    config::write_default_remote(&name)?;
    println!("✓ default remote is now '{name}'");
    Ok(())
}
