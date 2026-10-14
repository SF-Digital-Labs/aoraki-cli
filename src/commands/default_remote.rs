//! `aoraki default [remote]` — which console commands target by default.
//! With a name: re-point [defaults].remote (`whoami --default` uses the
//! same `set`). Without: list the remotes, → the default (offline).

use crate::config::{self, GlobalConfig};
use anyhow::{bail, Result};

pub fn run(remote: Option<String>) -> Result<()> {
    let global = config::load_global()?;
    if global.remotes.is_empty() {
        println!("no remotes configured — run `aoraki login`");
        return Ok(());
    }
    match remote {
        Some(name) => set(&global, &name),
        None => {
            list(&global);
            Ok(())
        }
    }
}

/// Make `name` the default remote (must be configured).
pub fn set(global: &GlobalConfig, name: &str) -> Result<()> {
    if !global.remotes.contains_key(name) {
        let names: Vec<&str> = global.remotes.keys().map(String::as_str).collect();
        bail!("unknown remote '{name}' (configured: {})", names.join(", "));
    }
    config::write_default_remote(name)?;
    println!("✓ default remote is now '{name}'");
    Ok(())
}

fn list(global: &GlobalConfig) {
    let set_to = global.defaults.remote.as_deref();
    // With one remote and no default, that one IS the default.
    let sole = if global.remotes.len() == 1 {
        global.remotes.keys().next().map(String::as_str)
    } else {
        None
    };
    let effective = set_to.or(sole);
    for (n, cfg) in &global.remotes {
        let marker = if Some(n.as_str()) == effective {
            "→"
        } else {
            " "
        };
        let org = cfg
            .org
            .as_deref()
            .map(|o| format!(" (org: {o})"))
            .unwrap_or_default();
        let key = if cfg.token.is_none() {
            " — not logged in"
        } else {
            ""
        };
        println!("{marker} {n}: {}{org}{key}", cfg.api_url);
    }
    match set_to {
        Some(d) if !global.remotes.contains_key(d) => println!(
            "\nthe default '{d}' isn't a configured remote — `aoraki default <remote>` to fix"
        ),
        None if global.remotes.len() > 1 => {
            println!("\nno default set — `aoraki default <remote>` to pick one")
        }
        _ => {}
    }
}
