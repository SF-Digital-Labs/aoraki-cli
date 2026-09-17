//! `aoraki launch` — deploy a container image to the Aoraki cloud.
//!
//! Container path (default): the console signs a Manifest-network lease
//! with the org tenant wallet and drives the provider (ADR 012 Model A).
//! GPU path (`--gpu`): the console starts the workload on an Aoraki GPU —
//! system-picked (cheapest available) or pinned by model name.
//! Both are org-scoped via the CLI token; environment is picked by remote.

use std::time::Duration;

use crate::console::Console;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const POLL_ATTEMPTS: u32 = 36; // 3 minutes (container lease)
const GPU_POLL_ATTEMPTS: u32 = 120; // 10 minutes (GPU nodes pull large images)

/// In-place status line (rewrites itself via \r): the terminal shows one
/// live line — "…starting (45s)" — instead of scrolling, and a transient
/// console/provider blip shows as "reconnecting" rather than killing the
/// wait. The deploy itself always continues server-side either way.
struct StatusLine {
    started: std::time::Instant,
}

impl StatusLine {
    fn new() -> Self {
        Self { started: std::time::Instant::now() }
    }
    fn update(&self, status: &str) {
        use std::io::Write;
        let secs = self.started.elapsed().as_secs();
        print!("\r\x1b[2K  …{status} ({secs}s)");
        let _ = std::io::stdout().flush();
    }
    fn reconnecting(&self, failures: u32) {
        use std::io::Write;
        let secs = self.started.elapsed().as_secs();
        print!("\r\x1b[2K  …reconnecting to console (attempt {failures}, {secs}s) — deploy continues server-side");
        let _ = std::io::stdout().flush();
    }
    fn finish(&self) {
        use std::io::Write;
        print!("\r\x1b[2K");
        let _ = std::io::stdout().flush();
    }
}

// Aoraki teal (#58c5d6) via truecolor; degrades to plain text elsewhere.
const TEAL: &str = "\x1b[38;2;88;197;214m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

pub struct LaunchArgs {
    pub image: String,
    pub port: u16,
    pub name: Option<String>,
    pub env: Vec<String>,
    pub size: String,
    pub domain: Option<String>,
    /// GPU deploy: Some("auto") = system picks, Some(model) pins a model.
    pub gpu: Option<String>,
    pub remote: Option<String>,
}

fn print_banner(title: &str, rows: &[(&str, String)]) {
    println!();
    println!("{TEAL}{BOLD}  ✓ {title}{RESET}");
    println!("{TEAL}  ──────────────────────────────────────────{RESET}");
    for (label, value) in rows {
        println!("  {label:<8} {value}");
    }
    println!("{TEAL}  ──────────────────────────────────────────{RESET}");
}

pub fn run(args: LaunchArgs) -> Result<()> {
    let console = Console::connect(args.remote.as_deref())?;

    // Deployment name defaults to the image basename (sans tag).
    let name = args.name.clone().unwrap_or_else(|| {
        let base = args.image.rsplit('/').next().unwrap_or(&args.image);
        base.split(':').next().unwrap_or(base).to_string()
    });

    let mut env_map = serde_json::Map::new();
    for pair in &args.env {
        let (k, v) = pair
            .split_once('=')
            .with_context(|| format!("--env '{pair}' is not KEY=VALUE"))?;
        env_map.insert(k.to_string(), Value::String(v.to_string()));
    }

    if let Some(gpu) = args.gpu.clone() {
        if args.domain.is_some() {
            bail!("--domain is not supported for GPU deploys yet (no FQDN/TLS on GPU endpoints)");
        }
        if args.size != "docker-xlarge-storage" {
            // Default value means "not set"; anything else was explicit.
            println!("note: --size does not apply to GPU deploys (the GPU model determines the unit) — ignoring");
        }
        return run_gpu(&console, &name, &args.image, args.port, &gpu, env_map);
    }

    println!(
        "launching '{name}' → {} (org: {}, remote: {})",
        args.image, console.org_name, console.remote_name
    );

    let body = json!({
        "name": name,
        "image": args.image,
        "port": args.port,
        "env": env_map,
        "size": args.size,
        "process_type": "web",
    });
    let created = console.post(&format!("/orgs/{}/deploys", console.org_hex), &body)?;
    let dep_hex = created["data"]["hex_id"]
        .as_str()
        .context("deploy accepted but no deployment id returned")?
        .to_string();
    println!("deployment {dep_hex} registered — waiting for the lease…");
    println!("{DIM}(safe to close this terminal — the deploy continues server-side; watch it at {}/deployments/{dep_hex}){RESET}", console.console_base());

    let fqdn = wait_for_lease_banner(&console, &dep_hex)?;

    if let Some(domain) = &args.domain {
        console.post(
            &format!("/orgs/{}/deployments/{dep_hex}/domain", console.org_hex),
            &json!({ "domain": domain }),
        )?;
        println!("domain {domain} claimed on-chain");
        println!("  → point DNS at it: CNAME {domain} → {fqdn} (DNS only, no proxy)");
        println!("  → TLS auto-issues once DNS resolves; if it stalls, `restart` the deployment");
    }

    Ok(())
}


