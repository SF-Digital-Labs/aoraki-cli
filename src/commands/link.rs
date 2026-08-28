use crate::config::{self};
use crate::hook;
use crate::ssh::Ssh;
use crate::util::{git_capture, sh_quote};
use anyhow::{Context, Result};

/// Set up server-side deploy plumbing for every environment in aoraki.toml:
/// bare repo, post-receive hook, hook config, deploy-log dir, and a local git
/// remote per environment. Idempotent — re-running repairs/upgrades hooks.
pub fn run() -> Result<()> {
    let (repo_root, repo) = config::find_repo_config()?;
    let global = config::load_global()?;
    let app = &repo.app.name;
    let mut warnings = 0usize;

    for (name, env) in &repo.environments {
        let target = config::resolve_target(&global, &env.server);
        println!(
            "Linking '{}' → {} (namespace {}, branch {})",
            name, target, env.namespace, env.branch
        );
        let ssh = Ssh::new(target.clone())?;
        let bare = format!("/data/git/{}.git", app);
        let workdir = env
            .workdir
            .clone()
            .unwrap_or_else(|| format!("/data/repos/{}", app));

        ssh.run(&format!(
            "mkdir -p /data/git && {{ [ -d {bare} ] || git init --bare -q {bare}; }}"
        ))
        .with_context(|| format!("creating bare repo on {} — is /data writable by this user?", target))?;

        // Deploy-log dir is best-effort here; the hook falls back to the bare
        // repo dir if /data/logs isn't writable.
        let _ = ssh.run(&format!("mkdir -p /data/logs/aoraki-deploys 2>/dev/null || true"));

        ssh.run_with_stdin(
            &format!(
                "cat > {bare}/hooks/post-receive.tmp && mv {bare}/hooks/post-receive.tmp {bare}/hooks/post-receive && chmod +x {bare}/hooks/post-receive"
            ),
            &hook::script(),
        )
        .context("installing post-receive hook")?;
        println!("  ✓ bare repo + hook (version {})", hook::HOOK_VERSION);

        ssh.run(&format!(
            "git --git-dir={bare} config aoraki.app {} && \
             git --git-dir={bare} config aoraki.branch {} && \
             git --git-dir={bare} config aoraki.workdir {} && \
             git --git-dir={bare} config aoraki.deployscript {}",
            sh_quote(app),
            sh_quote(&env.branch),
            sh_quote(&workdir),
            sh_quote(&env.deploy_script),
        ))
        .context("writing hook config")?;
        println!("  ✓ hook config");

        let checks = ssh.capture(&format!(
            "[ -d {wd} ] && echo dir_ok || echo dir_missing; \
             [ -e {wd}/.deploy.env ] && echo denv_ok || echo denv_missing; \
             [ -f {wd}/{script} ] && echo script_ok || echo script_missing",
            wd = workdir,
            script = env.deploy_script
        ))?;
        if checks.contains("dir_missing") {
            println!("  ⚠ workdir {} does not exist — clone the repo there first (playbook § 12)", workdir);
            warnings += 1;
        }
        if checks.contains("denv_missing") {
            println!("  ⚠ {}/.deploy.env missing — the manual ucn/dcn fallback won't work there", workdir);
            warnings += 1;
        }
        if checks.contains("script_missing") {
            println!("  ⚠ deploy script {}/{} not found (will exist after first checkout if it's committed)", workdir, env.deploy_script);
            warnings += 1;
        }

        // Local git remote: aoraki-<env>
        let remote_name = format!("aoraki-{}", name);
        let url = format!("ssh://{}{}", target, bare);
        let existing = git_capture(&repo_root, &["remote"])?;
        if existing.lines().any(|r| r == remote_name) {
            git_capture(&repo_root, &["remote", "set-url", &remote_name, &url])?;
        } else {
            git_capture(&repo_root, &["remote", "add", &remote_name, &url])?;
        }
        println!("  ✓ local remote '{}' → {}", remote_name, url);
    }

    if warnings > 0 {
        println!("\nLinked with {warnings} warning(s) — see above. Run `aoraki doctor` after fixing.");
    } else {
        println!("\nAll environments linked. Try `aoraki doctor`, then `aoraki deploy <env>`.");
    }
    Ok(())
}
