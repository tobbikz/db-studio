//! Remote session profiles (SSH PTY + RDP external launcher).
//!
//! SSOT file: `~/.config/tui-ide/sessions.toml`
//! Never stores plaintext passwords — identity files / ssh-agent / env only.

use anyhow::{Context, Result, bail};
use ide_term::TerminalPane;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    Ssh,
    Rdp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionProfile {
    pub id: String,
    pub name: String,
    pub kind: SessionKind,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub user: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// OpenSSH identity file (`-i`). Empty → agent / defaults.
    #[serde(default)]
    pub identity_file: String,
    /// ProxyJump / bastion host (`user@bastion` or hostname).
    #[serde(default)]
    pub jump_host: String,
    /// Preferred RDP client binary name (optional).
    #[serde(default)]
    pub rdp_client: String,
    /// Optional RDP gateway host.
    #[serde(default)]
    pub gateway: String,
}

fn default_ssh_port() -> u16 {
    22
}

impl SessionProfile {
    pub fn new_ssh(name: impl Into<String>, host: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            kind: SessionKind::Ssh,
            host: host.into(),
            user: user.into(),
            port: 22,
            identity_file: String::new(),
            jump_host: String::new(),
            rdp_client: String::new(),
            gateway: String::new(),
        }
    }

    pub fn new_rdp(name: impl Into<String>, host: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            kind: SessionKind::Rdp,
            host: host.into(),
            user: user.into(),
            port: 3389,
            identity_file: String::new(),
            jump_host: String::new(),
            rdp_client: String::new(),
            gateway: String::new(),
        }
    }

    pub fn display_label(&self) -> String {
        let kind = match self.kind {
            SessionKind::Ssh => "ssh",
            SessionKind::Rdp => "rdp",
        };
        let target = if self.user.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        };
        format!("[{kind}] {} — {target}", self.name)
    }

    /// Build `ssh` argv (without program name). No passwords.
    pub fn ssh_args(&self) -> Result<Vec<String>> {
        if self.kind != SessionKind::Ssh {
            bail!("not an SSH profile");
        }
        if self.host.trim().is_empty() {
            bail!("SSH host is empty");
        }
        let mut args = Vec::new();
        args.push("-o".into());
        args.push("BatchMode=yes".into());
        args.push("-o".into());
        args.push("StrictHostKeyChecking=accept-new".into());
        if self.port != 0 && self.port != 22 {
            args.push("-p".into());
            args.push(self.port.to_string());
        }
        let id = self.identity_file.trim();
        if !id.is_empty() {
            let path = expand_tilde(id);
            args.push("-i".into());
            args.push(path.display().to_string());
        }
        let jump = self.jump_host.trim();
        if !jump.is_empty() {
            args.push("-J".into());
            args.push(jump.to_string());
        }
        let dest = if self.user.trim().is_empty() {
            self.host.trim().to_string()
        } else {
            format!("{}@{}", self.user.trim(), self.host.trim())
        };
        args.push(dest);
        Ok(args)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionsFile {
    #[serde(default)]
    pub profiles: Vec<SessionProfile>,
}

pub struct LiveSsh {
    pub id: String,
    pub profile_id: String,
    pub label: String,
    pub pane: TerminalPane,
}

pub struct LiveRdp {
    pub id: String,
    pub profile_id: String,
    pub label: String,
    pub child: Child,
}

#[derive(Default)]
pub struct SessionManager {
    pub profiles: Vec<SessionProfile>,
    pub selected: usize,
    pub live_ssh: Vec<LiveSsh>,
    pub active_ssh: usize,
    pub live_rdp: Vec<LiveRdp>,
    pub path: Option<PathBuf>,
    /// Draft fields for add/edit form (S4).
    pub form_name: String,
    pub form_kind_ssh: bool,
    pub form_host: String,
    pub form_user: String,
    pub form_port: String,
    pub form_identity: String,
    pub form_jump: String,
    pub form_gateway: String,
    pub form_edit_id: Option<String>,
}

impl SessionManager {
    pub fn load() -> Self {
        let path = sessions_path();
        let mut s = Self {
            path: path.clone(),
            form_kind_ssh: true,
            form_port: "22".into(),
            ..Default::default()
        };
        if let Some(p) = path {
            if let Ok(file) = load_file(&p) {
                s.profiles = file.profiles;
            }
        }
        s
    }

    pub fn reload(&mut self) -> Result<()> {
        let Some(p) = self.path.clone().or_else(sessions_path) else {
            bail!("no config dir");
        };
        self.path = Some(p.clone());
        self.profiles = load_file(&p)?.profiles;
        if self.selected >= self.profiles.len() {
            self.selected = self.profiles.len().saturating_sub(1);
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        let Some(p) = &self.path else {
            bail!("no sessions path");
        };
        // Refuse accidental password keys if someone hand-edited drafts into structs later.
        let file = SessionsFile {
            profiles: self.profiles.clone(),
        };
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(&file)?;
        if text.to_ascii_lowercase().contains("password") {
            bail!("refusing to write sessions.toml containing 'password'");
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

    pub fn selected_profile(&self) -> Option<&SessionProfile> {
        self.profiles.get(self.selected)
    }

    pub fn begin_add(&mut self, ssh: bool) {
        self.form_edit_id = None;
        self.form_kind_ssh = ssh;
        self.form_name.clear();
        self.form_host.clear();
        self.form_user.clear();
        self.form_port = if ssh { "22".into() } else { "3389".into() };
        self.form_identity.clear();
        self.form_jump.clear();
        self.form_gateway.clear();
    }

    pub fn begin_edit_selected(&mut self) -> Result<()> {
        let Some(p) = self.selected_profile().cloned() else {
            bail!("no profile selected");
        };
        self.form_edit_id = Some(p.id.clone());
        self.form_kind_ssh = p.kind == SessionKind::Ssh;
        self.form_name = p.name;
        self.form_host = p.host;
        self.form_user = p.user;
        self.form_port = p.port.to_string();
        self.form_identity = p.identity_file;
        self.form_jump = p.jump_host;
        self.form_gateway = p.gateway;
        Ok(())
    }

    pub fn commit_form(&mut self) -> Result<()> {
        let name = self.form_name.trim();
        let host = self.form_host.trim();
        if name.is_empty() || host.is_empty() {
            bail!("name and host required");
        }
        let port: u16 = self.form_port.trim().parse().unwrap_or(if self.form_kind_ssh {
            22
        } else {
            3389
        });
        let mut profile = if self.form_kind_ssh {
            SessionProfile::new_ssh(name, host, self.form_user.trim())
        } else {
            SessionProfile::new_rdp(name, host, self.form_user.trim())
        };
        if let Some(id) = &self.form_edit_id {
            profile.id = id.clone();
        }
        profile.port = port;
        profile.identity_file = self.form_identity.trim().to_string();
        profile.jump_host = self.form_jump.trim().to_string();
        profile.gateway = self.form_gateway.trim().to_string();

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

    pub fn connect_selected(
        &mut self,
        cwd: &Path,
        cols: u16,
        rows: u16,
    ) -> Result<ConnectOutcome> {
        let Some(profile) = self.selected_profile().cloned() else {
            bail!("no profile selected");
        };
        match profile.kind {
            SessionKind::Ssh => {
                let args = profile.ssh_args()?;
                let pane = TerminalPane::spawn_command(cwd, cols, rows, "ssh", &args)?;
                let live = LiveSsh {
                    id: Uuid::new_v4().to_string(),
                    profile_id: profile.id.clone(),
                    label: profile.display_label(),
                    pane,
                };
                self.live_ssh.push(live);
                self.active_ssh = self.live_ssh.len() - 1;
                Ok(ConnectOutcome::SshAttached {
                    label: profile.display_label(),
                })
            }
            SessionKind::Rdp => {
                let child = spawn_rdp(&profile)?;
                let live = LiveRdp {
                    id: Uuid::new_v4().to_string(),
                    profile_id: profile.id.clone(),
                    label: profile.display_label(),
                    child,
                };
                self.live_rdp.push(live);
                Ok(ConnectOutcome::RdpSpawned {
                    label: profile.display_label(),
                })
            }
        }
    }

    pub fn active_ssh_pane_mut(&mut self) -> Option<&mut TerminalPane> {
        let i = self.active_ssh;
        self.live_ssh.get_mut(i).map(|s| &mut s.pane)
    }

    pub fn active_ssh_pane(&self) -> Option<&TerminalPane> {
        self.live_ssh.get(self.active_ssh).map(|s| &s.pane)
    }

    pub fn next_ssh_tab(&mut self) {
        if self.live_ssh.is_empty() {
            return;
        }
        self.active_ssh = (self.active_ssh + 1) % self.live_ssh.len();
    }

    pub fn prev_ssh_tab(&mut self) {
        if self.live_ssh.is_empty() {
            return;
        }
        self.active_ssh = if self.active_ssh == 0 {
            self.live_ssh.len() - 1
        } else {
            self.active_ssh - 1
        };
    }

    pub fn close_active_ssh(&mut self) {
        if self.live_ssh.is_empty() {
            return;
        }
        self.live_ssh.remove(self.active_ssh);
        if self.active_ssh >= self.live_ssh.len() {
            self.active_ssh = self.live_ssh.len().saturating_sub(1);
        }
    }

    pub fn resize_active_ssh(&mut self, cols: u16, rows: u16) {
        if let Some(p) = self.active_ssh_pane_mut() {
            p.resize(cols, rows);
        }
    }

    pub fn reap_rdp(&mut self) {
        self.live_rdp.retain_mut(|r| match r.child.try_wait() {
            Ok(None) => true,
            _ => false,
        });
    }

    pub fn kill_rdp_for_profile(&mut self, profile_id: &str) {
        self.live_rdp.retain_mut(|r| {
            if r.profile_id == profile_id {
                let _ = r.child.kill();
                let _ = r.child.wait();
                false
            } else {
                true
            }
        });
    }
}

#[derive(Debug)]
pub enum ConnectOutcome {
    SshAttached { label: String },
    RdpSpawned { label: String },
}

pub fn sessions_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("tui-ide");
    p.push("sessions.toml");
    Some(p)
}

fn load_file(path: &Path) -> Result<SessionsFile> {
    if !path.exists() {
        return Ok(SessionsFile::default());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let file: SessionsFile = toml::from_str(&text).context("parse sessions.toml")?;
    Ok(file)
}

fn expand_tilde(p: &str) -> PathBuf {
    if p.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&p[2..]);
        }
    }
    PathBuf::from(p)
}

pub fn find_rdp_client(preferred: &str) -> Option<&'static str> {
    let pref = preferred.trim();
    let order: &[&str] = if !pref.is_empty() {
        // Prefer configured name if present on PATH.
        match pref {
            "xfreerdp" => &["xfreerdp", "wlfreerdp", "remmina"],
            "wlfreerdp" => &["wlfreerdp", "xfreerdp", "remmina"],
            "remmina" => &["remmina", "xfreerdp", "wlfreerdp"],
            _ => &["xfreerdp", "wlfreerdp", "remmina"],
        }
    } else {
        &["xfreerdp", "wlfreerdp", "remmina"]
    };
    for name in order {
        if which(name).is_some() {
            return Some(*name);
        }
    }
    None
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let c = dir.join(name);
        if c.is_file() {
            return Some(c);
        }
    }
    None
}

