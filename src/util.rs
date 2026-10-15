use anyhow::{bail, Result};

/// Every request says which CLI made it, so the server can tell an
/// outdated CLI to update (its CLI_MIN_VERSION).
pub const USER_AGENT: &str = concat!("aoraki-cli/", env!("CARGO_PKG_VERSION"));
use std::path::Path;

/// Run a local git command in `dir` and capture trimmed stdout.
pub fn git_capture(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Single-quote a string for a remote POSIX shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Open an http(s) URL in the default browser. Anything else is refused:
/// `open`/`xdg-open` happily launch local files and app bundles, and URLs
/// here can come from a committed aoraki.toml.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    use anyhow::{bail, Context};
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bail!("refusing to open {url:?} — only http(s) URLs");
    }
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        // Not `cmd /C start`: cmd would interpret `&` in query strings.
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url)
        .status()
        .context("failed to launch the browser")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_refuses_non_http() {
        for bad in [
            "x.command",
            "/Applications/Calculator.app",
            "file:///etc/passwd",
            "-a Terminal",
        ] {
            assert!(open_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sh_quote_wraps_plain_strings() {
        assert_eq!(sh_quote("hello"), "'hello'");
        assert_eq!(sh_quote("a b/c.d"), "'a b/c.d'");
    }

    #[test]
    fn sh_quote_escapes_embedded_single_quotes() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote("''"), r"''\'''\'''");
    }
}
