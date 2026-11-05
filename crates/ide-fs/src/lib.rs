//! Workspace file tree and path helpers.

use anyhow::Context;
use ide_core::IdeResult;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileNode {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub children: Vec<FileNode>,
    pub expanded: bool,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub tree: FileNode,
    /// Flat visible rows for the tree widget (respecting expand state).
    pub flat: Vec<FlatEntry>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct FlatEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

impl Workspace {
    pub fn open(root: impl AsRef<Path>) -> IdeResult<Self> {
        let root = root.as_ref().canonicalize().with_context(|| {
            format!("canonicalize workspace root {}", root.as_ref().display())
        })?;
        let tree = build_tree(&root, &root)?;
        let mut ws = Self {
            root,
            tree,
            flat: Vec::new(),
            selected: 0,
        };
        ws.rebuild_flat();
        Ok(ws)
    }

    pub fn rebuild_flat(&mut self) {
        self.flat.clear();
        flatten(&self.tree, 0, &mut self.flat);
        if self.selected >= self.flat.len() {
            self.selected = self.flat.len().saturating_sub(1);
        }
    }

    pub fn selected_entry(&self) -> Option<&FlatEntry> {
        self.flat.get(self.selected)
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.flat.len() {
            self.selected += 1;
        }
    }

    pub fn toggle_or_open(&mut self) -> Option<PathBuf> {
        let Some(entry) = self.flat.get(self.selected).cloned() else {
            return None;
        };
        if entry.is_dir {
            toggle_expanded(&mut self.tree, &entry.path);
            self.rebuild_flat();
            None
        } else {
            Some(entry.path)
        }
    }

    /// All file paths under the workspace (for fuzzy open).
    pub fn list_files(&self) -> IdeResult<Vec<PathBuf>> {
        list_workspace_files(&self.root)
    }

    pub fn refresh(&mut self) -> IdeResult<()> {
        let expanded = collect_expanded(&self.tree);
        self.tree = build_tree(&self.root, &self.root)?;
        restore_expanded(&mut self.tree, &expanded);
        self.rebuild_flat();
        Ok(())
    }
}

fn build_tree(root: &Path, dir: &Path) -> IdeResult<FileNode> {
    let name = if dir == root {
        root.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(".")
            .to_string()
    } else {
        dir.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    };

    let mut children = Vec::new();
    let walker = WalkBuilder::new(dir)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "target" | "node_modules" | ".git" | "dist" | "build"
            )
        })
        .build();

    for entry in walker.flatten() {
        let path = entry.path();
        if path == dir {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let child_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if is_dir {
            // Lazy: empty children until expanded; store stub
            children.push(FileNode {
                name: child_name,
                path: path.to_path_buf(),
                is_dir: true,
                children: Vec::new(),
                expanded: false,
            });
        } else {
            children.push(FileNode {
                name: child_name,
                path: path.to_path_buf(),
                is_dir: false,
                children: Vec::new(),
                expanded: false,
            });
        }
    }

    children.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(FileNode {
        name,
        path: dir.to_path_buf(),
        is_dir: true,
        children,
        expanded: true,
    })
}

fn load_dir_children(node: &mut FileNode) -> IdeResult<()> {
    if !node.is_dir || !node.children.is_empty() {
        // may already have stubs from build_tree
    }
    // Always rebuild children when expanding first time if empty after toggle from stub
    let mut children = Vec::new();
    let walker = WalkBuilder::new(&node.path)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "target" | "node_modules" | ".git" | "dist" | "build"
            )
        })
        .build();

    for entry in walker.flatten() {
        let path = entry.path();
        if path == node.path {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let child_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        children.push(FileNode {
            name: child_name,
            path: path.to_path_buf(),
            is_dir,
            children: Vec::new(),
            expanded: false,
        });
    }
    children.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    node.children = children;
    Ok(())
}

fn flatten(node: &FileNode, depth: usize, out: &mut Vec<FlatEntry>) {
    // Skip painting the absolute root as a row when depth==0? Show it.
    out.push(FlatEntry {
        path: node.path.clone(),
        name: node.name.clone(),
        is_dir: node.is_dir,
        depth,
        expanded: node.expanded,
    });
    if node.is_dir && node.expanded {
        for child in &node.children {
            flatten(child, depth + 1, out);
        }
    }
}

fn toggle_expanded(node: &mut FileNode, path: &Path) -> bool {
    if node.path == path {
        node.expanded = !node.expanded;
        if node.expanded && node.is_dir {
            let _ = load_dir_children(node);
        }
        return true;
    }
    if node.is_dir && node.expanded {
        for child in &mut node.children {
            if toggle_expanded(child, path) {
                return true;
            }
        }
    }
    false
}

/// Full recursive file list for fuzzy finder.
pub fn list_workspace_files(root: &Path) -> IdeResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !matches!(
                name.as_ref(),
                "target" | "node_modules" | ".git" | "dist" | "build"
            )
        })
        .build();
    for entry in walker.flatten() {
        let path = entry.path();
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

fn collect_expanded(node: &FileNode) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if node.is_dir && node.expanded {
        out.push(node.path.clone());
        for c in &node.children {
            out.extend(collect_expanded(c));
        }
    }
    out
}

fn restore_expanded(node: &mut FileNode, expanded: &[PathBuf]) {
    if node.is_dir && expanded.iter().any(|p| p == &node.path) {
        node.expanded = true;
        let _ = load_dir_children(node);
        for c in &mut node.children {
            restore_expanded(c, expanded);
        }
    }
}