/// Poll a container deployment until its lease is active, then print the
/// DEPLOYED banner. Shared by `launch` (create) and `deploy` cloud
/// transport (create or blue-green update). Returns the origin fqdn.
pub(crate) fn wait_for_lease_banner(console: &Console, dep_hex: &str) -> Result<String> {
    // Poll until the lease is active (or the deploy fails). One in-place
    // status line; transient failures display as reconnecting and never
    // kill the wait — only the overall timeout does.
    let mut fqdn = None;
    let mut default_domain: Option<String> = None;
    let mut consecutive_failures = 0u32;
    let line = StatusLine::new();
    for _attempt in 0..POLL_ATTEMPTS {
        std::thread::sleep(POLL_INTERVAL);
        let dep = match console.get(&format!("/orgs/{}/deployments/{dep_hex}", console.org_hex)) {
            Ok(v) => {
                consecutive_failures = 0;
                v
            }
            Err(_) => {
                consecutive_failures += 1;
                line.reconnecting(consecutive_failures);
                continue;
            }
        };
        let d = &dep["data"]["deployment"];
        let status = d["status"].as_str().or(d["health"].as_str()).unwrap_or("pending");
        let lease = d["lease_uuid"].as_str().unwrap_or("");
        match status {
            "failed" => {
                line.finish();
                let note = dep["data"]["history"][0]["commit_message"]
                    .as_str()
                    .unwrap_or("no failure detail recorded");
                bail!("deploy failed: {note}");
            }
            "active" if !lease.is_empty() => {
                line.finish();
                let cost = d["hourly_cost_upwr"].as_i64().unwrap_or(0);
                println!("lease {lease} active — {cost} µPWR/hr");
                default_domain = d["default_domain"].as_str().map(String::from);
                let live = console
                    .get(&format!("/orgs/{}/deployments/{dep_hex}/live", console.org_hex))?;
                fqdn = live["data"]["connection"]["fqdn"]
                    .as_str()
                    .or_else(|| live["data"]["fqdn"].as_str())
                    .map(String::from);
                break;
            }
            _ => line.update(status),
        }
    }
    line.finish();
    let fqdn = fqdn.context("timed out waiting for the lease — check `deployments` in the console")?;
    // The branded default domain is assigned server-side just as the lease
    // goes active; it may not be on the row the instant we read it, so fall
    // back to the origin fqdn and mention it's still provisioning.
    let live_url = match &default_domain {
        Some(d) => format!("{TEAL}{BOLD}https://{d}/{RESET}"),
        None => format!("{TEAL}{BOLD}https://{fqdn}/{RESET}"),
    };
    let mut rows = vec![
        ("live", live_url),
        ("monitor", format!("{DIM}{}/deployments/{dep_hex}{RESET}", console.console_base())),
    ];
    if default_domain.is_some() {
        rows.push(("origin", format!("{DIM}{fqdn}{RESET}")));
    }
    print_banner("DEPLOYED", &rows);
    Ok(fqdn)
}

