mod config_cmd;
mod connect;
mod deploy;
mod deploys;
mod doctor;
mod launch;
mod link;
mod login;
mod logout;
mod logs;
mod open;
mod restart;
mod run;
mod status;
mod whoami;

use crate::cli::{Cli, Command};
use anyhow::Result;

pub fn dispatch(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Login {
            remote,
            url,
            no_browser,
        } => login::run(remote, url, no_browser),
        Command::Logout { remote } => logout::run(remote),
        Command::Whoami { remote } => whoami::run(remote),
        Command::Link => link::run(),
        Command::Connect { provider } => connect::run(provider),
        Command::Deploy { env, git_ref } => deploy::run(env, git_ref),
        Command::Launch {
            image,
            port,
            name,
            env,
            size,
            domain,
            remote,
        } => launch::run(launch::LaunchArgs {
            image,
            port,
            name,
            env,
            size,
            domain,
            remote,
        }),
        Command::Logs {
            env,
            target,
            app,
            follow,
            since,
            previous,
            lines,
        } => logs::run(env, target, app, follow, since, previous, lines),
        Command::Deploys { env, limit } => deploys::run(env, limit),
        Command::Status { env } => status::run(env),
        Command::Run { env, cmd } => run::run(env, cmd),
        Command::Restart { env } => restart::run(env),
        Command::Open { env } => open::run(env),
        Command::Doctor { env } => doctor::run(env),
        Command::Config { env } => config_cmd::run(env),
    }
}
