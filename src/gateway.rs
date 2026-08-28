//! aoraki-cli-api client (the customer deploy gateway, ADR 008): JSON
//! calls plus the SSE progress stream for deploys.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader};

use crate::config::{GatewayConfig, GlobalConfig};

pub struct Gateway {
    api_url: String,
    token: String,
}

impl Gateway {
    pub fn from_global(global: &GlobalConfig) -> Result<Self> {
        let cfg: &GatewayConfig = global.gateway.as_ref().context(
            "gateway transport needs [gateway] in ~/.config/aoraki/config.toml (api_url + token)",
        )?;
        let token = cfg
            .resolve_token()
            .context("no gateway token — set [gateway].token or the MANIFEST_TOKEN env var")?;
        Ok(Self {
            api_url: cfg.api_url.trim_end_matches('/').to_string(),
            token,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_url, path)
    }

    fn parse(resp: Result<ureq::Response, ureq::Error>) -> Result<Value> {
        match resp {
            Ok(r) => Ok(r.into_json()?),
            Err(ureq::Error::Status(code, r)) => {
                let body: Value = r.into_json().unwrap_or(Value::Null);
                let msg = body["error"]["message"]
                    .as_str()
                    .unwrap_or("request failed")
                    .to_string();
                bail!("gateway returned {code}: {msg}");
            }
            Err(e) => bail!("gateway unreachable: {e}"),
        }
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        Self::parse(
            ureq::post(&self.url(path))
                .set("Authorization", &format!("Bearer {}", self.token))
                .send_json(body),
        )
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        Self::parse(
            ureq::get(&self.url(path))
                .set("Authorization", &format!("Bearer {}", self.token))
                .call(),
        )
    }

    /// Follow a deploy's SSE stream, printing log lines as they arrive.
    /// Returns the terminal status ("succeeded" / "failed").
    pub fn stream_deploy_events(&self, deploy_id: &str) -> Result<String> {
        let resp = ureq::get(&self.url(&format!("/deploys/{deploy_id}/events")))
            .set("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| anyhow::anyhow!("event stream failed: {e}"))?;

        let reader = BufReader::new(resp.into_reader());
        for line in reader.lines() {
            let line = line?;
            let Some(data) = line.strip_prefix("data: ") else {
                continue; // event:/id:/keep-alive comment lines
            };
            let Ok(event) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            match event["kind"].as_str() {
                Some("log") => {
                    if let Some(text) = event["line"].as_str() {
                        println!("{text}");
                    }
                }
                Some("status") => {
                    let status = event["status"].as_str().unwrap_or("failed").to_string();
                    let dur = event["duration_secs"].as_i64().unwrap_or(0);
                    println!(
                        "{} deploy {status} in {dur}s",
                        if status == "succeeded" { "✓" } else { "✗" }
                    );
                    return Ok(status);
                }
                _ => {}
            }
        }
        bail!("event stream ended without a terminal status — check `aoraki deploys`");
    }
}
