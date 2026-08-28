use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Per-repo `aoraki.toml`, committed at the app repo root.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    pub app: AppSection,
    pub environments: BTreeMap<String, EnvConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSection {
    pub name: String,
    /// GitHub repo (owner/name) — required for gateway-transport environments.
    pub repo: Option<String>,
    /// Environment used when a command names none. Beats the global
    /// [defaults].environment — a per-app default belongs with the app.
    pub default_env: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvConfig {
    /// SSH host alias (resolved via [servers] in the global config, else ~/.ssh/config)
    pub server: String,
    /// Ref the post-receive hook accepts for this environment
    pub branch: String,
    pub namespace: String,
    /// Path to the deploy script, relative to the repo root on the box
    pub deploy_script: String,
    pub url: Option<String>,
    /// Working checkout on the box; defaults to /data/repos/<app>
    pub workdir: Option<String>,
    /// Require typed confirmation before deploying
    #[serde(default)]
    pub confirm: bool,
    /// "direct" (default): SSH push straight to the box (internal operators).
    /// "gateway": deploy through aoraki-cli-api (customer path, ADR 008).
    pub transport: Option<String>,
    /// Which Aoraki remote gets this environment's deploy events
    /// (default: [defaults].remote in the global config, or the sole remote).
    pub remote: Option<String>,
    /// Extra names this environment answers to (e.g. staging: ["qa", "stg"]).
    /// Unique prefixes of the real name (prod → production) work without this.
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl EnvConfig {
    pub fn is_gateway(&self) -> bool {
        self.transport.as_deref() == Some("gateway")
    }
}

/// Global `~/.config/aoraki/config.toml`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    /// Named Aoraki consoles ([remotes.company], [remotes.personal], …) so
    /// one machine can deploy to several clouds. Managed by `aoraki login`.
    #[serde(default)]
    pub remotes: BTreeMap<String, RemoteConfig>,
    pub gateway: Option<GatewayConfig>,
    #[serde(default)]
    pub servers: BTreeMap<String, ServerConfig>,
    #[serde(default)]
    pub defaults: Defaults,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteConfig {
    pub api_url: String,
    /// Absent until `aoraki login` pastes one in (or after `aoraki logout`).
    pub token: Option<String>,
}

impl GlobalConfig {
    /// Pick the Aoraki remote to talk to: explicit name → defaults.remote →
    /// the sole configured remote. Errors name the alternatives.
    pub fn resolve_remote(&self, name: Option<&str>) -> Result<(&str, &RemoteConfig)> {
        let available = || self.remotes.keys().cloned().collect::<Vec<_>>().join(", ");
        if let Some(name) = name {
            return match self.remotes.get_key_value(name) {
                Some((k, v)) => Ok((k, v)),
                None if self.remotes.is_empty() => bail!(
                    "no remotes configured — run `aoraki login {name} --url <aoraki-api-url>`"
                ),
                None => bail!("unknown remote '{}' (configured: {})", name, available()),
            };
        }
        if let Some(def) = &self.defaults.remote {
            if let Some((k, v)) = self.remotes.get_key_value(def) {
                return Ok((k, v));
            }
            bail!(
                "[defaults].remote = '{}' but no such remote (configured: {})",
                def,
                available()
            );
        }
        match self.remotes.len() {
            0 => bail!("no remotes configured — run `aoraki login --url <aoraki-api-url>`"),
            1 => {
                let (k, v) = self.remotes.iter().next().unwrap();
                Ok((k, v))
            }
            _ => bail!(
                "several remotes configured ({}) — name one, or set [defaults].remote",
                available()
            ),
        }
    }
}

/// aoraki-cli-api (the customer deploy gateway). Token may also come from
/// the MANIFEST_TOKEN env var (agents, CI).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    pub api_url: String,
    pub token: Option<String>,
}

impl GatewayConfig {
    pub fn resolve_token(&self) -> Option<String> {
        std::env::var("MANIFEST_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .or_else(|| self.token.clone())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub host: String,
    pub user: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub environment: Option<String>,
    /// Remote used when an environment doesn't pin one.
    pub remote: Option<String>,
}

pub fn config_home() -> PathBuf {
    if let Ok(dir) = std::env::var("MANIFEST_CONFIG_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/aoraki")
}

/// Walk up from the current directory looking for aoraki.toml. manifest.toml
/// is accepted as a fallback at each level so internal repos keep working
/// until they rename (ADR 011: aoraki.toml is the customer-facing name).
pub fn find_repo_config() -> Result<(PathBuf, RepoConfig)> {
    let mut dir = std::env::current_dir()?;
    loop {
        let candidate = ["aoraki.toml", "manifest.toml"]
            .iter()
            .map(|n| dir.join(n))
            .find(|p| p.exists())
            .unwrap_or_else(|| dir.join("aoraki.toml"));
        if candidate.exists() {
            let text = std::fs::read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            let cfg: RepoConfig = toml::from_str(&text)
                .with_context(|| format!("parsing {}", candidate.display()))?;
            if cfg.environments.is_empty() {
                bail!("{} defines no [environments.*]", candidate.display());
            }
            return Ok((dir, cfg));
        }
        if !dir.pop() {
            bail!("no aoraki.toml found in this directory or any parent — run from an app repo, or create one (see `examples/aoraki.toml` in the aoraki-cli repo)");
        }
    }
}

/// Write (or clear, with token=None) a remote in config.toml, creating the
/// file if needed. toml_edit keeps hand-written sections and comments
/// intact. The file ends up 0600 — it holds credentials.
pub fn write_remote(name: &str, api_url: &str, token: Option<&str>) -> Result<()> {
    let dir = config_home();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("config.toml");
    let text = if path.exists() {
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;

    let remotes = doc
        .entry("remotes")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .context("[remotes] in config.toml is not a table")?;
    remotes.set_implicit(true); // render [remotes.<name>], no bare [remotes] header
    let entry = remotes
        .entry(name)
        .or_insert(toml_edit::table())
        .as_table_mut()
        .with_context(|| format!("[remotes.{name}] in config.toml is not a table"))?;
    entry["api_url"] = toml_edit::value(api_url);
    match token {
        Some(t) => {
            entry["token"] = toml_edit::value(t);
        }
        None => {
            entry.remove("token");
        }
    }

    // First login pins [defaults].remote so later remotes don't make every
    // command ask which console it meant.
    if remotes.len() == 1 {
        if let Some(defaults) = doc
            .entry("defaults")
            .or_insert(toml_edit::table())
            .as_table_mut()
        {
            if !defaults.contains_key("remote") {
                defaults["remote"] = toml_edit::value(name);
            }
        }
    }

    std::fs::write(&path, doc.to_string())
        .with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn load_global() -> Result<GlobalConfig> {
    let path = config_home().join("config.toml");
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

pub fn resolve_env_name(
    arg: Option<String>,
    git_branch: Option<&str>,
    repo: &RepoConfig,
    global: &GlobalConfig,
) -> Result<String> {
    let available = || {
        repo.environments
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(name) = arg {
        if repo.environments.contains_key(&name) {
            return Ok(name);
        }
        // Declared aliases (staging: aliases = ["qa", "stg"]).
        for (env_name, env) in &repo.environments {
            if env.aliases.iter().any(|a| a == &name) {
                return Ok(env_name.clone());
            }
        }
        // Unique prefix of a real name (prod → production).
        let prefix_matches: Vec<&String> = repo
            .environments
            .keys()
            .filter(|k| k.starts_with(&name))
            .collect();
        if let [only] = prefix_matches[..] {
            return Ok(only.clone());
        }
        bail!(
            "environment '{}' not in aoraki.toml (available: {})",
            name,
            available()
        );
    }
    // No arg: the checked-out branch picks the environment that deploys it
    // (staging declares branch=qa, production branch=prod). Skipped when two
    // environments share a branch — ambiguous, fall through to the defaults.
    if let Some(branch) = git_branch {
        let branch_matches: Vec<&String> = repo
            .environments
            .iter()
            .filter(|(_, env)| env.branch == branch)
            .map(|(name, _)| name)
            .collect();
        if let [only] = branch_matches[..] {
            return Ok(only.clone());
        }
    }
    if let Some(def) = &repo.app.default_env {
        if repo.environments.contains_key(def) {
            return Ok(def.clone());
        }
        bail!(
            "default_env '{}' in aoraki.toml is not a defined environment (available: {})",
            def,
            available()
        );
    }
    if let Some(def) = &global.defaults.environment {
        if repo.environments.contains_key(def) {
            return Ok(def.clone());
        }
    }
    if repo.environments.len() == 1 {
        return Ok(repo.environments.keys().next().unwrap().clone());
    }
    bail!("specify an environment (available: {})", available());
}

/// Resolve a server alias to an ssh target: global [servers] entry wins,
/// otherwise the alias is passed through to ssh (~/.ssh/config).
pub fn resolve_target(global: &GlobalConfig, server: &str) -> String {
    match global.servers.get(server) {
        Some(s) => match &s.user {
            Some(u) => format!("{}@{}", u, s.host),
            None => s.host.clone(),
        },
        None => server.to_string(),
    }
}
