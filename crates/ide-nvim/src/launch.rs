//! Detect Neovim binary and AstroNvim installation.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NvimLaunchConfig {
    /// Override binary path.
    pub binary: Option<PathBuf>,
    /// `NVIM_APPNAME` (e.g. `astronvim` when installed that way).
    pub appname: Option<String>,
    pub env: HashMap<String, String>,
    /// Extra CLI args (e.g. `-u NONE` for smoke tests).
    #[serde(default)]
    pub extra_args: Vec<String>,
    pub astronvim_detected: bool,
    pub notes: Vec<String>,
}

pub fn resolve_nvim_bin(cfg: &NvimLaunchConfig) -> Result<PathBuf> {
    if let Some(p) = &cfg.binary {
        if p.is_file() {
            return Ok(p.clone());
        }
        bail!("configured nvim binary not found: {}", p.display());
    }
    if let Ok(p) = std::env::var("TUI_IDE_NVIM") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
    }
    which("nvim")
}

pub fn detect_astronvim() -> NvimLaunchConfig {
    let mut cfg = NvimLaunchConfig::default();

    // Explicit override
    if let Ok(app) = std::env::var("TUI_IDE_NVIM_APPNAME") {
        cfg.appname = Some(app);
        cfg.notes
            .push("Using TUI_IDE_NVIM_APPNAME from environment".into());
    }

    let home_cfg = dirs::config_dir();
    let Some(config) = home_cfg else {
        cfg.notes.push("No config dir; vanilla nvim".into());
        return cfg;
    };

    // Standard AstroNvim user template lives in ~/.config/nvim
    let nvim_dir = config.join("nvim");
    if looks_like_astronvim(&nvim_dir) {
        cfg.astronvim_detected = true;
        cfg.notes.push(format!(
            "AstroNvim-like config detected at {}",
            nvim_dir.display()
        ));
        return cfg;
    }

    // Alternate NVIM_APPNAME install: ~/.config/astronvim
    let astro_dir = config.join("astronvim");
    if looks_like_astronvim(&astro_dir) || astro_dir.join("lua").is_dir() {
        cfg.astronvim_detected = true;
        cfg.appname = Some("astronvim".into());
        cfg.notes.push(format!(
            "AstroNvim appname config at {} (NVIM_APPNAME=astronvim)",
            astro_dir.display()
        ));
        return cfg;
    }

    if nvim_dir.is_dir() {
        cfg.notes.push(format!(
            "Neovim config at {} (not clearly AstroNvim) — plugins still work",
            nvim_dir.display()
        ));
    } else {
        cfg.notes.push(
            "No ~/.config/nvim found. Install AstroNvim: https://astronvim.com/ — then restart tui-ide."
                .into(),
        );
    }
    cfg
}

fn looks_like_astronvim(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    // Heuristics for AstroNvim v3/v4 templates
    let markers = [
        dir.join("lua/astronvim"),
        dir.join("lua/plugins"),
        dir.join("lua/community.lua"),
        dir.join("lua/lazy_setup.lua"),
    ];
    if markers.iter().any(|p| p.exists()) {
        return true;
    }
    // Grep init for astronvim string
    let init = dir.join("init.lua");
    if let Ok(text) = std::fs::read_to_string(init) {
        if text.to_ascii_lowercase().contains("astronvim") {
            return true;
        }
    }
    false
}

fn which(name: &str) -> Result<PathBuf> {
    let path = std::env::var_os("PATH").ok_or_else(|| anyhow::anyhow!("PATH unset"))?;
    for dir in std::env::split_paths(&path) {
        let c = dir.join(name);
        if c.is_file() {
            return Ok(c);
        }
    }
    bail!("{name} not found in PATH")
}
