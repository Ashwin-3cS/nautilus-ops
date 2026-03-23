use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = ".nautilus.toml";

/// Template type for the TEE application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Template {
    /// Rust-based TEE app (nautilus-tee-app, uses nautilus-enclave library)
    Rust,
    /// TypeScript-based TEE app (nautilus-ts, uses Bun + argonaut)
    Ts,
    /// Python-based TEE app (nautilus-python, uses pynacl + stdlib)
    Python,
}

impl Template {
    /// Default HTTP port for this template's enclave server.
    pub fn default_http_port(self) -> u16 {
        match self {
            Template::Rust => 4000,
            Template::Ts => 3000,
            Template::Python => 5000,
        }
    }

    /// Attestation endpoint path for this template.
    pub fn attestation_path(self) -> &'static str {
        match self {
            Template::Rust => "/get_attestation",
            Template::Ts => "/attestation",
            Template::Python => "/attestation",
        }
    }

    /// Default signing endpoint path for this template.
    pub fn default_sign_endpoint(self) -> &'static str {
        match self {
            Template::Rust => "/sign_name",
            Template::Ts => "/sign",
            Template::Python => "/sign",
        }
    }

    /// Health check endpoint path for this template.
    pub fn health_path(self) -> &'static str {
        match self {
            Template::Rust => "/health",
            Template::Ts => "/health_check",
            Template::Python => "/health",
        }
    }

    /// Logs endpoint path for this template.
    pub fn logs_path(self) -> &'static str {
        "/logs"
    }

    /// GitHub repository name for this template.
    pub fn repo_name(self) -> &'static str {
        match self {
            Template::Rust => "nautilus-rust",
            Template::Ts => "nautilus-ts",
            Template::Python => "nautilus-python",
        }
    }

    /// Default on-chain verify function for this template.
    pub fn default_verify_function(self) -> &'static str {
        match self {
            Template::Rust => "verify_signed_name",
            Template::Ts => "verify_signed_data",
            Template::Python => "verify_signed_data",
        }
    }
}

impl std::fmt::Display for Template {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Template::Rust => write!(f, "rust"),
            Template::Ts => write!(f, "ts"),
            Template::Python => write!(f, "python"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    pub template: Option<Template>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct NautilusConfig {
    #[serde(default)]
    pub project: ProjectConfig,
    #[serde(default)]
    pub sui: SuiConfig,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SuiConfig {
    pub network: Option<String>,
    pub package_id: Option<String>,
    /// Original (first-published) package ID where types like ENCLAVE were defined.
    /// Used for `--type-args` in sui CLI calls. Falls back to `package_id` if unset.
    pub original_package_id: Option<String>,
    pub config_object_id: Option<String>,
    pub cap_object_id: Option<String>,
}

impl SuiConfig {
    /// Package ID to use for `--type-args`. Types are anchored to the original
    /// (first-published) package. Falls back to `package_id` if not set.
    pub fn type_arg_package_id(&self) -> Option<&str> {
        self.original_package_id.as_deref()
            .or(self.package_id.as_deref())
    }
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

/// Auto-detect project template by examining the directory structure.
pub fn detect_template(dir: Option<&Path>) -> Result<Template> {
    let base = dir.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));

    // nautilus-ts: has argonaut/ directory + package.json
    if base.join("argonaut").is_dir() && base.join("package.json").is_file() {
        return Ok(Template::Ts);
    }

    // Python template: has requirements.txt + app.py
    if base.join("requirements.txt").is_file() && base.join("app.py").is_file() {
        return Ok(Template::Python);
    }

    // Rust template: has Cargo.toml (but not the CLI workspace — look for src/ with Rust files)
    if base.join("Cargo.toml").is_file() {
        return Ok(Template::Rust);
    }

    anyhow::bail!(
        "Could not detect project template in {}.\n\
         Expected argonaut/ + package.json (TS), requirements.txt + app.py (Python), or Cargo.toml (Rust).\n\
         Use --template to specify manually.",
        base.display()
    )
}

/// Resolve template from CLI flag, config file, or auto-detection.
pub fn resolve_template(cli_override: Option<Template>, config: &NautilusConfig) -> Result<Template> {
    if let Some(t) = cli_override {
        return Ok(t);
    }
    if let Some(t) = config.project.template {
        return Ok(t);
    }
    detect_template(None)
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
        assert!(cfg.project.template.is_none());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cfg = NautilusConfig {
            project: ProjectConfig {
                template: Some(Template::Ts),
            },
            sui: SuiConfig {
                network: Some("testnet".into()),
                package_id: Some("0xabc".into()),
                original_package_id: Some("0x999".into()),
                config_object_id: Some("0xdef".into()),
                cap_object_id: Some("0x123".into()),
            },
        };
        cfg.save(Some(tmp.path())).unwrap();

        let loaded = NautilusConfig::load(Some(tmp.path())).unwrap();
        assert_eq!(loaded.sui.package_id.as_deref(), Some("0xabc"));
        assert_eq!(loaded.sui.network.as_deref(), Some("testnet"));
        assert_eq!(loaded.project.template, Some(Template::Ts));
    }

    #[test]
    fn test_detect_template_ts() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("argonaut")).unwrap();
        std::fs::write(tmp.path().join("package.json"), "{}").unwrap();
        assert_eq!(detect_template(Some(tmp.path())).unwrap(), Template::Ts);
    }

    #[test]
    fn test_detect_template_rust() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(detect_template(Some(tmp.path())).unwrap(), Template::Rust);
    }

    #[test]
    fn test_detect_template_python() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("requirements.txt"), "pynacl>=1.5").unwrap();
        std::fs::write(tmp.path().join("app.py"), "# python app").unwrap();
        assert_eq!(detect_template(Some(tmp.path())).unwrap(), Template::Python);
    }

    #[test]
    fn test_detect_template_unknown_fails() {
        let tmp = TempDir::new().unwrap();
        assert!(detect_template(Some(tmp.path())).is_err());
    }

    #[test]
    fn test_resolve_template_cli_override() {
        let config = NautilusConfig::default();
        assert_eq!(
            resolve_template(Some(Template::Ts), &config).unwrap(),
            Template::Ts
        );
    }

    #[test]
    fn test_resolve_template_from_config() {
        let config = NautilusConfig {
            project: ProjectConfig { template: Some(Template::Rust) },
            ..Default::default()
        };
        assert_eq!(
            resolve_template(None, &config).unwrap(),
            Template::Rust
        );
    }

    #[test]
    fn test_template_default_ports() {
        assert_eq!(Template::Rust.default_http_port(), 4000);
        assert_eq!(Template::Ts.default_http_port(), 3000);
        assert_eq!(Template::Python.default_http_port(), 5000);
    }

    #[test]
    fn test_template_attestation_paths() {
        assert_eq!(Template::Rust.attestation_path(), "/get_attestation");
        assert_eq!(Template::Ts.attestation_path(), "/attestation");
        assert_eq!(Template::Python.attestation_path(), "/attestation");
    }
}
