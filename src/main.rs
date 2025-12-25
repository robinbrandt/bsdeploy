mod config;
mod remote;
mod ui;
mod jail;
mod image;

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
    /// Setup the remote hosts
    Setup,
    /// Deploy the application
    Deploy,
    /// Restart the application without deploying code
    Restart,
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

fn deploy_host(config: &config::Config, host: &str, spinner: &ProgressBar, restart_service: &dyn Fn(&str, &ProgressBar) -> Result<()>) -> Result<()> {
    let app_dir = format!("/var/db/bsdeploy/{}/app", config.service);
    let env_file = format!("/usr/local/etc/bsdeploy/{}/env", config.service);
    
    // 1. Ensure app directory exists
    spinner.set_message(format!("[{}] Ensuring app directory...", host));
    remote::run(host, &maybe_doas(&format!("mkdir -p {}", app_dir), config.doas))?;

    // 2. Sync files
    spinner.set_message(format!("[{}] Syncing files...", host));
    
    let mut excludes = Vec::new();
    // No data_directories logic for host deployment currently implemented in mounting, 
    // but if user uses them, we should respect them if they overlap.
    // For now, pass empty excludes or check config? 
    // The data_directories config exists.
    for entry in &config.data_directories {
        let (host_path, _) = entry.get_paths(); // host_path is what matters on host
        if host_path.starts_with(&app_dir) {
             let rel = host_path.strip_prefix(&app_dir).unwrap().trim_start_matches('/');
             if !rel.is_empty() {
                 excludes.push(rel.to_string());
             }
        }
    }
    
    remote::sync(host, ".", &app_dir, &excludes, config.doas)?;
    
    // Fix permissions after sync if user is set
    if let Some(user) = &config.user {
            spinner.set_message(format!("[{}] Setting permissions...", host));
            remote::run(host, &maybe_doas(&format!("chown -R {}:{} {}", user, user, app_dir), config.doas))?;
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
            format!("su - {} -c \"{}\"", user, full_cmd.replace("\"", "\\\""))
        } else {
            full_cmd
        };

        remote::run(host, &maybe_doas(&exec_cmd, config.doas))?;
    }

    // 4. Start (via helper)
    restart_service(host, spinner)?;

    Ok(())
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
    
    let subnet = config.jail.as_ref().and_then(|j| j.ip_range.as_deref()).unwrap_or("10.0.0.0/24");

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
    let app_dir = "/app"; 
    let host_app_dir = format!("{}/app", jail_info.path);
    
    remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, host_app_dir))?;
    
    let mut excludes = Vec::new();
    for entry in &config.data_directories {
        let (_, jail_path) = entry.get_paths();
        // Check if jail_path is inside app_dir (e.g. /app/storage)
        if jail_path.starts_with(app_dir) {
             let rel = jail_path.strip_prefix(app_dir).unwrap().trim_start_matches('/');
             if !rel.is_empty() {
                 excludes.push(rel.to_string());
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
    
    let env_path = format!("{}/etc/bsdeploy.env", jail_info.path);
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
         let jail_run_dir = format!("{}/var/run/bsdeploy/{}", jail_info.path, config.service);
         let jail_log_dir = format!("{}/var/log/bsdeploy/{}", jail_info.path, config.service);
         
         remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, jail_run_dir))?;
         remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, jail_log_dir))?;
         
         remote::run(host, &format!("{}chown {}:{} {}", cmd_prefix, user, user, jail_run_dir))?;
         remote::run(host, &format!("{}chown {}:{} {}", cmd_prefix, user, user, jail_log_dir))?;
    }
    // /var/run and /var/log usually exist by default

    // 9. Start Service
    for cmd in &config.start {
        spinner.set_message(format!("[{}] Jail: Starting service...", host));
        
        let (pid_file, log_file) = if let Some(_) = &config.user {
             (
                format!("/var/run/bsdeploy/{}/service.pid", config.service),
                format!("/var/log/bsdeploy/{}/service.log", config.service)
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

            let caddy_conf_path = format!("/usr/local/etc/caddy/conf.d/{}.caddy", config.service);

            remote::write_file(host, &proxy_conf_content, &caddy_conf_path, config.doas)?;

            remote::run(host, &format!("{}service caddy reload", cmd_prefix))?;

        }

    // 11. Prune Old Jails
    spinner.set_message(format!("[{}] Pruning old jails...", host));
    
    // 1. Get all jail directories from filesystem
    let ls_cmd = format!("ls /usr/local/bsdeploy/jails/ | grep '^{}-' || true", config.service);
    if let Ok(ls_out) = remote::run_with_output(host, &ls_cmd) {
        let mut jails: Vec<String> = ls_out.lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        jails.sort(); // Sort by timestamp
        
        // Keep only the last 3 most recent jails
        if jails.len() > 3 {
            let to_remove_count = jails.len() - 3;
            let to_remove = &jails[0..to_remove_count];
            
            for jname in to_remove {
                if jname == &jail_info.name { continue; }
                spinner.set_message(format!("[{}] Removing stale/old jail directory {}...", host, jname));
                
                let jpath = format!("/usr/local/bsdeploy/jails/{}", jname);

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

    let config = match config::Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            ui::print_error(&format!("Error loading configuration: {}", e));
            std::process::exit(1);
        }
    };

    ui::print_step(&format!("Loaded configuration for service: {}", config.service));

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
        // Try to kill and wait for process to exit
        let stop_cmd = format!(
            "if [ -f {0} ]; then 
                pkill -F {0}; 
                count=0; 
                while [ -f {0} ] && pkill -0 -F {0} >/dev/null 2>&1; do 
                    sleep 0.5; 
                    count=$((count+1)); 
                    if [ $count -ge 20 ]; then 
                        pkill -9 -F {0}; 
                        break; 
                    fi; 
                done; 
            fi",
            pid_file
        );
        // We use 'sh -c' explicitly to handle the shell logic
        let _ = remote::run(host, &maybe_doas(&format!("sh -c '{}'", stop_cmd), config.doas));

        for cmd in &config.start {
            spinner.set_message(format!("[{}] Starting: {}...", host, cmd));
            
            // Construct daemon command
            let mut daemon_cmd = format!("daemon -f -p {} -o {}", pid_file, log_file);
            if let Some(u) = &config.user {
                daemon_cmd.push_str(&format!(" -u {}", u));
            }
            
            // Use bash instead of sh for better compatibility (e.g. mise)
            // We do NOT redirect the outer daemon command to /dev/null so we can see startup errors.
            // daemon -f closes standard streams upon success.
            let full_cmd = format!(
                "{} bash -c 'source {} && cd {} && {}'",
                daemon_cmd, env_file, app_dir, cmd
            );
            remote::run(host, &maybe_doas(&full_cmd, config.doas))?;
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
                    let pool = root_dataset.split('/').next().unwrap_or("zroot");
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
                                "/usr/local/bsdeploy".to_string()
                            } else {
                                format!("/usr/local/bsdeploy/{}", ds.split('/').last().unwrap())
                            };
                            
                            remote::run(host, &maybe_doas(&format!("zfs create -o mountpoint={} {}", mountpoint, ds), config.doas)).ok();
                        }
                    }
                }

                // 4. Install Mise and Tools (only for Host strategy)
                // For Jail strategy, mise is installed inside jails during image building
                if !config.mise.is_empty() && config.strategy == config::Strategy::Host {
                    spinner.set_message(format!("[{}] Installing Mise and build deps...", host));
                    // Install build deps: gmake, gcc, python3, pkgconf
                    remote::run(host, &maybe_doas("pkg install -y mise gmake gcc python3 pkgconf", config.doas))?;

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

                        remote::run(host, &maybe_doas(&exec_cmd, config.doas))?;
                    }
                }

                // 5. Setup directories
                spinner.set_message(format!("[{}] Creating directories...", host));
                let app_dir = format!("/var/db/bsdeploy/{}/app", config.service);
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", app_dir), config.doas))?;

                let config_dir = format!("/usr/local/etc/bsdeploy/{}", config.service);
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", config_dir), config.doas))?;

                for dir in &config.data_directories {
                    let (host_path, _) = dir.get_paths();
                    remote::run(host, &maybe_doas(&format!("mkdir -p {}", host_path), config.doas))?;
                }
                
                // Create user-specific run/log dirs if needed
                if let Some(user) = &config.user {
                    let run_dir = format!("/var/run/bsdeploy/{}", config.service);
                    let log_dir = format!("/var/log/bsdeploy/{}", config.service);
                    remote::run(host, &maybe_doas(&format!("mkdir -p {}", run_dir), config.doas))?;
                    remote::run(host, &maybe_doas(&format!("mkdir -p {}", log_dir), config.doas))?;
                    remote::run(host, &maybe_doas(&format!("chown {}:{} {}", user, user, run_dir), config.doas))?;
                    remote::run(host, &maybe_doas(&format!("chown {}:{} {}", user, user, log_dir), config.doas))?;
                    
                    // Also chown app_dir and data directories
                    remote::run(host, &maybe_doas(&format!("chown -R {}:{} {}", user, user, format!("/var/db/bsdeploy/{}", config.service)), config.doas))?;
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
                let caddy_conf_d = "/usr/local/etc/caddy/conf.d";
                remote::run(host, &maybe_doas(&format!("mkdir -p {}", caddy_conf_d), config.doas))?;

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
                    let hostname = if proxy.tls {
                        proxy.hostname.clone()
                    } else {
                        format!("http://{}", proxy.hostname)
                    };
                    let proxy_conf_content = format!(
                        "{} {{\n    reverse_proxy :{}\n}}\n", 
                        hostname, proxy.port
                    );
                    let proxy_conf_path = format!("{}/{}.caddy", caddy_conf_d, config.service);
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
                
                match config.strategy {
                    config::Strategy::Host => {
                        deploy_host(&config, host, &spinner, &restart_service)?;
                    },
                    config::Strategy::Jail => {
                        deploy_jail(&config, host, &spinner)?;
                    }
                }

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
        Commands::Destroy => {
            ui::print_step(&format!("Destroying all resources for service {} on {} hosts", config.service, config.hosts.len()));
            
            for host in &config.hosts {
                let spinner = ui::create_spinner(&format!("Destroying resources on {}", host));
                let cmd_prefix = if config.doas { "doas " } else { "" };

                // 1. Find and Remove Jails & IP Aliases
                spinner.set_message(format!("[{}] Removing jails and networking...", host));
                
                // Get list of jail directories from filesystem
                let ls_cmd = format!("ls /usr/local/bsdeploy/jails/ | grep '^{}-' || true", config.service);
                if let Ok(ls_out) = remote::run_with_output(host, &ls_cmd) {
                    for jname in ls_out.lines().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                        spinner.set_message(format!("[{}] Cleaning up jail {}...", host, jname));
                        
                        let jpath = format!("/usr/local/bsdeploy/jails/{}", jname);

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
                let caddy_conf = format!("/usr/local/etc/caddy/conf.d/{}.caddy", config.service);
                remote::run(host, &format!("{}rm -f {}", cmd_prefix, caddy_conf)).ok();
                remote::run(host, &format!("{}service caddy reload", cmd_prefix)).ok();

                // 3. Remove Service Directories (Env, App, Run, Log)
                spinner.set_message(format!("[{}] Removing service directories...", host));
                let dirs = vec![
                    format!("/usr/local/etc/bsdeploy/{}", config.service),
                    format!("/var/db/bsdeploy/{}", config.service),
                    format!("/var/run/bsdeploy/{}", config.service),
                    format!("/var/log/bsdeploy/{}", config.service),
                ];
                for dir in dirs {
                    remote::run(host, &format!("{}rm -rf {}", cmd_prefix, dir)).ok();
                }

                // 4. Note: We keep images because they might be shared by other apps with same hashes.
                // If the user wants to clear ALL images, they can use setup or manual cleanup.

                spinner.finish_with_message(format!("Resources destroyed for {}", host));
                ui::print_success(&format!("{} resources cleaned up", host));
            }
        }
    }

    Ok(())
}