fn spawn_rdp(profile: &SessionProfile) -> Result<Child> {
    let client = find_rdp_client(&profile.rdp_client)
        .ok_or_else(|| anyhow::anyhow!("no RDP client (xfreerdp / wlfreerdp / remmina) in PATH"))?;
    let host = profile.host.trim();
    if host.is_empty() {
        bail!("RDP host empty");
    }
    let mut cmd = Command::new(client);
    match client {
        "remmina" => {
            // Best-effort: remmina prefers .remmina files; pass rdp:// URI.
            let uri = if profile.user.trim().is_empty() {
                format!("rdp://{host}:{}", profile.port)
            } else {
                format!("rdp://{}@{host}:{}", profile.user.trim(), profile.port)
            };
            cmd.arg(uri);
        }
        _ => {
            // FreeRDP family
            cmd.arg(format!("/v:{host}:{}", if profile.port == 0 { 3389 } else { profile.port }));
            if !profile.user.trim().is_empty() {
                cmd.arg(format!("/u:{}", profile.user.trim()));
            }
            if !profile.gateway.trim().is_empty() {
                cmd.arg(format!("/g:{}", profile.gateway.trim()));
            }
            cmd.arg("+clipboard");
        }
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {client}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_args_with_jump_and_key() {
        let mut p = SessionProfile::new_ssh("lab", "10.0.0.5", "ops");
        p.port = 2222;
        p.identity_file = "/tmp/id_ed25519".into();
        p.jump_host = "bastion.example".into();
        let args = p.ssh_args().unwrap();
        assert!(args.iter().any(|a| a == "-J"));
        assert!(args.iter().any(|a| a == "bastion.example"));
        assert!(args.iter().any(|a| a == "-p"));
        assert!(args.iter().any(|a| a == "2222"));
        assert!(args.ends_with(&["ops@10.0.0.5".into()]));
    }

    #[test]
    fn save_roundtrip_no_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.toml");
        let mut mgr = SessionManager {
            path: Some(path.clone()),
            form_kind_ssh: true,
            form_port: "22".into(),
            ..Default::default()
        };
        mgr.profiles
            .push(SessionProfile::new_ssh("home", "127.0.0.1", "me"));
        mgr.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.to_ascii_lowercase().contains("password"));
        let loaded = load_file(&path).unwrap();
        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.profiles[0].name, "home");
    }
}
