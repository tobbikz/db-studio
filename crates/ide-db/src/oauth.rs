//! Browser OAuth helper — open authorize URL, paste token, persist for Studio.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use crate::DbDriver;

#[derive(Debug, Default, Serialize, Deserialize)]
struct OauthFile {
    #[serde(default)]
    tokens: HashMap<String, String>,
}

fn oauth_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("tui-ide").join("oauth.toml"))
}

fn load_file() -> OauthFile {
    let Some(path) = oauth_path() else {
        return OauthFile::default();
    };
    if !path.exists() {
        return OauthFile::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_file(file: &OauthFile) -> Result<()> {
    let path = oauth_path().context("no config dir for oauth")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml::to_string_pretty(file)?)?;
    Ok(())
}

fn env_key(driver: DbDriver) -> &'static str {
    match driver {
        DbDriver::Bigquery => "BIGQUERY_ACCESS_TOKEN",
        DbDriver::Snowflake => "SNOWFLAKE_PASSWORD",
        _ => "TUI_IDE_OAUTH_TOKEN",
    }
}

/// Default authorize URLs (MVP — paste token after browser consent).
pub fn authorize_url(driver: DbDriver) -> Option<&'static str> {
    match driver {
        DbDriver::Bigquery => Some(
            "https://accounts.google.com/o/oauth2/v2/auth?response_type=token&scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fbigquery",
        ),
        DbDriver::Snowflake => Some("https://app.snowflake.com/"),
        _ => None,
    }
}

/// Open URL in the desktop browser (xdg-open / open / cmd start).
pub fn open_browser(url: &str) -> Result<()> {
    let url = url.trim();
    if url.is_empty() {
        bail!("empty OAuth URL");
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(url)
            .spawn()
            .context("open browser")?;
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .context("open browser")?;
        return Ok(());
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        for bin in ["xdg-open", "gio", "sensible-browser"] {
            if Command::new(bin).arg(url).spawn().is_ok() {
                return Ok(());
            }
        }
        bail!("no browser opener (tried xdg-open/gio/sensible-browser)");
    }
}

/// Persist a pasted bearer/access token (process env + oauth.toml).
pub fn apply_pasted_token(driver: DbDriver, token: &str) -> Result<String> {
    let t = token.trim();
    if t.is_empty() {
        bail!("empty token");
    }
    let key = env_key(driver);
    std::env::set_var(key, t);
    let mut file = load_file();
    file.tokens.insert(key.to_string(), t.to_string());
    save_file(&file)?;
    Ok(format!("{key} set (process + ~/.config/tui-ide/oauth.toml)"))
}

/// Load persisted tokens into the process env (no overwrite if already set).
pub fn load_persisted_tokens() {
    let file = load_file();
    for (key, val) in file.tokens {
        if std::env::var(&key).ok().filter(|s| !s.is_empty()).is_none() {
            std::env::set_var(key, val);
        }
    }
}
