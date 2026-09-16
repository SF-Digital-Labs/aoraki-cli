//! `aoraki rename <old> <new>` — rename a remote in the global config.
//! Remotes are org identities, so names like "sarsondigital" read better
//! than machine-ish ones; this migrates old setups without re-pasting keys.

use crate::config;
use anyhow::{bail, Result};

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
    println!("  note: any aoraki.toml environment pinning remote = \"{old}\" needs the new name");
    Ok(())
}
