use crate::context::Ctx;
use crate::util::git_capture;
use anyhow::Result;

pub fn run(env: Option<String>) -> Result<()> {
    let ctx = Ctx::load(env)?;
    let ssh = ctx.ssh()?;

    let local = git_capture(&ctx.repo_root, &["rev-parse", "--short=7", "HEAD"])?;

    let last_line = ssh
        .capture(&format!("tail -n 1 {} 2>/dev/null || true", ctx.deploy_log()))
        .unwrap_or_default();
    let last: Option<serde_json::Value> = serde_json::from_str(&last_line).ok();

    let k8s = ssh
        .capture(&format!(
            "{} get deploy {} -o jsonpath='{{.spec.template.spec.containers[0].image}} {{.status.readyReplicas}}/{{.status.replicas}}' 2>/dev/null || echo unavailable",
            ctx.kubectl(),
            ctx.deployment()
        ))
        .unwrap_or_else(|_| "unavailable".to_string());

    println!(
        "App:        {} ({} → {})",
        ctx.app(),
        ctx.env_name,
        ctx.ssh_target()
    );
    println!("Local HEAD: {local}");

    let mut deployed_sha: Option<String> = None;
    match &last {
        Some(rec) => {
            let sha = rec["sha"].as_str().unwrap_or("?");
            let short = &sha[..sha.len().min(7)];
            deployed_sha = Some(short.to_string());
            let status = rec["status"].as_str().unwrap_or("?");
            let time = rec["time"].as_str().unwrap_or("?");
            let dur = rec["duration_secs"]
                .as_u64()
                .map(|d| format!(", {d}s"))
                .unwrap_or_default();
            println!("Deployed:   {short} ({status}, {time}{dur})");
        }
        None => println!("Deployed:   no aoraki deploys recorded on this box yet"),
    }
    println!("Pods:       {k8s}");

    if let Some(deployed) = deployed_sha {
        if deployed == local {
            println!("\n✓ box is running your local HEAD");
        } else {
            println!("\n⚠ local HEAD ({local}) differs from last deployed ({deployed})");
        }
    }
    Ok(())
}
