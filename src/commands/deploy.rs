use crate::aoraki::{self, DeployEvent};
use crate::context::Ctx;
use crate::util::git_capture;
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Instant;

pub fn run(env: Option<String>, git_ref: Option<String>) -> Result<()> {
    let ctx = Ctx::load(env)?;

    let refspec = git_ref.unwrap_or_else(|| "HEAD".to_string());
    let sha = git_capture(
        &ctx.repo_root,
        &["rev-parse", "--verify", &format!("{refspec}^{{commit}}")],
    )
    .with_context(|| format!("'{refspec}' does not resolve to a commit"))?;
    let short = sha[..7].to_string();

    if ctx.env().confirm && !sha_on_env_branch(&ctx, &sha) {
        println!(
            "⚠ {} is not on '{}' — deploying it would put off-branch code in {}",
            short,
            ctx.env().branch,
            ctx.env_name
        );
        print!(
            "Deploy {} to {}? Type '{}' to confirm: ",
            short, ctx.env_name, ctx.env_name
        );
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() != ctx.env_name {
            bail!("deploy cancelled");
        }
    }

    if ctx.env().is_gateway() {
        return gateway_deploy(&ctx, &sha, &short);
    }

    println!(
        "→ deploying {} {} to {} ({} on {})",
        ctx.app(),
        short,
        ctx.env_name,
        ctx.env().namespace,
        ctx.ssh_target()
    );

    let ssh = ctx.ssh()?;
    let started_at = chrono::Utc::now();
    let timer = Instant::now();

    // Force-push the exact SHA to the deploy branch: the bare repo is a
    // deploy channel, not a collaboration branch, so non-fast-forward
    // (e.g. redeploying an older commit) must work.
    let mut child = Command::new("git")
        .current_dir(&ctx.repo_root)
        .env("GIT_SSH_COMMAND", ssh.git_ssh_command())
        .args([
            "push",
            &ctx.push_url(),
            &format!("+{}:refs/heads/{}", sha, ctx.env().branch),
        ])
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn git push")?;

    // git sends remote (hook) output to stderr as "remote: ..." lines. The
    // hook's final sentinel line is our success signal — the push exit code
    // can't be, since post-receive failures don't fail the push.
    let mut outcome: Option<bool> = None;
    let stderr = child.stderr.take().expect("stderr was piped");
    for line in BufReader::new(stderr).lines() {
        let line = line?;
        if line.contains("aoraki: deploy status: succeeded") {
            outcome = Some(true);
        } else if line.contains("aoraki: deploy status: failed") {
            outcome = Some(false);
        }
        let cleaned = line.strip_prefix("remote: ").unwrap_or(&line);
        println!("{cleaned}");
    }
    let push_ok = child.wait()?.success();
    if !push_ok && outcome.is_none() {
        bail!("git push failed — run `aoraki doctor {}` to check the deploy chain", ctx.env_name);
    }

    // Ref unchanged (redeploy of the already-deployed SHA): the hook never
    // fires on an up-to-date push, so invoke it directly.
    if outcome.is_none() {
        println!("→ ref unchanged on box; triggering deploy hook directly");
        let trigger = format!(
            "echo '{sha} {sha} refs/heads/{branch}' | {bare}/hooks/post-receive",
            branch = ctx.env().branch,
            bare = ctx.bare_repo()
        );
        outcome = Some(ssh.run(&trigger).is_ok());
    }

    let succeeded = outcome == Some(true);
    let duration_secs = timer.elapsed().as_secs();
    let actor = git_capture(&ctx.repo_root, &["config", "user.email"]).unwrap_or_default();

    let event = DeployEvent {
        app: ctx.app().to_string(),
        environment: ctx.env_name.clone(),
        box_id: ctx.env().server.clone(),
        namespace: ctx.env().namespace.clone(),
        build_ref: short.clone(),
        ref_name: ctx.env().branch.clone(),
        status: if succeeded { "succeeded" } else { "failed" }.to_string(),
        started_at: started_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        duration_secs,
        actor,
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    aoraki::report(&ctx.global, ctx.env().remote.as_deref(), &event);

    if succeeded {
        println!(
            "✓ deployed {} to {} in {}s",
            short, ctx.env_name, duration_secs
        );
        if let Some(url) = &ctx.env().url {
            println!("→ {url}");
        }
        Ok(())
    } else {
        bail!("deploy failed — see output above (previous pods keep running; image tag never changed)");
    }
}

/// Is the SHA contained in the environment's branch (local, falling back to
/// origin/<branch>)? Deploying branch-matching content is the intended path,
/// so `confirm = true` environments skip the typed prompt for it and reserve
/// the prompt for off-branch SHAs — the wrong-branch footgun the prompt is
/// actually there to catch.
fn sha_on_env_branch(ctx: &Ctx, sha: &str) -> bool {
    let branch = &ctx.env().branch;
    for ref_name in [branch.clone(), format!("origin/{branch}")] {
        if git_capture(
            &ctx.repo_root,
            &["merge-base", "--is-ancestor", sha, &ref_name],
        )
        .is_ok()
        {
            return true;
        }
    }
    false
}

/// Customer path (ADR 008): the gateway pulls the pushed commit from the
/// customer's GitHub and deploys it — no SSH, no direct push from here.
fn gateway_deploy(ctx: &Ctx, sha: &str, short: &str) -> Result<()> {
    let repo = ctx
        .repo
        .app
        .repo
        .as_deref()
        .context("gateway transport needs `repo = \"owner/name\"` under [app] in aoraki.toml")?;

    // Build-contract preflight (platform § 5): a Dockerfile is the one build
    // input the customer owns. The gateway re-checks at the exact commit.
    if !ctx.repo_root.join("Dockerfile").exists() {
        bail!("no Dockerfile at the repo root — add one (or run `aoraki init` once it exists)");
    }

    // Only pushed commits deploy. This is a local heads-up; the gateway is
    // the authority (it 400s if the commit never reached GitHub).
    let on_remote = git_capture(
        &ctx.repo_root,
        &["branch", "-r", "--contains", sha],
    )
    .map(|out| !out.is_empty())
    .unwrap_or(false);
    if !on_remote {
        eprintln!("⚠ commit {short} doesn't appear on any remote branch — push first or this will fail");
    }

    let gw = crate::gateway::Gateway::from_global(&ctx.global)?;
    println!(
        "→ deploying {} {} to {} via gateway ({} @ {})",
        ctx.app(),
        short,
        ctx.env_name,
        repo,
        short
    );

    let resp = gw.post(
        "/deploys",
        serde_json::json!({
            "repo": repo,
            "git_ref": sha,
            "app": ctx.app(),
            "environment": ctx.env_name,
        }),
    )?;
    let deploy_id = resp["data"]["deploy_id"]
        .as_str()
        .context("gateway did not return a deploy id")?
        .to_string();

    let status = gw.stream_deploy_events(&deploy_id)?;
    if status == "succeeded" {
        if let Some(url) = &ctx.env().url {
            println!("→ {url}");
        }
        Ok(())
    } else {
        bail!("deploy failed — see output above");
    }
}
