//! `aoraki gpus` — list Aoraki GPUs available to deploy on
//! (`aoraki launch --gpu`). Provider-neutral: model, VRAM, price,
//! availability — the console handles where they physically run.

use anyhow::Result;

use crate::console::Console;

pub fn run(remote: Option<String>) -> Result<()> {
    let console = Console::connect(remote.as_deref())?;

    let resp = console.get(&format!("/orgs/{}/gpu-catalog", console.org_hex))?;
    let gpus = resp["data"].as_array().cloned().unwrap_or_default();
    if gpus.is_empty() {
        println!("no GPUs in the catalog yet — availability refreshes every ~15 minutes");
        return Ok(());
    }

    println!("{:<28} {:>6} {:>9} {:>7}", "GPU", "VRAM", "$/hr", "avail");
    println!("{}", "─".repeat(54));
    for g in &gpus {
        let name = g["name"].as_str().unwrap_or("?");
        let vram = g["vram_gb"].as_i64().map(|v| format!("{v}G")).unwrap_or_else(|| "—".into());
        let rate = g["hourly_rate_usd"].as_f64().map(|r| format!("{r:.2}")).unwrap_or_else(|| "—".into());
        let avail = g["available_count"].as_i64().unwrap_or(0);
        let usable = avail > 0 && g["is_available"].as_bool().unwrap_or(false);
        let dim = if usable { "" } else { "\x1b[2m" };
        let reset = if usable { "" } else { "\x1b[0m" };
        println!("{dim}{name:<28} {vram:>6} {rate:>9} {avail:>7}{reset}");
    }
    println!("\ndeploy: aoraki launch --image IMG --port PORT --gpu \"MODEL\"   (or bare --gpu for cheapest available)");
    println!("billing runs per-hour for the whole time a GPU deployment exists — un-deploy when done");
    Ok(())
}

/// `aoraki gpu-logs <id>` — container logs for a GPU deployment.
pub fn logs(id: String, remote: Option<String>) -> Result<()> {
    let console = Console::connect(remote.as_deref())?;
    let resp = console.get(&format!("/orgs/{}/gpu-deploys/{}/logs", console.org_hex, id))?;
    match resp["data"]["logs"].as_str() {
        Some(logs) if !logs.is_empty() => println!("{logs}"),
        _ => println!("no logs available yet — the workload may still be starting"),
    }
    Ok(())
}

/// `aoraki gpu-rm <id>` — un-deploy a GPU workload and stop its per-hour
/// billing. Always succeeds org-side; a provider that already lost the job
/// is cleaned up regardless.
pub fn rm(id: String, remote: Option<String>) -> Result<()> {
    let console = Console::connect(remote.as_deref())?;
    console.delete(&format!("/orgs/{}/gpu-deploys/{}", console.org_hex, id))?;
    println!("✓ un-deployed {id} — billing stopped");
    Ok(())
}
