//! Local SQL snippets under `~/.config/tui-ide/snippets/`.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

pub fn snippets_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("tui-ide").join("snippets"))
}

pub fn list_snippets() -> Vec<(String, String)> {
    let Some(dir) = snippets_dir() else {
        return Vec::new();
    };
    let _ = fs::create_dir_all(&dir);
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(&dir) else {
        return out;
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("sql") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("snippet")
            .to_string();
        if let Ok(body) = fs::read_to_string(&path) {
            out.push((name, body));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn ensure_example_snippet() -> Result<()> {
    let dir = snippets_dir().context("no config dir")?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("select_limit.sql");
    if !path.exists() {
        fs::write(
            path,
            "-- snippet: select_limit\nSELECT * FROM $0 LIMIT 100;\n",
        )?;
    }
    Ok(())
}
