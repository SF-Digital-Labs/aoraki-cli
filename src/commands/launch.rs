//! `aoraki launch` — deploy a container image to the Aoraki cloud.
//!
//! Talks to the console's server-side deploy API (ADR 012 Model A): the
//! console signs the lease with the org tenant wallet, drives Fred, and
//! records the deployment. The CLI token is org-scoped, so the target org
//! comes from the token — no flag needed. Environment (testnet vs mainnet)
//! is picked by remote: `aoraki launch --remote testnet …`.

use std::time::Duration;

use crate::config;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const POLL_ATTEMPTS: u32 = 36; // 3 minutes

pub struct LaunchArgs {
    pub image: String,
    pub port: u16,
    pub name: Option<String>,
    pub env: Vec<String>,
    pub size: String,
    pub domain: Option<String>,
    pub remote: Option<String>,
}

pub fn run(args: LaunchArgs) -> Result<()> {
    let global = config::load_global()?;
    let (remote_name, remote) = global.resolve_remote(args.remote.as_deref())?;
    let token = remote
        .token
        .as_deref()
        .with_context(|| format!("remote '{remote_name}' has no token — run `aoraki login {remote_name}`"))?;
    let api = remote.api_url.trim_end_matches('/');

    // Org comes from the token (org-scoped).
    let me = get(api, token, "/cli/me")?;
    let org_hex = me["data"]["org_hex"]
        .as_str()
        .context("console did not return org_hex — is it up to date?")?
        .to_string();
    let org_name = me["data"]["org"].as_str().unwrap_or(&org_hex).to_string();

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

    println!("launching '{name}' → {} (org: {org_name}, remote: {remote_name})", args.image);

    let body = json!({
        "name": name,
        "image": args.image,
        "port": args.port,
        "env": env_map,
        "size": args.size,
        "process_type": "web",
    });
    let created = post(api, token, &format!("/orgs/{org_hex}/deploys"), &body)?;
    let dep_hex = created["data"]["hex_id"]
        .as_str()
        .context("deploy accepted but no deployment id returned")?
        .to_string();
    println!("deployment {dep_hex} registered — waiting for the lease…");

    // Poll until the lease is active (or the deploy fails).
    let mut fqdn = None;
    for attempt in 0..POLL_ATTEMPTS {
        std::thread::sleep(POLL_INTERVAL);
        let dep = get(api, token, &format!("/orgs/{org_hex}/deployments/{dep_hex}"))?;
        let d = &dep["data"]["deployment"];
        let status = d["status"].as_str().or(d["health"].as_str()).unwrap_or("pending");
        let lease = d["lease_uuid"].as_str().unwrap_or("");
        match status {
            "failed" => {
                let note = dep["data"]["history"][0]["commit_message"]
                    .as_str()
                    .unwrap_or("no failure detail recorded");
                bail!("deploy failed: {note}");
            }
            "active" if !lease.is_empty() => {
                let cost = d["hourly_cost_upwr"].as_i64().unwrap_or(0);
                println!("lease {lease} active — {cost} µPWR/hr");
                let live = get(api, token, &format!("/orgs/{org_hex}/deployments/{dep_hex}/live"))?;
                fqdn = live["data"]["connection"]["fqdn"]
                    .as_str()
                    .or_else(|| live["data"]["fqdn"].as_str())
                    .map(String::from);
                break;
            }
            _ => {
                if attempt % 4 == 3 {
                    println!("  …{status} ({}s)", (attempt + 1) * POLL_INTERVAL.as_secs() as u32);
                }
            }
        }
    }
    let fqdn = fqdn.context("timed out waiting for the lease — check `deployments` in the console")?;
    println!("live: https://{fqdn}/");

    if let Some(domain) = &args.domain {
        post(
            api,
            token,
            &format!("/orgs/{org_hex}/deployments/{dep_hex}/domain"),
            &json!({ "domain": domain }),
        )?;
        println!("domain {domain} claimed on-chain");
        println!("  → point DNS at it: CNAME {domain} → {fqdn} (DNS only, no proxy)");
        println!("  → TLS auto-issues once DNS resolves; if it stalls, `restart` the deployment");
    }

    Ok(())
}

fn get(api: &str, token: &str, path: &str) -> Result<Value> {
    parse(
        ureq::get(&format!("{api}{path}"))
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(Duration::from_secs(30))
            .call(),
    )
}

fn post(api: &str, token: &str, path: &str, body: &Value) -> Result<Value> {
    parse(
        ureq::post(&format!("{api}{path}"))
            .set("Authorization", &format!("Bearer {token}"))
            .timeout(Duration::from_secs(60))
            .send_json(body.clone()),
    )
}

fn parse(resp: Result<ureq::Response, ureq::Error>) -> Result<Value> {
    match resp {
        Ok(r) => Ok(r.into_json()?),
        Err(ureq::Error::Status(code, r)) => {
            let body: Value = r.into_json().unwrap_or(Value::Null);
            let msg = body["error"]["message"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| format!("HTTP {code}"));
            bail!("{msg}");
        }
        Err(e) => bail!("request failed: {e}"),
    }
}
