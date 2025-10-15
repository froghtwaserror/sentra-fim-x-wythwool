
use serde::Deserialize;
use anyhow::{Context, Result};
use std::fs;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub baseline_db: String,
    pub metrics_bind: String,
    pub watch_paths: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "default_hash_alg")]
    pub hash_alg: String,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let s = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path))?;
        let cfg: Config = toml::from_str(&s)
            .with_context(|| format!("invalid TOML in {}", path))?;
        Ok(cfg)
    }
}


fn default_hash_alg() -> String { "blake3".to_string() }
fn default_debounce_ms() -> u64 { 250 }
