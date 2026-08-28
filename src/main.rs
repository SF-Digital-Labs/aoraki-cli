mod aoraki;
mod cli;
mod commands;
mod config;
mod context;
mod gateway;
mod hook;
mod ssh;
mod util;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    if let Err(err) = commands::dispatch(cli) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
