use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use anyhow::{Context, Result};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub service: String,
    pub user: Option<String>,
    pub hosts: Vec<String>,
    pub jail: Option<JailConfig>,
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub env: EnvConfig,
    #[serde(default)]
    pub before_start: Vec<String>,
    #[serde(default)]
    pub start: Vec<String>,
    #[serde(default)]
    pub data_directories: Vec<DataDirectory>,
    #[serde(default)]
    pub doas: bool,
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub mise: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum DataDirectory {
    Simple(String),
    Mapping(HashMap<String, String>),
}

impl DataDirectory {
    pub fn get_paths(&self) -> (String, String) {
        match self {
            DataDirectory::Simple(path) => (path.clone(), path.clone()),
            DataDirectory::Mapping(map) => {
                // Take the first entry
                if let Some((host, jail)) = map.iter().next() {
                    (host.clone(), jail.clone())
                } else {
                    ("".to_string(), "".to_string())
                }
            }
        }
    }
}


#[derive(Debug, Deserialize, Default)]
pub struct JailConfig {
    pub base_version: Option<String>,
    pub ip_range: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProxyConfig {
    pub hostname: String,
    pub port: u16,
    #[serde(default = "default_true")]
    pub tls: bool,
    /// Optional SSL certificate configuration (overrides ACME when present)
    pub ssl: Option<SslConfig>,
}

/// SSL certificate configuration using secrets (environment variables)
#[derive(Debug, Deserialize, Clone)]
pub struct SslConfig {
    /// Environment variable name containing certificate PEM
    pub certificate_pem: String,
    /// Environment variable name containing private key PEM
    pub private_key_pem: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, Default)]
pub struct EnvConfig {
    #[serde(default)]
    pub clear: Vec<HashMap<String, String>>,
    #[serde(default)]
    pub secret: Vec<String>,
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {:?}", path.as_ref()))?;

        // Check for deprecated 'strategy' field
        let value: serde_yaml::Value = serde_yaml::from_str(&content)
            .with_context(|| "Failed to parse YAML config")?;
        if let Some(mapping) = value.as_mapping() {
            if mapping.contains_key(&serde_yaml::Value::String("strategy".to_string())) {
                anyhow::bail!("The 'strategy' field is no longer supported. Remove it from your config - jail deployment is now the only mode.");
            }
        }

        let config: Config = serde_yaml::from_str(&content)
            .with_context(|| "Failed to parse YAML config")?;
        Ok(config)
    }

    /// Parse config from a YAML string (for testing)
    #[cfg(test)]
    pub fn from_str(content: &str) -> Result<Self> {
        let value: serde_yaml::Value = serde_yaml::from_str(content)
            .with_context(|| "Failed to parse YAML config")?;
        if let Some(mapping) = value.as_mapping() {
            if mapping.contains_key(&serde_yaml::Value::String("strategy".to_string())) {
                anyhow::bail!("The 'strategy' field is no longer supported. Remove it from your config - jail deployment is now the only mode.");
            }
        }
        let config: Config = serde_yaml::from_str(content)
            .with_context(|| "Failed to parse YAML config")?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn minimal_config() -> &'static str {
        r#"
service: myapp
hosts:
  - example.com
"#
    }

    fn full_config() -> &'static str {
        r#"
service: myapp
hosts:
  - host1.example.com
  - host2.example.com
user: deploy
doas: true
jail:
  base_version: "14.1-RELEASE"
  ip_range: "192.168.1.0/24"
packages:
  - curl
  - git
mise:
  ruby: "3.3.0"
  node: "20.0.0"
env:
  clear:
    - PORT: "3000"
    - RAILS_ENV: production
  secret:
    - SECRET_KEY_BASE
before_start:
  - bundle install
  - rake db:migrate
start:
  - bin/rails server
data_directories:
  - /var/data/storage: /app/storage
  - /var/data/uploads
proxy:
  hostname: myapp.example.com
  port: 3000
  tls: true
