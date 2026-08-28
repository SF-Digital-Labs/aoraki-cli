//! `aoraki connect github` — one-time GitHub App install for the org:
//! opens the install page in the browser, then polls the gateway until the
//! installation is recorded.

use anyhow::{bail, Result};

use crate::config;
use crate::gateway::Gateway;

pub fn run(provider: String) -> Result<()> {
    if provider != "github" {
        bail!("unknown provider '{provider}' — only 'github' is supported");
    }
    let global = config::load_global()?;
    let gw = Gateway::from_global(&global)?;

    let before = gw.get("/github/installations")?["data"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);

    let resp = gw.post("/github/connect", serde_json::json!({}))?;
    let install_url = resp["data"]["install_url"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if install_url.is_empty() {
        bail!("gateway did not return an install URL");
    }

    println!("→ opening GitHub to install the Manifest app:");
    println!("  {install_url}");
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener).arg(&install_url).status();

    println!("→ pick the repo(s) to grant read access, then come back here…");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_secs(3));
        let now = gw.get("/github/installations")?["data"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if now.len() > before {
            let account = now[0]["account_login"].as_str().unwrap_or("your account");
            println!("✓ GitHub connected ({account}) — `aoraki deploy` is ready");
            return Ok(());
        }
    }
    bail!("timed out waiting for the install (5 min) — finish it in the browser and re-run");
}
