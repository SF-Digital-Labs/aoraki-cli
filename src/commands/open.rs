use crate::context::Ctx;
use anyhow::{bail, Context, Result};

pub fn run(env: Option<String>) -> Result<()> {
    let ctx = Ctx::load(env)?;
    let Some(url) = &ctx.env().url else {
        bail!(
            "no url configured for environment '{}' in aoraki.toml",
            ctx.env_name
        );
    };
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(url)
        .status()
        .with_context(|| format!("failed to run {opener}"))?;
    Ok(())
}
