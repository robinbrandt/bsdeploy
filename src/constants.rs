/// Base directory for all bsdeploy data on remote hosts
pub const BSDEPLOY_BASE: &str = "/usr/local/bsdeploy";

/// Directory for storing FreeBSD base system versions
pub const BASE_DIR: &str = "/usr/local/bsdeploy/base";

/// Directory for storing built images (base + packages + mise)
pub const IMAGES_DIR: &str = "/usr/local/bsdeploy/images";

/// Directory for storing jail instances
pub const JAILS_DIR: &str = "/usr/local/bsdeploy/jails";

/// Default IP range for jail networking (CIDR notation)
pub const DEFAULT_IP_RANGE: &str = "10.0.0.0/24";

/// Default IP when subnet parsing fails
pub const DEFAULT_BASE_IP: &str = "10.0.0.0";

/// Environment file path inside jails
pub const JAIL_ENV_FILE: &str = "/etc/bsdeploy.env";

/// Application directory inside jails
pub const JAIL_APP_DIR: &str = "/app";

/// Application data storage on host
pub const APP_DATA_DIR: &str = "/var/db/bsdeploy";

/// Service configuration directory on host
pub const CONFIG_DIR: &str = "/usr/local/etc/bsdeploy";

/// Runtime directory for PID files
pub const RUN_DIR: &str = "/var/run/bsdeploy";

/// Log directory for service logs
pub const LOG_DIR: &str = "/var/log/bsdeploy";

/// Caddy configuration directory
pub const CADDY_CONF_DIR: &str = "/usr/local/etc/caddy/conf.d";

/// Main Caddyfile path
pub const CADDYFILE_PATH: &str = "/usr/local/etc/caddy/Caddyfile";

/// Directory for TLS certificates on remote host
pub const CADDY_CERTS_DIR: &str = "/usr/local/etc/caddy/certs";

/// Default ZFS pool name
pub const DEFAULT_ZFS_POOL: &str = "zroot";

/// Number of old jails to keep for rollback
pub const JAILS_TO_KEEP: usize = 3;
