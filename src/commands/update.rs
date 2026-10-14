//! `aoraki update [--check]` — update the CLI the way it was installed.
//!
//! The running binary's resolved path says how it got here:
//! - Homebrew (resolves into `…/Cellar/aoraki/…`): `brew upgrade
//!   sf-digital-labs/tap/aoraki`, then confirm the new version actually
//!   landed (the tap formula can trail a GitHub release).
//! - `cargo install` (`~/.cargo/bin`) or a binary still inside a build tree
//!   (`…/target/release/`): never self-replaced — says how to rebuild.
//! - anything else is treated as an install-script / release-binary
//!   install (`/usr/local/bin`, `$AORAKI_INSTALL_DIR`, a downloaded binary,
//!   or a source build copied onto PATH): re-run the official installer into
//!   that same directory — it downloads the latest release, checks its
//!   published SHA-256, proves the binary runs here, and only then replaces
//!   this one (sudo only if the directory needs it).
//! - Windows: no installer to re-run; says where the new zip is.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

const REPO: &str = "SF-Digital-Labs/aoraki-cli";
const INSTALLER: &str = "https://cli.aoraki.cloud/install.sh";
const BREW_FORMULA: &str = "sf-digital-labs/tap/aoraki";
const WINDOWS_ZIP: &str = "https://cli.aoraki.cloud/download/x86_64-pc-windows-msvc";

/// How this binary was installed.
#[derive(Debug, PartialEq)]
pub enum Install {
    /// `prefix`: the Homebrew prefix (the part before `/Cellar/`).
    Homebrew {
        prefix: PathBuf,
    },
    Script {
        dir: PathBuf,
    },
    Cargo,
    BuildTree {
        path: PathBuf,
    },
    Windows,
}

pub fn run(check: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let latest = latest_version().with_context(|| {
        format!(
            "couldn't check GitHub for the latest release (offline, or rate-limited — \
             retry in a minute; or update with the installer: curl -fsSL {INSTALLER} | sh)"
        )
    })?;
    let exe = std::env::current_exe().context("locating this aoraki binary")?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let how = detect_from(&exe, cfg!(windows));

    if !is_newer(&latest, current) {
        println!("✓ aoraki {current} is the latest release");
        return Ok(());
    }
    println!("aoraki {current} → {latest} available ({})", describe(&how));
    if check {
        println!("  {}", next_step(&how));
        return Ok(());
    }

    match how {
        Install::Homebrew { prefix } => {
            let brew = prefix.join("bin/brew");
            let brew = if brew.exists() {
                brew
            } else {
                PathBuf::from("brew")
            };
            println!("→ brew upgrade {BREW_FORMULA}");
            let ran = Command::new(&brew)
                .args(["upgrade", BREW_FORMULA])
                .status()
                .with_context(|| {
                    format!(
                        "running {} — run it yourself: brew upgrade {BREW_FORMULA}",
                        brew.display()
                    )
                })?;
            if !ran.success() {
                bail!("brew upgrade failed — run it yourself: brew upgrade {BREW_FORMULA}");
            }
            // brew exits 0 when its formula has nothing newer yet.
            let now = installed_version(&prefix.join("bin/aoraki"));
            if now.as_deref().is_none_or(|v| is_newer(&latest, v)) {
                bail!(
                    "Homebrew's formula hasn't caught up with {latest} yet (still {}) — try \
                     again later, or switch to the release build: curl -fsSL {INSTALLER} | sh",
                    now.as_deref().unwrap_or("unknown")
                );
            }
            println!("✓ updated to aoraki {latest}");
        }
        Install::Script { dir } => {
            println!("→ re-running the installer into {}", dir.display());
            let ok = Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "curl --proto '=https' --tlsv1.2 -fsSL {INSTALLER} | sh"
                ))
                .env("AORAKI_INSTALL_DIR", &dir)
                .env("AORAKI_UPDATING", "1")
                .status()
                .context("running the installer")?
                .success();
            if !ok {
                bail!(
                    "the installer failed — nothing was replaced. Retry, or run it yourself:\n  \
                     curl -fsSL {INSTALLER} | AORAKI_INSTALL_DIR='{}' sh",
                    dir.display()
                );
            }
        }
        other => println!("{}", next_step(&other)),
    }
    Ok(())
}

