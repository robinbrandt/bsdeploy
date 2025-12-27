mod config;
mod constants;
mod remote;
mod ui;
mod jail;
mod image;

use constants::*;

use anyhow::{Context, Result};

use indicatif::ProgressBar;
use std::path::PathBuf;
use clap::{Parser, Subcommand};

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
    /// Destroy all resources associated with the service on the remote hosts
    Destroy,
}

fn maybe_doas(cmd: &str, doas: bool) -> String {
    if doas {
        format!("doas {}", cmd)
    } else {
        cmd.to_string()
    }
}

fn deploy_jail(config: &config::Config, host: &str, spinner: &ProgressBar) -> Result<()> {
    // 1. Determine Base Version
    let base_version = if let Some(j) = &config.jail {
        if let Some(v) = &j.base_version {
            v.clone()
        } else {
            let os_release = remote::get_os_release(host)?;
            // Strip patch level (e.g., 14.1-RELEASE-p6 -> 14.1-RELEASE)
            os_release.split("-p").next().unwrap_or(&os_release).to_string()
        }
    } else {
        let os_release = remote::get_os_release(host)?;
        os_release.split("-p").next().unwrap_or(&os_release).to_string()
    };
    
    let subnet = config.jail.as_ref().and_then(|j| j.ip_range.as_deref()).unwrap_or(DEFAULT_IP_RANGE);

    spinner.set_message(format!("[{}] Ensuring base system {}...", host, base_version));
    jail::ensure_base(host, &base_version, config.doas)?;

    // 2. Ensure Image (Base + Packages + Mise)
    spinner.set_message(format!("[{}] Checking image...", host));
    let image_path = image::ensure_image(config, host, &base_version, spinner)?;
    
    // 3. Create Jail from Image
    spinner.set_message(format!("[{}] Creating new jail from image...", host));
    let jail_info = jail::create(host, &config.service, &base_version, subnet, Some(&image_path), &config.data_directories, config.doas)?;
    spinner.set_message(format!("[{}] Jail created: {} ({})", host, jail_info.name, jail_info.ip));

    let cmd_prefix = if config.doas { "doas " } else { "" };

    // 4. Start Jail (Phase 1: Inherit IP for Before Start hooks like bundle install)
    spinner.set_message(format!("[{}] Starting jail (build phase)...", host));
    let build_start_cmd = format!(
        "{}jail -c name={} path={} host.hostname={} ip4=inherit allow.raw_sockets=1 persist",
        cmd_prefix, jail_info.name, jail_info.path, jail_info.name
    );
    remote::run(host, &build_start_cmd)?;

    // 4.5 Ensure Data Directory Permissions (Must be after start for jexec)
    if let Some(user) = &config.user {
        for entry in &config.data_directories {
            let (_, jail_path) = entry.get_paths();
            if !jail_path.is_empty() {
                remote::run(host, &format!("{}jexec {} chown -R {} {}", cmd_prefix, jail_info.name, user, jail_path))?;
            }
        }
    }

    // 5. Sync App
    spinner.set_message(format!("[{}] Syncing app to jail...", host));
    let app_dir = JAIL_APP_DIR;
    let host_app_dir = format!("{}{}", jail_info.path, JAIL_APP_DIR);
    
    remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, host_app_dir))?;
    
    let mut excludes = Vec::new();
    for entry in &config.data_directories {
        let (_, jail_path) = entry.get_paths();
        // Check if jail_path is inside app_dir (e.g. /app/storage)
        if jail_path.starts_with(app_dir) {
             let rel = jail_path.strip_prefix(app_dir).unwrap().trim_start_matches('/');
             if !rel.is_empty() {
                 excludes.push(format!("/{}", rel));
             }
        }
    }

    remote::sync(host, ".", &host_app_dir, &excludes, config.doas)?;
    
    if let Some(user) = &config.user {
        remote::run(host, &format!("{}jexec {} chown -R {} {}", cmd_prefix, jail_info.name, user, app_dir))?;
    }

    // 6. Config & Env
    let mut env_content = String::new();
    for map in &config.env.clear {
        for (k, v) in map { env_content.push_str(&format!("export {}='{}'\n", k, v.replace("'", "'\\''"))); }
    }
    for k in &config.env.secret {
         let v = std::env::var(k)?;
         env_content.push_str(&format!("export {}='{}'\n", k, v.replace("'", "'\\''")));
    }
    if !config.mise.is_empty() { env_content.push_str("\neval \"$(mise activate bash)\"\n"); }

    let env_path = format!("{}{}", jail_info.path, JAIL_ENV_FILE);
    remote::write_file(host, &env_content, &env_path, config.doas)?;

    // 7. Before Start (Inherit IP)
    // Trust mise config first (as user)
    if let Some(user) = &config.user {
        let trust_cmd = format!("{}jexec {} su - {} -c 'mise trust {}'", cmd_prefix, jail_info.name, user, app_dir);
        remote::run(host, &trust_cmd).ok(); 
    } else {
        let trust_cmd = format!("{}jexec {} bash -c 'mise trust {}'", cmd_prefix, jail_info.name, app_dir);
        remote::run(host, &trust_cmd).ok();
    }

    for cmd in &config.before_start {
        spinner.set_message(format!("[{}] Jail: Running {}...", host, cmd));
        let full_cmd = format!("bash -c 'source /etc/bsdeploy.env && cd {} && {}'", app_dir, cmd);
        let exec_cmd = if let Some(user) = &config.user {
             format!("{}jexec {} su - {} -c \"{}\"", cmd_prefix, jail_info.name, user, full_cmd.replace("\"", "\\\""))
        } else {
             format!("{}jexec {} {}", cmd_prefix, jail_info.name, full_cmd)
        };
        remote::run(host, &exec_cmd)?;
    }

    // 8. Restart Jail (Private IP)
    spinner.set_message(format!("[{}] Restarting jail with isolated networking...", host));
    remote::run(host, &format!("{}jail -r {}", cmd_prefix, jail_info.name))?;
    
    let run_start_cmd = format!(
        "{}jail -c name={} path={} host.hostname={} ip4.addr={} allow.raw_sockets=1 persist",
        cmd_prefix, jail_info.name, jail_info.path, jail_info.name, jail_info.ip
    );
    remote::run(host, &run_start_cmd)?;

    // 8.5 Ensure Service Dirs in Jail
    if let Some(user) = &config.user {
         let jail_run_dir = format!("{}{}/{}", jail_info.path, RUN_DIR, config.service);
         let jail_log_dir = format!("{}{}/{}", jail_info.path, LOG_DIR, config.service);
         
         remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, jail_run_dir))?;
         remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, jail_log_dir))?;
         
         remote::run(host, &format!("{}chown {}:{} {}", cmd_prefix, user, user, jail_run_dir))?;
         remote::run(host, &format!("{}chown {}:{} {}", cmd_prefix, user, user, jail_log_dir))?;
    }
    // /var/run and /var/log usually exist by default

    // 9. Start Service
    for cmd in &config.start {
        spinner.set_message(format!("[{}] Jail: Starting service...", host));

        let (pid_file, log_file) = if config.user.is_some() {
             (
                format!("{}/{}/service.pid", RUN_DIR, config.service),
                format!("{}/{}/service.log", LOG_DIR, config.service)
             )
        } else {
             ("/var/run/service.pid".to_string(), "/var/log/service.log".to_string())
        };

        let mut daemon_cmd = format!("daemon -f -p {} -o {}", pid_file, log_file);
        if let Some(u) = &config.user { daemon_cmd.push_str(&format!(" -u {}", u)); }
        
        let full_cmd = format!("{} bash -c 'source /etc/bsdeploy.env && cd {} && {}'", daemon_cmd, app_dir, cmd);
        
        remote::run(host, &format!("{}jexec {} {}", cmd_prefix, jail_info.name, full_cmd))?;
    }

        // 10. Update Proxy

        if let Some(proxy) = &config.proxy {

             spinner.set_message(format!("[{}] Switching traffic to {}...", host, jail_info.ip));

             let hostname = if proxy.tls {

                proxy.hostname.clone()

             } else {

                format!("http://{}", proxy.hostname)

             };

             let proxy_conf_content = format!(

                "{} {{\n    reverse_proxy {}:{}\n}}\n", 

                hostname, jail_info.ip, proxy.port

            );

            let caddy_conf_path = format!("{}/{}.caddy", CADDY_CONF_DIR, config.service);

            remote::write_file(host, &proxy_conf_content, &caddy_conf_path, config.doas)?;

            remote::run(host, &format!("{}service caddy reload", cmd_prefix))?;

        }

    // 11. Stop processes in existing jails
    spinner.set_message(format!("[{}] Stopping processes in old jails...", host));

    let ls_cmd = format!("ls {}/ | grep '^{}-' || true", JAILS_DIR, config.service);
    if let Ok(ls_out) = remote::run_with_output(host, &ls_cmd) {
        let existing_jails: Vec<String> = ls_out.lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != &jail_info.name)
            .collect();

        for jname in existing_jails {
            spinner.set_message(format!("[{}] Stopping service in jail {}...", host, jname));

            let pid_file = if config.user.is_some() {
                format!("{}/{}/service.pid", RUN_DIR, config.service)
            } else {
                "/var/run/service.pid".to_string()
            };

            // Try to stop the process gracefully, then forcefully if needed
            let stop_cmd = format!(
                "if [ -f {0} ]; then \
                    pkill -F {0}; \
                    count=0; \
                    while [ -f {0} ] && pkill -0 -F {0} >/dev/null 2>&1; do \
                        sleep 0.5; \
                        count=$((count+1)); \
                        if [ $count -ge 20 ]; then \
                            pkill -9 -F {0}; \
                            break; \
                        fi; \
                    done; \
                fi",
                pid_file
            );

            let exec_cmd = format!("{}jexec {} sh -c '{}'", cmd_prefix, jname, stop_cmd);
            remote::run(host, &exec_cmd).ok();
        }
    }

    // 12. Prune Old Jails
    spinner.set_message(format!("[{}] Pruning old jails...", host));

    // 1. Get all jail directories from filesystem
    let ls_cmd = format!("ls {}/ | grep '^{}-' || true", JAILS_DIR, config.service);
    if let Ok(ls_out) = remote::run_with_output(host, &ls_cmd) {
        let mut jails: Vec<String> = ls_out.lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        jails.sort(); // Sort by timestamp

        // Keep only the last N most recent jails
        if jails.len() > JAILS_TO_KEEP {
            let to_remove_count = jails.len() - JAILS_TO_KEEP;
            let to_remove = &jails[0..to_remove_count];

            for jname in to_remove {
                if jname == &jail_info.name { continue; }
                spinner.set_message(format!("[{}] Removing stale/old jail directory {}...", host, jname));

                let jpath = format!("{}/{}", JAILS_DIR, jname);

                // 1. Try to stop jail if it's running
                remote::run(host, &format!("{}jail -r {} 2>/dev/null", cmd_prefix, jname)).ok();

                // 2. Cleanup IP alias (if we can find it)
                // We check 'ifconfig lo1' for any IP that isn't the current one or other active ones
                // This is slightly complex, let's at least try to get the IP from jls if it WAS running
                let info_cmd = format!("jls -j {} ip4.addr 2>/dev/null || echo '-'", jname);
                if let Ok(jip) = remote::run_with_output(host, &info_cmd) {
                    let jip = jip.trim();
                    if jip != "-" && !jip.is_empty() {
                        remote::run(host, &format!("{}ifconfig lo1 inet {} -alias 2>/dev/null", cmd_prefix, jip)).ok();
                    }
                }

                // 3. Unmount all under jpath
                let mount_check = format!("mount | grep '{}' | awk '{{print $3}}'", jpath);
                if let Ok(mounts) = remote::run_with_output(host, &mount_check) {
                    for mnt in mounts.lines().rev() {
                        if !mnt.trim().is_empty() {
                            remote::run(host, &format!("{}umount -f {}", cmd_prefix, mnt.trim())).ok();
                        }
                    }
                }

                // 4. Remove dir or ZFS dataset
                if let Ok(Some(dataset)) = remote::get_zfs_dataset(host, &jpath) {
                    remote::run(host, &format!("{}zfs destroy -r {}", cmd_prefix, dataset)).ok();
                }
                
                remote::run(host, &format!("{}chflags -R noschg {}", cmd_prefix, jpath)).ok();
                remote::run(host, &format!("{}rm -rf {}", cmd_prefix, jpath)).ok();
            }
        }
    }
    
    Ok(())
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => {
            // Check if config file already exists
            if cli.config.exists() {
                ui::print_error(&format!("Configuration file already exists at: {}", cli.config.display()));
                ui::print_step("Use a different path with --config or remove the existing file");
                std::process::exit(1);
            }

            // Create parent directory if needed
            if let Some(parent) = cli.config.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
            }

            // Template configuration with comments
            let template = r#"# bsdeploy configuration file
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

            std::fs::write(&cli.config, template)
                .with_context(|| format!("Failed to write config file: {}", cli.config.display()))?;

            ui::print_success(&format!("Created configuration file at: {}", cli.config.display()));
            ui::print_step("Edit the file to customize your deployment settings");

            return Ok(());
        }
        _ => {}
    }

    // Load config for all other commands
    let config = match config::Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            ui::print_error(&format!("Error loading configuration: {}", e));
            std::process::exit(1);
        }
    };

    ui::print_step(&format!("Loaded configuration for service: {}", config.service));

    match cli.command {
        Commands::Init => unreachable!(), // Already handled above
        Commands::Setup => {
            ui::print_step(&format!("Running setup for {} hosts", config.hosts.len()));

            // Build env content
            let mut env_content = String::new();
            for map in &config.env.clear {
                for (k, v) in map {
                     env_content.push_str(&format!("export {}='{}'\n", k, v.replace("'", "'\\''")));
                }
            }
            for k in &config.env.secret {
                let v = std::env::var(k).with_context(|| format!("Missing local secret environment variable: {}", k))?;
                env_content.push_str(&format!("export {}='{}'\n", k, v.replace("'", "'\\''")));
            }

            // Add mise activation if used
            if !config.mise.is_empty() {
                env_content.push_str("\neval \"$(mise activate bash)\"\n");
            }

            for host in &config.hosts {
                let spinner = ui::create_spinner(&format!("Setting up {}", host));
                
                // 1. Update pkg
                spinner.set_message(format!("[{}] Updating pkg repositories...", host));
                remote::run(host, &maybe_doas("pkg update", config.doas))?;
                
                // 2. Install default packages (including bash)
                spinner.set_message(format!("[{}] Installing default packages...", host));
                remote::run(host, &maybe_doas("pkg install -y caddy rsync git bash", config.doas))?;

                // 2.5 Create User if needed (Moved before Mise setup)
                if let Some(user) = &config.user {
                    spinner.set_message(format!("[{}] Ensure user {} exists...", host, user));
                    // Check if user exists, if not create
                    let check_user = remote::run(host, &format!("id {}", user));
                    if check_user.is_err() {
                        remote::run(host, &maybe_doas(&format!("pw useradd -n {} -m -s /usr/local/bin/bash", user), config.doas))?;
                    }
                }

                // 3. Install user packages
                if !config.packages.is_empty() {
                    spinner.set_message(format!("[{}] Installing user packages...", host));
                    let pkgs = config.packages.join(" ");
                    remote::run(host, &maybe_doas(&format!("pkg install -y {}", pkgs), config.doas))?;
                }

                // 3.5 Setup ZFS if available
                if let Ok(Some(root_dataset)) = remote::get_zfs_dataset(host, "/") {
                    spinner.set_message(format!("[{}] ZFS detected (dataset: {}). Setting up datasets...", host, root_dataset));
                    
                    // We want to create zroot/bsdeploy, zroot/bsdeploy/base, etc.
                    // But we need to know the pool name or parent.
                    let pool = root_dataset.split('/').next().unwrap_or(DEFAULT_ZFS_POOL);
                    let bsdeploy_root_dataset = format!("{}/bsdeploy", pool);
                    
                    let datasets = vec![
                        bsdeploy_root_dataset.clone(),
                        format!("{}/base", bsdeploy_root_dataset),
                        format!("{}/images", bsdeploy_root_dataset),
                        format!("{}/jails", bsdeploy_root_dataset),
                    ];
                    
                    for ds in datasets {
                        let check_ds = remote::run(host, &format!("zfs list -H -o name {}", ds));
                        if check_ds.is_err() {
                            // Determine mountpoint
                            let mountpoint = if ds == bsdeploy_root_dataset {
                                BSDEPLOY_BASE.to_string()
                            } else {
                                format!("{}/{}", BSDEPLOY_BASE, ds.split('/').last().unwrap())
                            };
                            
                            remote::run(host, &maybe_doas(&format!("zfs create -o mountpoint={} {}", mountpoint, ds), config.doas)).ok();
                        }
                    }
                }

                // 4. Setup directories
                spinner.set_message(format!("[{}] Creating directories...", host));
                let app_dir = format!("{}/{}/app", APP_DATA_DIR, config.service);
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", app_dir), config.doas))?;

                let config_dir = format!("{}/{}", CONFIG_DIR, config.service);
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", config_dir), config.doas))?;

                for dir in &config.data_directories {
                    let (host_path, _) = dir.get_paths();
                    remote::run(host, &maybe_doas(&format!("mkdir -p {}", host_path), config.doas))?;
                }
                
                // Create user-specific run/log dirs if needed
                if let Some(user) = &config.user {
                    let run_dir = format!("{}/{}", RUN_DIR, config.service);
                    let log_dir = format!("{}/{}", LOG_DIR, config.service);
                    remote::run(host, &maybe_doas(&format!("mkdir -p {}", run_dir), config.doas))?;
                    remote::run(host, &maybe_doas(&format!("mkdir -p {}", log_dir), config.doas))?;
                    remote::run(host, &maybe_doas(&format!("chown {}:{} {}", user, user, run_dir), config.doas))?;
                    remote::run(host, &maybe_doas(&format!("chown {}:{} {}", user, user, log_dir), config.doas))?;

                    // Also chown app_dir and data directories
                    remote::run(host, &maybe_doas(&format!("chown -R {}:{} {}", user, user, format!("{}/{}", APP_DATA_DIR, config.service)), config.doas))?;
                    for dir in &config.data_directories {
                        let (host_path, _) = dir.get_paths();
                        remote::run(host, &maybe_doas(&format!("chown -R {}:{} {}", user, user, host_path), config.doas))?;
                    }
                }

                // 6. Write env file
                spinner.set_message(format!("[{}] Configuring environment...", host));
                let env_path = format!("{}/env", config_dir);
                remote::write_file(host, &env_content, &env_path, config.doas)?;

                // 7. Caddy Setup
                spinner.set_message(format!("[{}] Configuring Caddy...", host));
                // Enable caddy
                remote::run(host, &maybe_doas("sysrc caddy_enable=YES", config.doas))?;

                // Ensure conf.d exists
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", CADDY_CONF_DIR), config.doas))?;

                // Check/Create main Caddyfile
                let check_caddyfile = remote::run(host, &format!("test -f {}", CADDYFILE_PATH));
                
                if check_caddyfile.is_err() {
                     // Create default Caddyfile importing conf.d
                     let default_caddy = "import conf.d/*.caddy\n";
                     remote::write_file(host, default_caddy, CADDYFILE_PATH, config.doas)?;
                } else {
                    // Check if import exists
                    let check_import = remote::run(host, &format!("grep -q 'import conf.d/\\*.caddy' {}", CADDYFILE_PATH));
                    if check_import.is_err() {
                        // Append import
                        ui::print_step(&format!("Appending import to {}", CADDYFILE_PATH));
                        let append_cmd = if config.doas {
                            format!("echo 'import conf.d/*.caddy' | doas tee -a {} > /dev/null", CADDYFILE_PATH)
                        } else {
                            format!("echo 'import conf.d/*.caddy' | tee -a {} > /dev/null", CADDYFILE_PATH)
                        };
                        remote::run(host, &append_cmd)?;
                    }
                }

                // Proxy Config
                if let Some(proxy) = &config.proxy {
                    let hostname = if proxy.tls {
                        proxy.hostname.clone()
                    } else {
                        format!("http://{}", proxy.hostname)
                    };
                    let proxy_conf_content = format!(
                        "{} {{\n    reverse_proxy :{}\n}}\n",
                        hostname, proxy.port
                    );
                    let proxy_conf_path = format!("{}/{}.caddy", CADDY_CONF_DIR, config.service);
                    remote::write_file(host, &proxy_conf_content, &proxy_conf_path, config.doas)?;
                }

                // Restart caddy
                remote::run(host, &maybe_doas("service caddy enable", config.doas))?;
                remote::run(host, &maybe_doas("service caddy restart", config.doas))?;
                
                spinner.finish_with_message(format!("Setup complete for {}", host));
                ui::print_success(&format!("{} setup successfully", host));
            }
        }
        Commands::Deploy => {
            ui::print_step(&format!("Running deploy for {} hosts", config.hosts.len()));
            
            for host in &config.hosts {
                let spinner = ui::create_spinner(&format!("Deploying to {}", host));

                deploy_jail(&config, host, &spinner)?;

                spinner.finish_with_message(format!("Deploy complete for {}", host));
                ui::print_success(&format!("{} deployed successfully", host));
            }
        }
        Commands::Destroy => {
            ui::print_step(&format!("Destroying all resources for service {} on {} hosts", config.service, config.hosts.len()));
            
            for host in &config.hosts {
                let spinner = ui::create_spinner(&format!("Destroying resources on {}", host));
                let cmd_prefix = if config.doas { "doas " } else { "" };

                // 1. Find and Remove Jails & IP Aliases
                spinner.set_message(format!("[{}] Removing jails and networking...", host));

                // Get list of jail directories from filesystem
                let ls_cmd = format!("ls {}/ | grep '^{}-' || true", JAILS_DIR, config.service);
                if let Ok(ls_out) = remote::run_with_output(host, &ls_cmd) {
                    for jname in ls_out.lines().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                        spinner.set_message(format!("[{}] Cleaning up jail {}...", host, jname));

                        let jpath = format!("{}/{}", JAILS_DIR, jname);

                        // Try to get IP before stopping
                        let info_cmd = format!("jls -j {} ip4.addr 2>/dev/null || echo '-'", jname);
                        if let Ok(jip) = remote::run_with_output(host, &info_cmd) {
                            let jip = jip.trim();
                            
                            // Stop jail
                            remote::run(host, &format!("{}jail -r {} 2>/dev/null", cmd_prefix, jname)).ok();

                            // Remove IP alias
                            if jip != "-" && !jip.is_empty() {
                                remote::run(host, &format!("{}ifconfig lo1 inet {} -alias 2>/dev/null", cmd_prefix, jip)).ok();
                            }
                        }

                        // Unmount everything under jpath
                        let mount_check = format!("mount | grep '{}' | awk '{{print $3}}'", jpath);
                        if let Ok(mounts) = remote::run_with_output(host, &mount_check) {
                            for mnt in mounts.lines().rev() {
                                if !mnt.trim().is_empty() {
                                    remote::run(host, &format!("{}umount -f {}", cmd_prefix, mnt.trim())).ok();
                                }
                            }
                        }

                        // Remove jail dir or ZFS dataset
                        if let Ok(Some(dataset)) = remote::get_zfs_dataset(host, &jpath) {
                            remote::run(host, &format!("{}zfs destroy -r {}", cmd_prefix, dataset)).ok();
                        }
                        
                        remote::run(host, &format!("{}chflags -R noschg {}", cmd_prefix, jpath)).ok();
                        remote::run(host, &format!("{}rm -rf {}", cmd_prefix, jpath)).ok();
                    }
                }

                // 2. Remove Caddy Proxy Config
                spinner.set_message(format!("[{}] Removing proxy configuration...", host));
                let caddy_conf = format!("{}/{}.caddy", CADDY_CONF_DIR, config.service);
                remote::run(host, &format!("{}rm -f {}", cmd_prefix, caddy_conf)).ok();
                remote::run(host, &format!("{}service caddy reload", cmd_prefix)).ok();

                // 3. Note: We keep images because they might be shared by other apps with same hashes.
                // If the user wants to clear ALL images, they can use setup or manual cleanup.

                spinner.finish_with_message(format!("Resources destroyed for {}", host));
                ui::print_success(&format!("{} resources cleaned up", host));
            }
        }
    }

    Ok(())
}