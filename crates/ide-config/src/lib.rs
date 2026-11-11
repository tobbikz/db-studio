//! User config: languages, theme, keymaps.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeConfig {
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub editor: EditorConfig,
    #[serde(default)]
    pub code: CodeConfig,
    #[serde(default)]
    pub languages: HashMap<String, LanguageServerConfig>,
    /// Optional keymap overlay: action name -> key description (informational + future remap).
    #[serde(default)]
    pub keymaps: HashMap<String, String>,
    /// Recently opened workspace folders (absolute paths, newest first).
    #[serde(default)]
    pub recent_projects: Vec<String>,
    /// Recently opened files (absolute paths, newest first).
    #[serde(default)]
    pub recent_files: Vec<String>,
    #[serde(default)]
    pub database: DatabaseConfig,
}

impl Default for IdeConfig {
    fn default() -> Self {
        Self {
            theme: ThemeConfig::default(),
            editor: EditorConfig::default(),
            code: CodeConfig::default(),
            languages: default_languages(),
            keymaps: default_keymaps(),
            recent_projects: Vec::new(),
            recent_files: Vec::new(),
            database: DatabaseConfig::default(),
        }
    }
}

/// Code / Neovim embed prefs (Wave 8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeConfig {
    /// Override Neovim binary (else `TUI_IDE_NVIM` / PATH `nvim`).
    #[serde(default)]
    pub nvim_binary: Option<String>,
    /// `NVIM_APPNAME` override (e.g. `astronvim`).
    #[serde(default)]
    pub nvim_appname: Option<String>,
}

