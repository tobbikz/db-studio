//! Agent CLI profiles (Cursor `agent`, Claude Code, OpenCode, custom).
//!
//! SSOT file: `~/.config/tui-ide/agents.toml`
//! Never stores API keys / passwords — env vars only.

use anyhow::{Context, Result, bail};
use ide_term::TerminalPane;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Cursor,
    Claude,
    Opencode,
    Custom,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Claude => "claude",
            Self::Opencode => "opencode",
            Self::Custom => "custom",
        }
    }

    pub fn default_binary(self) -> &'static str {
        match self {
            Self::Cursor => "agent",
            Self::Claude => "claude",
            Self::Opencode => "opencode",
            Self::Custom => "",
        }
    }

    pub fn from_form(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" => Self::Claude,
            "opencode" | "oc" => Self::Opencode,
            "custom" => Self::Custom,
            _ => Self::Cursor,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    pub kind: AgentKind,
    /// Override binary (empty → kind default).
    #[serde(default)]
    pub binary: String,
    /// Extra argv after the program (e.g. `--plan`, model flags).
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional text written to the PTY after spawn (seed prompt / DB context).
    #[serde(default)]
    pub seed_prompt: String,
}

impl AgentProfile {
    pub fn new(name: impl Into<String>, kind: AgentKind) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            kind,
            binary: String::new(),
            args: Vec::new(),
            seed_prompt: String::new(),
        }
    }

    pub fn display_label(&self) -> String {
        self.list_label()
    }

    /// Compact sidebar row: `● cursor  Cursor Agent`
    pub fn list_label(&self) -> String {
        let bin = self.resolved_binary();
        let mark = if which(&bin).is_some() { "●" } else { "○" };
        format!("{mark} {:<8} {}", self.kind.label(), self.name)
    }

    /// Short tab title for the agent PTY strip.
    pub fn tab_label(&self) -> String {
        self.name.chars().take(16).collect()
    }

    pub fn resolved_binary(&self) -> String {
        let b = self.binary.trim();
        if !b.is_empty() {
            return b.to_string();
        }
        self.kind.default_binary().to_string()
    }

    /// Program + argv for PTY spawn (no seed).
    pub fn command_argv(&self) -> Result<(String, Vec<String>)> {
        let program = self.resolved_binary();
        if program.is_empty() {
            bail!("custom profile needs a binary");
        }
        Ok((program, self.args.clone()))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentsFile {
    #[serde(default)]
    pub profiles: Vec<AgentProfile>,
}

pub struct LiveAgent {
    pub id: String,
    pub profile_id: String,
    pub label: String,
    pub pane: TerminalPane,
}

#[derive(Default)]
pub struct AgentManager {
    pub profiles: Vec<AgentProfile>,
    pub selected: usize,
    pub live: Vec<LiveAgent>,
    pub active: usize,
    pub path: Option<PathBuf>,
    /// Draft fields for add/edit form.
    pub form_name: String,
    pub form_kind: String,
    pub form_binary: String,
    pub form_args: String,
    pub form_seed: String,
    pub form_edit_id: Option<String>,
}

impl AgentManager {
    pub fn load() -> Self {
        let path = agents_path();
        let mut s = Self {
            path: path.clone(),
            form_kind: "cursor".into(),
            ..Default::default()
        };
        if let Some(p) = path {
            match load_file(&p) {
                Ok(file) if !file.profiles.is_empty() => s.profiles = file.profiles,
                Ok(_) => {
                    s.profiles = default_profiles();
                    let _ = s.save();
                }
                Err(_) => {
                    s.profiles = default_profiles();
                }
            }
        } else {
            s.profiles = default_profiles();
        }
        s
    }

    pub fn reload(&mut self) -> Result<()> {
        let Some(p) = self.path.clone().or_else(agents_path) else {
            bail!("no config dir");
        };
        self.path = Some(p.clone());
        let file = load_file(&p)?;
        self.profiles = if file.profiles.is_empty() {
            default_profiles()
        } else {
            file.profiles
        };
        if self.selected >= self.profiles.len() {
            self.selected = self.profiles.len().saturating_sub(1);
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let Some(p) = &self.path else {
            bail!("no agents path");
        };
        let file = AgentsFile {
            profiles: self.profiles.clone(),
        };
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(&file)?;
        let lower = text.to_ascii_lowercase();
        for forbidden in ["api_key", "apikey", "password", "secret", "token ="] {
            if lower.contains(forbidden) {
                bail!("refusing to write agents.toml containing '{forbidden}'");
            }
        }
        std::fs::write(p, text)?;
        Ok(())
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.profiles.len() {
            self.selected += 1;
        }
    }

    pub fn selected_profile(&self) -> Option<&AgentProfile> {
        self.profiles.get(self.selected)
    }

    pub fn begin_add(&mut self) {
        self.form_edit_id = None;
        self.form_name.clear();
        self.form_kind = "cursor".into();
        self.form_binary.clear();
        self.form_args.clear();
        self.form_seed.clear();
    }

    pub fn begin_edit_selected(&mut self) -> Result<()> {
        let Some(p) = self.selected_profile().cloned() else {
            bail!("no profile selected");
        };
        self.form_edit_id = Some(p.id.clone());
        self.form_name = p.name;
        self.form_kind = p.kind.label().to_string();
        self.form_binary = p.binary;
        self.form_args = p.args.join(" ");
        self.form_seed = p.seed_prompt;
        Ok(())
    }

    pub fn commit_form(&mut self) -> Result<()> {
        let name = self.form_name.trim();
        if name.is_empty() {
            bail!("name required");
        }
        let kind = AgentKind::from_form(&self.form_kind);
        let mut profile = AgentProfile::new(name, kind);
        if let Some(id) = &self.form_edit_id {
            profile.id = id.clone();
        }
        profile.binary = self.form_binary.trim().to_string();
        profile.args = split_args(&self.form_args);
        profile.seed_prompt = self.form_seed.trim().to_string();
        if kind == AgentKind::Custom && profile.binary.is_empty() {
            bail!("custom profile needs a binary");
        }

        if let Some(id) = &self.form_edit_id {
            if let Some(slot) = self.profiles.iter_mut().find(|p| &p.id == id) {
                *slot = profile;
            } else {
                self.profiles.push(profile);
            }
        } else {
            self.profiles.push(profile);
        }
        self.save()?;
        Ok(())
    }

    pub fn delete_selected(&mut self) -> Result<()> {
        if self.profiles.is_empty() {
            bail!("no profiles");
        }
        self.profiles.remove(self.selected);
        if self.selected >= self.profiles.len() {
            self.selected = self.profiles.len().saturating_sub(1);
        }
        self.save()
    }

    /// Launch selected profile. `extra_seed` is appended after profile seed (e.g. DB context).
    pub fn launch_selected(
        &mut self,
        cwd: &Path,
        cols: u16,
        rows: u16,
        extra_seed: Option<&str>,
    ) -> Result<LaunchOutcome> {
        let Some(profile) = self.selected_profile().cloned() else {
            bail!("no profile selected");
        };
        let (program, args) = profile.command_argv()?;
        if which(&program).is_none() {
            bail!("{program} not found in PATH — install the CLI or set binary override");
        }
        let pane = TerminalPane::spawn_command(cwd, cols, rows, &program, &args)?;
        let mut seed = String::new();
        if !profile.seed_prompt.trim().is_empty() {
            seed.push_str(profile.seed_prompt.trim());
            seed.push('\n');
        }
        if let Some(extra) = extra_seed {
            let t = extra.trim();
            if !t.is_empty() {
                seed.push_str(t);
                seed.push('\n');
            }
        }
        if !seed.is_empty() {
            // Give the TUI a moment; write seed as if typed.
            let _ = pane.write_bytes(seed.as_bytes());
        }
        let label = profile.tab_label();
        self.live.push(LiveAgent {
            id: Uuid::new_v4().to_string(),
            profile_id: profile.id.clone(),
            label: label.clone(),
            pane,
        });
        self.active = self.live.len() - 1;
        Ok(LaunchOutcome {
            label,
            binary: program,
        })
    }

    pub fn active_pane_mut(&mut self) -> Option<&mut TerminalPane> {
        let i = self.active;
        self.live.get_mut(i).map(|s| &mut s.pane)
    }

    pub fn active_pane(&self) -> Option<&TerminalPane> {
        self.live.get(self.active).map(|s| &s.pane)
    }

    pub fn next_tab(&mut self) {
        if self.live.is_empty() {
            return;
        }
        self.active = (self.active + 1) % self.live.len();
    }

    pub fn prev_tab(&mut self) {
        if self.live.is_empty() {
            return;
        }
        self.active = if self.active == 0 {
            self.live.len() - 1
        } else {
            self.active - 1
        };
    }

    pub fn close_active(&mut self) {
        if self.live.is_empty() {
            return;
        }
        self.live.remove(self.active);
        if self.active >= self.live.len() {
            self.active = self.live.len().saturating_sub(1);
        }
    }

    pub fn resize_active(&mut self, cols: u16, rows: u16) {
        if let Some(p) = self.active_pane_mut() {
            p.resize(cols, rows);
        }
    }
}

#[derive(Debug)]
pub struct LaunchOutcome {
    pub label: String,
    pub binary: String,
}

pub fn agents_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("tui-ide");
    p.push("agents.toml");
    Some(p)
}

pub fn default_profiles() -> Vec<AgentProfile> {
    vec![
        AgentProfile::new("Cursor Agent", AgentKind::Cursor),
        AgentProfile::new("Claude Code", AgentKind::Claude),
        AgentProfile::new("OpenCode", AgentKind::Opencode),
    ]
}

/// Build a safe DB context blurb (no passwords).
pub fn db_context_seed(
    driver: &str,
    host: &str,
    port: u16,
    database: &str,
    schema: &str,
) -> String {
    let mut parts = vec![format!("Database context (no secrets):")];
    if !driver.is_empty() {
        parts.push(format!("  driver: {driver}"));
    }
    if !host.is_empty() {
        parts.push(format!("  host: {host}:{port}"));
    }
    if !database.is_empty() {
        parts.push(format!("  database: {database}"));
    }
    if !schema.is_empty() {
        parts.push(format!("  schema: {schema}"));
    }
    parts.push("Help with SQL / schema for this connection.".into());
    parts.join("\n")
}

fn load_file(path: &Path) -> Result<AgentsFile> {
    if !path.exists() {
        return Ok(AgentsFile::default());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let file: AgentsFile = toml::from_str(&text).context("parse agents.toml")?;
    Ok(file)
}

fn which(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let c = dir.join(name);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

fn split_args(s: &str) -> Vec<String> {
    s.split_whitespace().map(|a| a.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_argv_default() {
        let p = AgentProfile::new("c", AgentKind::Cursor);
        let (prog, args) = p.command_argv().unwrap();
        assert_eq!(prog, "agent");
        assert!(args.is_empty());
    }

    #[test]
    fn custom_requires_binary() {
        let p = AgentProfile::new("x", AgentKind::Custom);
        assert!(p.command_argv().is_err());
    }

    #[test]
    fn save_roundtrip_no_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agents.toml");
        let mut mgr = AgentManager {
            path: Some(path.clone()),
            form_kind: "cursor".into(),
            ..Default::default()
        };
        mgr.profiles.push(AgentProfile::new("Cursor Agent", AgentKind::Cursor));
        mgr.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.to_ascii_lowercase().contains("api_key"));
        let loaded = load_file(&path).unwrap();
        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.profiles[0].kind, AgentKind::Cursor);
    }

    #[test]
    fn db_context_has_no_password_word_from_inputs() {
        let s = db_context_seed("postgres", "127.0.0.1", 5432, "brain", "public");
        assert!(s.contains("brain"));
        assert!(!s.to_ascii_lowercase().contains("password"));
    }
}
