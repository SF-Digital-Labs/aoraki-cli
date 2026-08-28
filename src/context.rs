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

    pub fn ssh_target(&self) -> String {
        config::resolve_target(&self.global, &self.env().server)
    }

    pub fn ssh(&self) -> Result<Ssh> {
        Ssh::new(self.ssh_target())
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

    pub fn deployment(&self) -> String {
        format!("{}-deployment", self.app())
    }

    pub fn push_url(&self) -> String {
        format!("ssh://{}{}", self.ssh_target(), self.bare_repo())
    }

    pub fn kubectl(&self) -> String {
        format!("sudo kubectl -n {}", self.env().namespace)
    }

    pub fn deploy_log(&self) -> String {
        format!("/data/logs/aoraki-deploys/{}.jsonl", self.app())
    }
}
