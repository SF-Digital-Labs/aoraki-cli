//! `aoraki update [--check]` — update the CLI the way it was installed.
//!
//! The running binary's (resolved) path says how it got here:
//! - Homebrew (`…/Cellar/aoraki/…`): `brew upgrade sf-digital-labs/tap/aoraki`.
//! - the install script (`/usr/local/bin`, or wherever $AORAKI_INSTALL_DIR
//!   put it): re-run the official installer into that same directory — it
//!   downloads the latest release, checks its published SHA-256, proves the
//!   binary runs here, and only then replaces this one (sudo only if the
//!   directory needs it). One verified install path, not two.
//! - a source build (`target/…`, `~/.cargo/bin`): never self-replaced; says
//!   how to rebuild.
//! - Windows: no installer to re-run; says where the new zip is.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

const REPO: &str = "SF-Digital-Labs/aoraki-cli";
const INSTALLER: &str = "https://cli.aoraki.cloud/install.sh";
const BREW_FORMULA: &str = "sf-digital-labs/tap/aoraki";

/// How this binary was installed.
#[derive(Debug, PartialEq)]
pub enum Install {
    Homebrew,
    Script { dir: PathBuf },
    Source { path: PathBuf },
    Windows,
}

pub fn run(check: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let latest = latest_version().context("checking the latest release on GitHub")?;
    let exe = std::env::current_exe().context("locating this aoraki binary")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let how = detect(&exe);

    if !is_newer(&latest, current) {
        println!("✓ aoraki {current} is the latest release");
        return Ok(());
    }
    println!("aoraki {current} → {latest} available ({})", describe(&how));
    if check {
        println!("  run `aoraki update` to install it");
        return Ok(());
    }

    match how {
        Install::Homebrew => {
            println!("→ brew upgrade {BREW_FORMULA}");
            let ok = Command::new("brew")
                .args(["upgrade", BREW_FORMULA])
                .status()
                .context("running brew")?
                .success();
            if !ok {
                bail!("brew upgrade failed — run it yourself: brew upgrade {BREW_FORMULA}");
            }
        }
        Install::Script { dir } => {
            println!("→ re-running the installer into {}", dir.display());
            let ok = Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "curl --proto '=https' --tlsv1.2 -fsSL {INSTALLER} | sh"
                ))
                .env("AORAKI_INSTALL_DIR", &dir)
                .status()
                .context("running the installer")?
                .success();
            if !ok {
                bail!(
                    "the installer failed — nothing was replaced. Retry, or run it yourself:\n  \
                     curl -fsSL {INSTALLER} | AORAKI_INSTALL_DIR={} sh",
                    dir.display()
                );
            }
        }
        Install::Source { path } => {
            println!(
                "this aoraki is a source build ({}) — update it the same way:\n  \
                 git pull && cargo build --release   (then copy target/release/aoraki onto PATH)\n  \
                 or switch to a release: curl -fsSL {INSTALLER} | sh",
                path.display()
            );
            return Ok(());
        }
        Install::Windows => {
            println!(
                "download the new release and replace aoraki.exe:\n  \
                 https://cli.aoraki.cloud/download/x86_64-pc-windows-msvc"
            );
            return Ok(());
        }
    }
    println!("✓ updated — `aoraki --version` to confirm");
    Ok(())
}

/// Classify an install from the binary's resolved path.
pub fn detect(exe: &Path) -> Install {
    if cfg!(windows) {
        return Install::Windows;
    }
    let s = exe.to_string_lossy();
    if s.contains("/Cellar/") || s.contains("/homebrew/") || s.contains("/linuxbrew/") {
        return Install::Homebrew;
    }
    if s.contains("/target/debug/") || s.contains("/target/release/") || s.contains("/.cargo/bin/")
    {
        return Install::Source {
            path: exe.to_path_buf(),
        };
    }
    Install::Script {
        dir: exe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| "/usr/local/bin".into()),
    }
}

fn describe(how: &Install) -> &'static str {
    match how {
        Install::Homebrew => "installed with Homebrew",
        Install::Script { .. } => "installed with the install script",
        Install::Source { .. } => "built from source",
        Install::Windows => "Windows",
    }
}

/// The latest release's version ("0.4.0"), from GitHub.
fn latest_version() -> Result<String> {
    let body: serde_json::Value = ureq::get(&format!(
        "https://api.github.com/repos/{REPO}/releases/latest"
    ))
    .set(
        "User-Agent",
        concat!("aoraki-cli/", env!("CARGO_PKG_VERSION")),
    )
    .set("Accept", "application/vnd.github+json")
    .timeout(std::time::Duration::from_secs(15))
    .call()?
    .into_json()?;
    let tag = body["tag_name"]
        .as_str()
        .context("no tag_name on the latest release")?;
    Ok(tag.trim_start_matches('v').to_string())
}

/// `latest` > `current`, comparing dotted numeric versions ("0.10.0" >
/// "0.9.3"); anything unparseable counts as not newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<Vec<u64>> {
        v.trim_start_matches('v')
            .split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse().ok())
            .collect()
    };
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.4.0", "0.2.1"));
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(!is_newer("0.4.0", "0.4.0"));
        assert!(!is_newer("0.3.9", "0.4.0"));
        assert!(!is_newer("garbage", "0.4.0"));
    }

    #[cfg(not(windows))]
    #[test]
    fn the_install_method_comes_from_the_binary_path() {
        assert_eq!(
            detect(Path::new("/opt/homebrew/Cellar/aoraki/0.4.0/bin/aoraki")),
            Install::Homebrew
        );
        assert_eq!(
            detect(Path::new("/usr/local/Cellar/aoraki/0.4.0/bin/aoraki")),
            Install::Homebrew
        );
        assert_eq!(
            detect(Path::new("/usr/local/bin/aoraki")),
            Install::Script {
                dir: "/usr/local/bin".into()
            }
        );
        assert_eq!(
            detect(Path::new("/home/me/bin/aoraki")),
            Install::Script {
                dir: "/home/me/bin".into()
            }
        );
        assert!(matches!(
            detect(Path::new("/Users/me/code/aoraki-cli/target/release/aoraki")),
            Install::Source { .. }
        ));
        assert!(matches!(
            detect(Path::new("/Users/me/.cargo/bin/aoraki")),
            Install::Source { .. }
        ));
    }
}
