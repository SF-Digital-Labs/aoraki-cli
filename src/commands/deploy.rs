use crate::aoraki::{self, DeployEvent};
use crate::context::Ctx;
use crate::util::git_capture;
use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Instant;

pub fn run(env: Option<String>, git_ref: Option<String>, gpu: Option<String>) -> Result<()> {
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
        return lease_deploy(&ctx, &short, gpu);
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
    aoraki::report(
        &ctx.global,
        ctx.env().remote.as_deref().or(ctx.repo.app.org.as_deref()),
        &event,
    );

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
fn lease_deploy(ctx: &Ctx, short: &str, gpu_override: Option<String>) -> Result<()> {
    let env_cfg = ctx.env();
    let dockerfile = env_cfg.dockerfile.as_deref().unwrap_or("Dockerfile");
    if !ctx.repo_root.join(dockerfile).exists() {
        bail!("no {dockerfile} in the repo — cloud deploys build it locally");
    }
    let size = env_cfg.size.as_deref().unwrap_or("docker-xlarge-storage");
    let process_type = env_cfg.process_type.as_deref().unwrap_or("web");
    let name = ctx.app().to_string();

    // Env remote pin → the app's org id → the machine default. The org
    // pin is also a GUARD: a resolved remote for some other org refuses.
    let remote_pref = env_cfg
        .remote
        .as_deref()
        .or(ctx.repo.app.org.as_deref());
    let console = crate::console::Console::connect(remote_pref)?;
    if let Some(org_pin) = ctx.repo.app.org.as_deref() {
        if org_pin.starts_with("org_") && console.org_hex != org_pin {
            bail!(
                "aoraki.toml pins org {org_pin} but remote '{}' is org {} ({}) — \
                 wrong console org; fix the remote or the pin",
                console.remote_name, console.org_hex, console.org_name
            );
        }
    }
    // Env override > the console's advertised registry (each env has its
    // own) > the prod default for older consoles.
    let registry = std::env::var("AORAKI_REGISTRY").unwrap_or_else(|_| {
        console
            .registry
            .clone()
            .unwrap_or_else(|| "registry.aoraki.cloud".to_string())
    });
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

    registry_login(&registry, &console)?;
    println!("→ pushing {image}");
    let mut pushed = Command::new("docker")
        .args(["push", &image])
        .status()
        .context("failed to run docker push")?
        .success();
    if !pushed {
        // Stale ~/.docker credentials are the common cause — refresh the
        // login from the org key and retry exactly once.
        println!("→ push failed; refreshing registry login and retrying once");
        registry_login(&registry, &console)?;
        pushed = Command::new("docker")
            .args(["push", &image])
            .status()
            .context("failed to run docker push")?
            .success();
    }
    if !pushed {
        bail!(
            "docker push failed. Likely causes, most common first: \
             a layer over ~100MB (the registry sits behind Cloudflare's \
             body cap — split big layers), network/registry outage, or — \
             rarely, since login just succeeded — pushing outside your \
             org's {}/… namespace",
            console.org_hex
        );
    }

    // GPU target (flag wins over the environment pin): same build+push,
    // then the workload runs on an Aoraki GPU instead of a lease.
    if let Some(gpu) = gpu_override.or_else(|| env_cfg.gpu.clone()) {
        let env_map = read_dotenv(&ctx.repo_root.join(".aoraki.env"))?;
        // Validate the model BEFORE touching any running workload: with
        // single-GPU capacity the old one must be removed first, so the
        // only safe order is check → remove → create → loud on failure.
        if gpu != "auto" {
            let catalog = console.get(&format!("/orgs/{}/gpu-catalog", console.org_hex))?;
            let known = catalog["data"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|g| g["name"].as_str() == Some(gpu.as_str()));
            if !known {
                bail!("GPU model '{gpu}' is not in the catalog — see `aoraki gpus` (nothing was removed)");
            }
        }
        let removed = replace_existing_gpu(&console, &name, &env_map)?;
        return crate::commands::launch::run_gpu(&console, &name, &image, port, &gpu, env_map)
            .map_err(|e| {
                if removed > 0 {
                    anyhow::anyhow!(
                        "{e:#}\n\n⚠ the previous GPU deployment was ALREADY REMOVED and nothing \
                         is running for '{name}' — rerun `aoraki deploy --gpu` (or `aoraki gpus` \
                         to check capacity) as soon as possible"
                    )
                } else {
                    e
                }
            });
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
        .and_then(|d| d["hex_id"].as_str().or_else(|| d["id"].as_str()))
        .map(String::from);

    let dep_hex = match existing_hex {
        Some(hex) => {
            println!("→ updating deployment {hex} (blue-green image swap; env from console groups)");
            console
                .post(
                    &format!("/orgs/{}/deployments/{hex}/redeploy", console.org_hex),
                    &serde_json::json!({ "image": image }),
                )
                .map_err(|e| {
                    anyhow::anyhow!("{e:#}\n(the previous version keeps serving — nothing changed)")
                })?;
            // Updates are blue-green server-side: the row stays `active`
            // while the swap happens, so polling it proves nothing. Say
            // exactly that instead of a false DEPLOYED banner.
            println!("✓ swap requested — the previous version keeps serving until the new one is healthy");
            println!("  monitor: {}/deployments/{hex}", console.console_base());
            return Ok(());
        }
        None => {
            let env_map = read_dotenv(&ctx.repo_root.join(".aoraki.env"))?;
            if !env_map.is_empty() && dotenv_is_tracked(&ctx.repo_root) {
                eprintln!("⚠ .aoraki.env is tracked by git — it holds secrets; add it to .gitignore");
            }
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
            println!("  (safe to close this terminal — the deploy continues server-side)");
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
        bail!("no {dockerfile} at the repo root — add one");
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

/// Non-interactive `docker login` against the platform registry using the
/// org key (token-auth realm treats it as the Basic password). Quiet on
/// success; docker stores the credential for the push that follows.
fn registry_login(registry: &str, console: &crate::console::Console) -> Result<()> {
    let mut child = Command::new("docker")
        .args(["login", registry, "-u", &console.org_hex, "--password-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run docker login")?;
    child
        .stdin
        .as_mut()
        .context("docker login stdin")?
        .write_all(console.token().as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "registry login failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// GPU deploys have no blue-green update, and capacity may be a single
/// card — so replace is REMOVE-FIRST by design (a create-first would
/// deadlock on the occupied GPU). The caller pre-validates the model and
/// screams if the create then fails. Returns how many were removed.
fn replace_existing_gpu(
    console: &crate::console::Console,
    name: &str,
    new_env: &serde_json::Map<String, serde_json::Value>,
) -> Result<usize> {
    let list = console.get(&format!("/orgs/{}/gpu-deploys", console.org_hex))?;
    let old: Vec<(String, bool)> = list["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| {
            d["name"].as_str() == Some(name)
                && matches!(
                    d["status"].as_str(),
                    Some("requested") | Some("starting") | Some("running")
                )
        })
        .filter_map(|d| {
            let hex = d["hex_id"].as_str().or_else(|| d["id"].as_str())?;
            let had_env = d["env"].as_object().map(|m| !m.is_empty()).unwrap_or(false);
            Some((hex.to_string(), had_env))
        })
        .collect();
    let mut removed = 0usize;
    for (hex, had_env) in old {
        if had_env && new_env.is_empty() {
            eprintln!(
                "⚠ the running deployment has env vars but .aoraki.env is empty/missing — \
                 the replacement will start with NO env (GPU deploys don't use console env groups)"
            );
        }
        println!("→ replacing previous GPU deployment {hex} (un-deploying; billing stops)");
        match console.delete(&format!("/orgs/{}/gpu-deploys/{hex}", console.org_hex)) {
            Ok(_) => removed += 1,
            Err(e) => eprintln!("⚠ could not remove {hex}: {e:#} — check the console, it may still be billing"),
        }
    }
    Ok(removed)
}

/// True when .aoraki.env is tracked or would be committed (not ignored).
fn dotenv_is_tracked(repo_root: &std::path::Path) -> bool {
    !Command::new("git")
        .current_dir(repo_root)
        .args(["check-ignore", "-q", ".aoraki.env"])
        .status()
        .map(|s| s.success())
        .unwrap_or(true)
}
