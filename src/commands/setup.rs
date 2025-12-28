use anyhow::{Context, Result};

use crate::config::Config;
use crate::constants::*;
use crate::{caddy, remote, shell, ui};

use super::maybe_doas;

pub fn run(config: &Config) -> Result<()> {
    ui::print_step(&format!("Running setup for {} hosts", config.hosts.len()));

    let env_content = build_env_content(config)?;

    for host in &config.hosts {
        let spinner = ui::create_spinner(&format!("Setting up {}", host));

        setup_host(config, host, &env_content, &spinner)?;

        spinner.finish_with_message(format!("Setup complete for {}", host));
        ui::print_success(&format!("{} setup successfully", host));
    }

    Ok(())
}

fn build_env_content(config: &Config) -> Result<String> {
    let mut env_content = String::new();

    for map in &config.env.clear {
        for (k, v) in map {
            env_content.push_str(&format!("export {}='{}'\n", k, shell::escape_env_value(v)));
        }
    }

    for k in &config.env.secret {
        let v = std::env::var(k)
            .with_context(|| format!("Missing local secret environment variable: {}", k))?;
        env_content.push_str(&format!("export {}='{}'\n", k, shell::escape_env_value(&v)));
    }

    if !config.mise.is_empty() {
        env_content.push_str("\neval \"$(mise activate bash)\"\n");
    }

    Ok(env_content)
}

fn setup_host(
    config: &Config,
    host: &str,
    env_content: &str,
    spinner: &indicatif::ProgressBar,
) -> Result<()> {
    // 1. Update pkg
    spinner.set_message(format!("[{}] Updating pkg repositories...", host));
    remote::run(host, &maybe_doas("pkg update", config.doas))?;

    // 2. Install default packages
    spinner.set_message(format!("[{}] Installing default packages...", host));
    remote::run(
        host,
        &maybe_doas("pkg install -y caddy rsync git bash", config.doas),
    )?;

    // 3. Create user if needed
    setup_user(config, host, spinner)?;

    // 4. Install user packages
    setup_packages(config, host, spinner)?;

    // 5. Setup ZFS if available
    setup_zfs(config, host, spinner)?;

    // 6. Setup directories
    setup_directories(config, host, spinner)?;

    // 7. Write env file
    let safe_service = shell::escape(&config.service);
    let config_dir = format!("{}/{}", CONFIG_DIR, safe_service);
    spinner.set_message(format!("[{}] Configuring environment...", host));
    let env_path = format!("{}/env", config_dir);
    remote::write_file(host, env_content, &env_path, config.doas)?;

    // 8. Setup Caddy
    setup_caddy(config, host, spinner)?;

    Ok(())
}

fn setup_user(config: &Config, host: &str, spinner: &indicatif::ProgressBar) -> Result<()> {
    if let Some(user) = &config.user {
        let safe_user = shell::escape(user);
        spinner.set_message(format!("[{}] Ensure user {} exists...", host, user));

        let check_user = remote::run(host, &format!("id {}", safe_user));
        if check_user.is_err() {
            remote::run(
                host,
                &maybe_doas(
                    &format!("pw useradd -n {} -m -s /usr/local/bin/bash", safe_user),
                    config.doas,
                ),
            )?;
        }
    }
    Ok(())
}

fn setup_packages(config: &Config, host: &str, spinner: &indicatif::ProgressBar) -> Result<()> {
    if !config.packages.is_empty() {
        spinner.set_message(format!("[{}] Installing user packages...", host));
        let safe_pkgs: Vec<String> = config.packages.iter().map(|p| shell::escape(p)).collect();
        let pkgs = safe_pkgs.join(" ");
        remote::run(
            host,
            &maybe_doas(&format!("pkg install -y {}", pkgs), config.doas),
        )?;
    }
    Ok(())
}

fn setup_zfs(config: &Config, host: &str, spinner: &indicatif::ProgressBar) -> Result<()> {
    if let Ok(Some(root_dataset)) = remote::get_zfs_dataset(host, "/") {
        spinner.set_message(format!(
            "[{}] ZFS detected (dataset: {}). Setting up datasets...",
            host, root_dataset
        ));

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
                let mountpoint = if ds == bsdeploy_root_dataset {
                    BSDEPLOY_BASE.to_string()
                } else {
                    format!(
                        "{}/{}",
                        BSDEPLOY_BASE,
                        ds.split('/').last().unwrap_or("unknown")
                    )
                };

                remote::run(
                    host,
                    &maybe_doas(
                        &format!("zfs create -o mountpoint={} {}", mountpoint, ds),
                        config.doas,
                    ),
                )
                .ok();
            }
        }
    }
    Ok(())
}

