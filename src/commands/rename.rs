//! `aoraki rename <old> <new>` — rename a remote in the global config.
//! Remotes are org identities, so names like "sarsondigital" read better
//! than machine-ish ones; this migrates old setups without re-pasting keys.
//!
//! Repo pins: environments pin remotes in a COMMITTED aoraki.toml, so
//! other checkouts/machines can't be reached from here — but the repo
//! you're standing in can: its pins are rewritten in the working tree
//! (you commit the change like any other).

use crate::config;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

pub fn run(old: String, new: String) -> Result<()> {
    if old == new {
        bail!("'{old}' and '{new}' are the same name");
    }
    let global = config::load_global()?;
    if !global.remotes.contains_key(&old) {
        bail!(
            "unknown remote '{}' (configured: {})",
            old,
            global.remotes.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    }
    if global.remotes.contains_key(&new) {
        bail!("a remote named '{new}' already exists");
    }
    let was_default = global.defaults.remote.as_deref() == Some(old.as_str());
    config::rename_remote(&old, &new)?;
    println!("✓ remote '{old}' is now '{new}'");
    if was_default {
        println!("  [defaults].remote updated to '{new}'");
    }

    // Fix pins in the repo we're standing in (working-tree edit — commit it).
    match update_repo_pins(&old, &new)? {
        Some((path, 0)) => println!(
            "  {} has no remote = \"{old}\" pins — nothing to update there",
            path.display()
        ),
        Some((path, n)) => println!(
            "  updated {n} pin{} in {} — commit that change",
            if n == 1 { "" } else { "s" },
            path.display()
        ),
        None => {}
    }
    println!(
        "  other repos: grep for 'remote = \"{old}\"' in their aoraki.toml / manifest.toml"
    );
    Ok(())
}

/// Walk up from cwd to the nearest aoraki.toml/manifest.toml and rewrite
/// `remote = "old"` pins under [environments.*]. Returns (path, count), or
/// None when not inside a repo with a config.
fn update_repo_pins(old: &str, new: &str) -> Result<Option<(PathBuf, usize)>> {
    let mut dir = std::env::current_dir()?;
    let path = loop {
        if let Some(p) = ["aoraki.toml", "manifest.toml"]
            .iter()
            .map(|n| dir.join(n))
            .find(|p| p.exists())
        {
            break p;
        }
        if !dir.pop() {
            return Ok(None);
        }
    };

    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;
    let mut updated = 0usize;
    if let Some(envs) = doc.get_mut("environments").and_then(|e| e.as_table_mut()) {
        for (_, env) in envs.iter_mut() {
            if let Some(env) = env.as_table_mut() {
                if env.get("remote").and_then(|v| v.as_str()) == Some(old) {
                    env["remote"] = toml_edit::value(new);
                    updated += 1;
                }
            }
        }
    }
    if updated > 0 {
        std::fs::write(&path, doc.to_string())
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(Some((path, updated)))
}
