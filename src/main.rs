mod config;
mod remote;
mod ui;

use clap::{Parser, Subcommand};
use indicatif::ProgressBar;
use std::path::PathBuf;
use anyhow::{Context, Result};

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
    /// Setup the remote hosts
    Setup,
    /// Deploy the application
    Deploy,
    /// Restart the application without deploying code
    Restart,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    let config = match config::Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            ui::print_error(&format!("Error loading configuration: {}", e));
            std::process::exit(1);
        }
    };

    ui::print_step(&format!("Loaded configuration for service: {}", config.service));

    let maybe_doas = |cmd: &str| -> String {
        if config.doas {
            format!("doas {}", cmd)
        } else {
            cmd.to_string()
        }
    };

    let restart_service = |host: &str, spinner: &ProgressBar| -> Result<()> {
        let app_dir = format!("/var/db/bsdeploy/{}/app", config.service);
        let env_file = format!("/usr/local/etc/bsdeploy/{}/env", config.service);
        
        // Use user-specific run/log dirs if user is configured
        let (pid_file, log_file) = if config.user.is_some() {
             (
                format!("/var/run/bsdeploy/{}/service.pid", config.service),
                format!("/var/log/bsdeploy/{}/service.log", config.service)
             )
        } else {
             (
                format!("/var/run/bsdeploy_{}.pid", config.service),
                format!("/var/log/bsdeploy_{}.log", config.service)
             )
        };
        
        spinner.set_message(format!("[{}] Stopping service...", host));
        let _ = remote::run(host, &maybe_doas(&format!("pkill -F {}", pid_file)));

        for cmd in &config.start {
            spinner.set_message(format!("[{}] Starting: {}...", host, cmd));
            
            // Construct daemon command
            let mut daemon_cmd = format!("daemon -f -p {} -o {}", pid_file, log_file);
            if let Some(u) = &config.user {
                daemon_cmd.push_str(&format!(" -u {}", u));
            }
            
            // Use bash instead of sh for better compatibility (e.g. mise)
            let full_cmd = format!(
                "{} bash -c 'source {} && cd {} && {}' > /dev/null 2>&1 < /dev/null",
                daemon_cmd, env_file, app_dir, cmd
            );
            remote::run(host, &maybe_doas(&full_cmd))?;
        }
        Ok(())
    };

    match cli.command {
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
                remote::run(host, &maybe_doas("pkg update"))?;
                
                // 2. Install default packages (including bash)
                spinner.set_message(format!("[{}] Installing default packages...", host));
                remote::run(host, &maybe_doas("pkg install -y caddy rsync git bash"))?;

                // 2.5 Create User if needed (Moved before Mise setup)
                if let Some(user) = &config.user {
                    spinner.set_message(format!("[{}] Ensure user {} exists...", host, user));
                    // Check if user exists, if not create
                    let check_user = remote::run(host, &format!("id {}", user));
                    if check_user.is_err() {
                        remote::run(host, &maybe_doas(&format!("pw useradd -n {} -m -s /usr/local/bin/bash", user)))?;
                    }
                }

                // 3. Install user packages
                if !config.packages.is_empty() {
                    spinner.set_message(format!("[{}] Installing user packages...", host));
                    let pkgs = config.packages.join(" ");
                    remote::run(host, &maybe_doas(&format!("pkg install -y {}", pkgs)))?;
                }

                // 4. Install Mise and Tools
                if !config.mise.is_empty() {
                    spinner.set_message(format!("[{}] Installing Mise and build deps...", host));
                    // Install build deps: gmake, gcc, python3, pkgconf
                    remote::run(host, &maybe_doas("pkg install -y mise gmake gcc python3 pkgconf"))?;

                    for (tool, version) in &config.mise {
                        spinner.set_message(format!("[{}] Installing {}@{}...", host, tool, version));
                        // Set environment variables to help compilation on FreeBSD
                        let cmd = format!("export CC=gcc CXX=g++ MAKE=gmake && mise use --global {}@{}", tool, version);
                        
                        // Run as user if configured, otherwise as doas/root
                        let exec_cmd = if let Some(user) = &config.user {
                             // Use su - to run as user with clean environment (loading .profile etc if needed, though mise use global writes to config)
                             // Actually, 'mise use --global' writes to ~/.config/mise/config.toml of the USER running it.
                             format!("su - {} -c \"{}\"", user, cmd.replace("\"", "\\\""))
                        } else {
                             format!("bash -c '{}'", cmd)
                        };

                        remote::run(host, &maybe_doas(&exec_cmd))?;
                    }
                }

                // 5. Setup directories
                spinner.set_message(format!("[{}] Creating directories...", host));
                let app_dir = format!("/var/db/bsdeploy/{}/app", config.service);
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", app_dir)))?;

                let config_dir = format!("/usr/local/etc/bsdeploy/{}", config.service);
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", config_dir)))?;

                for dir in &config.data_directories {
                    remote::run(host, &maybe_doas(&format!("mkdir -p {}", dir)))?;
                }
                
                // Create user-specific run/log dirs if needed
                if let Some(user) = &config.user {
                    let run_dir = format!("/var/run/bsdeploy/{}", config.service);
                    let log_dir = format!("/var/log/bsdeploy/{}", config.service);
                    remote::run(host, &maybe_doas(&format!("mkdir -p {}", run_dir)))?;
                    remote::run(host, &maybe_doas(&format!("mkdir -p {}", log_dir)))?;
                    remote::run(host, &maybe_doas(&format!("chown {}:{} {}", user, user, run_dir)))?;
                    remote::run(host, &maybe_doas(&format!("chown {}:{} {}", user, user, log_dir)))?;
                    
                    // Also chown app_dir and data directories
                    remote::run(host, &maybe_doas(&format!("chown -R {}:{} {}", user, user, format!("/var/db/bsdeploy/{}", config.service))))?;
                    for dir in &config.data_directories {
                        remote::run(host, &maybe_doas(&format!("chown -R {}:{} {}", user, user, dir)))?;
                    }
                }

                // 6. Write env file
                spinner.set_message(format!("[{}] Configuring environment...", host));
                let env_path = format!("{}/env", config_dir);
                remote::write_file(host, &env_content, &env_path, config.doas)?;

                // 7. Caddy Setup
                spinner.set_message(format!("[{}] Configuring Caddy...", host));
                // Enable caddy
                remote::run(host, &maybe_doas("sysrc caddy_enable=YES"))?;
                
                // Ensure conf.d exists
                let caddy_conf_d = "/usr/local/etc/caddy/conf.d";
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", caddy_conf_d)))?;

                // Check/Create main Caddyfile
                let caddyfile = "/usr/local/etc/caddy/Caddyfile";
                let check_caddyfile = remote::run(host, &format!("test -f {}", caddyfile));
                
                if check_caddyfile.is_err() {
                     // Create default Caddyfile importing conf.d
                     let default_caddy = "import conf.d/*.caddy\n";
                     remote::write_file(host, default_caddy, caddyfile, config.doas)?;
                } else {
                    // Check if import exists
                    let check_import = remote::run(host, &format!("grep -q 'import conf.d/\\*.caddy' {}", caddyfile));
                    if check_import.is_err() {
                        // Append import
                        ui::print_step(&format!("Appending import to {}", caddyfile));
                        let append_cmd = if config.doas {
                            format!("echo 'import conf.d/*.caddy' | doas tee -a {} > /dev/null", caddyfile)
                        } else {
                            format!("echo 'import conf.d/*.caddy' | tee -a {} > /dev/null", caddyfile)
                        };
                        remote::run(host, &append_cmd)?;
                    }
                }

                // Proxy Config
                if let Some(proxy) = &config.proxy {
                    let proxy_conf_content = format!(
                        "{} {{\n    reverse_proxy :{}
}}\n", 
                        proxy.hostname, proxy.port
                    );
                    let proxy_conf_path = format!("{}/{}.caddy", caddy_conf_d, config.service);
                    remote::write_file(host, &proxy_conf_content, &proxy_conf_path, config.doas)?;
                }

                // Restart caddy
                remote::run(host, &maybe_doas("service caddy enable"))?;
                remote::run(host, &maybe_doas("service caddy restart"))?;
                
                spinner.finish_with_message(format!("Setup complete for {}", host));
                ui::print_success(&format!("{} setup successfully", host));
            }
        }
        Commands::Deploy => {
            ui::print_step(&format!("Running deploy for {} hosts", config.hosts.len()));
            
            for host in &config.hosts {
                let spinner = ui::create_spinner(&format!("Deploying to {}", host));
                let app_dir = format!("/var/db/bsdeploy/{}/app", config.service);
                let env_file = format!("/usr/local/etc/bsdeploy/{}/env", config.service);
                
                // 1. Ensure app directory exists
                spinner.set_message(format!("[{}] Ensuring app directory...", host));
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", app_dir)))?;

                // 2. Sync files
                spinner.set_message(format!("[{}] Syncing files...", host));
                remote::sync(host, ".", &app_dir, config.doas)?;
                
                // Fix permissions after sync if user is set
                if let Some(user) = &config.user {
                     spinner.set_message(format!("[{}] Setting permissions...", host));
                     remote::run(host, &maybe_doas(&format!("chown -R {}:{} {}", user, user, app_dir)))?;
                }

                // 3. Before Start
                for cmd in &config.before_start {
                    spinner.set_message(format!("[{}] Running before_start: {}...", host, cmd));
                    // Use bash instead of sh
                    let full_cmd = format!(
                        "bash -c 'source {} && cd {} && {}'",
                        env_file, app_dir, cmd
                    );
                    
                    // Run before_start as user if set? Usually yes.
                    let exec_cmd = if let Some(user) = &config.user {
                        format!("su -m {} -c \"{}\"", user, full_cmd.replace("\"", "\\\""))
                    } else {
                        full_cmd
                    };

                    remote::run(host, &maybe_doas(&exec_cmd))?;
                }

                // 4. Start (via helper)
                restart_service(host, &spinner)?;
                
                spinner.finish_with_message(format!("Deploy complete for {}", host));
                ui::print_success(&format!("{} deployed successfully", host));
            }
        }
        Commands::Restart => {
            ui::print_step(&format!("Running restart for {} hosts", config.hosts.len()));
            
            for host in &config.hosts {
                let spinner = ui::create_spinner(&format!("Restarting {}", host));
                
                restart_service(host, &spinner)?;
                
                spinner.finish_with_message(format!("Restart complete for {}", host));
                ui::print_success(&format!("{} restarted successfully", host));
            }
        }
    }

    Ok(())
}
