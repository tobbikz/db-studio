//! Password vault: keep secrets out of `datasources.toml`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
struct SecretsFile {
    #[serde(default)]
    passwords: HashMap<String, String>,
}

fn secrets_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("tui-ide").join("secrets.toml"))
}

fn load() -> SecretsFile {
    let Some(path) = secrets_path() else {
        return SecretsFile::default();
    };
    if !path.exists() {
        return SecretsFile::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(file: &SecretsFile) -> Result<()> {
    let path = secrets_path().context("no config dir for secrets")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(file)?)?;
    Ok(())
}

/// Resolve a password field: `env:VAR`, secrets.toml by source id, or literal.
pub fn resolve_password(source_id: &str, stored: &str) -> String {
    let s = stored.trim();
    if let Some(var) = s.strip_prefix("env:") {
        return std::env::var(var.trim()).unwrap_or_default();
    }
    if !s.is_empty() {
        return s.to_string();
    }
    load()
        .passwords
        .get(source_id)
        .cloned()
        .unwrap_or_default()
}

/// Persist plaintext into secrets.toml; returns value to keep in datasources.toml
/// (`env:…` unchanged, plaintext → cleared).
pub fn store_password_for_save(source_id: &str, password: &str) -> Result<String> {
    let s = password.trim();
    if s.starts_with("env:") {
        // Drop vault entry so env wins.
        let mut file = load();
        file.passwords.remove(source_id);
        let _ = save(&file);
        return Ok(s.to_string());
    }
    if s.is_empty() {
        return Ok(String::new());
    }
    let mut file = load();
    file.passwords.insert(source_id.to_string(), s.to_string());
    save(&file)?;
    Ok(String::new())
}

pub fn remove_password(source_id: &str) {
    let mut file = load();
    if file.passwords.remove(source_id).is_some() {
        let _ = save(&file);
    }
}

pub fn has_vault_password(source_id: &str) -> bool {
    load().passwords.contains_key(source_id)
}

/// UI hint — never echo plaintext secrets.
pub fn password_display_hint(source_id: &str, stored: &str) -> String {
    let s = stored.trim();
    if s.starts_with("env:") {
        return s.to_string();
    }
    if !s.is_empty() {
        return "••••".into();
    }
    if has_vault_password(source_id) {
        return "(vault)".into();
    }
    String::new()
}
