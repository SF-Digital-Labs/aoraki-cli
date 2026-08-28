use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aoraki",
    version,
    about = "Aoraki — deploy code straight to your servers, Heroku-style"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Connect this machine to an Aoraki console (opens the tokens page, paste a token)
    Login {
        /// Remote name, e.g. company / personal (default: sole or [defaults].remote)
        remote: Option<String>,
        /// Aoraki API URL (e.g. https://aoraki.manifest.network/api/v1) — required for a new remote
        #[arg(long)]
        url: Option<String>,
        /// Don't open the browser; just prompt for the token
        #[arg(long)]
        no_browser: bool,
    },
    /// Forget the stored token for a remote (the remote itself is kept)
    Logout { remote: Option<String> },
    /// Show configured remotes and who each token belongs to
    Whoami {
        /// Check just this remote (default: all)
        remote: Option<String>,
    },
    /// Set up bare repos + deploy hooks on each configured server (idempotent)
    Link,
    /// Connect an external provider (one-time): `aoraki connect github`
    Connect {
        /// Provider to connect (currently: github)
        provider: String,
    },
    /// Push a commit to a box and run its build-and-deploy pipeline
    Deploy {
        /// Target environment from aoraki.toml (default: global default or sole env)
        env: Option<String>,
        /// Commit/branch to deploy (default: HEAD)
        #[arg(long = "ref")]
        git_ref: Option<String>,
    },
    /// Show application logs from the running pods
    Logs {
        env: Option<String>,
        /// Which pods: all (default, every pod interleaved), web, or workers
        target: Option<String>,
        /// Override the environment (default: the one whose branch you're on)
        #[arg(long = "app")]
        app: Option<String>,
        /// Follow the log stream live
        #[arg(short = 'f', long)]
        follow: bool,
        /// Only logs newer than this (e.g. 1h, 30m)
        #[arg(long)]
        since: Option<String>,
        /// Logs from the previous (crashed) container
        #[arg(long)]
        previous: bool,
        /// Number of lines to show
        #[arg(short = 'n', long, default_value_t = 200)]
        lines: u32,
    },
    /// Recent deploy history for this app (from the box-local deploy log)
    Deploys {
        env: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Deployed commit, pod readiness, and drift vs local HEAD
    Status { env: Option<String> },
    /// Run a one-off command in a temporary pod with the app's image
    Run {
        env: Option<String>,
        /// Command to run after --  (e.g. aoraki run staging -- bash)
        #[arg(last = true)]
        cmd: Vec<String>,
    },
    /// Rolling-restart the deployment without building
    Restart { env: Option<String> },
    /// Open the environment's URL in the browser
    Open { env: Option<String> },
    /// Verify the whole deploy chain end-to-end
    Doctor { env: Option<String> },
    /// Show configmap/secret keys for the environment (names only, never values)
    Config { env: Option<String> },
}
