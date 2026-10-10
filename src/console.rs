//! Console API client shared by cloud commands (launch, gpus, …): one
//! ureq Agent (connection keep-alive across polls), bearer auth, and the
//! console's `{success, data | error.message}` envelope handling in one
//! place so every command surfaces server messages the same way.

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::config;

pub struct Console {
    agent: ureq::Agent,
    api: String,
    token: String,
    pub org_hex: String,
    pub org_name: String,
    /// This env's image registry host, as advertised by the console
    /// (envs have separate registries). None on older consoles.
    pub registry: Option<String>,
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
            registry: None,
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
        console.registry = me["data"]["registry"].as_str().map(String::from);
        Ok(console)
    }

    /// The org key — also the registry password (token-auth realm).
    pub fn token(&self) -> &str {
        &self.token
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

/// A console answer that wasn't success. `status` is None when the request
/// never got an HTTP answer (network, timeout) — the retryable kind.
#[derive(Debug)]
pub struct ConsoleError {
    pub status: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for ConsoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConsoleError {}

impl ConsoleError {
    /// True for answers that won't change by asking again: bad key,
    /// no access, gone. Polls stop on these instead of "reconnecting".
    pub fn is_permanent(err: &anyhow::Error) -> bool {
        err.downcast_ref::<ConsoleError>()
            .and_then(|e| e.status)
            .is_some_and(|s| matches!(s, 401 | 403 | 404))
    }
}

fn parse(resp: std::result::Result<ureq::Response, ureq::Error>) -> Result<Value> {
    match resp {
        Ok(r) => Ok(r.into_json()?),
        Err(ureq::Error::Status(code, r)) => {
            let body: Value = r.into_json().unwrap_or(Value::Null);
            let message = body["error"]["message"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| format!("HTTP {code}"));
            Err(ConsoleError {
                status: Some(code),
                message,
            }
            .into())
        }
        Err(e) => Err(ConsoleError {
            status: None,
            message: format!("request failed: {e}"),
        }
        .into()),
    }
}
