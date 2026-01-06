use anyhow::Result;

use crate::config::Config;
use crate::constants::*;
use crate::{remote, ui};

pub fn run(config: &Config) -> Result<()> {
    ui::print_step(&format!(
        "Destroying all resources for service {} on {} hosts",
        config.service,
        config.hosts.len()
    ));

    for host in &config.hosts {
        let spinner = ui::create_spinner(&format!("Destroying resources on {}", host));

        destroy_host(config, host, &spinner)?;

        spinner.finish_with_message(format!("Resources destroyed for {}", host));
        ui::print_success(&format!("{} resources cleaned up", host));
    }

    Ok(())
}

fn destroy_host(config: &Config, host: &str, spinner: &indicatif::ProgressBar) -> Result<()> {
    let cmd_prefix = if config.doas { "doas " } else { "" };

    // 1. Find and remove jails
    remove_jails(config, host, cmd_prefix, spinner)?;

    // 2. Remove active symlink
    remove_active_symlink(config, host, cmd_prefix, spinner)?;

    // 3. Remove Caddy proxy config
    remove_proxy_config(config, host, cmd_prefix, spinner)?;

    Ok(())
}

fn remove_jails(
    config: &Config,
    host: &str,
    cmd_prefix: &str,
    spinner: &indicatif::ProgressBar,
) -> Result<()> {
    spinner.set_message(format!("[{}] Removing jails and networking...", host));

    let ls_cmd = format!("ls {}/ | grep '^{}-' || true", JAILS_DIR, config.service);

    if let Ok(ls_out) = remote::run_with_output(host, &ls_cmd) {
        for jname in ls_out
            .lines()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            spinner.set_message(format!("[{}] Cleaning up jail {}...", host, jname));

            let jpath = format!("{}/{}", JAILS_DIR, jname);

            // Get IP before stopping
            let info_cmd = format!("jls -j {} ip4.addr 2>/dev/null || echo '-'", jname);
            if let Ok(jip) = remote::run_with_output(host, &info_cmd) {
                let jip = jip.trim();

                // Stop jail
                remote::run(host, &format!("{}jail -r {} 2>/dev/null", cmd_prefix, jname)).ok();

                // Remove IP alias
                if jip != "-" && !jip.is_empty() {
                    remote::run(
                        host,
                        &format!("{}ifconfig lo1 inet {} -alias 2>/dev/null", cmd_prefix, jip),
                    )
                    .ok();
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

    Ok(())
}

fn remove_active_symlink(
    config: &Config,
    host: &str,
    cmd_prefix: &str,
    spinner: &indicatif::ProgressBar,
) -> Result<()> {
    spinner.set_message(format!("[{}] Removing active symlink...", host));

    let symlink_path = format!("{}/{}", ACTIVE_DIR, config.service);
    remote::run(host, &format!("{}rm -f {}", cmd_prefix, symlink_path)).ok();

    Ok(())
}

fn remove_proxy_config(
    config: &Config,
    host: &str,
    cmd_prefix: &str,
    spinner: &indicatif::ProgressBar,
) -> Result<()> {
    spinner.set_message(format!("[{}] Removing proxy configuration...", host));

    let caddy_conf = format!("{}/{}.caddy", CADDY_CONF_DIR, config.service);
    remote::run(host, &format!("{}rm -f {}", cmd_prefix, caddy_conf)).ok();
    remote::run(host, &format!("{}service caddy reload", cmd_prefix)).ok();

    Ok(())
}
