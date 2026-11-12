//! Git integration via the `git` CLI.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
    Staged,
}

#[derive(Debug, Clone)]
pub struct GitFile {
    pub path: PathBuf,
    pub status: FileStatus,
    pub staged: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GitState {
    pub root: Option<PathBuf>,
    pub branch: String,
    pub files: Vec<GitFile>,
    pub selected: usize,
    pub diff: String,
    pub blame: String,
    pub branches: Vec<String>,
    pub branch_selected: usize,
    pub commit_msg: String,
    pub last_log: String,
    /// `origin` URL (or first remote), for GitLab/GitHub badge in Studio.
    pub remote_url: String,
    /// Normalized host label: `gitlab` | `github` | `origin` | empty.
    pub remote_kind: String,
}

impl GitState {
    pub async fn discover(cwd: &Path) -> Result<Self> {
        let root = git_output(cwd, &["rev-parse", "--show-toplevel"])
            .await?
            .trim()
            .to_string();
        if root.is_empty() {
            bail!("not a git repository");
        }
        let root = PathBuf::from(root);
        let mut s = Self {
            root: Some(root.clone()),
            ..Default::default()
        };
        s.refresh().await?;
        Ok(s)
    }

    pub async fn refresh(&mut self) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        self.branch = git_output(&root, &["branch", "--show-current"])
            .await?
            .trim()
            .to_string();
        if self.branch.is_empty() {
            self.branch = "DETACHED".into();
        }

        let porcelain = git_output(&root, &["status", "--porcelain", "-u"]).await?;
        self.files = parse_status(&porcelain);
        if self.selected >= self.files.len() {
            self.selected = self.files.len().saturating_sub(1);
        }

        let branches = git_output(&root, &["branch", "--format=%(refname:short)"]).await?;
        self.branches = branches
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        if self.branch_selected >= self.branches.len() {
            self.branch_selected = self
                .branches
                .iter()
                .position(|b| b == &self.branch)
                .unwrap_or(0);
        }

        self.last_log = git_output(&root, &["log", "-5", "--oneline"])
            .await
            .unwrap_or_default();
        self.remote_url = git_output(&root, &["remote", "get-url", "origin"])
            .await
            .unwrap_or_default()
            .trim()
            .to_string();
        if self.remote_url.is_empty() {
            if let Ok(remotes) = git_output(&root, &["remote", "-v"]).await {
                self.remote_url = remotes
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
            }
        }
        self.remote_kind = classify_remote(&self.remote_url);
        self.load_diff().await?;
        Ok(())
    }

    pub async fn load_diff(&mut self) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let Some(file) = self.files.get(self.selected) else {
            self.diff.clear();
            return Ok(());
        };
        let rel = file.path.to_string_lossy();
        if file.status == FileStatus::Untracked {
            self.diff = format!(
                "(untracked)\n{}",
                std::fs::read_to_string(root.join(&file.path)).unwrap_or_default()
            );
            return Ok(());
        }
        let args = if file.staged {
            vec!["diff", "--cached", "--", rel.as_ref()]
        } else {
            vec!["diff", "--", rel.as_ref()]
        };
        self.diff = git_output(&root, &args).await.unwrap_or_default();
        if self.diff.is_empty() {
            self.diff = "(no diff)".into();
        }
        Ok(())
    }

    pub async fn stage_selected(&mut self) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let Some(file) = self.files.get(self.selected).cloned() else {
            return Ok(());
        };
        git_output(
            &root,
            &["add", "--", file.path.to_string_lossy().as_ref()],
        )
        .await?;
        self.refresh().await
    }

    pub async fn unstage_selected(&mut self) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let Some(file) = self.files.get(self.selected).cloned() else {
            return Ok(());
        };
        git_output(
            &root,
            &[
                "restore",
                "--staged",
                "--",
                file.path.to_string_lossy().as_ref(),
            ],
        )
        .await?;
        self.refresh().await
    }

    pub async fn commit(&mut self, message: &str) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let message = message.trim();
        if message.is_empty() {
            bail!("empty commit message");
        }
        git_output(&root, &["commit", "-m", message]).await?;
        self.commit_msg.clear();
        self.refresh().await
    }

    pub async fn push(&mut self) -> Result<String> {
        let Some(root) = self.root.clone() else {
            bail!("no repo");
        };
        let out = git_output(&root, &["push", "-u", "origin", "HEAD"]).await?;
        self.refresh().await?;
        Ok(out)
    }

    pub async fn pull(&mut self) -> Result<String> {
        let Some(root) = self.root.clone() else {
            bail!("no repo");
        };
        let out = git_output(&root, &["pull", "--ff-only"]).await?;
        self.refresh().await?;
        Ok(out)
    }

    pub async fn checkout_selected_branch(&mut self) -> Result<()> {
        let Some(branch) = self.branches.get(self.branch_selected).cloned() else {
            return Ok(());
        };
        self.checkout(&branch).await
    }

    pub async fn blame(&mut self, path: &Path) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let rel = path.strip_prefix(&root).unwrap_or(path);
        self.blame = git_output(&root, &["blame", "--", rel.to_string_lossy().as_ref()])
            .await
            .unwrap_or_else(|e| format!("blame failed: {e}"));
        Ok(())
    }

    pub async fn checkout(&mut self, branch: &str) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        git_output(&root, &["checkout", branch]).await?;
        self.refresh().await
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.files.len() {
            self.selected += 1;
        }
    }

    pub fn branch_prev(&mut self) {
        if self.branches.is_empty() {
            return;
        }
        self.branch_selected = if self.branch_selected == 0 {
            self.branches.len() - 1
        } else {
            self.branch_selected - 1
        };
    }

    pub fn branch_next(&mut self) {
        if self.branches.is_empty() {
            return;
        }
        self.branch_selected = (self.branch_selected + 1) % self.branches.len();
    }
}

async fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    // Never open `less` / system pager — Studio captures stdout via pipe, but some
    // git builds still honor pager when mis-detected; force cat.
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .context("spawn git")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {err}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn classify_remote(url: &str) -> String {
    let u = url.to_ascii_lowercase();
    if u.contains("gitlab") {
        "gitlab".into()
    } else if u.contains("github.com") || u.contains("github.") {
        "github".into()
    } else if u.is_empty() {
        String::new()
    } else {
        "origin".into()
    }
}

fn parse_status(porcelain: &str) -> Vec<GitFile> {
    let mut files = Vec::new();
    for line in porcelain.lines() {
        if line.len() < 3 {
            continue;
        }
        let x = line.as_bytes()[0] as char;
        let y = line.as_bytes()[1] as char;
        let path = line[3..].trim().to_string();
        let path = if let Some((_, new)) = path.split_once(" -> ") {
            PathBuf::from(new)
        } else {
            PathBuf::from(path)
        };
        let staged = x != ' ' && x != '?';
        let status = match (x, y) {
            ('?', '?') => FileStatus::Untracked,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => FileStatus::Conflict,
            ('A', _) | (_, 'A') => FileStatus::Added,
            ('D', _) | (_, 'D') => FileStatus::Deleted,
            ('R', _) | (_, 'R') => FileStatus::Renamed,
            _ => FileStatus::Modified,
        };
        files.push(GitFile { path, status, staged });
    }
    files
}
