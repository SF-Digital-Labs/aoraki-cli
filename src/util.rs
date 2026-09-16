use anyhow::{bail, Result};
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

#[cfg(test)]
mod tests {
    use super::*;

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
