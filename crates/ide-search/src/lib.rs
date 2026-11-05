//! Project-wide text search (ripgrep-compatible via WalkBuilder + regex).

use anyhow::Result;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::task;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub selected: usize,
    pub searching: bool,
    pub error: Option<String>,
}

impl SearchState {
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.hits.len() {
            self.selected += 1;
        }
    }

    pub fn selected_hit(&self) -> Option<&SearchHit> {
        self.hits.get(self.selected)
    }
}

pub async fn search_project(root: PathBuf, query: String, max_hits: usize) -> Result<Vec<SearchHit>> {
    task::spawn_blocking(move || search_sync(&root, &query, max_hits))
        .await?
}

fn search_sync(root: &Path, query: &str, max_hits: usize) -> Result<Vec<SearchHit>> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let re = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()?;

    let mut hits = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "target" | "node_modules" | ".git" | "dist" | "build" | ".cache"
            )
        })
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        // skip likely binaries
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if let Some(m) = re.find(line) {
                hits.push(SearchHit {
                    path: path.to_path_buf(),
                    line: i + 1,
                    column: m.start() + 1,
                    text: line.chars().take(200).collect(),
                });
                if hits.len() >= max_hits {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}
