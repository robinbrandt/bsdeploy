mod caddy;
mod commands;
mod config;
mod constants;
mod image;
mod jail;
mod remote;
mod shell;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the configuration file
    #[arg(short, long, default_value = "config/bsdeploy.yml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new configuration file
    Init,
    /// Setup the remote hosts
    Setup,
    /// Deploy the application
    Deploy,
    /// Show status of jails and services
    Status,
    /// Destroy all resources associated with the service on the remote hosts
    Destroy,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => {
            commands::init(&cli.config)?;
        }
        Commands::Setup | Commands::Deploy | Commands::Status | Commands::Destroy => {
            let config = match config::Config::load(&cli.config) {
                Ok(c) => c,
                Err(e) => {
                    ui::print_error(&format!("Error loading configuration: {}", e));
                    std::process::exit(1);
                }
            };

            ui::print_step(&format!(
                "Loaded configuration for service: {}",
                config.service
            ));

            match cli.command {
                Commands::Setup => commands::setup(&config)?,
                Commands::Deploy => commands::deploy(&config)?,
                Commands::Status => commands::status(&config)?,
                Commands::Destroy => commands::destroy(&config)?,
                Commands::Init => unreachable!(),
            }
        }
    }

    Ok(())
}