/// GPU deploy path. The console picks the Aoraki GPU (or honours a pinned
/// model) and starts the workload; we poll for the public endpoint. GPU
/// workloads get a raw http://host:port endpoint — no FQDN/TLS layer yet.
pub(crate) fn run_gpu(
    console: &Console,
    name: &str,
    image: &str,
    port: u16,
    gpu: &str,
    env_map: serde_json::Map<String, Value>,
) -> Result<()> {
    let gpu_field = if gpu == "auto" { Value::Null } else { Value::String(gpu.to_string()) };
    let which = if gpu == "auto" { "cheapest available GPU".to_string() } else { format!("GPU '{gpu}'") };
    println!(
        "launching '{name}' → {image} on {which} (org: {}, remote: {})",
        console.org_name, console.remote_name
    );

    let body = json!({
        "name": name,
        "image": image,
        "port": port,
        "env": env_map,
        "gpu": gpu_field,
    });
    let created = console.post(&format!("/orgs/{}/gpu-deploys", console.org_hex), &body)?;
    let d = &created["data"];
    let gpu_hex = d["hex_id"].as_str().context("gpu deploy accepted but no id returned")?.to_string();
    let gpu_name = d["gpu_name"].as_str().unwrap_or("?").to_string();
    // Id printed BEFORE the wait: if anything below fails, the deploy is
    // still visible/cancellable from the console or another CLI call.
    println!("gpu deployment {gpu_hex} on {gpu_name} — billing runs until you un-deploy");
    println!("{DIM}(safe to close this terminal — the deploy continues server-side; watch it at {}/compute){RESET}", console.console_base());

    let mut node_url = None;
    let mut consecutive_failures = 0u32;
    let line = StatusLine::new();
    for _attempt in 0..GPU_POLL_ATTEMPTS {
        std::thread::sleep(POLL_INTERVAL);
        let dep = match console.get(&format!("/orgs/{}/gpu-deploys/{gpu_hex}", console.org_hex)) {
            Ok(v) => {
                consecutive_failures = 0;
                v
            }
            Err(_) => {
                consecutive_failures += 1;
                line.reconnecting(consecutive_failures);
                continue;
            }
        };
        let d = &dep["data"];
        let status = d["status"].as_str().unwrap_or("starting").to_string();
        match status.as_str() {
            "failed" | "exited" => {
                line.finish();
                let err = d["error"].as_str().unwrap_or("no failure detail recorded");
                bail!("gpu deploy did not start: {err}");
            }
            "cancelled" => {
                line.finish();
                match d["error"].as_str().filter(|s| !s.is_empty()) {
                    Some(err) => bail!("gpu deploy was cancelled — {err}"),
                    None => bail!(
                        "gpu deploy was cancelled by the provider (no reason given) — try another GPU (`aoraki gpus`)"
                    ),
                }
            }
            "running" => {
                if let Some(url) = d["node_url"].as_str() {
                    node_url = Some(url.to_string());
                    break;
                }
                line.update("running — waiting for the public endpoint");
            }
            _ => line.update(&status),
        }
    }
    line.finish();
    let node_url = node_url.with_context(|| {
        format!("timed out waiting for the GPU node — deployment {gpu_hex} may still come up; check the console")
    })?;

    print_banner(
        "DEPLOYED ON GPU",
        &[
            ("gpu", gpu_name.clone()),
            ("live", format!("{TEAL}{BOLD}{node_url}/{RESET}")),
            ("monitor", format!("{DIM}{}/compute{RESET}", console.console_base())),
        ],
    );
    println!("{DIM}  note: GPU endpoints are plain http on the node's port — no TLS/domain layer yet{RESET}");
    println!("{DIM}  billing runs per-hour until you un-deploy this workload{RESET}");
    Ok(())
}
