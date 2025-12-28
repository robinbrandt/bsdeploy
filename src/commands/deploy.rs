use anyhow::Result;
use indicatif::ProgressBar;

use crate::config::Config;
use crate::constants::*;
use crate::{caddy, image, jail, remote, shell, ui};

pub fn run(config: &Config) -> Result<()> {
    ui::print_step(&format!("Running deploy for {} hosts", config.hosts.len()));

    for host in &config.hosts {
        let spinner = ui::create_spinner(&format!("Deploying to {}", host));

        deploy_to_host(config, host, &spinner)?;

        spinner.finish_with_message(format!("Deploy complete for {}", host));
        ui::print_success(&format!("{} deployed successfully", host));
    }

    Ok(())
}

fn deploy_to_host(config: &Config, host: &str, spinner: &ProgressBar) -> Result<()> {
    // 1. Determine Base Version
    let base_version = determine_base_version(config, host)?;
    let subnet = config
        .jail
        .as_ref()
        .and_then(|j| j.ip_range.as_deref())
        .unwrap_or(DEFAULT_IP_RANGE);

    // 2. Ensure base system
    spinner.set_message(format!("[{}] Ensuring base system {}...", host, base_version));
    jail::ensure_base(host, &base_version, config.doas)?;

    // 3. Ensure Image (Base + Packages + Mise)
    spinner.set_message(format!("[{}] Checking image...", host));
    let image_path = image::ensure_image(config, host, &base_version, spinner)?;

    // 4. Create Jail from Image
    spinner.set_message(format!("[{}] Creating new jail from image...", host));
    let jail_info = jail::create(
        host,
        &config.service,
        &base_version,
        subnet,
        Some(&image_path),
        &config.data_directories,
        config.doas,
    )?;
    spinner.set_message(format!(
        "[{}] Jail created: {} ({})",
        host, jail_info.name, jail_info.ip
    ));

    let cmd_prefix = if config.doas { "doas " } else { "" };

    // Run remaining deployment steps, cleaning up the jail on failure
    let result = deploy_jail_steps(config, host, &jail_info, cmd_prefix, spinner);

    if let Err(ref e) = result {
        spinner.set_message(format!("[{}] Deployment failed, cleaning up jail {}...", host, jail_info.name));
        cleanup_failed_jail(host, &jail_info, cmd_prefix);
        spinner.set_message(format!("[{}] Cleanup complete. Error: {}", host, e));
    }

    result
}

/// Execute deployment steps after jail creation. Returns error if any step fails.
fn deploy_jail_steps(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    cmd_prefix: &str,
    spinner: &ProgressBar,
) -> Result<()> {
    // 5. Start Jail (Phase 1: Inherit IP for build hooks)
    start_jail_build_phase(config, host, jail_info, cmd_prefix, spinner)?;

    // 6. Sync application code
    sync_application(config, host, jail_info, cmd_prefix, spinner)?;

    // 7. Configure environment
    configure_environment(config, host, jail_info, cmd_prefix)?;

    // 8. Run before_start hooks
    run_before_start_hooks(config, host, jail_info, cmd_prefix, spinner)?;

    // 9. Restart jail with private networking
    restart_jail_production(config, host, jail_info, cmd_prefix, spinner)?;

    // 10. Start services
    start_services(config, host, jail_info, cmd_prefix, spinner)?;

    // 11. Update proxy configuration
    update_proxy(config, host, jail_info, cmd_prefix, spinner)?;

    // 12. Stop old jails
    stop_old_jails(config, host, jail_info, cmd_prefix, spinner)?;

    // 13. Prune old jails
    prune_old_jails(config, host, jail_info, cmd_prefix, spinner)?;

    Ok(())
}

