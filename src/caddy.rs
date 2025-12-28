//! Caddy reverse proxy configuration utilities.

use anyhow::{Context, Result};

use crate::config::{Config, ProxyConfig, SslConfig};
use crate::constants::CADDY_CERTS_DIR;
use crate::remote;

/// Generate Caddyfile content for a proxy configuration.
pub fn generate_caddyfile(proxy: &ProxyConfig, service: &str, backend: &str) -> String {
    // Determine hostname format based on TLS mode
    let hostname = if proxy.ssl.is_some() || proxy.tls {
        proxy.hostname.clone()
    } else {
        format!("http://{}", proxy.hostname)
    };

    let mut content = format!("{} {{\n", hostname);

    // Add TLS directive for manual certificates
    if proxy.ssl.is_some() {
        content.push_str(&format!(
            "    tls {}/{}.crt {}/{}.key\n",
            CADDY_CERTS_DIR, service, CADDY_CERTS_DIR, service
        ));
    }

    content.push_str(&format!("    reverse_proxy {}\n", backend));
    content.push_str("}\n");

    content
}

/// Write SSL certificates from environment variables to remote host.
pub fn write_ssl_certificates(
    config: &Config,
    host: &str,
    ssl: &SslConfig,
) -> Result<()> {
    let cmd_prefix = if config.doas { "doas " } else { "" };

    // Ensure certs directory exists
    remote::run(
        host,
        &format!("{}mkdir -p {}", cmd_prefix, CADDY_CERTS_DIR),
    )?;

    // Read certificate from environment variable
    let cert_content = std::env::var(&ssl.certificate_pem).with_context(|| {
        format!(
            "Missing SSL certificate environment variable: {}",
            ssl.certificate_pem
        )
    })?;

    // Read private key from environment variable
    let key_content = std::env::var(&ssl.private_key_pem).with_context(|| {
        format!(
            "Missing SSL private key environment variable: {}",
            ssl.private_key_pem
        )
    })?;

    let cert_path = format!("{}/{}.crt", CADDY_CERTS_DIR, config.service);
    let key_path = format!("{}/{}.key", CADDY_CERTS_DIR, config.service);

    // Write certificate
    remote::write_file(host, &cert_content, &cert_path, config.doas)?;

    // Write private key
    remote::write_file(host, &key_content, &key_path, config.doas)?;

    // Set secure permissions (600) and ownership to www (Caddy user on FreeBSD)
    remote::run(
        host,
        &format!("{}chmod 600 {} {}", cmd_prefix, cert_path, key_path),
    )?;
    remote::run(
        host,
        &format!("{}chown www:www {} {}", cmd_prefix, cert_path, key_path),
    )?;

    Ok(())
}
