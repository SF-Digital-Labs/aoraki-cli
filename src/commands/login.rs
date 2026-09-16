//! `aoraki login [remote] [--url …]` — the paste-a-key flow (v1; the
//! device flow can replace the paste step later without changing config).
//! Opens the console's CLI keys page, validates the pasted key against
//! GET /cli/me, then writes it into ~/.config/aoraki/config.toml.
//!
//! Naming: a remote is an org identity, so a NEW remote with no explicit
//! name is named after the org behind the pasted key (e.g. logging in with
//! a SarsonDigital key creates [remotes.sarsondigital]). `--url` is only
//! needed off the beaten path: new remotes point at the mainnet console —
//! where customers live — so onboarding is "install, `aoraki login`,
//! paste key".

use crate::{aoraki, config};
use anyhow::{bail, Context, Result};
use std::io::{BufRead, Write};

/// The customer console. Internal/testnet consoles are reached with --url.
const MAINNET_API_URL: &str = "https://aoraki.cloud/api/v1";

pub fn run(remote: Option<String>, url: Option<String>, no_browser: bool) -> Result<()> {
    let global = config::load_global()?;

    // Resolve the target console; the remote NAME may stay open until we
    // know which org the key belongs to.
    let mut defaulted_url = false;
    let (explicit_name, api_url, is_existing) = match (&remote, &url) {
        // Explicit --url always wins.
        (_, Some(url)) => (
            remote.clone(),
            url.trim_end_matches('/').to_string(),
            remote
                .as_deref()
                .is_some_and(|n| global.remotes.contains_key(n)),
        ),
        // Named, no URL: existing remote keeps its URL (re-login); a new
        // name means the mainnet console.
        (Some(name), None) => match global.remotes.get(name.as_str()) {
            Some(cfg) => (Some(name.clone()), cfg.api_url.clone(), true),
            None => {
                defaulted_url = true;
                (Some(name.clone()), MAINNET_API_URL.to_string(), false)
            }
        },
        // Bare `aoraki login`: re-login to the default/sole remote, or —
        // fresh machine — a new remote on mainnet, named after the org.
        (None, None) => {
            if global.remotes.is_empty() {
                defaulted_url = true;
                (None, MAINNET_API_URL.to_string(), false)
            } else {
                let (name, cfg) = global.resolve_remote(None)?;
                (Some(name.to_string()), cfg.api_url.clone(), true)
            }
        }
    };

    match &explicit_name {
        Some(name) => println!("Log in to '{name}' ({api_url})"),
        None => println!("Log in to {api_url} (remote will be named after your org)"),
    }
    if defaulted_url {
        println!("  (mainnet console — use --url for testnet or another Aoraki)");
    }
    let tokens_page = format!("{}/cli", console_base(&api_url));
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

    // New remote without an explicit name: name it after the org. A
    // same-named remote on a DIFFERENT console needs a human-chosen name.
    let name = match explicit_name {
        Some(name) => name,
        None => {
            let derived = slugify(&identity.org);
            if let Some(existing) = global.remotes.get(derived.as_str()) {
                if existing.api_url != api_url && !is_existing {
                    bail!(
                        "a remote named '{derived}' already exists for {} — \
                         run `aoraki login <name> --url {api_url}` to pick a name",
                        existing.api_url
                    );
                }
            }
            derived
        }
    };

    config::write_remote(&name, &api_url, Some(token), Some(&identity.org), identity.org_hex.as_deref())?;
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

/// Org name → remote name: lowercase, alphanumerics kept, runs of anything
/// else collapse to '-'. "Sarson Funds" → "sarson-funds".
fn slugify(org: &str) -> String {
    let mut out = String::with_capacity(org.len());
    let mut last_hyphen = false;
    for c in org.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_hyphen = false;
        } else if !last_hyphen && !out.is_empty() {
            out.push('-');
            last_hyphen = true;
        }
    }
    let s = out.trim_end_matches('-').to_string();
    if s.is_empty() {
        "default".into()
    } else {
        s
    }
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

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn org_names_become_clean_slugs() {
        assert_eq!(slugify("SarsonDigital"), "sarsondigital");
        assert_eq!(slugify("Sarson Funds"), "sarson-funds");
        assert_eq!(slugify("  Acme, Inc. "), "acme-inc");
        assert_eq!(slugify("日本"), "default");
    }
}
