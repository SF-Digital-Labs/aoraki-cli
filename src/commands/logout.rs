//! `aoraki logout [remote]` — drop the stored token; the remote's URL is
//! kept so `aoraki login` can reuse it. Revoke the token in the console
//! too if the machine is compromised — logout only forgets the local copy.

use crate::config;
use anyhow::Result;

pub fn run(remote: Option<String>) -> Result<()> {
    let global = config::load_global()?;
    let (name, cfg) = global.resolve_remote(remote.as_deref())?;
    if cfg.token.is_none() {
        println!("'{name}' has no stored token — nothing to do");
        return Ok(());
    }
    let (name, api_url) = (name.to_string(), cfg.api_url.clone());
    config::write_remote(&name, &api_url, None)?;
    println!("✓ logged out of '{name}' — token removed from config.toml");
    println!("  (revoke it in the console too if this machine is no longer trusted)");
    Ok(())
}