/// Clean up a failed jail deployment: stop jail, remove IP alias, unmount, remove directory
fn cleanup_failed_jail(host: &str, jail_info: &jail::JailInfo, cmd_prefix: &str) {
    // Stop jail if running
    remote::run(host, &format!("{}jail -r {} 2>/dev/null", cmd_prefix, jail_info.name)).ok();

    // Remove IP alias
    if !jail_info.ip.is_empty() {
        remote::run(
            host,
            &format!("{}ifconfig lo1 inet {} -alias 2>/dev/null", cmd_prefix, jail_info.ip),
        ).ok();
    }

    // Unmount all filesystems under jail path
    let mount_check = format!("mount | grep '{}' | awk '{{print $3}}'", jail_info.path);
    if let Ok(mounts) = remote::run_with_output(host, &mount_check) {
        // Unmount in reverse order (deepest first)
        for mnt in mounts.lines().rev() {
            let mnt = mnt.trim();
            if !mnt.is_empty() {
                remote::run(host, &format!("{}umount -f {}", cmd_prefix, mnt)).ok();
            }
        }
    }

    // Remove jail directory or ZFS dataset
    if let Ok(Some(dataset)) = remote::get_zfs_dataset(host, &jail_info.path) {
        remote::run(host, &format!("{}zfs destroy -r {}", cmd_prefix, dataset)).ok();
    }

    // Remove directory (handles non-ZFS case or if ZFS destroy failed)
    remote::run(host, &format!("{}chflags -R noschg {}", cmd_prefix, jail_info.path)).ok();
    remote::run(host, &format!("{}rm -rf {}", cmd_prefix, jail_info.path)).ok();
}

fn determine_base_version(config: &Config, host: &str) -> Result<String> {
    if let Some(j) = &config.jail {
        if let Some(v) = &j.base_version {
            return Ok(v.clone());
        }
    }

    let os_release = remote::get_os_release(host)?;
    // Strip patch level (e.g., 14.1-RELEASE-p6 -> 14.1-RELEASE)
    Ok(os_release
        .split("-p")
        .next()
        .unwrap_or(&os_release)
        .to_string())
}

fn start_jail_build_phase(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    cmd_prefix: &str,
    spinner: &ProgressBar,
) -> Result<()> {
    spinner.set_message(format!("[{}] Starting jail (build phase)...", host));

    let build_start_cmd = format!(
        "{}jail -c name={} path={} host.hostname={} ip4=inherit allow.raw_sockets=1 persist",
        cmd_prefix, jail_info.name, jail_info.path, jail_info.name
    );
    remote::run(host, &build_start_cmd)?;

    // Ensure data directory permissions
    if let Some(user) = &config.user {
        let safe_user = shell::escape(user);
        for entry in &config.data_directories {
            let (_, jail_path) = entry.get_paths();
            if !jail_path.is_empty() {
                let safe_path = shell::escape(&jail_path);
                remote::run(
                    host,
                    &format!(
                        "{}jexec {} chown -R {} {}",
                        cmd_prefix, jail_info.name, safe_user, safe_path
                    ),
                )?;
            }
        }
    }

    Ok(())
}

fn sync_application(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    cmd_prefix: &str,
    spinner: &ProgressBar,
) -> Result<()> {
    spinner.set_message(format!("[{}] Syncing app to jail...", host));

    let app_dir = JAIL_APP_DIR;
    let host_app_dir = format!("{}{}", jail_info.path, JAIL_APP_DIR);

    remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, host_app_dir))?;

    // Build excludes for data directories inside app
    let mut excludes = Vec::new();
    for entry in &config.data_directories {
        let (_, jail_path) = entry.get_paths();
        if jail_path.starts_with(app_dir) {
            if let Some(rel) = jail_path.strip_prefix(app_dir) {
                let rel = rel.trim_start_matches('/');
                if !rel.is_empty() {
                    excludes.push(format!("/{}", rel));
                }
            }
        }
    }

    remote::sync(host, ".", &host_app_dir, &excludes, config.doas)?;

    // Set ownership
    if let Some(user) = &config.user {
        let safe_user = shell::escape(user);
        remote::run(
            host,
            &format!(
                "{}jexec {} chown -R {} {}",
                cmd_prefix, jail_info.name, safe_user, app_dir
            ),
        )?;
    }

    Ok(())
}

fn configure_environment(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    _cmd_prefix: &str,
) -> Result<()> {
    let mut env_content = String::new();

    for map in &config.env.clear {
        for (k, v) in map {
            env_content.push_str(&format!("export {}='{}'\n", k, shell::escape_env_value(v)));
        }
    }

    for k in &config.env.secret {
        let v = std::env::var(k)?;
        env_content.push_str(&format!("export {}='{}'\n", k, shell::escape_env_value(&v)));
    }

    if !config.mise.is_empty() {
        env_content.push_str("\neval \"$(mise activate bash)\"\n");
    }

    let env_path = format!("{}{}", jail_info.path, JAIL_ENV_FILE);
    remote::write_file(host, &env_content, &env_path, config.doas)?;

    Ok(())
}

