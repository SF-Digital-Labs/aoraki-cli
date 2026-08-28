use crate::context::Ctx;
use crate::util::sh_quote;
use anyhow::{bail, Result};

pub fn run(env: Option<String>, cmd: Vec<String>) -> Result<()> {
    if cmd.is_empty() {
        bail!("provide a command after -- , e.g. `aoraki run staging -- bash`");
    }
    let ctx = Ctx::load(env)?;
    let ssh = ctx.ssh()?;

    // Use the currently deployed image so the one-off pod matches production
    // code exactly (sha-tagged images are already in k3s containerd).
    let image = ssh.capture(&format!(
        "{} get deploy {} -o jsonpath='{{.spec.template.spec.containers[0].image}}'",
        ctx.kubectl(),
        ctx.deployment()
    ))?;
    if image.is_empty() {
        bail!("could not resolve the deployed image for {}", ctx.deployment());
    }

    let quoted = cmd.iter().map(|c| sh_quote(c)).collect::<Vec<_>>().join(" ");
    println!("→ one-off pod with image {image}");
    ssh.run_interactive(&format!(
        "{} run aoraki-run-$RANDOM --rm -it --restart=Never --image={} --command -- {}",
        ctx.kubectl(),
        image,
        quoted
    ))
}
