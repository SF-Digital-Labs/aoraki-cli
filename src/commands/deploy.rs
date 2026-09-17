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

    if ctx.env().is_lease() {
        return lease_deploy(&ctx, &short);
    }
    if ctx.env().is_gateway() {
        return gateway_deploy(&ctx, &sha, &short);
    }

    println!(
        "→ deploying {} {} to {} ({} on {})",
        ctx.app(),
        short,
        ctx.env_name,
        ctx.namespace()?,
        ctx.ssh_target()?
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
            &ctx.push_url()?,
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
        box_id: ctx.env().server()?.to_string(),
        namespace: ctx.namespace()?.to_string(),
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

/// LEASE deploys (the default customer path): three moves, all visible —
/// build the Dockerfile LOCALLY, push to the platform registry, then
/// lease via the console (create, or blue-green image update when the
/// deployment already exists). Env on first create comes from an
/// optional uncommitted `.aoraki.env`; ongoing env lives in the
/// console's env groups (applied on every redeploy).
fn lease_deploy(ctx: &Ctx, short: &str) -> Result<()> {
    let env_cfg = ctx.env();
    let dockerfile = env_cfg.dockerfile.as_deref().unwrap_or("Dockerfile");
    if !ctx.repo_root.join(dockerfile).exists() {
        bail!("no {dockerfile} in the repo — cloud deploys build it locally");
    }
    let size = env_cfg.size.as_deref().unwrap_or("docker-xlarge-storage");
    let process_type = env_cfg.process_type.as_deref().unwrap_or("web");
    let name = ctx.app().to_string();

    let console = crate::console::Console::connect(env_cfg.remote.as_deref())?;
    let registry =
        std::env::var("AORAKI_REGISTRY").unwrap_or_else(|_| "registry.aoraki.cloud".to_string());
    let image = format!("{registry}/{}/{name}:{short}", console.org_hex);

    if git_capture(&ctx.repo_root, &["status", "--porcelain"])
        .map(|o| !o.trim().is_empty())
        .unwrap_or(false)
    {
        println!("⚠ working tree has uncommitted changes — the image is built from the tree, tagged {short}");
    }

    println!(
        "→ building {name} {short} from {dockerfile} → {image} (org: {}, remote: {})",
        console.org_name, console.remote_name
    );
    let status = Command::new("docker")
        .current_dir(&ctx.repo_root)
        .args([
            "build", "-f", dockerfile, "-t", &image,
            "--build-arg", &format!("BUILD_HASH={short}"), ".",
        ])
        .status()
        .context("failed to run docker build — is docker installed and running?")?;
    if !status.success() {
        bail!("docker build failed");
    }

    // The Dockerfile declares the port (EXPOSE); `port =` in aoraki.toml
    // only overrides it (multi-EXPOSE images must pick one).
    let port = match env_cfg.port {
        Some(p) => p,
        None => exposed_port(&image)?,
    };

    println!("→ pushing {image}");
    let status = Command::new("docker")
        .args(["push", &image])
        .status()
        .context("failed to run docker push")?;
    if !status.success() {
        bail!("docker push failed — logged in? try: docker login {registry}");
    }

    // Upsert: an existing live deployment gets a blue-green image update;
    // otherwise create (with first-boot env from .aoraki.env if present).
    let existing = console.get(&format!("/orgs/{}/deployments", console.org_hex))?;
    let existing_hex = existing["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| {
            d["name"].as_str() == Some(name.as_str())
                && d["status"].as_str() != Some("closed")
                && d["status"].as_str() != Some("failed")
        })
        .and_then(|d| d["id"].as_str().or_else(|| d["hex_id"].as_str()))
        .map(String::from);

    let dep_hex = match existing_hex {
        Some(hex) => {
            println!("→ updating deployment {hex} (blue-green image swap; env from console groups)");
            console.post(
                &format!("/orgs/{}/deployments/{hex}/redeploy", console.org_hex),
                &serde_json::json!({ "image": image }),
            )?;
            hex
        }
        None => {
            let env_map = read_dotenv(&ctx.repo_root.join(".aoraki.env"))?;
            if !env_map.is_empty() {
                println!(
                    "→ first deploy: {} env var(s) from .aoraki.env (manage ongoing env in the console)",
                    env_map.len()
                );
            }
            let created = console.post(
                &format!("/orgs/{}/deploys", console.org_hex),
                &serde_json::json!({
                    "name": name,
                    "image": image,
                    "port": port,
                    "env": env_map,
                    "size": size,
                    "process_type": process_type,
                }),
            )?;
            let hex = created["data"]["hex_id"]
                .as_str()
                .or_else(|| created["data"]["id"].as_str())
                .context("deploy accepted but no deployment id returned")?
                .to_string();
            println!("deployment {hex} registered — waiting for the lease…");
            hex
        }
    };

    crate::commands::launch::wait_for_lease_banner(&console, &dep_hex)?;
    Ok(())
}

