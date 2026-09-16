use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aoraki",
    version,
    about = "Aoraki — ship code to the Aoraki cloud, by hand or by agent"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Connect this machine to an Aoraki console (opens the CLI keys page, paste a key)
    Login {
        /// Remote name, e.g. company / personal (default: sole or [defaults].remote)
        remote: Option<String>,
        /// Aoraki API URL — only needed for testnet/other consoles; a new
        /// remote defaults to mainnet (https://aoraki.cloud/api/v1)
        #[arg(long)]
        url: Option<String>,
        /// Don't open the browser; just prompt for the key
        #[arg(long)]
        no_browser: bool,
    },
    /// Forget the stored key for a remote (the remote itself is kept)
    Logout { remote: Option<String> },
    /// Rename a remote (e.g. after the org: `aoraki rename prod sarsondigital`)
    Rename {
        /// Current remote name
        old: String,
        /// New name
        new: String,
    },
    /// Who this CLI is: remotes, the account behind each key, → the default
    Whoami {
        /// Check just this remote (default: all)
        remote: Option<String>,
        /// Make this remote the default for commands (sets [defaults].remote)
        #[arg(long, value_name = "REMOTE")]
        default: Option<String>,
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
    /// Deploy a container image to the Aoraki cloud (lease on the Manifest network)
    Launch {
        /// Container image, e.g. ghcr.io/acme/site:v1 (public registry: docker.io / ghcr.io)
        #[arg(long)]
        image: String,
        /// Port the container listens on
        #[arg(long)]
        port: u16,
        /// Deployment name (default: image basename)
        #[arg(long)]
        name: Option<String>,
        /// Environment variable KEY=VALUE (repeatable)
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// SKU size name on the target chain
        #[arg(long, default_value = "docker-xlarge-storage")]
        size: String,
        /// Custom domain to claim on-chain once live (e.g. site.example.com)
        #[arg(long)]
        domain: Option<String>,
        /// Deploy onto a GPU. Bare `--gpu` lets the system pick the cheapest
        /// available GPU; `--gpu "RTX 4090"` pins a model (see `aoraki gpus`)
        #[arg(long, num_args = 0..=1, default_missing_value = "auto", value_name = "MODEL")]
        gpu: Option<String>,
        /// Remote console to target, e.g. testnet / company (default: [defaults].remote)
        #[arg(long)]
        remote: Option<String>,
    },
    /// List GPUs available to deploy on (models, VRAM, $/hr, availability)
    Gpus {
        /// Remote console to target (default: [defaults].remote)
        #[arg(long)]
        remote: Option<String>,
    },
    /// Show container logs for a GPU deployment (hashrate, crashes, startup)
    GpuLogs {
        /// GPU deployment id (gpu_...) — shown by `launch --gpu` and the console
        id: String,
        /// Remote console to target (default: [defaults].remote)
        #[arg(long)]
        remote: Option<String>,
    },
    /// Un-deploy a GPU workload and stop its per-hour billing
    GpuRm {
        /// GPU deployment id (gpu_...) — shown by `launch --gpu` and the console
        id: String,
        /// Remote console to target (default: [defaults].remote)
        #[arg(long)]
        remote: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("args should parse")
    }

    #[test]
    fn launch_size_defaults_to_the_testnet_tier() {
        let cli = parse(&["aoraki", "launch", "--image", "nginx:1", "--port", "8080"]);
        match cli.command {
            Command::Launch { size, gpu, name, .. } => {
                assert_eq!(size, "docker-xlarge-storage");
                assert!(gpu.is_none(), "no --gpu means CPU deploy");
                assert!(name.is_none(), "name defaults from image later");
            }
            _ => panic!("expected Launch"),
        }
    }

    #[test]
    fn bare_gpu_flag_means_auto_pick() {
        let cli = parse(&["aoraki", "launch", "--image", "n:1", "--port", "80", "--gpu"]);
        match cli.command {
            Command::Launch { gpu, .. } => assert_eq!(gpu.as_deref(), Some("auto")),
            _ => panic!("expected Launch"),
        }
    }

    #[test]
    fn gpu_flag_accepts_a_pinned_model() {
        let cli = parse(&["aoraki", "launch", "--image", "n:1", "--port", "80", "--gpu", "RTX 4090"]);
        match cli.command {
            Command::Launch { gpu, .. } => assert_eq!(gpu.as_deref(), Some("RTX 4090")),
            _ => panic!("expected Launch"),
        }
    }

    #[test]
    fn launch_requires_image_and_port() {
        assert!(Cli::try_parse_from(["aoraki", "launch", "--port", "80"]).is_err());
        assert!(Cli::try_parse_from(["aoraki", "launch", "--image", "n:1"]).is_err());
    }

    #[test]
    fn launch_env_flag_repeats() {
        let cli = parse(&[
            "aoraki", "launch", "--image", "n:1", "--port", "80",
            "--env", "A=1", "--env", "B=2",
        ]);
        match cli.command {
            Command::Launch { env, .. } => assert_eq!(env, vec!["A=1", "B=2"]),
            _ => panic!("expected Launch"),
        }
    }

    #[test]
    fn logs_defaults_are_200_lines_not_following() {
        let cli = parse(&["aoraki", "logs"]);
        match cli.command {
            Command::Logs { lines, follow, previous, .. } => {
                assert_eq!(lines, 200);
                assert!(!follow);
                assert!(!previous);
            }
            _ => panic!("expected Logs"),
        }
    }

    #[test]
    fn logs_follow_and_line_count_flags_parse() {
        let cli = parse(&["aoraki", "logs", "staging", "-f", "-n", "50"]);
        match cli.command {
            Command::Logs { env, lines, follow, .. } => {
                assert_eq!(env.as_deref(), Some("staging"));
                assert_eq!(lines, 50);
                assert!(follow);
            }
            _ => panic!("expected Logs"),
        }
    }

    #[test]
    fn deploy_takes_a_ref_override() {
        let cli = parse(&["aoraki", "deploy", "production", "--ref", "abc123"]);
        match cli.command {
            Command::Deploy { env, git_ref } => {
                assert_eq!(env.as_deref(), Some("production"));
                assert_eq!(git_ref.as_deref(), Some("abc123"));
            }
            _ => panic!("expected Deploy"),
        }
    }

    #[test]
    fn run_captures_the_command_after_double_dash() {
        let cli = parse(&["aoraki", "run", "staging", "--", "bash", "-c", "echo hi"]);
        match cli.command {
            Command::Run { env, cmd } => {
                assert_eq!(env.as_deref(), Some("staging"));
                assert_eq!(cmd, vec!["bash", "-c", "echo hi"]);
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn connect_requires_a_provider() {
        assert!(Cli::try_parse_from(["aoraki", "connect"]).is_err());
        let cli = parse(&["aoraki", "connect", "github"]);
        match cli.command {
            Command::Connect { provider } => assert_eq!(provider, "github"),
            _ => panic!("expected Connect"),
        }
    }

    #[test]
    fn login_flags_parse() {
        let cli = parse(&["aoraki", "login", "sarson", "--url", "https://x/api/v1", "--no-browser"]);
        match cli.command {
            Command::Login { remote, url, no_browser } => {
                assert_eq!(remote.as_deref(), Some("sarson"));
                assert_eq!(url.as_deref(), Some("https://x/api/v1"));
                assert!(no_browser);
            }
            _ => panic!("expected Login"),
        }
    }

    #[test]
    fn env_is_optional_on_maintenance_commands() {
        for cmd in ["doctor", "status", "restart", "open", "config"] {
            assert!(Cli::try_parse_from(["aoraki", cmd]).is_ok(), "{cmd} bare");
            assert!(Cli::try_parse_from(["aoraki", cmd, "staging"]).is_ok(), "{cmd} with env");
        }
    }
}