"#
    }

    #[test]
    fn test_load_minimal_config() {
        let config = Config::from_str(minimal_config()).unwrap();
        assert_eq!(config.service, "myapp");
        assert_eq!(config.hosts, vec!["example.com"]);
        assert!(config.user.is_none());
        assert!(!config.doas);
        assert!(config.jail.is_none());
        assert!(config.packages.is_empty());
        assert!(config.mise.is_empty());
        assert!(config.before_start.is_empty());
        assert!(config.start.is_empty());
        assert!(config.data_directories.is_empty());
        assert!(config.proxy.is_none());
    }

    #[test]
    fn test_load_full_config() {
        let config = Config::from_str(full_config()).unwrap();

        assert_eq!(config.service, "myapp");
        assert_eq!(config.hosts.len(), 2);
        assert_eq!(config.user, Some("deploy".to_string()));
        assert!(config.doas);

        let jail = config.jail.as_ref().unwrap();
        assert_eq!(jail.base_version, Some("14.1-RELEASE".to_string()));
        assert_eq!(jail.ip_range, Some("192.168.1.0/24".to_string()));

        assert_eq!(config.packages, vec!["curl", "git"]);
        assert_eq!(config.mise.get("ruby"), Some(&"3.3.0".to_string()));
        assert_eq!(config.mise.get("node"), Some(&"20.0.0".to_string()));

        assert_eq!(config.env.clear.len(), 2);
        assert_eq!(config.env.secret, vec!["SECRET_KEY_BASE"]);

        assert_eq!(config.before_start.len(), 2);
        assert_eq!(config.start, vec!["bin/rails server"]);

        assert_eq!(config.data_directories.len(), 2);

        let proxy = config.proxy.as_ref().unwrap();
        assert_eq!(proxy.hostname, "myapp.example.com");
        assert_eq!(proxy.port, 3000);
        assert!(proxy.tls);
    }

    #[test]
    fn test_load_from_file() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(minimal_config().as_bytes()).unwrap();

        let config = Config::load(file.path()).unwrap();
        assert_eq!(config.service, "myapp");
    }

    #[test]
    fn test_load_missing_file() {
        let result = Config::load("/nonexistent/path/config.yml");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to read config file"));
    }

    #[test]
    fn test_load_invalid_yaml() {
        let result = Config::from_str("not: valid: yaml: [");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_missing_required_fields() {
        let result = Config::from_str("service: myapp");
        assert!(result.is_err());
    }

    #[test]
    fn test_deprecated_strategy_field() {
        let config_with_strategy = r#"
service: myapp
hosts:
  - example.com
strategy: host
"#;
        let result = Config::from_str(config_with_strategy);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("strategy"));
        assert!(err.contains("no longer supported"));
    }

    #[test]
    fn test_data_directory_simple() {
        let dir = DataDirectory::Simple("/var/data".to_string());
        let (host, jail) = dir.get_paths();
        assert_eq!(host, "/var/data");
        assert_eq!(jail, "/var/data");
    }

    #[test]
    fn test_data_directory_mapping() {
        let mut map = HashMap::new();
        map.insert("/host/path".to_string(), "/jail/path".to_string());
        let dir = DataDirectory::Mapping(map);
        let (host, jail) = dir.get_paths();
        assert_eq!(host, "/host/path");
        assert_eq!(jail, "/jail/path");
    }

    #[test]
    fn test_data_directory_empty_mapping() {
        let dir = DataDirectory::Mapping(HashMap::new());
        let (host, jail) = dir.get_paths();
        assert_eq!(host, "");
        assert_eq!(jail, "");
    }

    #[test]
    fn test_env_config_defaults() {
        let config = Config::from_str(minimal_config()).unwrap();
        assert!(config.env.clear.is_empty());
        assert!(config.env.secret.is_empty());
    }

    #[test]
    fn test_proxy_tls_defaults_to_true() {
        let config_yaml = r#"
service: myapp
hosts:
  - example.com
proxy:
  hostname: myapp.example.com
  port: 3000
"#;
        let config = Config::from_str(config_yaml).unwrap();
        let proxy = config.proxy.unwrap();
        assert!(proxy.tls);
    }

    #[test]
    fn test_proxy_tls_can_be_disabled() {
        let config_yaml = r#"
service: myapp
hosts:
  - example.com
proxy:
  hostname: myapp.example.com
  port: 3000
  tls: false
"#;
        let config = Config::from_str(config_yaml).unwrap();
        let proxy = config.proxy.unwrap();
        assert!(!proxy.tls);
    }

    #[test]
    fn test_doas_defaults_to_false() {
        let config = Config::from_str(minimal_config()).unwrap();
        assert!(!config.doas);
    }

    #[test]
    fn test_jail_config_optional_fields() {
        let config_yaml = r#"
service: myapp
hosts:
  - example.com
jail: {}
"#;
        let config = Config::from_str(config_yaml).unwrap();
        let jail = config.jail.unwrap();
        assert!(jail.base_version.is_none());
        assert!(jail.ip_range.is_none());
    }

    #[test]
    fn test_proxy_ssl_not_set_by_default() {
        let config_yaml = r#"
service: myapp
hosts:
  - example.com
proxy:
  hostname: myapp.example.com
  port: 3000
"#;
        let config = Config::from_str(config_yaml).unwrap();
        let proxy = config.proxy.unwrap();
        assert!(proxy.ssl.is_none());
        assert!(proxy.tls); // ACME enabled by default
    }

    #[test]
    fn test_proxy_ssl_manual_certificates() {
        let config_yaml = r#"
service: myapp
hosts:
  - example.com
proxy:
  hostname: myapp.example.com
  port: 3000
  ssl:
    certificate_pem: SSL_CERT
    private_key_pem: SSL_KEY
"#;
        let config = Config::from_str(config_yaml).unwrap();
        let proxy = config.proxy.unwrap();
        let ssl = proxy.ssl.unwrap();
        assert_eq!(ssl.certificate_pem, "SSL_CERT");
        assert_eq!(ssl.private_key_pem, "SSL_KEY");
    }

    #[test]
    fn test_proxy_ssl_with_tls_false() {
        // SSL config takes precedence, tls:false is ignored when ssl is set
        let config_yaml = r#"
service: myapp
hosts:
  - example.com
proxy:
  hostname: myapp.example.com
  port: 3000
  tls: false
  ssl:
    certificate_pem: SSL_CERT
    private_key_pem: SSL_KEY
"#;
        let config = Config::from_str(config_yaml).unwrap();
        let proxy = config.proxy.unwrap();
        assert!(proxy.ssl.is_some());
        // Note: ssl being present means TLS is enabled with manual certs
    }
}
