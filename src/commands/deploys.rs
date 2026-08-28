use crate::context::Ctx;
use anyhow::Result;

pub fn run(env: Option<String>, limit: usize) -> Result<()> {
    let ctx = Ctx::load(env)?;

    if ctx.env().is_gateway() {
        let gw = crate::gateway::Gateway::from_global(&ctx.global)?;
        let resp = gw.get(&format!(
            "/deploys?app={}&environment={}&limit={}",
            ctx.app(),
            ctx.env_name,
            limit
        ))?;
        let rows = resp["data"].as_array().cloned().unwrap_or_default();
        if rows.is_empty() {
            println!("No gateway deploys recorded for {} yet.", ctx.app());
            return Ok(());
        }
        println!("{:<22} {:<10} {:<9} {}", "TIME", "STATUS", "SHA", "DURATION");
        for rec in rows {
            let sha = rec["commit_sha"].as_str().unwrap_or("?");
            println!(
                "{:<22} {:<10} {:<9} {}",
                rec["created_at"].as_str().unwrap_or("?"),
                rec["status"].as_str().unwrap_or("?"),
                &sha[..sha.len().min(7)],
                rec["duration_secs"]
                    .as_i64()
                    .map(|d| format!("{d}s"))
                    .unwrap_or_else(|| "-".to_string()),
            );
        }
        return Ok(());
    }

    let ssh = ctx.ssh()?;

    let raw = ssh.capture(&format!(
        "tail -n {} {} 2>/dev/null || true",
        limit * 2, // records include 'started' lines we fold below
        ctx.deploy_log()
    ))?;
    if raw.is_empty() {
        println!(
            "No aoraki deploys recorded for {} on {} yet.",
            ctx.app(),
            ctx.env().server
        );
        return Ok(());
    }

    println!("{:<22} {:<10} {:<9} {}", "TIME", "STATUS", "SHA", "DURATION");
    let mut shown = 0;
    for line in raw.lines().rev() {
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let status = rec["status"].as_str().unwrap_or("?");
        if status == "started" {
            continue; // terminal record for the same deploy already shown
        }
        let sha = rec["sha"].as_str().unwrap_or("?");
        let dur = rec["duration_secs"]
            .as_u64()
            .map(|d| format!("{d}s"))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<22} {:<10} {:<9} {}",
            rec["time"].as_str().unwrap_or("?"),
            status,
            &sha[..sha.len().min(7)],
            dur
        );
        shown += 1;
        if shown >= limit {
            break;
        }
    }
    Ok(())
}
