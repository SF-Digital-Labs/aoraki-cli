//! Console API client shared by cloud commands (launch, gpus, …): one
//! ureq Agent (connection keep-alive across polls), bearer auth, and the
//! console's `{success, data | error.message}` envelope handling in one
//! place so every command surfaces server messages the same way.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::config;

pub struct Console {
    agent: ureq::Agent,
    api: String,
    token: String,
    pub org_hex: String,
    pub org_name: String,
    pub remote_name: String,
}

impl Console {
    /// Resolve the remote, require a token, and identify the org — the
    /// prologue every cloud command shares.
    pub fn connect(remote: Option<&str>) -> Result<Self> {
        let global = config::load_global()?;
        let (remote_name, remote) = global.resolve_remote(remote)?;
        let token = remote
            .token
            .as_deref()
            .with_context(|| {
                format!("remote '{remote_name}' has no token — run `aoraki login {remote_name}`")
            })?
            .to_string();
        let api = remote.api_url.trim_end_matches('/').to_string();
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(60))
            .build();

        let mut console = Self {
            agent,
            api,
            token,
            org_hex: String::new(),
            org_name: String::new(),
            remote_name: remote_name.to_string(),
        };
        let me = console.get("/cli/me")?;
        console.org_hex = me["data"]["org_hex"]
            .as_str()
            .context("console did not return org_hex — is it up to date?")?
            .to_string();
        console.org_name = me["data"]["org"]
            .as_str()
            .unwrap_or(&console.org_hex)
            .to_string();
        Ok(console)
    }

    pub fn console_base(&self) -> &str {
        self.api.trim_end_matches("/api/v1")
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        parse(
            self.agent
                .get(&format!("{}{path}", self.api))
                .set("Authorization", &format!("Bearer {}", self.token))
                .call(),
        )
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        parse(
            self.agent
                .post(&format!("{}{path}", self.api))
                .set("Authorization", &format!("Bearer {}", self.token))
                .send_json(body.clone()),
        )
    }

    pub fn delete(&self, path: &str) -> Result<Value> {
        parse(
            self.agent
                .delete(&format!("{}{path}", self.api))
                .set("Authorization", &format!("Bearer {}", self.token))
                .call(),
        )
    }
}

fn parse(resp: std::result::Result<ureq::Response, ureq::Error>) -> Result<Value> {
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
