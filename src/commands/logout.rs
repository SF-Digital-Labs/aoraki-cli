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
    // `aoraki deploy` logs docker into the org registry WITH this key, and
    // docker keeps it in its own credential store — forget it there too,
    // or the key outlives the logout. Best-effort: needs the console to
    // name the registry, and docker to be installed.
    if let Some(registry) = crate::console::Console::connect(Some(&name))
        .ok()
        .and_then(|c| c.registry)
    {
        let out = std::process::Command::new("docker")
            .args(["logout", &registry])
            .output();
        if out.is_ok_and(|o| o.status.success()) {
            println!("✓ docker logged out of {registry}");
        } else {
            eprintln!("⚠ could not run `docker logout {registry}` — run it yourself to drop the stored key");
        }
    }
    config::write_remote(&name, &api_url, None, None, None)?;
    println!("✓ logged out of '{name}' — token removed from config.toml");
    println!("  (revoke it in the console too if this machine is no longer trusted)");
    Ok(())
}
