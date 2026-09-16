//! `aoraki login [remote] [--url …]` — the paste-a-key flow (v1; the
//! device flow can replace the paste step later without changing config).
//! Opens the console's CLI keys page, validates the pasted key against
//! GET /cli/me, then writes it into ~/.config/aoraki/config.toml.
//!
//! `--url` is only needed off the beaten path: a bare `aoraki login` (or a
//! new remote name) points at the mainnet console — the place customers
//! live — so onboarding is "install, `aoraki login`, paste key".

use crate::{aoraki, config};
use anyhow::{bail, Context, Result};
use std::io::{BufRead, Write};

/// The customer console. Internal/testnet consoles are reached with --url.
const MAINNET_API_URL: &str = "https://aoraki.cloud/api/v1";

pub fn run(remote: Option<String>, url: Option<String>, no_browser: bool) -> Result<()> {
    let global = config::load_global()?;

    let mut defaulted_url = false;
    let (name, api_url) = match (&remote, &url) {
        // Explicit --url wins; name defaults to "default".
        (_, Some(url)) => (
            remote.clone().unwrap_or_else(|| "default".to_string()),
            url.trim_end_matches('/').to_string(),
        ),
        // Named remote, no URL: existing keeps its URL (re-login); a new
        // name means the mainnet console.
        (Some(name), None) => match global.remotes.get(name.as_str()) {
            Some(cfg) => (name.clone(), cfg.api_url.clone()),
            None => {
                defaulted_url = true;
                (name.clone(), MAINNET_API_URL.to_string())
            }
        },
        // Bare `aoraki login`: re-login to the default/sole remote, or —
        // fresh machine — create "default" on mainnet.
        (None, None) => {
            if global.remotes.is_empty() {
                defaulted_url = true;
                ("default".to_string(), MAINNET_API_URL.to_string())
            } else {
                let (name, cfg) = global.resolve_remote(None)?;
                (name.to_string(), cfg.api_url.clone())
            }
        }
    };

    let tokens_page = format!("{}/cli", console_base(&api_url));
    println!("Log in to '{name}' ({api_url})");
    if defaulted_url {
        println!("  (mainnet console — use --url for testnet or another Aoraki)");
    }
    println!("Create a CLI key in the console: {tokens_page}");
    if !no_browser {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = std::process::Command::new(opener).arg(&tokens_page).status();
    }

    print!("Paste key (cli_…): ");
    std::io::stdout().flush()?;
    let mut token = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut token)
        .context("reading key from stdin")?;
    let token = token.trim();
    if token.is_empty() {
        bail!("no key entered");
    }
    if !token.starts_with("cli_") {
        bail!("that doesn't look like a CLI key (they start with cli_)");
    }

    let identity = aoraki::whoami(&api_url, token)?;

    config::write_remote(&name, &api_url, Some(token))?;
    let expires = identity.expires_at.split('T').next().unwrap_or_default();
    println!(
        "✓ logged in to '{}' as {} (org: {}, key: {})",
        name,
        identity.user.as_deref().unwrap_or("you"),
        identity.org,
        identity.token_name,
    );
    println!("  valid until {expires} if unused — every CLI call extends it 90 days");
    Ok(())
}

/// The console origin for an API url: strip everything after the host, so
/// https://aoraki.cloud/api/v1 → https://aoraki.cloud
fn console_base(api_url: &str) -> String {
    if let Some(scheme_end) = api_url.find("://") {
        if let Some(path_start) = api_url[scheme_end + 3..].find('/') {
            return api_url[..scheme_end + 3 + path_start].to_string();
        }
    }
    api_url.trim_end_matches('/').to_string()
}
