use anyhow::Result;
use clap::{Parser, Subcommand};
use shakehands::{config::Config, linux};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "shakehands",
    about = "Filter accidental keyboard and mouse input on Linux"
)]
struct Cli {
    #[arg(long, global = true, default_value = "/etc/shakehands/config.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Daemon,
    Devices,
    Validate,
    Monitor,
    Status,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Devices => {
            let config = Config::load(&cli.config).ok();
            linux::print_devices(config.as_ref())
        }
        Command::Validate => {
            Config::load(&cli.config)?;
            println!("configuration is valid");
            Ok(())
        }
        Command::Daemon => linux::run_daemon(Config::load(&cli.config)?),
        Command::Monitor => linux::monitor(&Config::load(&cli.config)?),
        Command::Status => linux::service_status(&Config::load(&cli.config)?),
    }
}
