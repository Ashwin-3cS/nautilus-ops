use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = ".nautilus.toml";

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct NautilusConfig {
    #[serde(default)]
    pub sui: SuiConfig,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SuiConfig {
    pub network: Option<String>,
    pub package_id: Option<String>,
    pub config_object_id: Option<String>,
    pub cap_object_id: Option<String>,
}

impl NautilusConfig {
    /// Load config from `.nautilus.toml` in the given directory (or current dir).
    pub fn load(dir: Option<&Path>) -> Result<Self> {
        let path = config_path(dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))
    }

    /// Save config to `.nautilus.toml` in the given directory (or current dir).
    pub fn save(&self, dir: Option<&Path>) -> Result<()> {
        let path = config_path(dir);
        let content = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;
        std::fs::write(&path, content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }
}

fn config_path(dir: Option<&Path>) -> PathBuf {
    match dir {
        Some(d) => d.join(CONFIG_FILE),
        None => PathBuf::from(CONFIG_FILE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_missing_config_returns_default() {
        let tmp = TempDir::new().unwrap();
        let cfg = NautilusConfig::load(Some(tmp.path())).unwrap();
        assert!(cfg.sui.package_id.is_none());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cfg = NautilusConfig {
            sui: SuiConfig {
                network: Some("testnet".into()),
                package_id: Some("0xabc".into()),
                config_object_id: Some("0xdef".into()),
                cap_object_id: Some("0x123".into()),
            },
        };
        cfg.save(Some(tmp.path())).unwrap();

        let loaded = NautilusConfig::load(Some(tmp.path())).unwrap();
        assert_eq!(loaded.sui.package_id.as_deref(), Some("0xabc"));
        assert_eq!(loaded.sui.network.as_deref(), Some("testnet"));
    }
}
