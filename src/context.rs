use crate::config::{self, EnvConfig, GlobalConfig, RepoConfig};
use crate::ssh::Ssh;
use anyhow::Result;
use std::path::PathBuf;

/// Everything a command needs to act on one environment of one app.
pub struct Ctx {
    pub repo_root: PathBuf,
    pub repo: RepoConfig,
    pub global: GlobalConfig,
    pub env_name: String,
}

impl Ctx {
    pub fn load(env_arg: Option<String>) -> Result<Self> {
        let (repo_root, repo) = config::find_repo_config()?;
        let global = config::load_global()?;
        // The checked-out branch drives the default environment (an env whose
        // `branch` matches wins) — visible state, unlike a sticky "last used".
        let git_branch =
            crate::util::git_capture(&repo_root, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();
        let env_name = config::resolve_env_name(env_arg, git_branch.as_deref(), &repo, &global)?;
        Ok(Self {
            repo_root,
            repo,
            global,
            env_name,
        })
    }

    pub fn env(&self) -> &EnvConfig {
        &self.repo.environments[&self.env_name]
    }

    pub fn app(&self) -> &str {
        &self.repo.app.name
    }

    pub fn ssh_target(&self) -> Result<String> {
        Ok(config::resolve_target(&self.global, self.env().server()?))
    }

    /// Box namespace; lease environments run in the Aoraki cloud instead.
    pub fn namespace(&self) -> Result<&str> {
        self.env().namespace.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "this is a lease environment - it runs in the Aoraki cloud, not \
                 a k8s namespace; watch it in the console or use `aoraki deploy`"
            )
        })
    }

    pub fn ssh(&self) -> Result<Ssh> {
        Ssh::new(self.ssh_target()?)
    }

    pub fn bare_repo(&self) -> String {
        format!("/data/git/{}.git", self.app())
    }

    pub fn workdir(&self) -> String {
        self.env()
            .workdir
            .clone()
            .unwrap_or_else(|| format!("/data/repos/{}", self.app()))
    }

    /// The cloud deployment this environment deploys to — its identity in
    /// the org. `name =` in the environment wins; production (or a repo's
    /// only environment) is the bare app name; any other environment is
    /// `<app>-<env>`, so `aoraki deploy staging` can never update
    /// production's deployment.
    pub fn deployment_name(&self) -> String {
        deployment_name(&self.repo, &self.env_name)
    }

    pub fn deployment(&self) -> String {
        format!("{}-deployment", self.app())
    }

    pub fn push_url(&self) -> Result<String> {
        Ok(format!("ssh://{}{}", self.ssh_target()?, self.bare_repo()))
    }

    pub fn kubectl(&self) -> String {
        format!(
            "sudo kubectl -n {}",
            self.env().namespace.as_deref().unwrap_or("default")
        )
    }

    pub fn deploy_log(&self) -> String {
        format!("/data/logs/aoraki-deploys/{}.jsonl", self.app())
    }
}

pub fn deployment_name(repo: &RepoConfig, env_name: &str) -> String {
    let env = &repo.environments[env_name];
    match &env.name {
        Some(n) => n.clone(),
        None if env_name == "production" || repo.environments.len() == 1 => repo.app.name.clone(),
        None => format!("{}-{env_name}", repo.app.name),
    }
}

#[cfg(test)]
mod tests {
    use super::deployment_name;
    use crate::config::RepoConfig;

    fn repo(envs: &str) -> RepoConfig {
        toml::from_str(&format!("[app]\nname = \"web\"\n{envs}")).unwrap()
    }

    #[test]
    fn environments_never_share_a_deployment() {
        let r = repo(
            "[environments.production]\nbranch = \"main\"\n\
             [environments.staging]\nbranch = \"qa\"\n\
             [environments.demo]\nbranch = \"demo\"\nname = \"web-demo-site\"\n",
        );
        assert_eq!(deployment_name(&r, "production"), "web");
        assert_eq!(deployment_name(&r, "staging"), "web-staging");
        assert_eq!(deployment_name(&r, "demo"), "web-demo-site");
    }

    #[test]
    fn a_single_environment_keeps_the_app_name() {
        let r = repo("[environments.staging]\nbranch = \"main\"\n");
        assert_eq!(deployment_name(&r, "staging"), "web");
    }
}