/// KEY=VALUE lines, # comments; missing file → empty map. Never committed
/// (add .aoraki.env to .gitignore) — it exists for the FIRST cloud deploy;
/// console env groups own ongoing configuration.
fn read_dotenv(path: &std::path::Path) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut map = serde_json::Map::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(map);
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line
            .split_once('=')
            .with_context(|| format!(".aoraki.env: '{line}' is not KEY=VALUE"))?;
        map.insert(
            k.trim().to_string(),
            serde_json::Value::String(v.trim().to_string()),
        );
    }
    Ok(map)
}

/// "gateway" transport (ADR 008, server-side build): the gateway pulls the
/// pushed commit from the customer's GitHub and deploys it — no SSH, no
/// direct push from here.
fn gateway_deploy(ctx: &Ctx, sha: &str, short: &str) -> Result<()> {
    let repo = ctx
        .repo
        .app
        .repo
        .as_deref()
        .context("gateway transport needs `repo = \"owner/name\"` under [app] in aoraki.toml")?;

    // Build-contract preflight (platform § 5): a Dockerfile is the one build
    // input the customer owns. The gateway re-checks at the exact commit.
    let dockerfile = ctx.env().dockerfile.as_deref().unwrap_or("Dockerfile");
    if !ctx.repo_root.join(dockerfile).exists() {
        bail!("no {dockerfile} at the repo root — add one (or run `aoraki init` once it exists)");
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

    let mut body = serde_json::json!({
        "repo": repo,
        "git_ref": sha,
        "app": ctx.app(),
        "environment": ctx.env_name,
    });
    // Release deploys: port makes the gateway build → push → lease.
    if let Some(port) = ctx.env().port {
        body["port"] = serde_json::json!(port);
        if let Some(size) = &ctx.env().size {
            body["size"] = serde_json::json!(size);
        }
        if let Some(df) = &ctx.env().dockerfile {
            body["dockerfile"] = serde_json::json!(df);
        }
        if let Some(pt) = &ctx.env().process_type {
            body["process_type"] = serde_json::json!(pt);
        }
    }
    let resp = gw.post("/deploys", body)?;
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

/// Single EXPOSEd tcp port of a built image, from docker inspect.
fn exposed_port(image: &str) -> Result<u16> {
    let out = Command::new("docker")
        .args(["image", "inspect", "--format", "{{json .Config.ExposedPorts}}", image])
        .output()
        .context("docker image inspect failed")?;
    let raw = String::from_utf8_lossy(&out.stdout);
    let ports: Vec<u16> = raw
        .trim()
        .trim_matches('"')
        .trim_start_matches('{')
        .trim_end_matches('}')
        .split(',')
        .filter_map(|entry| entry.split('"').nth(1))
        .filter_map(|spec| spec.strip_suffix("/tcp"))
        .filter_map(|p| p.parse().ok())
        .collect();
    match ports[..] {
        [one] => Ok(one),
        [] => bail!("the Dockerfile has no EXPOSE — add one, or set `port =` in this environment"),
        _ => bail!(
            "the Dockerfile EXPOSEs several ports ({ports:?}) — set `port =` in this environment to pick one"
        ),
    }
}
