use crate::context::Ctx;
use anyhow::{bail, Result};

/// Log targets: which pods of the app to tail.
const TARGETS: [&str; 4] = ["web", "worker", "workers", "all"];

pub fn run(
    env: Option<String>,
    target: Option<String>,
    app: Option<String>,
    follow: bool,
    since: Option<String>,
    previous: bool,
    lines: u32,
) -> Result<()> {
    // `aoraki logs workers` — with no env given, the target keyword lands
    // in the env slot. Shift it over so the branch-derived default applies.
    let (env, target) = match (env, target) {
        (Some(e), None) if TARGETS.contains(&e.as_str()) => (None, Some(e)),
        pair => pair,
    };
    // --app is the explicit override; it beats the positional.
    let ctx = Ctx::load(app.or(env))?;
    let ssh = ctx.ssh()?;
    let k = ctx.kubectl();

    let mut opts = format!(" --tail={lines}");
    if let Some(since) = since {
        opts.push_str(&format!(" --since={since}"));
    }
    if previous {
        opts.push_str(" --previous");
    }
    if follow {
        opts.push_str(" -f");
    }

    let cmd = match target.as_deref().unwrap_or("all") {
        "web" => format!("{k} logs deployment/{}{opts}", ctx.deployment()),
        // Worker deployment names vary slightly per app (<app>-worker by
        // convention) — discover it on the box like khelp's klw does.
        "worker" | "workers" => format!(
            "dep=$({k} get deployments -o name | grep -m1 worker) \
             || {{ echo 'no worker deployment in namespace {ns}' >&2; exit 1; }}; \
             {k} logs \"$dep\"{opts}",
            ns = ctx.env().namespace
        ),
        // Every pod in the namespace (web + worker + redis), interleaved.
        // --prefix attributes each line to its pod; without it the merged
        // stream is unreadable.
        "all" => format!(
            "{k} logs -l app --all-containers --prefix --max-log-requests=10{opts}"
        ),
        other => bail!("unknown logs target '{other}' — use web, workers, or all"),
    };

    if follow {
        ssh.run_interactive(&cmd)
    } else {
        ssh.run(&cmd)
    }
}
