use anyhow::Result;

use crate::config::Config;
use crate::constants::*;
use crate::{remote, ui};

pub fn run(config: &Config) -> Result<()> {
    ui::print_step(&format!(
        "Status for service '{}' on {} host(s)",
        config.service,
        config.hosts.len()
    ));

    for host in &config.hosts {
        println!();
        show_host_status(config, host)?;
    }

    Ok(())
}

fn show_host_status(config: &Config, host: &str) -> Result<()> {
    println!("Host: {}", host);
    println!("{}", "─".repeat(60));

    // Get list of jails for this service
    let ls_cmd = format!(
        "ls -1t {}/ 2>/dev/null | grep '^{}-' || true",
        JAILS_DIR, config.service
    );
    let jails_output = remote::run_with_output(host, &ls_cmd)?;
    let jails: Vec<&str> = jails_output
        .lines()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if jails.is_empty() {
        println!("  No jails found for service '{}'", config.service);
        println!();
        return Ok(());
    }

    // Get running jails
    let running_cmd = format!(
        "jls -N name 2>/dev/null | grep '^{}-' || true",
        config.service
    );
    let running_output = remote::run_with_output(host, &running_cmd)?;
    let running_jails: Vec<&str> = running_output
        .lines()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    println!("  Jails ({} total, {} running):", jails.len(), running_jails.len());
    println!();

    for (i, jail_name) in jails.iter().enumerate() {
        let is_running = running_jails.contains(jail_name);
        let status_icon = if is_running { "●" } else { "○" };
        let status_text = if is_running { "running" } else { "stopped" };

        // Get IP if running
        let ip = if is_running {
            let ip_cmd = format!("jls -j {} ip4.addr 2>/dev/null || echo '-'", jail_name);
            remote::run_with_output(host, &ip_cmd)
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "-".to_string())
        } else {
            "-".to_string()
        };

        // Parse timestamp from jail name (format: service-YYYYMMDD-HHMMSS)
        let created = parse_jail_timestamp(jail_name).unwrap_or_else(|| "-".to_string());

        let marker = if i == 0 && is_running { " (current)" } else { "" };

        println!(
            "  {} {:<40} {:>8}  IP: {:<15}  Created: {}{}",
            status_icon, jail_name, status_text, ip, created, marker
        );
    }

    // Show proxy info if configured
    if let Some(proxy) = &config.proxy {
        println!();
        let caddy_conf = format!("{}/{}.caddy", CADDY_CONF_DIR, config.service);
        let cat_cmd = format!("cat {} 2>/dev/null || echo 'not configured'", caddy_conf);
        if let Ok(conf) = remote::run_with_output(host, &cat_cmd) {
            let conf = conf.trim();
            if conf != "not configured" {
                // Extract backend from reverse_proxy line
                if let Some(line) = conf.lines().find(|l| l.contains("reverse_proxy")) {
                    let backend = line
                        .trim()
                        .strip_prefix("reverse_proxy ")
                        .unwrap_or("-");
                    println!("  Proxy: {} → {}", proxy.hostname, backend);
                }
            } else {
                println!("  Proxy: not configured");
            }
        }
    }

    println!();
    Ok(())
}

/// Parse timestamp from jail name format: service-YYYYMMDD-HHMMSS
fn parse_jail_timestamp(jail_name: &str) -> Option<String> {
    // Find the timestamp part (last two hyphen-separated segments)
    let parts: Vec<&str> = jail_name.rsplitn(3, '-').collect();
    if parts.len() >= 2 {
        let time = parts[0]; // HHMMSS
        let date = parts[1]; // YYYYMMDD

        if date.len() == 8 && time.len() == 6 {
            // Format as YYYY-MM-DD HH:MM:SS
            return Some(format!(
                "{}-{}-{} {}:{}:{}",
                &date[0..4],
                &date[4..6],
                &date[6..8],
                &time[0..2],
                &time[2..4],
                &time[4..6]
            ));
        }
    }
    None
}
