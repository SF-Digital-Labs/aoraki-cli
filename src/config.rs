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
    /// Gateway release deploys: container port. Present = the gateway
    /// builds, pushes to the platform registry, and leases via the console.
    pub port: Option<u16>,
    /// Chain SKU for the lease (console default when absent).
    pub size: Option<String>,
    /// Dockerfile path relative to the repo root (monorepos).
    pub dockerfile: Option<String>,
    /// Lease service name (default "web").
    pub process_type: Option<String>,
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
    /// Org behind the key (recorded by login/whoami). Lets committed
    /// aoraki.toml files pin the ORG — portable — while local names stay
    /// machine-local aliases.
    pub org: Option<String>,
    /// The org's immutable public id (org_…): survives org renames, so
    /// it's the canonical pin for committed configs and CI.
    pub org_id: Option<String>,
}

impl GlobalConfig {
    /// Pick the Aoraki remote to talk to: explicit name → defaults.remote →
    /// the sole configured remote. Errors name the alternatives.
    pub fn resolve_remote(&self, name: Option<&str>) -> Result<(&str, &RemoteConfig)> {
        let available = || self.remotes.keys().cloned().collect::<Vec<_>>().join(", ");
        if let Some(name) = name {
            if let Some((k, v)) = self.remotes.get_key_value(name) {
                return Ok((k, v));
            }
            if self.remotes.is_empty() {
                bail!("no remotes configured — run `aoraki login` (mainnet) or `aoraki login --url <aoraki-api-url>`");
            }
            // Committed configs pin the ORG — by immutable org_… id, name,
            // or console URL; local names are just this machine's aliases.
            if name.starts_with("org_") {
                let hits: Vec<(&str, &RemoteConfig)> = self
                    .remotes
                    .iter()
                    .filter(|(_, c)| c.org_id.as_deref() == Some(name))
                    .map(|(k, v)| (k.as_str(), v))
                    .collect();
                return match hits[..] {
                    [only] => Ok(only),
                    [] => bail!(
                        "no remote for org {name} on this machine — `aoraki login` with a key for that org \
                         (or `aoraki whoami` to refresh org ids on older setups)"
                    ),
                    _ => bail!(
                        "org {name} is logged in on several consoles ({}) — pin the local remote name or console URL",
                        hits.iter()
                            .map(|(k, v)| format!("{k}: {}", v.api_url))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
            }
            if name.starts_with("http://") || name.starts_with("https://") {
                let target = name.trim_end_matches('/');
                let hits: Vec<(&str, &RemoteConfig)> = self
                    .remotes
                    .iter()
                    .filter(|(_, c)| c.api_url.trim_end_matches('/') == target)
                    .map(|(k, v)| (k.as_str(), v))
                    .collect();
                return match hits[..] {
                    [only] => Ok(only),
                    [] => bail!(
                        "no remote for {target} on this machine — run `aoraki login --url {target}`"
                    ),
                    _ => bail!(
                        "several orgs are logged in at {target} ({}) — pin the org or a local remote name",
                        hits.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
                    ),
                };
            }
            let want = normalize(name);
            let hits: Vec<(&str, &RemoteConfig)> = self
                .remotes
                .iter()
                .filter(|(_, c)| c.org.as_deref().is_some_and(|o| normalize(o) == want))
                .map(|(k, v)| (k.as_str(), v))
                .collect();
            return match hits[..] {
                [only] => Ok(only),
                [] => bail!(
                    "'{}' matches no remote name or org on this machine (configured: {}) — \
                     `aoraki whoami` shows orgs; `aoraki login` adds one",
                    name,
                    available()
                ),
                _ => bail!(
                    "org '{}' is logged in on several consoles ({}) — pin the local remote name or the console URL",
                    name,
                    hits.iter()
                        .map(|(k, v)| format!("{k}: {}", v.api_url))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
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

/// Case/punctuation-insensitive comparison key: "Sarson Funds" ==
/// "sarson-funds" == "sarsonfunds".
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
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
pub fn write_remote(
    name: &str,
    api_url: &str,
    token: Option<&str>,
    org: Option<&str>,
    org_id: Option<&str>,
) -> Result<()> {
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
    if let Some(o) = org {
        entry["org"] = toml_edit::value(o);
    }
    if let Some(id) = org_id {
        entry["org_id"] = toml_edit::value(id);
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

/// Rename a remote in config.toml, carrying its url/token and re-pointing
/// [defaults].remote if it referenced the old name.
pub fn rename_remote(old: &str, new: &str) -> Result<()> {
    let path = config_home().join("config.toml");
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("parsing {}", path.display()))?;
    let remotes = doc
        .get_mut("remotes")
        .and_then(|r| r.as_table_mut())
        .context("no [remotes] in config.toml")?;
    let entry = remotes.remove(old).context("remote vanished mid-rename")?;
    remotes.insert(new, entry);
    if let Some(defaults) = doc.get_mut("defaults").and_then(|d| d.as_table_mut()) {
        if defaults.get("remote").and_then(|v| v.as_str()) == Some(old) {
            defaults["remote"] = toml_edit::value(new);
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

/// Set [defaults].remote in config.toml (toml_edit keeps comments intact).
pub fn write_default_remote(name: &str) -> Result<()> {
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
    let defaults = doc
        .entry("defaults")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .context("[defaults] in config.toml is not a table")?;
    defaults["remote"] = toml_edit::value(name);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(toml: &str) -> RepoConfig {
        toml::from_str(toml).expect("valid repo config")
    }
    fn global(toml: &str) -> GlobalConfig {
        toml::from_str(toml).expect("valid global config")
    }
    const MINIMAL_ENV: &str = r#"
        [app]
        name = "demo"
        [environments.production]
        server = "core2"
        branch = "prod"
        namespace = "demo"
        deploy_script = "k8s/deploy.sh"
    "#;
    const TWO_ENVS: &str = r#"
        [app]
        name = "demo"
        [environments.staging]
        server = "lab"
        branch = "qa"
        namespace = "demo-stg"
        deploy_script = "k8s/deploy.sh"
        aliases = ["testnet", "stg"]
        [environments.production]
        server = "core2"
        branch = "prod"
        namespace = "demo"
        deploy_script = "k8s/deploy.sh"
    "#;

    // ── RepoConfig parsing ────────────────────────────────────────────

    #[test]
    fn minimal_repo_config_parses_with_defaults() {
        let cfg = repo(MINIMAL_ENV);
        let env = &cfg.environments["production"];
        assert_eq!(cfg.app.name, "demo");
        assert!(!env.confirm, "confirm defaults to false");
        assert!(env.aliases.is_empty(), "aliases default to empty");
        assert!(env.workdir.is_none());
        assert!(!env.is_gateway(), "no transport = direct");
    }

    #[test]
    fn unknown_field_in_env_is_rejected() {
        let bad = MINIMAL_ENV.replace("namespace", "namspace");
        assert!(toml::from_str::<RepoConfig>(&bad).is_err());
    }

    #[test]
    fn unknown_field_in_app_is_rejected() {
        let bad = format!("{MINIMAL_ENV}\n[app.extra]\nx = 1\n");
        assert!(toml::from_str::<RepoConfig>(&bad).is_err());
    }

    #[test]
    fn missing_app_name_is_rejected() {
        let bad = MINIMAL_ENV.replace("name = \"demo\"", "");
        assert!(toml::from_str::<RepoConfig>(&bad).is_err());
    }

    #[test]
    fn gateway_transport_is_detected() {
        let cfg = repo(&MINIMAL_ENV.replace(
            "deploy_script = \"k8s/deploy.sh\"",
            "deploy_script = \"k8s/deploy.sh\"\ntransport = \"gateway\"",
        ));
        assert!(cfg.environments["production"].is_gateway());
    }

    #[test]
    fn explicit_direct_transport_is_not_gateway() {
        let cfg = repo(&MINIMAL_ENV.replace(
            "deploy_script = \"k8s/deploy.sh\"",
            "deploy_script = \"k8s/deploy.sh\"\ntransport = \"direct\"",
        ));
        assert!(!cfg.environments["production"].is_gateway());
    }

    // ── resolve_env_name ──────────────────────────────────────────────

    fn resolve(arg: Option<&str>, branch: Option<&str>, toml_src: &str) -> Result<String> {
        resolve_env_name(
            arg.map(String::from),
            branch,
            &repo(toml_src),
            &GlobalConfig::default(),
        )
    }

    #[test]
    fn exact_env_name_wins() {
        assert_eq!(resolve(Some("staging"), None, TWO_ENVS).unwrap(), "staging");
    }

    #[test]
    fn declared_alias_resolves() {
        assert_eq!(resolve(Some("testnet"), None, TWO_ENVS).unwrap(), "staging");
        assert_eq!(resolve(Some("stg"), None, TWO_ENVS).unwrap(), "staging");
    }

    #[test]
    fn unique_prefix_resolves() {
        assert_eq!(resolve(Some("prod"), None, TWO_ENVS).unwrap(), "production");
        assert_eq!(resolve(Some("st"), None, TWO_ENVS).unwrap(), "staging");
    }

    #[test]
    fn ambiguous_prefix_errors() {
        let src = TWO_ENVS.replace("environments.production", "environments.stable");
        let err = resolve(Some("st"), None, &src).err().unwrap().to_string();
        assert!(err.contains("not in aoraki.toml"), "got: {err}");
    }

    #[test]
    fn unknown_env_error_lists_available() {
        let err = resolve(Some("nope"), None, TWO_ENVS).err().unwrap().to_string();
        assert!(err.contains("staging") && err.contains("production"), "got: {err}");
    }

    #[test]
    fn explicit_arg_beats_checked_out_branch() {
        assert_eq!(
            resolve(Some("staging"), Some("prod"), TWO_ENVS).unwrap(),
            "staging"
        );
    }

    #[test]
    fn checked_out_branch_picks_its_environment() {
        assert_eq!(resolve(None, Some("qa"), TWO_ENVS).unwrap(), "staging");
        assert_eq!(resolve(None, Some("prod"), TWO_ENVS).unwrap(), "production");
    }

    #[test]
    fn unknown_branch_with_two_envs_errors() {
        let err = resolve(None, Some("feature-x"), TWO_ENVS).err().unwrap().to_string();
        assert!(err.contains("specify an environment"), "got: {err}");
    }

    #[test]
    fn shared_branch_is_ambiguous_and_falls_through() {
        let src = TWO_ENVS.replace("branch = \"prod\"", "branch = \"qa\"");
        // both envs deploy qa → branch can't decide; no defaults → error
        assert!(resolve(None, Some("qa"), &src).is_err());
    }

    #[test]
    fn app_default_env_breaks_the_tie() {
        let src = TWO_ENVS.replace("name = \"demo\"", "name = \"demo\"\ndefault_env = \"staging\"");
        assert_eq!(resolve(None, None, &src).unwrap(), "staging");
    }

    #[test]
    fn invalid_app_default_env_errors() {
        let src = TWO_ENVS.replace("name = \"demo\"", "name = \"demo\"\ndefault_env = \"nope\"");
        let err = resolve(None, None, &src).err().unwrap().to_string();
        assert!(err.contains("default_env"), "got: {err}");
    }

    #[test]
    fn global_default_environment_is_honored() {
        let g = global("[defaults]\nenvironment = \"staging\"\n");
        let name = resolve_env_name(None, None, &repo(TWO_ENVS), &g).unwrap();
        assert_eq!(name, "staging");
    }

    #[test]
    fn sole_environment_needs_no_selection() {
        assert_eq!(resolve(None, None, MINIMAL_ENV).unwrap(), "production");
    }

    // ── resolve_remote ────────────────────────────────────────────────

    const REMOTES: &str = r#"
        [remotes.company]
        api_url = "https://aoraki.cloud/api/v1"
        org = "Manifest"
        org_id = "org_aaaaaaaaaa"
        [remotes.personal]
        api_url = "https://testnet.aoraki.cloud/api/v1"
        org = "Sarson Funds"
        org_id = "org_bbbbbbbbbb"
    "#;

    #[test]
    fn explicit_local_name_resolves() {
        let g = global(REMOTES);
        assert_eq!(g.resolve_remote(Some("company")).unwrap().0, "company");
    }

    #[test]
    fn org_id_pin_resolves() {
        let g = global(REMOTES);
        assert_eq!(g.resolve_remote(Some("org_bbbbbbbbbb")).unwrap().0, "personal");
    }

    #[test]
    fn unknown_org_id_errors_with_login_hint() {
        let g = global(REMOTES);
        let err = g.resolve_remote(Some("org_zzzzzzzzzz")).err().unwrap().to_string();
        assert!(err.contains("aoraki login"), "got: {err}");
    }

    #[test]
    fn org_id_on_multiple_consoles_is_ambiguous() {
        let src = REMOTES.replace("org_bbbbbbbbbb", "org_aaaaaaaaaa");
        let err = global(&src)
            .resolve_remote(Some("org_aaaaaaaaaa"))
            .err().unwrap()
            .to_string();
        assert!(err.contains("several consoles"), "got: {err}");
    }

    #[test]
    fn console_url_pin_resolves_ignoring_trailing_slash() {
        let g = global(REMOTES);
        let (name, _) = g
            .resolve_remote(Some("https://testnet.aoraki.cloud/api/v1/"))
            .unwrap();
        assert_eq!(name, "personal");
    }

    #[test]
    fn unknown_console_url_errors_with_login_hint() {
        let g = global(REMOTES);
        let err = g
            .resolve_remote(Some("https://other.example/api/v1"))
            .err().unwrap()
            .to_string();
        assert!(err.contains("aoraki login --url"), "got: {err}");
    }

    #[test]
    fn url_with_multiple_orgs_is_ambiguous() {
        let src = REMOTES.replace(
            "https://testnet.aoraki.cloud/api/v1",
            "https://aoraki.cloud/api/v1",
        );
        let err = global(&src)
            .resolve_remote(Some("https://aoraki.cloud/api/v1"))
            .err().unwrap()
            .to_string();
        assert!(err.contains("several orgs"), "got: {err}");
    }

    #[test]
    fn org_name_matches_case_and_punctuation_insensitively() {
        let g = global(REMOTES);
        assert_eq!(g.resolve_remote(Some("sarson-funds")).unwrap().0, "personal");
        assert_eq!(g.resolve_remote(Some("SARSONFUNDS")).unwrap().0, "personal");
    }

    #[test]
    fn unmatched_name_lists_configured_remotes() {
        let g = global(REMOTES);
        let err = g.resolve_remote(Some("nope")).err().unwrap().to_string();
        assert!(err.contains("company") && err.contains("personal"), "got: {err}");
    }

    #[test]
    fn defaults_remote_is_used_when_unnamed() {
        let src = format!("{REMOTES}\n[defaults]\nremote = \"personal\"\n");
        assert_eq!(global(&src).resolve_remote(None).unwrap().0, "personal");
    }

    #[test]
    fn dangling_defaults_remote_errors() {
        let src = format!("{REMOTES}\n[defaults]\nremote = \"ghost\"\n");
        let err = global(&src).resolve_remote(None).err().unwrap().to_string();
        assert!(err.contains("ghost"), "got: {err}");
    }

    #[test]
    fn sole_remote_needs_no_selection() {
        let g = global(
            "[remotes.only]\napi_url = \"https://aoraki.cloud/api/v1\"\n",
        );
        assert_eq!(g.resolve_remote(None).unwrap().0, "only");
    }

    #[test]
    fn zero_remotes_errors_with_login_hint() {
        let err = GlobalConfig::default().resolve_remote(None).err().unwrap().to_string();
        assert!(err.contains("aoraki login"), "got: {err}");
    }

    #[test]
    fn multiple_remotes_without_default_must_be_named() {
        let err = global(REMOTES).resolve_remote(None).err().unwrap().to_string();
        assert!(err.contains("[defaults].remote"), "got: {err}");
    }

    // ── resolve_target ────────────────────────────────────────────────

    #[test]
    fn servers_entry_with_user_builds_user_at_host() {
        let g = global("[servers.core2]\nhost = \"1.2.3.4\"\nuser = \"deploy\"\n");
        assert_eq!(resolve_target(&g, "core2"), "deploy@1.2.3.4");
    }

    #[test]
    fn servers_entry_without_user_is_bare_host() {
        let g = global("[servers.core2]\nhost = \"1.2.3.4\"\n");
        assert_eq!(resolve_target(&g, "core2"), "1.2.3.4");
    }

    #[test]
    fn unlisted_alias_passes_through_to_ssh_config() {
        assert_eq!(resolve_target(&GlobalConfig::default(), "deploymfx"), "deploymfx");
    }

    // ── find_repo_config (cwd-sensitive: serialized) ──────────────────

    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn in_dir<T>(dir: &std::path::Path, f: impl FnOnce() -> T) -> T {
        let _guard = CWD_LOCK.lock().unwrap();
        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        let out = f();
        std::env::set_current_dir(orig).unwrap();
        out
    }

    #[test]
    fn aoraki_toml_is_preferred_over_legacy_manifest_toml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("manifest.toml"), MINIMAL_ENV.replace("demo", "old")).unwrap();
        std::fs::write(tmp.path().join("aoraki.toml"), MINIMAL_ENV).unwrap();
        let cfg = in_dir(tmp.path(), || find_repo_config().unwrap().1);
        assert_eq!(cfg.app.name, "demo");
    }

    #[test]
    fn config_is_found_by_walking_up_from_a_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("aoraki.toml"), MINIMAL_ENV).unwrap();
        let sub = tmp.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        let (root, cfg) = in_dir(&sub, || find_repo_config().unwrap());
        assert_eq!(cfg.app.name, "demo");
        assert_eq!(
            root.canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn config_with_no_environments_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("aoraki.toml"), "[app]\nname = \"demo\"\n[environments]\n").unwrap();
        let err = in_dir(tmp.path(), || find_repo_config().err().unwrap().to_string());
        assert!(err.contains("no [environments"), "got: {err}");
    }
}