/// What updating means for an install, in one line.
fn next_step(how: &Install) -> String {
    match how {
        Install::Homebrew { .. } | Install::Script { .. } => {
            "run `aoraki update` to install it".into()
        }
        Install::Cargo => {
            format!("update it with: cargo install --git https://github.com/{REPO} --force")
        }
        Install::BuildTree { path } => format!(
            "this is a build from source ({}) — update with: git pull && cargo build --release",
            path.display()
        ),
        Install::Windows => {
            format!("download the new release and replace aoraki.exe: {WINDOWS_ZIP}")
        }
    }
}

/// Classify an install from the binary's resolved path.
pub fn detect_from(exe: &Path, windows: bool) -> Install {
    if windows {
        return Install::Windows;
    }
    let s = exe.to_string_lossy();
    // A Homebrew binary always resolves into its Cellar (Linuxbrew too);
    // `/opt/homebrew/bin` alone can hold a script install.
    if let Some(i) = s.find("/Cellar/") {
        return Install::Homebrew {
            prefix: PathBuf::from(&s[..i]),
        };
    }
    if s.contains("/.cargo/bin/") {
        return Install::Cargo;
    }
    if s.contains("/target/debug/") || s.contains("/target/release/") {
        return Install::BuildTree {
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
        Install::Homebrew { .. } => "installed with Homebrew",
        Install::Script { .. } => "installed with the install script",
        Install::Cargo => "installed with cargo install",
        Install::BuildTree { .. } => "built from source",
        Install::Windows => "a Windows install",
    }
}

/// "aoraki 0.5.0" → "0.5.0", from running a binary.
fn installed_version(bin: &Path) -> Option<String> {
    let out = Command::new(bin).arg("--version").output().ok()?;
    String::from_utf8(out.stdout)
        .ok()?
        .split_whitespace()
        .nth(1)
        .map(str::to_string)
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

/// `latest` > `current` for "MAJOR.MINOR.PATCH[-pre]" versions: numeric
/// parts compare as numbers (missing ones are 0), and a pre-release sorts
/// before its release ("0.5.0-rc.1" < "0.5.0"). Unparseable → not newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<([u64; 3], bool)> {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once(['-', '+']) {
            Some((c, rest)) => (c, v[c.len()..].starts_with('-') && !rest.is_empty()),
            None => (v, false),
        };
        let mut n = [0u64; 3];
        for (i, part) in core.split('.').enumerate() {
            if i >= 3 {
                return None;
            }
            n[i] = part.parse().ok()?;
        }
        Some((n, pre))
    }
    match (parse(latest), parse(current)) {
        // Same numbers: a release beats its pre-release (`!pre` true > false).
        (Some((l, lp)), Some((c, cp))) => (l, !lp) > (c, !cp),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_like_releases() {
        assert!(is_newer("0.5.0", "0.4.0"));
        assert!(is_newer("v0.10.0", "0.9.3"));
        assert!(!is_newer("0.4.0", "0.4.0"));
        assert!(!is_newer("0.4.0", "0.4"), "missing parts are zero");
        assert!(!is_newer("0.3.9", "0.4.0"));
        assert!(is_newer("0.5.0", "0.5.0-rc.1"), "the release beats its rc");
        assert!(!is_newer("0.5.0-rc.1", "0.5.0"));
        assert!(!is_newer("garbage", "0.4.0"));
    }

    #[test]
    fn the_install_method_comes_from_the_binary_path() {
        let d = |p: &str| detect_from(Path::new(p), false);
        assert_eq!(
            d("/opt/homebrew/Cellar/aoraki/0.4.0/bin/aoraki"),
            Install::Homebrew {
                prefix: "/opt/homebrew".into()
            }
        );
        assert_eq!(
            d("/home/linuxbrew/.linuxbrew/Cellar/aoraki/0.4.0/bin/aoraki"),
            Install::Homebrew {
                prefix: "/home/linuxbrew/.linuxbrew".into()
            }
        );
        // A plain file in Homebrew's bin dir (installer with
        // AORAKI_INSTALL_DIR=/opt/homebrew/bin) is NOT a brew install.
        assert_eq!(
            d("/opt/homebrew/bin/aoraki"),
            Install::Script {
                dir: "/opt/homebrew/bin".into()
            }
        );
        assert_eq!(
            d("/usr/local/bin/aoraki"),
            Install::Script {
                dir: "/usr/local/bin".into()
            }
        );
        assert_eq!(d("/Users/me/.cargo/bin/aoraki"), Install::Cargo);
        assert!(matches!(
            d("/Users/me/code/aoraki-cli/target/release/aoraki"),
            Install::BuildTree { .. }
        ));
        assert_eq!(
            detect_from(Path::new(r"C:\tools\aoraki.exe"), true),
            Install::Windows
        );
    }
}
