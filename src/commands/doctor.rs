use crate::config::{self};
use crate::context::Ctx;
use crate::hook;
use anyhow::Result;

/// End-to-end preflight for one env (or all). Every check prints pass/fail
/// with a fix hint; exits non-zero if anything failed.
pub fn run(env: Option<String>) -> Result<()> {
    let (_, repo) = config::find_repo_config()?;
    let global = config::load_global()?;

    let envs: Vec<String> = match env {
        Some(name) => vec![config::resolve_env_name(Some(name), None, &repo, &global)?],
        None => repo.environments.keys().cloned().collect(),
    };

    let mut failed = false;
    for name in envs {
        let ctx = Ctx::load(Some(name))?;
        println!(
            "\nEnvironment '{}' ({} → {}):",
            ctx.env_name,
            ctx.env().namespace,
            ctx.ssh_target()
        );
        failed |= !check_env(&ctx);
    }

    println!("\nAoraki:");
    if global.remotes.is_empty() {
        println!("  - no remotes configured — run `aoraki login --url <aoraki-api-url>` (deploy events won't be reported)");
    }
    for (name, cfg) in &global.remotes {
        match &cfg.token {
            None => println!("  - {name}: not logged in — run `aoraki login {name}`"),
            Some(token) => match crate::aoraki::whoami(&cfg.api_url, token) {
                Ok(id) => println!("  ✓ {name}: token accepted (org: {})", id.org),
                Err(err) => {
                    println!("  ✗ {name}: {err} — run `aoraki login {name}`");
                    failed = true;
                }
            },
        }
    }

    if failed {
        anyhow::bail!("doctor found problems — see above");
    }
    println!("\nAll checks passed.");
    Ok(())
}

fn check_env(ctx: &Ctx) -> bool {
    let mut ok = true;
    let ssh = match ctx.ssh() {
        Ok(ssh) => ssh,
        Err(err) => {
            println!("  ✗ ssh setup failed: {err}");
            return false;
        }
    };

    match ssh.capture("echo ok") {
        Ok(_) => println!("  ✓ ssh connectivity"),
        Err(err) => {
            println!("  ✗ ssh connectivity: {err}");
            println!("    (check ~/.ssh/config for '{}' or add it under [servers] in the global config)", ctx.env().server);
            return false; // nothing else can run
        }
    }

    let bare = ctx.bare_repo();
    match ssh.capture(&format!("[ -d {bare} ] && echo yes || echo no")) {
        Ok(v) if v == "yes" => println!("  ✓ bare repo {bare}"),
        _ => {
            println!("  ✗ bare repo {bare} missing — run `aoraki link`");
            ok = false;
        }
    }

    match ssh.capture(&format!(
        "grep -m1 -o 'aoraki-hook-version: [0-9]*' {bare}/hooks/post-receive 2>/dev/null || echo missing"
    )) {
        Ok(v) if v.ends_with(&hook::HOOK_VERSION.to_string()) && v != "missing" => {
            println!("  ✓ hook version {}", hook::HOOK_VERSION)
        }
        Ok(v) if v == "missing" => {
            println!("  ✗ post-receive hook missing — run `aoraki link`");
            ok = false;
        }
        Ok(v) => {
            println!("  ✗ hook outdated ({v}, want {}) — run `aoraki link`", hook::HOOK_VERSION);
            ok = false;
        }
        Err(_) => {
            println!("  ✗ could not inspect hook");
            ok = false;
        }
    }

    let wd = ctx.workdir();
    match ssh.capture(&format!(
        "[ -d {wd} ] && echo dir_ok || echo dir_missing; [ -e {wd}/.deploy.env ] && echo denv_ok || echo denv_missing"
    )) {
        Ok(v) => {
            if v.contains("dir_ok") {
                println!("  ✓ workdir {wd}");
            } else {
                println!("  ✗ workdir {wd} missing — clone the repo there (playbook § 12)");
                ok = false;
            }
            if v.contains("denv_ok") {
                println!("  ✓ .deploy.env present");
            } else {
                println!("  ⚠ .deploy.env missing (manual ucn/dcn fallback unavailable)");
            }
        }
        Err(_) => {
            println!("  ✗ could not inspect workdir");
            ok = false;
        }
    }

    match ssh.capture("docker info --format ok 2>/dev/null || echo fail") {
        Ok(v) if v == "ok" => println!("  ✓ docker"),
        _ => {
            println!("  ✗ docker not usable by this user — add the user to the docker group");
            ok = false;
        }
    }

    match ssh.capture("sudo -n kubectl get nodes --no-headers 2>/dev/null | wc -l") {
        Ok(v) if v.trim().parse::<u32>().unwrap_or(0) > 0 => println!("  ✓ kubectl (via sudo)"),
        _ => {
            println!("  ✗ `sudo kubectl` failed — check passwordless sudo for kubectl (playbook § 2)");
            ok = false;
        }
    }

    match ssh.capture("df -h /data 2>/dev/null | awk 'NR==2 {print $5\" used, \"$4\" free\"}'") {
        Ok(v) if !v.is_empty() => println!("  ✓ /data disk: {v}"),
        _ => println!("  ⚠ could not read /data disk usage"),
    }

    ok
}
