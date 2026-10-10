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
        bail!(
            "could not resolve the deployed image for {}",
            ctx.deployment()
        );
    }

    let quoted = cmd
        .iter()
        .map(|c| sh_quote(c))
        .collect::<Vec<_>>()
        .join(" ");
    println!("→ one-off pod with image {image}");
    // Not -t when stdin isn't a terminal (CI): kubectl -it would refuse.
    let tty = if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        "-it"
    } else {
        "-i"
    };
    let code = ssh.run_interactive_status(&format!(
        "{} run aoraki-run-$RANDOM --rm {tty} --restart=Never --image={} --command -- {}",
        ctx.kubectl(),
        sh_quote(&image),
        quoted
    ))?;
    // The remote command's exit code is ours: `aoraki run prod -- migrate`
    // in CI must fail the job when the migration fails.
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
