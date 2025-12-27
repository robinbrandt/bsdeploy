use anyhow::{Context, Result};
use std::path::Path;

use crate::ui;

/// Template configuration with comments
const CONFIG_TEMPLATE: &str = r#"# bsdeploy configuration file
# See https://github.com/yourusername/bsdeploy for full documentation

# Service name (required)
service: myapp

# Remote FreeBSD hosts to deploy to (required)
hosts:
  - bsd.example.com

# Run commands with doas privilege escalation (optional, default: false)
doas: true

# User to run the application as (optional)
# If set, the user will be created inside the jail
user: myapp

# Jail-specific configuration (optional)
jail:
  # FreeBSD base version to use (optional, defaults to host version)
  # base_version: "14.1-RELEASE"

  # IP range for jail networking (optional, default: 10.0.0.0/24)
  ip_range: "10.0.0.0/24"

# Reverse proxy configuration (optional)
# Caddy will proxy traffic from hostname to the jail
proxy:
  hostname: myapp.example.com
  port: 3000
  # tls: true  # default: true

# System packages to install in the jail (optional)
packages:
  - curl
  - libyaml

# Development tools to install via mise (optional)
# Tools are installed inside the jail during image building
mise:
  ruby: 3.4.7
  # node: 20.0.0
  # python: 3.11.0

# Environment variables (optional)
env:
  # Clear environment variables (written to config)
  clear:
    - PORT: "3000"
    - RAILS_ENV: production

  # Secret environment variables (read from local environment)
  # These should be set in your local shell before running bsdeploy
  secret:
    - SECRET_KEY_BASE

# Commands to run before starting the application (optional)
# Run inside the jail with the configured user and environment
before_start:
  - bundle install
  - bin/rails assets:precompile
  - bin/rails db:migrate

# Commands to start the application (required)
# Run inside the jail as daemonized processes
start:
  - bin/rails server

# Data directories to persist across deployments (optional)
# Format: "host_path: jail_path" or just "path" for same path
data_directories:
  - /var/bsdeploy/myapp/storage: /app/storage
  # - /var/bsdeploy/myapp/uploads: /app/uploads
"#;

pub fn run(config_path: &Path) -> Result<()> {
    // Check if config file already exists
    if config_path.exists() {
        ui::print_error(&format!(
            "Configuration file already exists at: {}",
            config_path.display()
        ));
        ui::print_step("Use a different path with --config or remove the existing file");
        std::process::exit(1);
    }

    // Create parent directory if needed
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    std::fs::write(config_path, CONFIG_TEMPLATE)
        .with_context(|| format!("Failed to write config file: {}", config_path.display()))?;

    ui::print_success(&format!(
        "Created configuration file at: {}",
        config_path.display()
    ));
    ui::print_step("Edit the file to customize your deployment settings");

    Ok(())
}
