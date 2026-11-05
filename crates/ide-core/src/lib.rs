//! Shared types, errors, and editor mode for tui-ide.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdeError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type IdeResult<T> = Result<T, IdeError>;

impl IdeError {
    pub fn msg(m: impl Into<String>) -> Self {
        Self::Message(m.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Command,
    Palette,
    Search,
    Rename,
    Find,
    Replace,
    Commit,
    DbConnect,
    EditCell,
    Settings,
    OpenFolder,
    /// Prompt for a CSV path to import into the active table.
    ImportCsv,
    /// Prompt for a SQL dump path to restore.
    RestoreDump,
    /// Prompt for schema diff: `left..right` (optional `db.schema`).
    SchemaDiff,
    /// Prompt for table data compare: `left_table..right_table`.
    DataCompare,
    /// Prompt for SQL query param values (`:name` / `$n`).
    QueryParams,
    /// Confirm removing a data source from `datasources.toml`.
    ConfirmDeleteSource,
    /// Confirm quit / workspace switch with dirty Neovim buffers (Wave 3).
    ConfirmQuit,
    /// Name for CREATE TEMP TABLE / VIEW from last SELECT.
    SaveMaterialize,
    /// Optional Brain RAG ask (Database studio).
    BrainAsk,
    /// Add/edit SSH or RDP session profile.
    SessionForm,
    /// Add/edit agent CLI profile.
    AgentForm,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Command => "COMMAND",
            Self::Palette => "PALETTE",
            Self::Search => "SEARCH",
            Self::Rename => "RENAME",
            Self::Find => "FIND",
            Self::Replace => "REPLACE",
            Self::Commit => "COMMIT",
            Self::DbConnect => "DB-CONNECT",
            Self::EditCell => "EDIT-CELL",
            Self::Settings => "SETTINGS",
            Self::OpenFolder => "OPEN-FOLDER",
            Self::ImportCsv => "IMPORT-CSV",
            Self::RestoreDump => "RESTORE-DUMP",
            Self::SchemaDiff => "SCHEMA-DIFF",
            Self::DataCompare => "DATA-COMPARE",
            Self::QueryParams => "QUERY-PARAMS",
            Self::ConfirmDeleteSource => "CONFIRM-DELETE",
            Self::ConfirmQuit => "CONFIRM-QUIT",
            Self::SaveMaterialize => "SAVE-RESULT",
            Self::BrainAsk => "BRAIN-ASK",
            Self::SessionForm => "SESSION",
            Self::AgentForm => "AGENT",
        }
    }
}

/// Top-level product face: code IDE vs database IDE.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProductMode {
    #[default]
    Code,
    Database,
}

/// Within Code mode: our home dashboard vs Neovim editor grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CodeSurface {
    #[default]
    Home,
    Editor,
}

/// Within Database mode: home hub vs full studio (explorer/SQL/results).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DbSurface {
    #[default]
    Home,
    Studio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Editor,
    Tree,
    Diagnostics,
    Git,
    Search,
    Terminal,
    Outline,
    Debug,
    /// Remote Sessions list (SSH/RDP profiles).
    Sessions,
    /// Agent CLI profiles (Cursor / Claude / OpenCode / custom).
    Agents,
    Database,
    /// Database studio — Structure pane (columns / indexes / grants / DDL).
    Structure,
    Results,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

impl Default for Cursor {
    fn default() -> Self {
        Self { row: 0, col: 0 }
    }
}

impl Cursor {
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

pub fn expand_path(path: impl AsRef<Path>) -> PathBuf {
    let p = path.as_ref();
    if p.starts_with("~") {
        if let Some(home) = dirs::home_dir() {
            if p == Path::new("~") {
                return home;
            }
            if let Ok(rest) = p.strip_prefix("~/") {
                return home.join(rest);
            }
        }
    }
    p.to_path_buf()
}

pub fn language_from_path(path: &Path) -> LanguageId {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name == "Dockerfile" {
        return LanguageId::Dockerfile;
    }
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => LanguageId::Rust,
        "md" | "markdown" => LanguageId::Markdown,
        "toml" => LanguageId::Toml,
        "json" | "jsonc" => LanguageId::Json,
        "sql" => LanguageId::Sql,
        "py" => LanguageId::Python,
        "ts" | "tsx" => LanguageId::TypeScript,
        "js" | "jsx" | "mjs" | "cjs" => LanguageId::JavaScript,
        "go" => LanguageId::Go,
        "c" | "h" => LanguageId::C,
        "cpp" | "cc" | "cxx" | "hpp" => LanguageId::Cpp,
        "yaml" | "yml" => LanguageId::Yaml,
        "sh" | "bash" | "zsh" => LanguageId::Shell,
        "lua" => LanguageId::Lua,
        "html" | "htm" => LanguageId::Html,
        "css" => LanguageId::Css,
        _ => LanguageId::Plain,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LanguageId {
    Rust,
    Markdown,
    Toml,
    Json,
    Sql,
    Python,
    TypeScript,
    JavaScript,
    Go,
    C,
    Cpp,
    Yaml,
    Shell,
    Lua,
    Html,
    Css,
    Dockerfile,
    Plain,
}

impl LanguageId {
    pub fn lsp_id(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Markdown => "markdown",
            Self::Toml => "toml",
            Self::Json => "json",
            Self::Sql => "sql",
            Self::Python => "python",
            Self::TypeScript => "typescript",
            Self::JavaScript => "javascript",
            Self::Go => "go",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Yaml => "yaml",
            Self::Shell => "shellscript",
            Self::Lua => "lua",
            Self::Html => "html",
            Self::Css => "css",
            Self::Dockerfile => "dockerfile",
            Self::Plain => "plaintext",
        }
    }
}

/// Bottom panel tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BottomPanel {
    #[default]
    Problems,
    Terminal,
    Debug,
    Results,
}

/// Left sidebar mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sidebar {
    #[default]
    Files,
    Git,
    Search,
    Outline,
    Debug,
    Sessions,
    Agents,
    Database,
}