fn setup_directories(config: &Config, host: &str, spinner: &indicatif::ProgressBar) -> Result<()> {
    spinner.set_message(format!("[{}] Creating directories...", host));

    let safe_service = shell::escape(&config.service);
    let app_dir = format!("{}/{}/app", APP_DATA_DIR, safe_service);
    remote::run(
        host,
        &maybe_doas(&format!("mkdir -p {}", app_dir), config.doas),
    )?;

    let config_dir = format!("{}/{}", CONFIG_DIR, safe_service);
    remote::run(
        host,
        &maybe_doas(&format!("mkdir -p {}", config_dir), config.doas),
    )?;

    for dir in &config.data_directories {
        let (host_path, _) = dir.get_paths();
        let safe_path = shell::escape(&host_path);
        remote::run(
            host,
            &maybe_doas(&format!("mkdir -p {}", safe_path), config.doas),
        )?;
    }

    // Create user-specific directories
    if let Some(user) = &config.user {
        let safe_user = shell::escape(user);
        let run_dir = format!("{}/{}", RUN_DIR, safe_service);
        let log_dir = format!("{}/{}", LOG_DIR, safe_service);

        remote::run(
            host,
            &maybe_doas(&format!("mkdir -p {}", run_dir), config.doas),
        )?;
        remote::run(
            host,
            &maybe_doas(&format!("mkdir -p {}", log_dir), config.doas),
        )?;
        remote::run(
            host,
            &maybe_doas(
                &format!("chown {}:{} {}", safe_user, safe_user, run_dir),
                config.doas,
            ),
        )?;
        remote::run(
            host,
            &maybe_doas(
                &format!("chown {}:{} {}", safe_user, safe_user, log_dir),
                config.doas,
            ),
        )?;

        // Chown app and data directories
        let app_data_service = format!("{}/{}", APP_DATA_DIR, safe_service);
        remote::run(
            host,
            &maybe_doas(
                &format!("chown -R {}:{} {}", safe_user, safe_user, app_data_service),
                config.doas,
            ),
        )?;

        for dir in &config.data_directories {
            let (host_path, _) = dir.get_paths();
            let safe_path = shell::escape(&host_path);
            remote::run(
                host,
                &maybe_doas(
                    &format!("chown -R {}:{} {}", safe_user, safe_user, safe_path),
                    config.doas,
                ),
            )?;
        }
    }

    Ok(())
}

fn setup_caddy(config: &Config, host: &str, spinner: &indicatif::ProgressBar) -> Result<()> {
    spinner.set_message(format!("[{}] Configuring Caddy...", host));

    remote::run(host, &maybe_doas("sysrc caddy_enable=YES", config.doas))?;
    remote::run(
        host,
        &maybe_doas(&format!("mkdir -p {}", CADDY_CONF_DIR), config.doas),
    )?;

    // Create certs directory if SSL config is present
    if let Some(proxy) = &config.proxy {
        if proxy.ssl.is_some() {
            remote::run(
                host,
                &maybe_doas(&format!("mkdir -p {}", CADDY_CERTS_DIR), config.doas),
            )?;
        }
    }

    // Check/Create main Caddyfile
    let check_caddyfile = remote::run(host, &format!("test -f {}", CADDYFILE_PATH));

    if check_caddyfile.is_err() {
        let default_caddy = "import conf.d/*.caddy\n";
        remote::write_file(host, default_caddy, CADDYFILE_PATH, config.doas)?;
    } else {
        let check_import = remote::run(
            host,
            &format!("grep -q 'import conf.d/\\*.caddy' {}", CADDYFILE_PATH),
        );
        if check_import.is_err() {
            ui::print_step(&format!("Appending import to {}", CADDYFILE_PATH));
            let append_cmd = if config.doas {
                format!(
                    "echo 'import conf.d/*.caddy' | doas tee -a {} > /dev/null",
                    CADDYFILE_PATH
                )
            } else {
                format!(
                    "echo 'import conf.d/*.caddy' | tee -a {} > /dev/null",
                    CADDYFILE_PATH
                )
            };
            remote::run(host, &append_cmd)?;
        }
    }

    // Proxy config
    if let Some(proxy) = &config.proxy {
        // Handle SSL certificates if configured
        if let Some(ssl) = &proxy.ssl {
            spinner.set_message(format!("[{}] Writing TLS certificates...", host));
            caddy::write_ssl_certificates(config, host, ssl)?;
        }

        let backend = format!(":{}", proxy.port);
        let proxy_conf_content = caddy::generate_caddyfile(proxy, &config.service, &backend);
        let proxy_conf_path = format!("{}/{}.caddy", CADDY_CONF_DIR, config.service);
        remote::write_file(host, &proxy_conf_content, &proxy_conf_path, config.doas)?;
    }

    // Restart caddy
    remote::run(host, &maybe_doas("service caddy enable", config.doas))?;
    remote::run(host, &maybe_doas("service caddy restart", config.doas))?;

    Ok(())
}