fn run_before_start_hooks(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    cmd_prefix: &str,
    spinner: &ProgressBar,
) -> Result<()> {
    let app_dir = JAIL_APP_DIR;

    // Trust mise config first
    if let Some(user) = &config.user {
        let safe_user = shell::escape(user);
        let trust_cmd = format!(
            "{}jexec {} su - {} -c 'mise trust {}'",
            cmd_prefix, jail_info.name, safe_user, app_dir
        );
        remote::run(host, &trust_cmd).ok();
    } else {
        let trust_cmd = format!(
            "{}jexec {} bash -c 'mise trust {}'",
            cmd_prefix, jail_info.name, app_dir
        );
        remote::run(host, &trust_cmd).ok();
    }

    // Run before_start commands
    for cmd in &config.before_start {
        spinner.set_message(format!("[{}] Jail: Running {}...", host, cmd));

        let full_cmd = format!(
            "bash -c 'source {} && cd {} && {}'",
            JAIL_ENV_FILE, app_dir, cmd
        );

        let exec_cmd = if let Some(user) = &config.user {
            let safe_user = shell::escape(user);
            format!(
                "{}jexec {} su - {} -c \"{}\"",
                cmd_prefix,
                jail_info.name,
                safe_user,
                full_cmd.replace("\"", "\\\"")
            )
        } else {
            format!("{}jexec {} {}", cmd_prefix, jail_info.name, full_cmd)
        };

        remote::run(host, &exec_cmd)?;
    }

    Ok(())
}

fn restart_jail_production(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    cmd_prefix: &str,
    spinner: &ProgressBar,
) -> Result<()> {
    spinner.set_message(format!(
        "[{}] Restarting jail with isolated networking...",
        host
    ));

    remote::run(host, &format!("{}jail -r {}", cmd_prefix, jail_info.name))?;

    let run_start_cmd = format!(
        "{}jail -c name={} path={} host.hostname={} ip4.addr={} allow.raw_sockets=1 persist",
        cmd_prefix, jail_info.name, jail_info.path, jail_info.name, jail_info.ip
    );
    remote::run(host, &run_start_cmd)?;

    // Ensure service directories in jail
    if let Some(user) = &config.user {
        let safe_user = shell::escape(user);
        let safe_service = shell::escape(&config.service);
        let jail_run_dir = format!("{}{}/{}", jail_info.path, RUN_DIR, safe_service);
        let jail_log_dir = format!("{}{}/{}", jail_info.path, LOG_DIR, safe_service);

        remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, jail_run_dir))?;
        remote::run(host, &format!("{}mkdir -p {}", cmd_prefix, jail_log_dir))?;
        remote::run(
            host,
            &format!(
                "{}chown {}:{} {}",
                cmd_prefix, safe_user, safe_user, jail_run_dir
            ),
        )?;
        remote::run(
            host,
            &format!(
                "{}chown {}:{} {}",
                cmd_prefix, safe_user, safe_user, jail_log_dir
            ),
        )?;
    }

    Ok(())
}

fn start_services(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    cmd_prefix: &str,
    spinner: &ProgressBar,
) -> Result<()> {
    let app_dir = JAIL_APP_DIR;

    for cmd in &config.start {
        spinner.set_message(format!("[{}] Jail: Starting service...", host));

        let safe_service = shell::escape(&config.service);
        let (pid_file, log_file) = if config.user.is_some() {
            (
                format!("{}/{}/service.pid", RUN_DIR, safe_service),
                format!("{}/{}/service.log", LOG_DIR, safe_service),
            )
        } else {
            (
                "/var/run/service.pid".to_string(),
                "/var/log/service.log".to_string(),
            )
        };

        let mut daemon_cmd = format!("daemon -f -p {} -o {}", pid_file, log_file);
        if let Some(u) = &config.user {
            daemon_cmd.push_str(&format!(" -u {}", shell::escape(u)));
        }

        let full_cmd = format!(
            "{} bash -c 'source {} && cd {} && {}'",
            daemon_cmd, JAIL_ENV_FILE, app_dir, cmd
        );

        remote::run(
            host,
            &format!("{}jexec {} {}", cmd_prefix, jail_info.name, full_cmd),
        )?;
    }

    Ok(())
}

