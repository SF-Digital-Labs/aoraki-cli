use crate::context::Ctx;
use anyhow::Result;

pub fn run(env: Option<String>) -> Result<()> {
    let ctx = Ctx::load(env)?;
    let ssh = ctx.ssh()?;
    println!("→ restarting {} in {}", ctx.deployment(), ctx.env().namespace);
    ssh.run(&format!(
        "{k} rollout restart deployment/{d} && {k} rollout status deployment/{d} --timeout=600s",
        k = ctx.kubectl(),
        d = ctx.deployment()
    ))
}
