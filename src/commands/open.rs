use crate::context::Ctx;
use anyhow::{bail, Result};

pub fn run(env: Option<String>) -> Result<()> {
    let ctx = Ctx::load(env)?;
    let Some(url) = &ctx.env().url else {
        bail!(
            "no url configured for environment '{}' in aoraki.toml",
            ctx.env_name
        );
    };
    crate::util::open_url(url)
}
