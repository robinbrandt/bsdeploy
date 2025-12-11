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
    #[serde(default)]
    pub packages: Vec<String>,
    #[serde(default)]
    pub env: EnvConfig,
    #[serde(default)]
    pub before_start: Vec<String>,
    #[serde(default)]
    pub start: Vec<String>,
    #[serde(default)]
    pub data_directories: Vec<String>,
    #[serde(default)]
    pub doas: bool,
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub mise: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct ProxyConfig {
    pub hostname: String,
    pub port: u16,
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
        let config: Config = serde_yaml::from_str(&content)
            .with_context(|| "Failed to parse YAML config")?;
        Ok(config)
    }
}
