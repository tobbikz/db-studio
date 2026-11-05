//! Lightweight command plugins loaded from TOML (no WASM marketplace — local scripts/commands).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub commands: Vec<PluginCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCommand {
    pub id: String,
    pub title: String,
    /// Shell command; `{file}`, `{root}`, `{line}` placeholders supported.
    pub run: String,
}

#[derive(Debug, Clone)]
pub struct PluginHost {
    pub plugins: Vec<PluginManifest>,
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::load()
    }
}

impl PluginHost {
    pub fn load() -> Self {
        let mut plugins = Vec::new();
        if let Some(dir) = plugins_dir() {
            let _ = std::fs::create_dir_all(&dir);
            // Seed example plugin once
            let example = dir.join("example-echo.toml");
            if !example.exists() {
                let sample = PluginManifest {
                    name: "example".into(),
                    description: "Sample local plugin".into(),
                    commands: vec![PluginCommand {
                        id: "example.echo".into(),
                        title: "Echo current file path".into(),
                        run: "echo \"file={file} root={root} line={line}\"".into(),
                    }],
                };
                let _ = std::fs::write(
                    &example,
                    toml::to_string_pretty(&sample).unwrap_or_default(),
                );
            }
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for entry in rd.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                        continue;
                    }
                    match std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|t| toml::from_str::<PluginManifest>(&t).ok())
                    {
                        Some(m) => plugins.push(m),
                        None => warn!("failed to load plugin {}", path.display()),
                    }
                }
            }
        }
        Self { plugins }
    }

    pub fn all_commands(&self) -> Vec<(String, String, String)> {
        // (plugin, id, title)
        let mut out = Vec::new();
        for p in &self.plugins {
            for c in &p.commands {
                out.push((p.name.clone(), c.id.clone(), c.title.clone()));
            }
        }
        out
    }

    pub fn run(
        &self,
        command_id: &str,
        root: &std::path::Path,
        file: Option<&std::path::Path>,
        line: usize,
    ) -> Result<String> {
        for p in &self.plugins {
            if let Some(cmd) = p.commands.iter().find(|c| c.id == command_id) {
                let mut script = cmd.run.clone();
                script = script.replace("{root}", &root.display().to_string());
                script = script.replace(
                    "{file}",
                    &file
                        .map(|f| f.display().to_string())
                        .unwrap_or_default(),
                );
                script = script.replace("{line}", &line.to_string());
                let out = Command::new("sh")
                    .arg("-c")
                    .arg(&script)
                    .current_dir(root)
                    .output()
                    .with_context(|| format!("run plugin {}", cmd.id))?;
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                if !out.stderr.is_empty() {
                    text.push_str(&String::from_utf8_lossy(&out.stderr));
                }
                if !out.status.success() {
                    anyhow::bail!("plugin exited {:?}: {text}", out.status.code());
                }
                return Ok(text);
            }
        }
        anyhow::bail!("unknown plugin command {command_id}")
    }
}

fn plugins_dir() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("tui-ide");
    p.push("plugins");
    Some(p)
}