fn update_proxy(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    cmd_prefix: &str,
    spinner: &ProgressBar,
) -> Result<()> {
    if let Some(proxy) = &config.proxy {
        spinner.set_message(format!("[{}] Switching traffic to {}...", host, jail_info.ip));

        // Update SSL certificates if configured (they may have been rotated)
        if let Some(ssl) = &proxy.ssl {
            spinner.set_message(format!("[{}] Updating TLS certificates...", host));
            caddy::write_ssl_certificates(config, host, ssl)?;
        }

        let backend = format!("{}:{}", jail_info.ip, proxy.port);
        let proxy_conf_content = caddy::generate_caddyfile(proxy, &config.service, &backend);

        let caddy_conf_path = format!("{}/{}.caddy", CADDY_CONF_DIR, config.service);
        remote::write_file(host, &proxy_conf_content, &caddy_conf_path, config.doas)?;
        remote::run(host, &format!("{}service caddy reload", cmd_prefix))?;
    }

    Ok(())
}

fn stop_old_jails(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    cmd_prefix: &str,
    spinner: &ProgressBar,
) -> Result<()> {
    spinner.set_message(format!("[{}] Stopping processes in old jails...", host));

    let ls_cmd = format!("ls {}/ | grep '^{}-' || true", JAILS_DIR, config.service);

    if let Ok(ls_out) = remote::run_with_output(host, &ls_cmd) {
        let existing_jails: Vec<String> = ls_out
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != &jail_info.name)
            .collect();

        for jname in existing_jails {
            spinner.set_message(format!("[{}] Stopping service in jail {}...", host, jname));

            let safe_service = shell::escape(&config.service);
            let pid_file = if config.user.is_some() {
                format!("{}/{}/service.pid", RUN_DIR, safe_service)
            } else {
                "/var/run/service.pid".to_string()
            };

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

    Ok(())
}

fn prune_old_jails(
    config: &Config,
    host: &str,
    jail_info: &jail::JailInfo,
    cmd_prefix: &str,
    spinner: &ProgressBar,
) -> Result<()> {
    spinner.set_message(format!("[{}] Pruning old jails...", host));

    let ls_cmd = format!("ls {}/ | grep '^{}-' || true", JAILS_DIR, config.service);

    if let Ok(ls_out) = remote::run_with_output(host, &ls_cmd) {
        let mut jails: Vec<String> = ls_out
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        jails.sort();

        if jails.len() > JAILS_TO_KEEP {
            let to_remove_count = jails.len() - JAILS_TO_KEEP;
            let to_remove = &jails[0..to_remove_count];

            for jname in to_remove {
                if jname == &jail_info.name {
                    continue;
                }

                spinner.set_message(format!(
                    "[{}] Removing stale/old jail directory {}...",
                    host, jname
                ));

                let jpath = format!("{}/{}", JAILS_DIR, jname);

                // Stop jail if running
                remote::run(host, &format!("{}jail -r {} 2>/dev/null", cmd_prefix, jname)).ok();

                // Cleanup IP alias
                let info_cmd = format!("jls -j {} ip4.addr 2>/dev/null || echo '-'", jname);
                if let Ok(jip) = remote::run_with_output(host, &info_cmd) {
                    let jip = jip.trim();
                    if jip != "-" && !jip.is_empty() {
                        remote::run(
                            host,
                            &format!("{}ifconfig lo1 inet {} -alias 2>/dev/null", cmd_prefix, jip),
                        )
                        .ok();
                    }
                }

                // Unmount all under jpath
                let mount_check = format!("mount | grep '{}' | awk '{{print $3}}'", jpath);
                if let Ok(mounts) = remote::run_with_output(host, &mount_check) {
                    for mnt in mounts.lines().rev() {
                        if !mnt.trim().is_empty() {
                            remote::run(
                                host,
                                &format!("{}umount -f {}", cmd_prefix, mnt.trim()),
                            )
                            .ok();
                        }
                    }
                }

                // Remove dir or ZFS dataset
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
