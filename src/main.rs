mod aoraki;
mod cli;
mod commands;
mod config;
mod console;
mod context;
mod gateway;
mod hook;
mod ssh;
mod update_notice;
mod util;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    // `aoraki update` reports versions itself.
    let notice = (!matches!(cli.command, cli::Command::Update { .. }))
        .then(update_notice::start)
        .flatten();
    let result = commands::dispatch(cli);
    if let Err(err) = &result {
        eprintln!("error: {err:#}");
    }
    if let Some(notice) = notice {
        notice.finish();
    }
    if result.is_err() {
        std::process::exit(1);
    }
}