impl Default for CodeConfig {
    fn default() -> Self {
        Self {
            nvim_binary: None,
            nvim_appname: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Soft SQL timeout in seconds (0 = disabled). Cancel still works via Esc.
    #[serde(default = "default_query_timeout")]
    pub query_timeout_secs: u64,
    /// Studio Explorer pane width (cols). Ctrl+← / Ctrl+→ in Studio.
    #[serde(default = "default_explorer_width")]
    pub explorer_width: u16,
    /// Explorer + Structure stack width (cols). Grows with Explorer as needed.
    #[serde(default = "default_studio_left_width")]
    pub studio_left_width: u16,
    /// Results pane height percent (25–70). Ctrl+↑ / Ctrl+↓ in Studio.
    #[serde(default = "default_results_pct")]
    pub results_pct: u16,
    /// Auto LIMIT for SELECT/WITH without an explicit LIMIT (0 = off). Alt-L cycles.
    #[serde(default = "default_select_limit")]
    pub select_limit: usize,
}

fn default_query_timeout() -> u64 {
    120
}
fn default_explorer_width() -> u16 {
    28
}
fn default_studio_left_width() -> u16 {
    // Explorer (≤28) + Structure (≥24) needs ~52+; prefer readable Structure empty state.
    56
}
fn default_results_pct() -> u16 {
    40
}
fn default_select_limit() -> usize {
    50
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            query_timeout_secs: default_query_timeout(),
            explorer_width: default_explorer_width(),
            studio_left_width: default_studio_left_width(),
            results_pct: default_results_pct(),
            select_limit: default_select_limit(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    #[serde(default = "default_theme_name")]
    pub name: String,
    #[serde(default)]
    pub status_bg: String,
    #[serde(default)]
    pub accent: String,
}

fn default_theme_name() -> String {
    "black".into()
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: default_theme_name(),
            status_bg: "#000000".into(),
            accent: "mono".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorConfig {
    #[serde(default = "default_tab_width")]
    pub tab_width: usize,
    #[serde(default = "default_true")]
    pub inlay_hints: bool,
    #[serde(default = "default_true")]
    pub format_on_save: bool,
    #[serde(default = "default_true")]
    pub soft_wrap: bool,
}

fn default_tab_width() -> usize {
    4
}
fn default_true() -> bool {
    true
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            tab_width: 4,
            inlay_hints: true,
            format_on_save: true,
            soft_wrap: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Language ids this server handles (LSP languageId strings).
    pub languages: Vec<String>,
    #[serde(default)]
    pub file_extensions: Vec<String>,
}

fn default_languages() -> HashMap<String, LanguageServerConfig> {
    let mut m = HashMap::new();
    m.insert(
        "rust".into(),
        LanguageServerConfig {
            command: "rust-analyzer".into(),
            args: vec![],
            languages: vec!["rust".into()],
            file_extensions: vec!["rs".into()],
        },
    );
    m.insert(
        "python".into(),
        LanguageServerConfig {
            command: "pyright-langserver".into(),
            args: vec!["--stdio".into()],
            languages: vec!["python".into()],
            file_extensions: vec!["py".into()],
        },
    );
    m.insert(
        "typescript".into(),
        LanguageServerConfig {
            command: "typescript-language-server".into(),
            args: vec!["--stdio".into()],
            languages: vec!["typescript".into(), "javascript".into()],
            file_extensions: vec![
                "ts".into(),
                "tsx".into(),
                "js".into(),
                "jsx".into(),
                "mjs".into(),
            ],
        },
    );
    m.insert(
        "go".into(),
        LanguageServerConfig {
            command: "gopls".into(),
            args: vec![],
            languages: vec!["go".into()],
            file_extensions: vec!["go".into()],
        },
    );
    m.insert(
        "clangd".into(),
        LanguageServerConfig {
            command: "clangd".into(),
            args: vec![],
            languages: vec!["c".into(), "cpp".into()],
            file_extensions: vec!["c".into(), "h".into(), "cpp".into(), "cc".into(), "hpp".into()],
        },
    );
    m.insert(
        "lua".into(),
        LanguageServerConfig {
            command: "lua-language-server".into(),
            args: vec![],
            languages: vec!["lua".into()],
            file_extensions: vec!["lua".into()],
        },
    );
    m
}

fn default_keymaps() -> HashMap<String, String> {
    HashMap::from([
        ("save".into(), "Ctrl-S".into()),
        ("palette".into(), "Space".into()),
        ("open_file".into(), "Ctrl-P".into()),
        ("search".into(), "Ctrl-Shift-F".into()),
        ("format".into(), "Ctrl-Shift-I".into()),
        ("rename".into(), "F2".into()),
        ("code_action".into(), "Ctrl-.".into()),
        ("goto_definition".into(), "g".into()),
        ("hover".into(), "K".into()),
        ("toggle_terminal".into(), "Ctrl-`".into()),
        ("git".into(), "Ctrl-Shift-G".into()),
        ("split_vertical".into(), "Ctrl-\\".into()),
        ("debug_toggle_breakpoint".into(), "F9".into()),
        ("debug_continue".into(), "F5".into()),
    ])
}

impl IdeConfig {
    pub fn config_path() -> Option<PathBuf> {
        let mut p = dirs::config_dir()?;
        p.push("tui-ide");
        p.push("config.toml");
        Some(p)
    }

    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        Self::load_from(&path).unwrap_or_default()
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            let cfg = Self::default();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let text = toml::to_string_pretty(&cfg)?;
            std::fs::write(path, text)?;
            return Ok(cfg);
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        let mut cfg: IdeConfig = toml::from_str(&text)?;
        // Merge defaults for missing language servers
        for (k, v) in default_languages() {
            cfg.languages.entry(k).or_insert(v);
        }
        Ok(cfg)
    }

    pub fn server_for_language(&self, language_id: &str) -> Option<&LanguageServerConfig> {
        self.languages.values().find(|c| {
            c.languages.iter().any(|l| l == language_id)
                || c.file_extensions.iter().any(|e| {
                    language_id.contains(e.as_str()) // weak fallback
                })
        })
    }

    pub fn server_for_extension(&self, ext: &str) -> Option<(&str, &LanguageServerConfig)> {
        self.languages.iter().find_map(|(name, cfg)| {
            if cfg.file_extensions.iter().any(|e| e == ext) {
                Some((name.as_str(), cfg))
            } else {
                None
            }
        })
    }

    pub fn remember_project(&mut self, path: &Path) {
        let Ok(canon) = path.canonicalize() else {
            return;
        };
        let s = canon.display().to_string();
        self.recent_projects.retain(|p| p != &s);
        self.recent_projects.insert(0, s);
        self.recent_projects.truncate(20);
        let _ = self.save();
    }

    pub fn remember_file(&mut self, path: &Path) {
        let Ok(canon) = path.canonicalize() else {
            return;
        };
        if !canon.is_file() {
            return;
        }
        let s = canon.display().to_string();
        self.recent_files.retain(|p| p != &s);
        self.recent_files.insert(0, s);
        self.recent_files.truncate(30);
        let _ = self.save();
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = Self::config_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(&path, text)
            .with_context(|| format!("write config {}", path.display()))?;
        Ok(())
    }
}
