//! Database IDE core: data sources, introspection, SQL execution.
//!
//! Drivers: PostgreSQL (primary), MySQL, SQLite, MSSQL, ClickHouse, DuckDB, Redis, MongoDB, …

mod alt_engines;
mod brain_ask;
mod engines;
mod er_canvas;
mod explain_format;
mod http_engines;
mod oauth;
mod query_params;
mod query_paging;
mod result_chart;
mod schema_cache;
mod secrets;
mod snippets;

pub use brain_ask::brain_ask;
pub use er_canvas::render_er_canvas;
pub use oauth::{apply_pasted_token, authorize_url, load_persisted_tokens, open_browser};
pub use secrets::{
    has_vault_password, password_display_hint, resolve_password, store_password_for_save,
};
pub use snippets::{ensure_example_snippet, list_snippets};

pub use query_params::{query_param_prompt_seed, sql_has_query_params, substitute_query_params};
pub use query_paging::{paginate_select_sql, RESULT_PAGE_SIZE};
pub use result_chart::chart_result;
pub use schema_cache::{SchemaNameCache, SqlCompletionItem};

use anyhow::{Context, Result, bail};
use engines::{ClickhouseHttp, MssqlClient};
use chrono::Local;
use serde::{Deserialize, Serialize};
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::{Column, Executor, MySql as MySqlDb, Postgres, Row, Sqlite as SqliteDb, Statement, TypeInfo, ValueRef};
use sqlx::pool::PoolConnection;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DbDriver {
    #[default]
    Postgres,
    Mysql,
    Sqlite,
    Mssql,
    Clickhouse,
    Duckdb,
    Redis,
    Mongo,
    Snowflake,
    Bigquery,
    Elasticsearch,
    Trino,
}

impl DbDriver {
    pub fn label(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::Mysql => "MySQL",
            Self::Sqlite => "SQLite",
            Self::Mssql => "MSSQL",
            Self::Clickhouse => "ClickHouse",
            Self::Duckdb => "DuckDB",
            Self::Redis => "Redis",
            Self::Mongo => "MongoDB",
            Self::Snowflake => "Snowflake",
            Self::Bigquery => "BigQuery",
            Self::Elasticsearch => "Elasticsearch",
            Self::Trino => "Trino",
        }
    }

    pub fn uses_file_path(self) -> bool {
        matches!(self, Self::Sqlite | Self::Duckdb)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SourceRole {
    Viewer,
    Editor,
    #[default]
    Admin,
}

impl SourceRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Editor => "editor",
            Self::Admin => "admin",
        }
    }

    pub fn cycle(self, forward: bool) -> Self {
        match (self, forward) {
            (Self::Viewer, true) | (Self::Admin, false) => Self::Editor,
            (Self::Editor, true) | (Self::Viewer, false) => Self::Admin,
            (Self::Admin, true) | (Self::Editor, false) => Self::Viewer,
        }
    }

    pub fn allows_write(self) -> bool {
        !matches!(self, Self::Viewer)
    }

    pub fn allows_admin(self) -> bool {
        matches!(self, Self::Admin)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSource {
    pub id: String,
    pub name: String,
    pub driver: DbDriver,
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub database: String,
    /// SQLite file path
    #[serde(default)]
    pub file: String,
    /// Jump host for OpenSSH local forward (`ssh -L`). Empty = direct connect.
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default)]
    pub ssh_user: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    /// Optional identity file (`-i`). Empty → ssh agent / default keys.
    #[serde(default)]
    pub ssh_key: String,
    /// Postgres/MySQL/ClickHouse: `disable` · `prefer` · `require` · `verify-full`
    #[serde(default = "default_ssl_mode")]
    pub ssl_mode: String,
    /// MSSQL: trust server certificate (default true for Docker/dev).
    #[serde(default = "default_true")]
    pub ssl_trust_cert: bool,
    /// Optional PEM CA for verify modes (Postgres/MySQL/ClickHouse).
    #[serde(default)]
    pub ssl_ca: String,
    /// When true, block non-read SQL (INSERT/UPDATE/DDL/restore/…).
    #[serde(default)]
    pub read_only: bool,
    /// Local RBAC: viewer (read) · editor (DML) · admin (DDL/dump/restore).
    #[serde(default)]
    pub role: SourceRole,
    /// Explorer grouping folder label (empty = root level).
    #[serde(default)]
    pub group: String,
    /// Accent for Explorer marker: `#RRGGBB` or named (`cyan`, `green`, …).
    #[serde(default)]
    pub accent: String,
    /// Pin connection to the top of Explorer (Wave 10).
    #[serde(default)]
    pub pinned: bool,
}

fn default_ssl_mode() -> String {
    "prefer".into()
}

fn default_true() -> bool {
    true
}

fn default_ssh_port() -> u16 {
    22
}

fn default_port() -> u16 {
    5432
}

impl DataSource {
    pub fn postgres_local(name: &str, database: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            driver: DbDriver::Postgres,
            host: "127.0.0.1".into(),
            port: 5432,
            user: whoami_user(),
            password: String::new(),
            database: database.into(),
            file: String::new(),
            ssh_host: String::new(),
            ssh_user: String::new(),
            ssh_port: 22,
            ssh_key: String::new(),
            ssl_mode: default_ssl_mode(),
            ssl_trust_cert: true,
            ssl_ca: String::new(),
            read_only: false,
            role: SourceRole::default(),
            group: String::new(),
            accent: String::new(),
            pinned: false,
        }
    }

    pub fn uses_ssh_tunnel(&self) -> bool {
        !self.ssh_host.trim().is_empty()
    }

    pub fn tls_required(&self) -> bool {
        matches!(
            self.ssl_mode.as_str(),
            "require" | "verify-full" | "verify_ca"
        )
    }

    /// Password for live connect (resolves `env:VAR` / secrets.toml).
    pub fn resolved_password(&self) -> String {
        secrets::resolve_password(&self.id, &self.password)
    }

    /// Clone with password field filled from vault / env (for CLIs & engines).
    pub fn for_connect(&self) -> DataSource {
        let mut s = self.clone();
        s.password = self.resolved_password();
        s
    }

    /// Connection URL. When `local_port` is set (SSH tunnel), host is loopback.
    pub fn url_via(&self, local_port: Option<u16>) -> String {
        let pass = self.resolved_password();
        let (host, port) = match local_port {
            Some(lp) => ("127.0.0.1", lp),
            None => (self.host.as_str(), self.port),
        };
        match self.driver {
            DbDriver::Postgres => {
                let pass_part = if pass.is_empty() {
                    String::new()
                } else {
                    format!(":{}", urlencoding_lite(&pass))
                };
                let mut url = format!(
                    "postgres://{}{}@{}:{}/{}",
                    urlencoding_lite(&self.user),
                    pass_part,
                    host,
                    port,
                    self.database
                );
                append_pg_ssl_query(&mut url, &self.ssl_mode, &self.ssl_ca);
                url
            }
            DbDriver::Mysql => {
                let pass_part = if pass.is_empty() {
                    String::new()
                } else {
                    format!(":{}", urlencoding_lite(&pass))
                };
                let mut url = format!(
                    "mysql://{}{}@{}:{}/{}",
                    urlencoding_lite(&self.user),
                    pass_part,
                    host,
                    port,
                    self.database
                );
                append_mysql_ssl_query(&mut url, &self.ssl_mode, &self.ssl_ca);
                url
            }
            DbDriver::Sqlite => {
                if self.file.is_empty() || self.file == ":memory:" {
                    "sqlite::memory:".into()
                } else if self.file.starts_with('/') {
                    format!("sqlite://{}", self.file)
                } else {
                    format!("sqlite://{}", self.file)
                }
            }
            DbDriver::Mssql => {
                // Informational only — real connects use tiberius.
                format!(
                    "mssql://{}@{}:{}/{}",
                    urlencoding_lite(&self.user),
                    host,
                    port,
                    self.database
                )
            }
            DbDriver::Clickhouse => {
                let scheme = if self.tls_required() {
                    "https"
                } else {
                    "http"
                };
                format!("{scheme}://{host}:{port}/?database={}", self.database)
            }
            DbDriver::Duckdb => {
                if self.file.is_empty() || self.file == ":memory:" {
                    "duckdb://:memory:".into()
                } else {
                    format!("duckdb://{}", self.file)
                }
            }
            DbDriver::Redis => {
                format!("redis://{host}:{port}/{}", self.database)
            }
            DbDriver::Mongo => {
                format!("mongodb://{host}:{port}/{}", self.database)
            }
            DbDriver::Snowflake | DbDriver::Bigquery => self.name.clone(),
            DbDriver::Elasticsearch | DbDriver::Trino => {
                format!("{}://{}:{}/{}", self.driver.label().to_ascii_lowercase(), host, port, self.database)
            }
        }
    }

    pub fn url(&self) -> String {
        self.url_via(None)
    }
}

fn whoami_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "postgres".into())
}

fn append_pg_ssl_query(url: &mut String, ssl_mode: &str, ssl_ca: &str) {
    let mode = ssl_mode.trim();
    if mode.is_empty() || mode == "prefer" {
        return;
    }
    url.push('?');
    url.push_str("sslmode=");
    url.push_str(mode);
    if !ssl_ca.trim().is_empty() && (mode == "verify-full" || mode == "verify-ca") {
        url.push_str("&sslrootcert=");
        url.push_str(&urlencoding_lite(ssl_ca.trim()));
    }
}

fn append_mysql_ssl_query(url: &mut String, ssl_mode: &str, ssl_ca: &str) {
    let mode = match ssl_mode.trim() {
        "disable" => "DISABLED",
        "prefer" | "" => return,
        "require" => "REQUIRED",
        "verify-full" | "verify_ca" => "VERIFY_IDENTITY",
        other => other,
    };
    url.push('?');
    url.push_str("ssl-mode=");
    url.push_str(mode);
    if !ssl_ca.trim().is_empty() {
        url.push_str("&ssl-ca=");
        url.push_str(&urlencoding_lite(ssl_ca.trim()));
    }
}

fn pg_quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbNodeKind {
    /// Virtual folder grouping connections (`DataSource.group`).
    Group,
    Source,
    /// Server catalog / database (Postgres & MySQL).
    Database,
    Schema,
    Table,
    View,
    Column,
    /// Virtual folder: Admin (sessions, locks, …).
    Admin,
    /// Leaf under Admin — Enter runs `query_sql`.
    AdminQuery,
    /// Leaf under Admin — Enter runs a tool action (dump/restore).
    AdminAction,
    /// Stored procedure / function / trigger under Programmability.
    Routine,
}

#[derive(Debug, Clone)]
pub struct DbTreeNode {
    pub id: String,
    pub label: String,
    pub kind: DbNodeKind,
    pub source_id: String,
    /// Catalog / database name when nested under a server.
    pub database: Option<String>,
    pub schema: Option<String>,
    pub table: Option<String>,
    /// SQL for AdminQuery leaves.
    pub query_sql: Option<String>,
    pub children: Vec<DbTreeNode>,
    pub expanded: bool,
}

/// Column metadata for the Structure pane (not nested under the explorer tree).
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

impl ColumnInfo {
    pub fn label(&self) -> String {
        format!(
            "{}: {}{}",
            self.name,
            self.data_type,
            if self.nullable { "?" } else { "" }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureLineKind {
    Section,
    Item,
    Ddl,
}

/// Jump target for a Structure row (e.g. FK → referenced table).
#[derive(Debug, Clone)]
pub struct StructureNav {
    pub source_id: String,
    pub database: Option<String>,
    pub schema: String,
    pub table: String,
}

/// Flat Structure pane rows (Columns / Indexes / FKs / Grants / DDL).
#[derive(Debug, Clone)]
pub struct StructureLine {
    pub kind: StructureLineKind,
    pub text: String,
    pub nav: Option<StructureNav>,
}

struct ForeignKeyLine {
    label: String,
    nav: StructureNav,
}

fn group_fk_rows(
    source_id: &str,
    database: Option<String>,
    rows: impl Iterator<Item = (String, String, String, String, String)>,
) -> Vec<ForeignKeyLine> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, (String, String, Vec<String>, Vec<String>)> = BTreeMap::new();
    for (name, col, ref_schema, ref_table, ref_col) in rows {
        let entry = map
            .entry(name)
            .or_insert_with(|| (ref_schema, ref_table, Vec::new(), Vec::new()));
        entry.2.push(col);
        entry.3.push(ref_col);
    }
    map.into_iter()
        .map(|(name, (ref_schema, ref_table, cols, ref_cols))| {
            let col_list = cols.join(", ");
            let ref_col_list = ref_cols.join(", ");
            let label = format!(
                "{name} → {ref_schema}.{ref_table}({ref_col_list})  [{col_list}]"
            );
            ForeignKeyLine {
                label,
                nav: StructureNav {
                    source_id: source_id.to_string(),
                    database: database.clone(),
                    schema: ref_schema,
                    table: ref_table,
                },
            }
        })
        .collect()
}

impl StructureLine {
    fn section(text: impl Into<String>) -> Self {
        Self {
            kind: StructureLineKind::Section,
            text: text.into(),
            nav: None,
        }
    }

    fn item(text: impl Into<String>) -> Self {
        Self {
            kind: StructureLineKind::Item,
            text: text.into(),
            nav: None,
        }
    }

    fn ddl(text: impl Into<String>) -> Self {
        Self {
            kind: StructureLineKind::Ddl,
            text: text.into(),
            nav: None,
        }
    }

    fn fk(text: impl Into<String>, nav: StructureNav) -> Self {
        Self {
            kind: StructureLineKind::Item,
            text: text.into(),
            nav: Some(nav),
        }
    }
}

/// Console tab — Query scratch or a DataGrip-style table data tab.
#[derive(Debug, Clone)]
pub struct SqlTab {
    pub id: String,
    pub title: String,
    pub sql: String,
    pub result: Option<QueryResult>,
    /// When set, this tab is bound to a table (re-open focuses it).
    pub table_key: Option<String>,
    pub source_id: Option<String>,
    pub database: Option<String>,
    /// Pinned tabs refuse casual close.
    pub pinned: bool,
}

impl SqlTab {
    pub fn query(n: usize) -> Self {
        Self {
            id: format!("query-{n}"),
            title: if n <= 1 {
                "Query".into()
            } else {
                format!("Query {n}")
            },
            sql: "SELECT 1;\n".into(),
            result: None,
            table_key: None,
            source_id: None,
            database: None,
            pinned: false,
        }
    }

    pub fn display_title(&self) -> String {
        if self.pinned {
            format!("📌{}", self.title)
        } else if self
            .result
            .as_ref()
            .is_some_and(|r| !r.dirty_rows().is_empty())
        {
            format!("{}*", self.title)
        } else {
            self.title.clone()
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub affected: Option<u64>,
    pub elapsed_ms: u128,
    pub truncated: bool,
    /// Set when the query looks like `SELECT … FROM schema.table`.
    pub source_schema: Option<String>,
    pub source_table: Option<String>,
    pub primary_keys: Vec<String>,
    /// Original row snapshots for dirty detection (same shape as rows).
    pub original_rows: Vec<Vec<String>>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub explain: Option<String>,
    /// ASCII chart / sparkline text (Results footer).
    pub chart: Option<String>,
    /// Zero-based page when results were fetched with LIMIT/OFFSET.
    pub page: usize,
    /// More rows may exist beyond this page.
    pub has_more: bool,
}

/// One executed statement in persisted query history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryHistoryEntry {
    pub sql: String,
    #[serde(default)]
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostgresMaintenance {
    VacuumAnalyze,
    Analyze,
    Reindex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlterColumnKind {
    Add,
    Drop,
    Rename,
}

impl QueryHistoryEntry {
    pub fn label(&self, max: usize) -> String {
        let base = truncate_sql_label(&self.sql, max);
        if self.elapsed_ms > 0 {
            format!("{base}  ({}{})", self.elapsed_ms, "ms")
        } else {
            base
        }
    }
}

impl QueryResult {
    pub fn empty() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            affected: None,
            elapsed_ms: 0,
            truncated: false,
            source_schema: None,
            source_table: None,
            primary_keys: Vec::new(),
            original_rows: Vec::new(),
            cursor_row: 0,
            cursor_col: 0,
            explain: None,
            chart: None,
            page: 0,
            has_more: false,
        }
    }

    pub fn with_meta(mut self, schema: Option<String>, table: Option<String>, pks: Vec<String>) -> Self {
        self.original_rows = self.rows.clone();
        self.source_schema = schema;
        self.source_table = table;
        self.primary_keys = pks;
        self
    }

    pub fn move_cell(&mut self, d_row: isize, d_col: isize) {
        if self.rows.is_empty() || self.columns.is_empty() {
            return;
        }
        let nr = self.rows.len() as isize;
        let nc = self.columns.len() as isize;
        let r = (self.cursor_row as isize + d_row).clamp(0, nr - 1);
        let c = (self.cursor_col as isize + d_col).clamp(0, nc - 1);
        self.cursor_row = r as usize;
        self.cursor_col = c as usize;
    }

    pub fn set_cell(&mut self, value: String) -> Option<String> {
        let row = self.rows.get_mut(self.cursor_row)?;
        let cell = row.get_mut(self.cursor_col)?;
        let prev = cell.clone();
        *cell = value;
        Some(prev)
    }

    pub fn dirty_rows(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(i, row)| self.original_rows.get(*i) != Some(*row))
            .map(|(i, _)| i)
            .collect()
    }

    fn table_qual_sql(&self) -> Result<String> {
        let table = self
            .source_table
            .as_ref()
            .context("result has no source table — run SELECT from a single table")?;
        let schema = self.source_schema.as_deref();
        Ok(match schema {
            Some(s) if s != "main" => format!("\"{s}\".\"{table}\""),
            _ => format!("\"{table}\""),
        })
    }

    /// Build UPDATE statements for dirty *existing* rows (requires table + PK metadata).
    pub fn pending_updates(&self) -> Result<Vec<String>> {
        let qual = self.table_qual_sql()?;
        if self.primary_keys.is_empty() {
            bail!("no primary key metadata — cannot generate UPDATE");
        }
        let mut stmts = Vec::new();
        for i in self.dirty_rows() {
            // New insert rows live past original_rows — handled by pending_inserts.
            if i >= self.original_rows.len() {
                continue;
            }
            let row = &self.rows[i];
            let orig = &self.original_rows[i];
            let mut sets = Vec::new();
            for (ci, col) in self.columns.iter().enumerate() {
                if self.primary_keys.iter().any(|pk| pk == col) {
                    continue;
                }
                if row.get(ci) != orig.get(ci) {
                    let v = row.get(ci).map(|s| s.as_str()).unwrap_or("NULL");
                    let lit = sql_literal(v);
                    sets.push(format!("\"{col}\" = {lit}"));
                }
            }
            if sets.is_empty() {
                continue;
            }
            let mut wheres = Vec::new();
            for pk in &self.primary_keys {
                let Some(ci) = self.columns.iter().position(|c| c == pk) else {
                    continue;
                };
                let v = orig.get(ci).map(|s| s.as_str()).unwrap_or("NULL");
                wheres.push(format!("\"{pk}\" = {}", sql_literal(v)));
            }
            if wheres.is_empty() {
                continue;
            }
            stmts.push(format!(
                "UPDATE {qual} SET {} WHERE {};",
                sets.join(", "),
                wheres.join(" AND ")
            ));
        }
        Ok(stmts)
    }

    /// INSERT for rows appended after the original snapshot.
    pub fn pending_inserts(&self) -> Result<Vec<String>> {
        let qual = self.table_qual_sql()?;
        if self.columns.is_empty() {
            return Ok(Vec::new());
        }
        let cols = self
            .columns
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmts = Vec::new();
        for i in self.original_rows.len()..self.rows.len() {
            let row = &self.rows[i];
            let vals = (0..self.columns.len())
                .map(|ci| sql_literal(row.get(ci).map(|s| s.as_str()).unwrap_or("NULL")))
                .collect::<Vec<_>>()
                .join(", ");
            stmts.push(format!("INSERT INTO {qual} ({cols}) VALUES ({vals});"));
        }
        Ok(stmts)
    }

    pub fn delete_sql_for_original_row(&self, orig_idx: usize) -> Result<String> {
        let qual = self.table_qual_sql()?;
        if self.primary_keys.is_empty() {
            bail!("no primary key metadata — cannot generate DELETE");
        }
        let orig = self
            .original_rows
            .get(orig_idx)
            .context("row index out of range")?;
        let mut wheres = Vec::new();
        for pk in &self.primary_keys {
            let Some(ci) = self.columns.iter().position(|c| c == pk) else {
                continue;
            };
            let v = orig.get(ci).map(|s| s.as_str()).unwrap_or("NULL");
            wheres.push(format!("\"{pk}\" = {}", sql_literal(v)));
        }
        if wheres.is_empty() {
            bail!("cannot build DELETE WHERE — missing PK columns in result");
        }
        Ok(format!(
            "DELETE FROM {qual} WHERE {};",
            wheres.join(" AND ")
        ))
    }

    /// Append a blank (NULL-filled) row for editing → INSERT on Apply.
    pub fn insert_blank_row(&mut self) -> Result<()> {
        if self.source_table.is_none() {
            bail!("open a single-table SELECT first");
        }
        if self.columns.is_empty() {
            bail!("no columns");
        }
        let row = vec!["NULL".to_string(); self.columns.len()];
        self.rows.push(row);
        self.cursor_row = self.rows.len() - 1;
        self.cursor_col = 0;
        Ok(())
    }

    /// Set current cell to SQL NULL display value.
    pub fn set_cell_null(&mut self) -> Option<String> {
        self.set_cell("NULL".into())
    }

    /// Full text for wide-cell viewer (unwraps `<blob Nb: hex…>` when present).
    pub fn wide_cell_text(&self) -> Result<String> {
        let cell = self
            .rows
            .get(self.cursor_row)
            .and_then(|r| r.get(self.cursor_col))
            .context("no cell")?;
        let col = self
            .columns
            .get(self.cursor_col)
            .map(|s| s.as_str())
            .unwrap_or("?");
        Ok(format!(
            "── {}.{} · row {} ──\n{}",
            self.source_table.as_deref().unwrap_or("result"),
            col,
            self.cursor_row + 1,
            format_wide_cell_body(cell)
        ))
    }

    /// Paste TSV/CSV lines starting at cursor (extra rows become pending INSERTs).
    pub fn paste_tsv_at_cursor(&mut self, text: &str) -> Result<usize> {
        if self.columns.is_empty() {
            bail!("no result grid");
        }
        let lines = parse_paste_grid(text);
        if lines.is_empty() {
            bail!("clipboard/paste empty");
        }
        let start_row = self.cursor_row;
        let start_col = self.cursor_col;
        let mut touched = 0usize;
        for (li, cols) in lines.iter().enumerate() {
            let ri = start_row + li;
            while self.rows.len() <= ri {
                if self.source_table.is_none() {
                    bail!("cannot append rows — open a single-table SELECT first");
                }
                let row = vec!["NULL".to_string(); self.columns.len()];
                self.rows.push(row);
            }
            for (di, val) in cols.iter().enumerate() {
                let ci = start_col + di;
                if ci < self.columns.len() {
                    if let Some(cell) = self.rows[ri].get_mut(ci) {
                        *cell = val.clone();
                    }
                }
            }
            touched += 1;
        }
        self.cursor_row = (start_row + touched.saturating_sub(1)).min(self.rows.len() - 1);
        Ok(touched)
    }

    pub fn to_csv(&self) -> String {
        self.to_csv_filtered(None)
    }

    /// CSV of rows matching an optional Results `/` filter.
    pub fn to_csv_filtered(&self, filter: Option<&str>) -> String {
        let mut out = String::new();
        out.push_str(&self.columns.iter().map(|c| csv_escape(c)).collect::<Vec<_>>().join(","));
        out.push('\n');
        for row in &self.rows {
            if !row_matches_filter(row, filter.unwrap_or("")) {
                continue;
            }
            out.push_str(
                &row.iter()
                    .map(|c| csv_escape(c))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push('\n');
        }
        out
    }

    /// Generate INSERT statements for the result rows (portable-ish SQL).
    pub fn to_sql_inserts(&self, schema: Option<&str>, table: &str, mysql: bool) -> String {
        self.to_sql_inserts_filtered(schema, table, mysql, None)
    }

    pub fn to_sql_inserts_filtered(
        &self,
        schema: Option<&str>,
        table: &str,
        mysql: bool,
        filter: Option<&str>,
    ) -> String {
        let q = |id: &str| -> String {
            if mysql {
                format!("`{}`", id.replace('`', "``"))
            } else {
                format!("\"{}\"", id.replace('"', "\"\""))
            }
        };
        let qual = match schema {
            Some(s) if !s.is_empty() && s != "main" => format!("{}.{}", q(s), q(table)),
            _ => q(table),
        };
        let cols = self
            .columns
            .iter()
            .map(|c| q(c))
            .collect::<Vec<_>>()
            .join(", ");
        let mut out = String::new();
        for row in &self.rows {
            if !row_matches_filter(row, filter.unwrap_or("")) {
                continue;
            }
            let vals = row
                .iter()
                .map(|v| sql_literal(v))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("INSERT INTO {qual} ({cols}) VALUES ({vals});\n"));
        }
        out
    }

    /// JSON array of row objects (optional Results `/` filter).
    pub fn to_json_filtered(&self, filter: Option<&str>) -> String {
        let mut rows_out = Vec::new();
        for row in &self.rows {
            if !row_matches_filter(row, filter.unwrap_or("")) {
                continue;
            }
            let mut obj = String::from("{");
            for (i, col) in self.columns.iter().enumerate() {
                if i > 0 {
                    obj.push(',');
                }
                let val = row.get(i).map(|s| s.as_str()).unwrap_or("");
                obj.push_str(&format!(
                    "{}:{}",
                    json_escape_str(col),
                    json_escape_value(val)
                ));
            }
            obj.push('}');
            rows_out.push(obj);
        }
        format!("[\n  {}\n]\n", rows_out.join(",\n  "))
    }

    /// GitHub-flavored markdown table (optional Results `/` filter).
    pub fn to_markdown_filtered(&self, filter: Option<&str>) -> String {
        if self.columns.is_empty() {
            return String::new();
        }
        let esc = |s: &str| s.replace('|', "\\|").replace('\n', " ");
        let mut out = String::new();
        out.push('|');
        out.push_str(
            &self
                .columns
                .iter()
                .map(|c| format!(" {} ", esc(c)))
                .collect::<Vec<_>>()
                .join("|"),
        );
        out.push_str("|\n|");
        out.push_str(
            &self
                .columns
                .iter()
                .map(|_| " --- ")
                .collect::<Vec<_>>()
                .join("|"),
        );
        out.push_str("|\n");
        for row in &self.rows {
            if !row_matches_filter(row, filter.unwrap_or("")) {
                continue;
            }
            out.push('|');
            for i in 0..self.columns.len() {
                let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
                out.push_str(&format!(" {} |", esc(cell)));
            }
            out.push('\n');
        }
        out
    }
}

fn json_escape_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn json_escape_value(v: &str) -> String {
    if v == "NULL" {
        "null".into()
    } else if v.parse::<i64>().is_ok() || v.parse::<f64>().is_ok() {
        v.to_string()
    } else if v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("false") {
        v.to_ascii_lowercase()
    } else {
        json_escape_str(v)
    }
}

fn sql_literal(v: &str) -> String {
    let t = v.trim();
    if t.is_empty() || t == "NULL" || t.eq_ignore_ascii_case("<null>") {
        "NULL".into()
    } else {
        format!("'{}'", v.replace('\'', "''"))
    }
}

fn csv_escape(v: &str) -> String {
    if v.contains(',') || v.contains('"') || v.contains('\n') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v.to_string()
    }
}

/// Minimal RFC4180-ish CSV parser (header + rows).
fn parse_csv(text: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut field = String::new();
    let mut row: Vec<String> = Vec::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                row.push(std::mem::take(&mut field));
            }
            '\n' if !in_quotes => {
                row.push(std::mem::take(&mut field));
                if row.iter().any(|c| !c.is_empty()) || row.len() > 1 {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            '\r' if !in_quotes => {}
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(std::mem::take(&mut field));
        rows.push(row);
    }
    if rows.is_empty() {
        bail!("empty CSV");
    }
    let headers = rows.remove(0);
    // Normalize row widths to header length.
    for r in &mut rows {
        r.resize(headers.len(), String::new());
    }
    Ok((headers, rows))
}

/// Parse a JSON array of objects (or NDJSON) into header + row cells.
fn parse_json_table(text: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("empty JSON");
    }
    let values: Vec<serde_json::Value> = if trimmed.starts_with('[') {
        serde_json::from_str(trimmed).context("parse JSON array")?
    } else {
        let mut out = Vec::new();
        for line in trimmed.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            out.push(serde_json::from_str(line).context("parse NDJSON line")?);
        }
        out
    };
    if values.is_empty() {
        bail!("JSON has no rows");
    }
    let mut keys = std::collections::BTreeSet::new();
    for v in &values {
        let obj = v
            .as_object()
            .context("JSON rows must be objects")?;
        for k in obj.keys() {
            keys.insert(k.clone());
        }
    }
    let headers: Vec<String> = keys.into_iter().collect();
    let mut rows = Vec::new();
    for v in values {
        let obj = v.as_object().context("JSON row object")?;
        let mut row = Vec::with_capacity(headers.len());
        for h in &headers {
            let cell = match obj.get(h) {
                None | Some(serde_json::Value::Null) => "NULL".into(),
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Bool(b)) => b.to_string(),
                Some(serde_json::Value::Number(n)) => n.to_string(),
                Some(other) => other.to_string(),
            };
            row.push(cell);
        }
        rows.push(row);
    }
    Ok((headers, rows))
}

#[derive(Debug, Clone)]
pub struct FlatDbEntry {
    pub id: String,
    pub label: String,
    pub kind: DbNodeKind,
    pub source_id: String,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub table: Option<String>,
    pub query_sql: Option<String>,
    pub depth: usize,
    pub expanded: bool,
}

pub(crate) enum LivePool {
    Pg(PgPool),
    Mysql(MySqlPool),
    Sqlite(SqlitePool),
    Mssql(std::sync::Arc<tokio::sync::Mutex<MssqlClient>>),
    Clickhouse(ClickhouseHttp),
    Duckdb(alt_engines::DuckDbConn),
    Redis(alt_engines::RedisConn),
    Mongo(alt_engines::MongoConn),
    CloudStub(DbDriver),
    Http(http_engines::HttpSqlConn),
}

enum TxnConnection {
    Pg(PoolConnection<Postgres>),
    Mysql(PoolConnection<MySqlDb>),
    Sqlite(PoolConnection<SqliteDb>),
    /// BEGIN issued on the shared MSSQL client (Wave 11).
    Mssql,
}

pub struct DbWorkspace {
    pub sources: Vec<DataSource>,
    pub tree: Vec<DbTreeNode>,
    pub flat: Vec<FlatDbEntry>,
    pub selected: usize,
    /// First visible row in the explorer list (scroll offset).
    pub explorer_scroll: usize,
    pub active_source: Option<String>,
    /// Catalog override for the active query connection (Postgres/MySQL).
    pub active_database: Option<String>,
    pub sql: String,
    pub history: Vec<QueryHistoryEntry>,
    pub bookmarks: Vec<String>,
    /// Workspace (or config) directory for saved `.sql` query files.
    pub queries_dir: PathBuf,
    pub last_result: Option<QueryResult>,
    pub last_explain: Option<String>,
    pub status: String,
    pub tabs: Vec<SqlTab>,
    pub active_tab: usize,
    /// Structure pane — columns / indexes / grants / DDL for the table.
    pub structure: Vec<ColumnInfo>,
    pub structure_lines: Vec<StructureLine>,
    pub structure_title: String,
    pub structure_scroll: usize,
    /// Console transaction open on `txn_pool_key` (held connection; not pool auto-commit).
    pub in_txn: bool,
    pools: HashMap<String, LivePool>,
    txn_pool_key: Option<String>,
    txn_conn: Option<TxnConnection>,
    /// Active OpenSSH local-forward children keyed by source_id.
    ssh_tunnels: HashMap<String, SshTunnel>,
    pub schema_cache: SchemaNameCache,
    /// Last SELECT used for result paging (without auto LIMIT).
    pub last_page_sql: Option<String>,
    pub result_page: usize,
    pub query_cancel: Arc<AtomicBool>,
    pub query_running: bool,
    /// Soft timeout for SQL (0 = off). Set from IdeConfig.
    pub query_timeout_secs: u64,
    /// Explorer health after last expand/ping.
    pub connection_health: HashMap<String, ConnHealth>,
    /// Cell edit undo stack: (row, col, previous value).
    pub cell_undo: Vec<(usize, usize, String)>,
    /// Backend pid/id for the in-flight query connection (PG/MySQL).
    pub query_backend_pid: Arc<AtomicI64>,
    /// Recent table keys `source|schema|table` (newest first).
    pub recent_tables: Vec<String>,
    /// Optional Results row filter (substring, case-insensitive).
    pub results_filter: String,
    /// When true, Studio re-runs Health · Activity periodically (~5s).
    pub health_activity_auto_refresh: bool,
    /// Queued DELETE statements from Results row deletes (applied with Ctrl-S).
    pub pending_row_deletes: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ConnHealth {
    Ok {
        /// unix millis when last verified
        checked_at_ms: u128,
    },
    Dead(String),
    /// Previously ok; age exceeded soft TTL (still usable until re-ping).
    Stale {
        checked_at_ms: u128,
    },
}

impl ConnHealth {
    pub fn ok_now() -> Self {
        Self::Ok {
            checked_at_ms: now_millis(),
        }
    }

    /// Short mono badge for Home / Explorer.
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Ok { .. } => "ok",
            Self::Stale { .. } => "stale",
            Self::Dead(_) => "dead",
        }
    }

    /// Human error text when Dead; None otherwise.
    pub fn error_message(&self) -> Option<&str> {
        match self {
            Self::Dead(msg) => Some(msg.as_str()),
            _ => None,
        }
    }
}

fn now_millis() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

struct SshTunnel {
    local_port: u16,
    child: Child,
}

impl Default for DbWorkspace {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            tree: Vec::new(),
            flat: Vec::new(),
            selected: 0,
            explorer_scroll: 0,
            active_source: None,
            active_database: None,
            sql: "SELECT 1;\n".into(),
            history: Vec::new(),
            bookmarks: Vec::new(),
            queries_dir: default_queries_dir(),
            last_result: None,
            status: "no connection".into(),
            pools: HashMap::new(),
            ssh_tunnels: HashMap::new(),
            last_explain: None,
            tabs: vec![SqlTab::query(1)],
            active_tab: 0,
            structure: Vec::new(),
            structure_lines: Vec::new(),
            structure_title: String::new(),
            structure_scroll: 0,
            in_txn: false,
            txn_pool_key: None,
            txn_conn: None,
            schema_cache: SchemaNameCache::default(),
            last_page_sql: None,
            result_page: 0,
            query_cancel: Arc::new(AtomicBool::new(false)),
            query_running: false,
            query_timeout_secs: 120,
            connection_health: HashMap::new(),
            cell_undo: Vec::new(),
            query_backend_pid: Arc::new(AtomicI64::new(0)),
            recent_tables: Vec::new(),
            results_filter: String::new(),
            health_activity_auto_refresh: false,
            pending_row_deletes: Vec::new(),
        }
    }
}

impl Drop for DbWorkspace {
    fn drop(&mut self) {
        for (_, mut t) in self.ssh_tunnels.drain() {
            let _ = t.child.kill();
            let _ = t.child.wait();
        }
    }
}

impl DbWorkspace {
    pub fn load() -> Self {
        oauth::load_persisted_tokens();
        let mut ws = Self::default();
        if let Some(path) = sources_path() {
            if path.exists() {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(file) = toml::from_str::<SourcesFile>(&text) {
                        ws.sources = file.sources;
                        if let Some(id) = file.active_source {
                            if ws.sources.iter().any(|s| s.id == id) {
                                ws.active_source = Some(id);
                            }
                        }
                    }
                }
            }
        }
        // Fallback: pinned first, else first source (name only — connect on Enter).
        if ws.active_source.is_none() {
            if let Some(s) = ws
                .sources
                .iter()
                .find(|s| s.pinned)
                .or_else(|| ws.sources.first())
            {
                ws.active_source = Some(s.id.clone());
            }
        }
        ws.history = load_query_history();
        ws.bookmarks = load_query_bookmarks();
        ws.rebuild_tree_roots();
        ws.rebuild_flat();
        ws
    }

    pub fn save_sources(&self) -> Result<()> {
        let Some(path) = sources_path() else {
            bail!("no config dir");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut sources = self.sources.clone();
        for s in &mut sources {
            s.password = secrets::store_password_for_save(&s.id, &s.password)?;
        }
        let file = SourcesFile {
            sources,
            active_source: self.active_source.clone(),
        };
        std::fs::write(path, toml::to_string_pretty(&file)?)?;
        Ok(())
    }

    pub fn add_source(&mut self, src: DataSource) {
        self.sources.push(src);
        let _ = self.save_sources();
        // Reload password fields as stored (cleared / env:) after vault write.
        if let Some(path) = sources_path() {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(file) = toml::from_str::<SourcesFile>(&text) {
                    self.sources = file.sources;
                }
            }
        }
        self.rebuild_tree_roots();
        self.rebuild_flat();
    }

    pub fn update_source(&mut self, src: DataSource) -> Result<()> {
        let id = src.id.clone();
        let pos = self
            .sources
            .iter()
            .position(|s| s.id == id)
            .context("unknown data source")?;
        let mut src = src;
        // Empty password on edit = keep prior env:/vault (do not wipe).
        if src.password.trim().is_empty() {
            let old = self.sources[pos].password.clone();
            if old.starts_with("env:") || old.is_empty() {
                src.password = old;
            }
        }
        self.sources[pos] = src;
        self.pools.retain(|k, _| k != &id && !k.starts_with(&format!("{id}#")));
        if let Some(mut t) = self.ssh_tunnels.remove(&id) {
            let _ = t.child.kill();
        }
        let _ = self.save_sources();
        if let Some(path) = sources_path() {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(file) = toml::from_str::<SourcesFile>(&text) {
                    self.sources = file.sources;
                }
            }
        }
        self.rebuild_tree_roots();
        self.rebuild_flat();
        Ok(())
    }

    /// Remove a data source from config and drop live pools / tunnels.
    pub fn remove_source(&mut self, source_id: &str) -> Result<()> {
        let pos = self
            .sources
            .iter()
            .position(|s| s.id == source_id)
            .context("unknown data source")?;
        let name = self.sources[pos].name.clone();
        self.sources.remove(pos);
        secrets::remove_password(source_id);
        self.pools
            .retain(|k, _| k != source_id && !k.starts_with(&format!("{source_id}#")));
        self.connection_health.remove(source_id);
        if let Some(mut t) = self.ssh_tunnels.remove(source_id) {
            let _ = t.child.kill();
        }
        if self.active_source.as_deref() == Some(source_id) {
            self.active_source = None;
            self.active_database = None;
            self.last_result = None;
        }
        self.tabs.retain(|t| t.source_id.as_deref() != Some(source_id));
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len().saturating_sub(1);
        }
        self.save_sources()?;
        self.rebuild_tree_roots();
        self.rebuild_flat();
        self.status = format!("removed connection \"{name}\"");
        Ok(())
    }

    /// Clone a data source with a new id and display name.
    pub fn duplicate_source(&mut self, source_id: &str) -> Result<DataSource> {
        let src = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .cloned()
            .context("unknown data source")?;
        let orig_name = src.name.clone();
        let mut copy = src;
        copy.id = uuid::Uuid::new_v4().to_string();
        copy.name = format!("{orig_name} (copy)");
        self.add_source(copy.clone());
        self.status = format!("duplicated → \"{}\"", copy.name);
        Ok(copy)
    }

    /// ASCII histogram of recent query durations (last 40 entries with timing).
    pub fn timing_histogram_ascii(&self) -> String {
        const BUCKETS: &[u128] = &[10, 50, 100, 250, 500, 1000, 2500, 5000, 10000];
        let samples: Vec<u128> = self
            .history
            .iter()
            .rev()
            .filter(|e| e.elapsed_ms > 0)
            .take(40)
            .map(|e| e.elapsed_ms)
            .collect();
        if samples.is_empty() {
            return "no timed queries in history yet — run SQL first".into();
        }
        let mut counts = vec![0usize; BUCKETS.len() + 1];
        for ms in &samples {
            let mut placed = false;
            for (i, edge) in BUCKETS.iter().enumerate() {
                if *ms <= *edge {
                    counts[i] += 1;
                    placed = true;
                    break;
                }
            }
            if !placed {
                *counts.last_mut().unwrap() += 1;
            }
        }
        let max = counts.iter().copied().max().unwrap_or(1).max(1);
        let bar_w = 24usize;
        let mut lines = vec![format!(
            "query timing · last {} runs (ms)",
            samples.len()
        )];
        for (i, &c) in counts.iter().enumerate() {
            let label = if i < BUCKETS.len() {
                if i == 0 {
                    format!("≤{}", BUCKETS[0])
                } else {
                    format!("{}–{}", BUCKETS[i - 1] + 1, BUCKETS[i])
                }
            } else {
                format!(">{}", BUCKETS[BUCKETS.len() - 1])
            };
            let fill = (c * bar_w / max).max(if c > 0 { 1 } else { 0 });
            let bar: String = "#".repeat(fill);
            lines.push(format!("{label:>12} |{bar:<bar_w$} {c}"));
        }
        let sum: u128 = samples.iter().sum();
        let avg = sum / samples.len() as u128;
        lines.push(format!("avg {avg} ms · max {} ms", samples.iter().max().unwrap_or(&0)));
        lines.join("\n")
    }

    fn source_cfg(&self, source_id: &str) -> Option<&DataSource> {
        self.sources.iter().find(|s| s.id == source_id)
    }

    fn guard_writable(&self, source_id: &str, sql: &str) -> Result<()> {
        let Some(src) = self.source_cfg(source_id) else {
            return Ok(());
        };
        let role_blocks = !src.role.allows_write();
        let ro = src.read_only || role_blocks;
        if !ro {
            // Admin-only DDL still gated below for editor role.
            if !src.role.allows_admin() && looks_like_ddl(sql) {
                bail!(
                    "RBAC editor on \"{}\" — DDL/admin SQL requires admin role",
                    src.name
                );
            }
            return Ok(());
        }
        if src.driver == DbDriver::Redis {
            if alt_engines::is_redis_read_only(sql) {
                return Ok(());
            }
            bail!(
                "read-only/viewer \"{}\" — Redis: GET/SCAN/KEYS/PING only",
                src.name
            );
        }
        if is_read_only_sql(sql) {
            return Ok(());
        }
        bail!(
            "read-only/viewer \"{}\" — only SELECT/SHOW/EXPLAIN/WITH queries allowed",
            src.name
        );
    }

    async fn introspect_redis_keys(&self, source_id: &str) -> Result<Vec<DbTreeNode>> {
        let key = Self::pool_key(source_id, None);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Redis(r) => alt_engines::redis_key_nodes(r, source_id).await,
            _ => Ok(Vec::new()),
        }
    }

    async fn introspect_mongo_collections(
        &self,
        source_id: &str,
        database: &str,
    ) -> Result<Vec<DbTreeNode>> {
        let key = Self::pool_key(source_id, Some(database));
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Mongo(m) => alt_engines::mongo_collections(m, source_id, database).await,
            _ => Ok(Vec::new()),
        }
    }

    fn guard_writable_op(&self, source_id: &str, op: &str) -> Result<()> {
        let Some(src) = self.source_cfg(source_id) else {
            return Ok(());
        };
        if src.read_only || !src.role.allows_write() {
            bail!(
                "read-only/viewer \"{}\" — {op} is not allowed",
                src.name
            );
        }
        if !src.role.allows_admin()
            && matches!(
                op,
                "dump" | "restore" | "vacuum" | "reindex" | "schema diff apply"
            )
        {
            bail!("RBAC editor on \"{}\" — {op} requires admin", src.name);
        }
        Ok(())
    }

    fn source_tree_node(s: &DataSource) -> DbTreeNode {
        let pin = if s.pinned { "★ " } else { "" };
        DbTreeNode {
            id: s.id.clone(),
            label: format!("{pin}{} ({})", s.name, s.driver.label()),
            kind: DbNodeKind::Source,
            source_id: s.id.clone(),
            database: None,
            schema: None,
            table: None,
            query_sql: None,
            children: Vec::new(),
            expanded: false,
        }
    }

    pub fn rebuild_tree_roots(&mut self) {
        let mut groups: BTreeMap<String, Vec<&DataSource>> = BTreeMap::new();
        let mut ungrouped = Vec::new();
        for s in &self.sources {
            let g = s.group.trim();
            if g.is_empty() {
                ungrouped.push(s);
            } else {
                groups.entry(g.to_string()).or_default().push(s);
            }
        }
        ungrouped.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(a.name.cmp(&b.name)));
        self.tree.clear();
        for s in ungrouped {
            self.tree.push(Self::source_tree_node(s));
        }
        for (group_name, mut sources) in groups {
            sources.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(a.name.cmp(&b.name)));
            self.tree.push(DbTreeNode {
                id: format!("group::{group_name}"),
                label: group_name.clone(),
                kind: DbNodeKind::Group,
                source_id: String::new(),
                database: None,
                schema: None,
                table: None,
                query_sql: None,
                children: sources.iter().map(|s| Self::source_tree_node(s)).collect(),
                expanded: false,
            });
        }
    }

    /// Toggle pin on a connection; pinned sources float to the top of Explorer.
    pub fn toggle_pin_source(&mut self, source_id: &str) -> Result<bool> {
        let src = self
            .sources
            .iter_mut()
            .find(|s| s.id == source_id)
            .context("unknown source")?;
        src.pinned = !src.pinned;
        let pinned = src.pinned;
        let name = src.name.clone();
        self.save_sources()?;
        self.rebuild_tree_roots();
        self.rebuild_flat();
        self.status = if pinned {
            format!("pinned · {name}")
        } else {
            format!("unpinned · {name}")
        };
        Ok(pinned)
    }

    /// Palette items for recent query history (newest first).
    pub fn query_history_palette_items(&self) -> Vec<(String, String)> {
        self.history
            .iter()
            .rev()
            .take(60)
            .enumerate()
            .map(|(i, e)| {
                let hist_idx = self.history.len().saturating_sub(1 + i);
                (
                    e.label(56),
                    format!("hist:{hist_idx}"),
                )
            })
            .collect()
    }

    /// Load history entry `idx` into a new console tab (does not run).
    pub fn open_history_entry(&mut self, idx: usize) -> Result<()> {
        let entry = self
            .history
            .get(idx)
            .context("history entry not found")?
            .clone();
        self.flush_active_tab();
        let n = self.tabs.len() + 1;
        self.tabs.push(SqlTab {
            id: format!("history-{idx}-{}", n),
            title: format!("History · {}", truncate_sql_label(&entry.sql, 28)),
            sql: entry.sql.clone(),
            result: None,
            table_key: None,
            source_id: self.active_source.clone(),
            database: self.active_database.clone(),
            pinned: false,
        });
        self.active_tab = self.tabs.len() - 1;
        self.sql = entry.sql;
        self.last_result = None;
        self.status = "history loaded — F5 to run".into();
        Ok(())
    }

    fn pool_key(source_id: &str, database: Option<&str>) -> String {
        match database {
            Some(db) if !db.is_empty() => format!("{source_id}#{db}"),
            _ => source_id.to_string(),
        }
    }

    fn source_name(&self, source_id: &str) -> String {
        self.sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| source_id.to_string())
    }

    pub fn rebuild_flat(&mut self) {
        self.flat.clear();
        for node in &self.tree {
            flatten(node, 0, &mut self.flat);
        }
        if self.selected >= self.flat.len() {
            self.selected = self.flat.len().saturating_sub(1);
        }
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

    /// Keep the selected explorer row inside the visible window.
    pub fn ensure_explorer_visible(&mut self, view_h: usize) {
        let h = view_h.max(1);
        if self.selected < self.explorer_scroll {
            self.explorer_scroll = self.selected;
        } else if self.selected >= self.explorer_scroll + h {
            self.explorer_scroll = self.selected.saturating_sub(h.saturating_sub(1));
        }
        let max_scroll = self.flat.len().saturating_sub(h.min(self.flat.len()));
        if self.explorer_scroll > max_scroll {
            self.explorer_scroll = max_scroll;
        }
    }

    pub fn flush_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.sql = self.sql.clone();
            tab.result = self.last_result.clone();
        }
    }

    pub fn load_active_tab(&mut self) {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            self.sql = tab.sql.clone();
            self.last_result = tab.result.clone();
            if let Some(sid) = &tab.source_id {
                self.active_source = Some(sid.clone());
            }
            self.active_database = tab.database.clone();
        }
    }

    pub fn select_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.flush_active_tab();
        self.active_tab = idx;
        self.load_active_tab();
    }

    pub fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let next = (self.active_tab + 1) % self.tabs.len();
        self.select_tab(next);
    }

    pub fn prev_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let prev = if self.active_tab == 0 {
            self.tabs.len() - 1
        } else {
            self.active_tab - 1
        };
        self.select_tab(prev);
    }

    pub fn new_query_tab(&mut self) {
        self.flush_active_tab();
        let n = self
            .tabs
            .iter()
            .filter(|t| t.table_key.is_none())
            .count()
            .saturating_add(1);
        self.tabs.push(SqlTab::query(n));
        self.active_tab = self.tabs.len() - 1;
        self.load_active_tab();
        self.status = format!("new tab · {}", self.tabs[self.active_tab].title);
    }

    /// After Home→Studio: ensure a console tab and a calm starter buffer when still default.
    pub fn prepare_studio_entry(&mut self) {
        if self.tabs.is_empty() {
            self.tabs.push(SqlTab::query(1));
            self.active_tab = 0;
            self.load_active_tab();
        }
        let trimmed = self.sql.trim();
        let is_default = trimmed.is_empty() || trimmed == "SELECT 1;" || trimmed == "SELECT 1";
        if !is_default || self.last_result.is_some() {
            if let Some(tab) = self.tabs.get_mut(self.active_tab) {
                if tab.source_id.is_none() {
                    tab.source_id = self.active_source.clone();
                }
            }
            return;
        }
        let (name, driver) = self
            .active_source
            .as_ref()
            .and_then(|id| self.sources.iter().find(|s| &s.id == id))
            .map(|s| (s.name.as_str(), s.driver.label()))
            .unwrap_or(("database", "SQL"));
        self.sql = format!(
            "-- {name} ({driver}) · connected\n-- Explorer: Enter on a table · F5 / Ctrl-↵ run\nSELECT 1 AS ok;\n"
        );
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.sql = self.sql.clone();
            tab.source_id = self.active_source.clone();
            tab.database = self.active_database.clone();
        }
    }

    /// Close the active SQL tab (keeps at least one). Returns true if closed.
    pub fn close_active_tab(&mut self) -> bool {
        if self.tabs.len() <= 1 {
            self.status = "need at least one console tab".into();
            return false;
        }
        if self.tabs.get(self.active_tab).is_some_and(|t| t.pinned) {
            self.status = "tab is pinned — unpin first (palette)".into();
            return false;
        }
        if self
            .tabs
            .get(self.active_tab)
            .and_then(|t| t.result.as_ref())
            .is_some_and(|r| !r.dirty_rows().is_empty())
        {
            self.status = "tab has dirty cells — Apply or Undo first".into();
            return false;
        }
        let idx = self.active_tab;
        self.tabs.remove(idx);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.load_active_tab();
        true
    }

    pub fn toggle_pin_active_tab(&mut self) -> bool {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return false;
        };
        tab.pinned = !tab.pinned;
        self.status = if tab.pinned {
            format!("pinned · {}", tab.title)
        } else {
            format!("unpinned · {}", tab.title)
        };
        true
    }

    pub fn rename_active_tab(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            self.status = "empty tab name".into();
            return false;
        }
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return false;
        };
        tab.title = name.chars().take(40).collect();
        self.status = format!("renamed · {}", tab.title);
        true
    }

    /// Palette rows for connection fuzzy search: (label, `conn:{id}`).
    pub fn connection_search_items(&self) -> Vec<(String, String)> {
        self.sources
            .iter()
            .map(|s| {
                let group = if s.group.trim().is_empty() {
                    String::new()
                } else {
                    format!(" · {}", s.group.trim())
                };
                let host = if s.driver.uses_file_path() {
                    s.file.clone()
                } else {
                    format!("{}:{}", s.host, s.port)
                };
                let pin = if s.pinned { "★ " } else { "" };
                let label = format!("{pin}{}{group} · {} · {host}", s.name, s.driver.label());
                (label, format!("conn:{}", s.id))
            })
            .collect()
    }

    pub fn studio_help_text() -> &'static str {
        "══ Database Studio v1.0 MVP (Waves 3–17) ══\n\
         \n\
         Run / cancel\n\
           F5 / Ctrl-Enter     run SQL\n\
           Ctrl-E              EXPLAIN (PG structured tree)\n\
           Esc                 cancel running query\n\
           :name / @name / $n  prompt values before run\n\
         \n\
         Results\n\
           /                   filter rows\n\
           i / Enter           edit cell\n\
           u                   undo cell\n\
           a / Ctrl-S          apply updates (confirms)\n\
           [ ]                 result pages\n\
           palette Chart       ASCII spark/bars from numeric col\n\
           Export MD/JSON/CSV  honors /\n\
         \n\
         Tabs\n\
           t / +               new query tab\n\
           x / Ctrl-W          close tab (not if pinned/dirty)\n\
           Alt-1..9            jump tab\n\
           Ctrl-Tab            next tab\n\
         \n\
         Connections\n\
           Ctrl-T / F6         test connection (form)\n\
           palette             Find connection / health / OAuth / snippets\n\
           Fan-out SQL         run console SQL on all ★ pinned sources\n\
         \n\
         Health (Admin → Health)\n\
           PG / MySQL / CH / MSSQL   Overview · Activity · (+ engine views)\n\
           palette                   Health overview / Activity (active driver)\n\
           palette                   Toggle Health Activity auto-refresh (~5s)\n\
           K on Activity             Kill session (pid / id / session_id)\n\
           MSSQL                     Agent Jobs · Index frag (under Health)\n\
         \n\
         Objects & scripting (Wave 10)\n\
           palette Script object     CREATE for table/view/proc → new tab\n\
           Programmability           procs · functions · triggers under schema\n\
           Enter on routine          script DDL into console\n\
           palette Query history     fuzzy search · Enter loads SQL (F5 run)\n\
           palette Pin connection    favorites float to top of Explorer\n\
           Admin → Backup history    MSSQL msdb · PG archiver\n\
         \n\
         Migrate & maintain (Wave 11)\n\
           Schema diff → migration   save queries/migrate_*.sql (no auto-apply)\n\
           DROP / TRUNCATE           palette · confirm before run\n\
           Structure STATS           approx rows · size\n\
           palette Format SQL        light pretty-print\n\
           Export Results JSON       honors /\n\
           Connection pack           export/import (env:/empty only)\n\
           MSSQL TXN                 Begin / Commit / Rollback\n\
         \n\
         Data ops & cloud (Wave 12)\n\
           Data compare              left..right table row diffs\n\
           Import CSV/JSON           into selected/open table\n\
           ALTER ADD/DROP/RENAME     column helpers · confirm\n\
           Fan-out SQL               pinned connections\n\
           Snowflake                 SHOW warehouses/databases/schemas/tables\n\
         \n\
         Maintain & visualize (Wave 13)\n\
           Vacuum / Analyze / Reindex  PG · SQLite VACUUM/ANALYZE\n\
           Timing histogram          recent query durations\n\
           Find table/view           explorer object search\n\
           Structure GRANTS          when engine supports\n\
         \n\
         Data editor (Wave 14)\n\
           Insert row / Delete row   Results · Ctrl-S apply mutations\n\
           Toggle cell NULL          empty / NULL literal on Apply\n\
         \n\
         Migration runner (Wave 15–19)\n\
           Dry-run / Apply / Status  versioned migrate_NNNN + journal\n\
           Rollback last             runs sibling .down.sql (fill stub first)\n\
         \n\
         Grid power (Wave 18)\n\
           View wide / blob cell     hex + utf8 preview\n\
           Paste TSV                 multi-row into Results from cursor\n\
         \n\
         Engine parity (Wave 20)\n\
           Redis EXPIRE/DEL/DBSIZE/INFO · Mongo count/indexes · CH parts\n\
         \n\
         Cloud close (Wave 21)\n\
           BQ INFORMATION_SCHEMA.TABLES · Snowflake GET_DDL script hint\n\
         \n\
         ══ Gap slice CLOSED (W18–21) ══\n\
           DBeaver/DataGrip/Navicat parity stop — not infinite clone.\n\
           New DB work only for regressions or explicit requests.\n\
         \n\
         Secrets: env:VAR or ~/.config/tui-ide/secrets.toml\n\
         OAuth tokens: ~/.config/tui-ide/oauth.toml\n"
    }

    /// Toggle periodic re-run of Health · Activity (~5s in the UI loop).
    pub fn toggle_health_activity_auto_refresh(&mut self) -> bool {
        self.health_activity_auto_refresh = !self.health_activity_auto_refresh;
        self.status = if self.health_activity_auto_refresh {
            "Health Activity auto-refresh ON (~5s)".into()
        } else {
            "Health Activity auto-refresh OFF".into()
        };
        self.health_activity_auto_refresh
    }

    /// True when the active SQL tab is a Health · Activity admin query.
    pub fn active_tab_is_health_activity(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .map(|t| t.id.contains("health-activity") || t.title.contains("Health · Activity"))
            .unwrap_or(false)
    }

    /// Open a canned Health query for the active (or first matching) engine.
    pub async fn open_health(&mut self, which: &str) -> Result<String> {
        let source_id = self
            .active_source
            .clone()
            .or_else(|| {
                self.sources
                    .iter()
                    .find(|s| {
                        matches!(
                            s.driver,
                            DbDriver::Postgres
                                | DbDriver::Mysql
                                | DbDriver::Mssql
                                | DbDriver::Clickhouse
                        )
                    })
                    .map(|s| s.id.clone())
            })
            .context("connect a Postgres/MySQL/MSSQL/ClickHouse source first")?;
        let driver = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| s.driver)
            .context("missing source")?;
        let (slug, title, sql) = health_view_for(driver, which)?;
        self.ensure_connected(&source_id, None).await?;
        let tab_key = format!("admin::{source_id}::{slug}");
        self.flush_active_tab();
        if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_key) {
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.title = title.into();
                tab.sql = sql.into();
                tab.source_id = Some(source_id.clone());
            }
        } else {
            self.tabs.push(SqlTab {
                id: tab_key,
                title: title.into(),
                sql: sql.into(),
                result: None,
                table_key: None,
                source_id: Some(source_id.clone()),
                database: None,
                pinned: false,
            });
            self.active_tab = self.tabs.len() - 1;
        }
        self.active_source = Some(source_id);
        self.sql = sql.into();
        self.last_result = None;
        self.status = format!("{driver:?} {title}");
        Ok(sql.to_string())
    }

    /// Script selected table/view/routine as CREATE into a new SQL tab.
    pub async fn script_object_at_selection(&mut self) -> Result<()> {
        let Some(entry) = self.flat.get(self.selected).cloned() else {
            bail!("nothing selected");
        };
        match entry.kind {
            DbNodeKind::Table | DbNodeKind::View => {
                let schema = entry.schema.clone().context("missing schema")?;
                let table = entry.table.clone().context("missing table")?;
                let db = entry.database.clone();
                self.ensure_connected(&entry.source_id, db.as_deref())
                    .await?;
                let cols = self
                    .introspect_column_info(&entry.source_id, db.as_deref(), &schema, &table)
                    .await
                    .unwrap_or_default();
                let indexes = self
                    .introspect_indexes(&entry.source_id, db.as_deref(), &schema, &table)
                    .await
                    .unwrap_or_default();
                let ddl = self
                    .introspect_ddl(
                        &entry.source_id,
                        db.as_deref(),
                        &schema,
                        &table,
                        &cols,
                        &indexes,
                    )
                    .await
                    .unwrap_or_else(|e| format!("-- DDL unavailable: {e:#}"));
                self.open_script_tab(
                    &entry.source_id,
                    db.as_deref(),
                    &format!("Script · {schema}.{table}"),
                    &ddl,
                );
                Ok(())
            }
            DbNodeKind::Routine => self.script_routine_at_selection().await,
            _ => bail!("select a table, view, or routine to script"),
        }
    }

    /// Script selected routine (proc/function/trigger) into a new SQL tab.
    pub async fn script_routine_at_selection(&mut self) -> Result<()> {
        let Some(entry) = self.flat.get(self.selected).cloned() else {
            bail!("nothing selected");
        };
        if !matches!(entry.kind, DbNodeKind::Routine) {
            bail!("select a routine");
        }
        let schema = entry.schema.clone().unwrap_or_default();
        let name = entry.table.clone().context("missing routine name")?;
        let kind = entry
            .query_sql
            .clone()
            .unwrap_or_else(|| "FUNCTION".into());
        let db = entry.database.clone();
        self.ensure_connected(&entry.source_id, db.as_deref())
            .await?;
        let ddl = self
            .fetch_routine_ddl(&entry.source_id, db.as_deref(), &schema, &name, &kind)
            .await?;
        self.open_script_tab(
            &entry.source_id,
            db.as_deref(),
            &format!("Script · {kind} {schema}.{name}"),
            &ddl,
        );
        Ok(())
    }

    fn open_script_tab(
        &mut self,
        source_id: &str,
        database: Option<&str>,
        title: &str,
        ddl: &str,
    ) {
        self.flush_active_tab();
        let tab_key = format!(
            "script::{source_id}::{}::{}",
            database.unwrap_or(""),
            title
        );
        if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_key) {
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.sql = ddl.into();
                tab.title = title.into();
            }
        } else {
            self.tabs.push(SqlTab {
                id: tab_key,
                title: title.into(),
                sql: ddl.into(),
                result: None,
                table_key: None,
                source_id: Some(source_id.into()),
                database: database.map(|s| s.to_string()),
                pinned: false,
            });
            self.active_tab = self.tabs.len() - 1;
        }
        self.active_source = Some(source_id.into());
        if let Some(db) = database {
            self.active_database = Some(db.into());
        }
        self.sql = ddl.into();
        self.last_result = None;
        self.status = format!("scripted · {title}");
    }

    async fn fetch_routine_ddl(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        name: &str,
        kind: &str,
    ) -> Result<String> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        let kind_u = kind.to_ascii_uppercase();
        match pool {
            LivePool::Pg(p) => {
                if kind_u.contains("TRIGGER") {
                    let def: Option<String> = sqlx::query_scalar(
                        "SELECT pg_get_triggerdef(t.oid, true) \
                         FROM pg_trigger t \
                         JOIN pg_class c ON c.oid = t.tgrelid \
                         JOIN pg_namespace n ON n.oid = c.relnamespace \
                         WHERE NOT t.tgisinternal AND n.nspname = $1 AND t.tgname = $2 \
                         LIMIT 1",
                    )
                    .bind(schema)
                    .bind(name)
                    .fetch_optional(p)
                    .await?;
                    Ok(def.unwrap_or_else(|| format!("-- trigger {schema}.{name} not found")))
                } else {
                    let def: Option<String> = sqlx::query_scalar(
                        "SELECT pg_get_functiondef(p.oid) \
                         FROM pg_proc p \
                         JOIN pg_namespace n ON n.oid = p.pronamespace \
                         WHERE n.nspname = $1 AND p.proname = $2 \
                         ORDER BY p.oid LIMIT 1",
                    )
                    .bind(schema)
                    .bind(name)
                    .fetch_optional(p)
                    .await?;
                    Ok(def.unwrap_or_else(|| {
                        format!("-- routine {schema}.{name} not found (or no privilege)")
                    }))
                }
            }
            LivePool::Mysql(p) => {
                let db = database.unwrap_or(schema);
                let sql = if kind_u.contains("TRIGGER") {
                    format!("SHOW CREATE TRIGGER `{db}`.`{name}`")
                } else if kind_u.contains("PROC") {
                    format!("SHOW CREATE PROCEDURE `{db}`.`{name}`")
                } else {
                    format!("SHOW CREATE FUNCTION `{db}`.`{name}`")
                };
                let row = sqlx::query(&sql).fetch_optional(p).await?;
                if let Some(r) = row {
                    // Column 2 usually holds the CREATE text.
                    let ddl: String = r
                        .try_get(2)
                        .or_else(|_| r.try_get(1))
                        .unwrap_or_else(|_| format!("-- empty CREATE for {name}"));
                    Ok(ddl)
                } else {
                    Ok(format!("-- routine {db}.{name} not found"))
                }
            }
            LivePool::Mssql(c) => {
                let ddl = engines::mssql_show_create(c, database, schema, name).await?;
                if ddl.is_empty() {
                    let esc_s = schema.replace('\'', "''");
                    let esc_n = name.replace('\'', "''");
                    let qr = engines::exec_mssql(
                        c,
                        &format!(
                            "SELECT m.definition FROM sys.sql_modules m \
                             JOIN sys.objects o ON m.object_id = o.object_id \
                             JOIN sys.schemas s ON o.schema_id = s.schema_id \
                             WHERE s.name = N'{esc_s}' AND o.name = N'{esc_n}'"
                        ),
                    )
                    .await?;
                    Ok(qr
                        .rows
                        .first()
                        .and_then(|r| r.first().cloned())
                        .filter(|s| !s.is_empty() && s != "NULL")
                        .unwrap_or_else(|| {
                            format!("-- definition empty for {schema}.{name}")
                        }))
                } else {
                    Ok(ddl)
                }
            }
            _ => bail!("scripting routines not supported for this driver"),
        }
    }

    /// Backward-compatible alias (MSSQL-only).
    pub async fn open_mssql_health(&mut self, which: &str) -> Result<String> {
        let source_id = self
            .active_source
            .clone()
            .or_else(|| {
                self.sources
                    .iter()
                    .find(|s| s.driver == DbDriver::Mssql)
                    .map(|s| s.id.clone())
            })
            .context("connect an MSSQL source first")?;
        let src = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .context("missing source")?;
        if src.driver != DbDriver::Mssql {
            bail!("active source is not MSSQL");
        }
        self.active_source = Some(source_id);
        self.open_health(which).await
    }

    /// Open (or focus) an Admin query tab and load its SQL. Returns true → caller should run.
    pub async fn open_admin_query_tab(&mut self) -> Result<bool> {
        let Some(entry) = self.flat.get(self.selected).cloned() else {
            return Ok(false);
        };
        if !matches!(entry.kind, DbNodeKind::AdminQuery) {
            return Ok(false);
        }
        let sql = entry
            .query_sql
            .clone()
            .context("admin query missing SQL")?;
        self.ensure_connected(&entry.source_id, None).await?;

        let tab_key = format!("admin::{}::{}", entry.source_id, entry.label);
        self.flush_active_tab();
        if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_key) {
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.sql = sql.clone();
                tab.source_id = Some(entry.source_id.clone());
                tab.database = None;
            }
            self.sql = sql;
            self.last_result = None;
        } else {
            self.tabs.push(SqlTab {
                id: tab_key,
                title: format!("Admin · {}", entry.label),
                sql: sql.clone(),
                result: None,
                table_key: None,
                source_id: Some(entry.source_id.clone()),
                database: None,
                pinned: false,
            });
            self.active_tab = self.tabs.len() - 1;
            self.sql = sql;
            self.last_result = None;
        }
        self.structure_title = format!("Admin · {}", entry.label);
        self.structure_lines = vec![
            StructureLine::section("ADMIN QUERY"),
            StructureLine::item(entry.label.clone()),
            StructureLine::ddl("Enter / F5 runs the canned admin SELECT."),
        ];
        self.structure_scroll = 0;
        self.status = format!("admin: {}", entry.label);
        Ok(true)
    }

    /// DataGrip-style: open (or focus) a table data tab. Does not expand columns in-tree.
    /// Returns true when the caller should run the SQL.
    pub async fn open_selected_table_tab(&mut self) -> Result<bool> {
        let Some(entry) = self.flat.get(self.selected).cloned() else {
            return Ok(false);
        };
        if !matches!(entry.kind, DbNodeKind::Table | DbNodeKind::View) {
            return Ok(false);
        }
        let schema = entry.schema.clone().context("missing schema")?;
        let table = entry.table.clone().context("missing table")?;
        let db = entry.database.clone();
        self.ensure_connected(&entry.source_id, db.as_deref())
            .await?;
        self.load_structure_for(&entry.source_id, db.as_deref(), &schema, &table)
            .await?;

        let table_key = format!(
            "{}::{}::{}.{}",
            entry.source_id,
            db.as_deref().unwrap_or(""),
            schema,
            table
        );
        let driver = self
            .sources
            .iter()
            .find(|s| s.id == entry.source_id)
            .map(|s| s.driver)
            .unwrap_or_default();
        let sql = match driver {
            DbDriver::Redis => format!("GET {table}\n"),
            DbDriver::Mongo => format!("find {table}\n"),
            _ => format!("SELECT * FROM \"{schema}\".\"{table}\" LIMIT 100;\n"),
        };

        self.flush_active_tab();
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.table_key.as_deref() == Some(table_key.as_str()))
        {
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.sql = sql.clone();
                tab.source_id = Some(entry.source_id.clone());
                tab.database = db.clone();
            }
            self.sql = sql;
            self.last_result = None;
        } else {
            self.tabs.push(SqlTab {
                id: table_key.clone(),
                title: table.clone(),
                sql: sql.clone(),
                result: None,
                table_key: Some(table_key),
                source_id: Some(entry.source_id.clone()),
                database: db.clone(),
                pinned: false,
            });
            self.active_tab = self.tabs.len() - 1;
            self.sql = sql;
            self.last_result = None;
        }
        self.status = format!("opened {schema}.{table}");
        Ok(true)
    }

    pub async fn refresh_structure_for_selection(&mut self) -> Result<()> {
        let Some(entry) = self.flat.get(self.selected).cloned() else {
            self.structure.clear();
            self.structure_lines.clear();
            self.structure_title.clear();
            self.structure_scroll = 0;
            return Ok(());
        };
        match entry.kind {
            DbNodeKind::Table | DbNodeKind::View => {
                let schema = entry.schema.as_deref().unwrap_or("");
                let table = entry.table.as_deref().unwrap_or("");
                if schema.is_empty() || table.is_empty() {
                    return Ok(());
                }
                let db = entry.database.as_deref();
                self.ensure_connected(&entry.source_id, db).await?;
                self.load_structure_for(&entry.source_id, db, schema, table)
                    .await?;
            }
            DbNodeKind::Column => {
                // Keep current structure when walking columns (legacy trees).
            }
            _ => {
                // Keep last structure when navigating parents so the pane stays useful.
            }
        }
        Ok(())
    }

    /// Refresh Structure from the active SQL tab when it is bound to a table.
    pub async fn refresh_structure_for_active_tab(&mut self) -> Result<()> {
        let Some(tab) = self.tabs.get(self.active_tab).cloned() else {
            return Ok(());
        };
        let Some(key) = tab.table_key.as_deref() else {
            return Ok(());
        };
        // key = "{source}::{db}::{schema}.{table}"
        let parts: Vec<&str> = key.splitn(3, "::").collect();
        if parts.len() != 3 {
            return Ok(());
        }
        let source_id = parts[0];
        let database = if parts[1].is_empty() {
            None
        } else {
            Some(parts[1])
        };
        let (schema, table) = parts[2].split_once('.').unwrap_or(("public", parts[2]));
        self.ensure_connected(source_id, database).await?;
        self.load_structure_for(source_id, database, schema, table)
            .await
    }

    async fn load_structure_for(
        &mut self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
    ) -> Result<()> {
        let cols = self
            .introspect_column_info(source_id, database, schema, table)
            .await?;
        let indexes = self
            .introspect_indexes(source_id, database, schema, table)
            .await
            .unwrap_or_default();
        let grants = self
            .introspect_grants(source_id, database, schema, table)
            .await
            .unwrap_or_default();
        let ddl = self
            .introspect_ddl(source_id, database, schema, table, &cols, &indexes)
            .await
            .unwrap_or_else(|_| String::from("-- DDL unavailable"));
        let fks = self
            .introspect_foreign_keys(source_id, database, schema, table)
            .await
            .unwrap_or_default();
        let triggers = self
            .introspect_triggers(source_id, database, schema, table)
            .await
            .unwrap_or_default();
        let routines = self
            .introspect_routines(source_id, database, schema)
            .await
            .unwrap_or_default();
        let stats = self
            .introspect_table_stats(source_id, database, schema, table)
            .await
            .unwrap_or_default();

        self.structure = cols.clone();
        self.structure_title = if let Some(db) = database {
            format!("{db} · {schema}.{table}")
        } else {
            format!("{schema}.{table}")
        };
        self.structure_scroll = 0;

        let mut lines = Vec::new();
        if !stats.is_empty() {
            lines.push(StructureLine::section("STATS"));
            for s in &stats {
                lines.push(StructureLine::item(s.clone()));
            }
        }
        lines.push(StructureLine::section(format!("COLUMNS ({})", cols.len())));
        for c in &cols {
            lines.push(StructureLine::item(c.label()));
        }
        lines.push(StructureLine::section(format!("INDEXES ({})", indexes.len())));
        if indexes.is_empty() {
            lines.push(StructureLine::item("(none)"));
        } else {
            for ix in &indexes {
                // Prefer "name" then DDL on the next dim line (avoids one huge ·-joined row).
                if let Some((name, def)) = ix.split_once(" · ") {
                    lines.push(StructureLine::item(name.to_string()));
                    lines.push(StructureLine::ddl(def.to_string()));
                } else {
                    lines.push(StructureLine::item(ix.clone()));
                }
            }
        }
        lines.push(StructureLine::section(format!(
            "FOREIGN KEYS ({})",
            fks.len()
        )));
        if fks.is_empty() {
            lines.push(StructureLine::item("(none)"));
        } else {
            for fk in fks {
                lines.push(StructureLine::fk(fk.label, fk.nav));
            }
        }
        lines.push(StructureLine::section(format!(
            "TRIGGERS ({})",
            triggers.len()
        )));
        if triggers.is_empty() {
            lines.push(StructureLine::item("(none)"));
        } else {
            for t in &triggers {
                if let Some((name, def)) = t.split_once(" · ") {
                    lines.push(StructureLine::item(name.to_string()));
                    lines.push(StructureLine::ddl(def.to_string()));
                } else {
                    lines.push(StructureLine::item(t.clone()));
                }
            }
        }
        lines.push(StructureLine::section(format!(
            "ROUTINES ({})",
            routines.len()
        )));
        if routines.is_empty() {
            lines.push(StructureLine::item("(none)"));
        } else {
            for r in &routines {
                lines.push(StructureLine::item(r.clone()));
            }
        }
        lines.push(StructureLine::section(format!("GRANTS ({})", grants.len())));
        if grants.is_empty() {
            lines.push(StructureLine::ddl("-- none / not visible"));
        } else {
            for g in &grants {
                lines.push(StructureLine::ddl(g.clone()));
            }
        }
        lines.push(StructureLine::section("DDL"));
        for line in ddl.lines() {
            lines.push(StructureLine::ddl(line.to_string()));
        }
        self.remember_table(source_id, schema, table);
        // Wave 4/5: ER sketch + canvas for the active schema (best-effort).
        if let Ok(sketch) = self.er_sketch_for_schema(source_id, database, schema).await {
            lines.push(StructureLine::section("ER"));
            for line in sketch.lines().take(40) {
                lines.push(StructureLine::ddl(line.to_string()));
            }
            let edges = parse_er_edges(&sketch);
            let canvas = render_er_canvas(schema, &edges);
            lines.push(StructureLine::section("ER CANVAS"));
            for line in canvas.lines().take(60) {
                lines.push(StructureLine::ddl(line.to_string()));
            }
        }
        self.structure_lines = lines;
        Ok(())
    }

    async fn introspect_indexes(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT indexname, indexdef FROM pg_indexes \
                     WHERE schemaname = $1 AND tablename = $2 ORDER BY 1",
                )
                .bind(schema)
                .bind(table)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("indexname");
                        let def: String = r.get("indexdef");
                        format!("{name} · {def}")
                    })
                    .collect())
            }
            LivePool::Mysql(p) => {
                let q = format!("SHOW INDEX FROM `{schema}`.`{table}`");
                let rows = sqlx::query(&q).fetch_all(p).await?;
                let mut out = Vec::new();
                for r in rows {
                    let name: String = r.try_get("Key_name").unwrap_or_default();
                    let col: String = r.try_get("Column_name").unwrap_or_default();
                    let non_unique: i64 = r.try_get("Non_unique").unwrap_or(1);
                    let kind = if non_unique == 0 { "UNIQUE" } else { "INDEX" };
                    out.push(format!("{kind} {name} ({col})"));
                }
                out.sort();
                out.dedup();
                Ok(out)
            }
            LivePool::Sqlite(p) => {
                let q = format!("PRAGMA index_list('{table}')");
                let rows = sqlx::query(&q).fetch_all(p).await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| r.try_get::<String, _>(1).ok())
                    .collect())
            }
            LivePool::Mssql(_)
            | LivePool::Clickhouse(_)
            | LivePool::Duckdb(_)
            | LivePool::Redis(_)
            | LivePool::Mongo(_)
            | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(Vec::new()),
        }
    }

    async fn introspect_foreign_keys(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
    ) -> Result<Vec<ForeignKeyLine>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        let db_owned = database.map(|s| s.to_string());
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT tc.constraint_name, kcu.column_name, \
                            ccu.table_schema AS ref_schema, ccu.table_name AS ref_table, \
                            ccu.column_name AS ref_column \
                     FROM information_schema.table_constraints tc \
                     JOIN information_schema.key_column_usage kcu \
                       ON tc.constraint_name = kcu.constraint_name \
                      AND tc.table_schema = kcu.table_schema \
                     JOIN information_schema.constraint_column_usage ccu \
                       ON ccu.constraint_name = tc.constraint_name \
                      AND ccu.table_schema = tc.table_schema \
                     WHERE tc.constraint_type = 'FOREIGN KEY' \
                       AND tc.table_schema = $1 AND tc.table_name = $2 \
                     ORDER BY tc.constraint_name, kcu.ordinal_position",
                )
                .bind(schema)
                .bind(table)
                .fetch_all(p)
                .await?;
                Ok(group_fk_rows(
                    source_id,
                    db_owned.clone(),
                    rows.into_iter().map(|r| {
                        (
                            r.get::<String, _>("constraint_name"),
                            r.get::<String, _>("column_name"),
                            r.get::<String, _>("ref_schema"),
                            r.get::<String, _>("ref_table"),
                            r.get::<String, _>("ref_column"),
                        )
                    }),
                ))
            }
            LivePool::Mysql(p) => {
                let db = database.context("mysql FK introspection needs active database")?;
                let rows = sqlx::query(
                    "SELECT CONSTRAINT_NAME, COLUMN_NAME, \
                            REFERENCED_TABLE_SCHEMA, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
                     FROM information_schema.KEY_COLUMN_USAGE \
                     WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? \
                       AND REFERENCED_TABLE_NAME IS NOT NULL \
                     ORDER BY CONSTRAINT_NAME, ORDINAL_POSITION",
                )
                .bind(db)
                .bind(table)
                .fetch_all(p)
                .await?;
                Ok(group_fk_rows(
                    source_id,
                    db_owned.clone(),
                    rows.into_iter().filter_map(|r| {
                        Some((
                            r.try_get(0).ok()?,
                            r.try_get(1).ok()?,
                            r.try_get(2).ok()?,
                            r.try_get(3).ok()?,
                            r.try_get(4).ok()?,
                        ))
                    }),
                ))
            }
            LivePool::Sqlite(p) => {
                let rows = sqlx::query(&format!("PRAGMA foreign_key_list('{table}')"))
                    .fetch_all(p)
                    .await?;
                let mut out = Vec::new();
                for r in rows {
                    let id: i64 = r.try_get(0).unwrap_or(0);
                    let _seq: i64 = r.try_get(1).unwrap_or(0);
                    let ref_table: String = r.try_get(2).unwrap_or_default();
                    let from_col: String = r.try_get(3).unwrap_or_default();
                    let ref_col: String = r.try_get(4).unwrap_or_default();
                    let name = format!("fk_{table}_{id}");
                    let ref_schema = schema.to_string();
                    let label = format!(
                        "{name} → {ref_schema}.{ref_table}({ref_col})  [{from_col}]"
                    );
                    out.push(ForeignKeyLine {
                        label,
                        nav: StructureNav {
                            source_id: source_id.to_string(),
                            database: db_owned.clone(),
                            schema: ref_schema,
                            table: ref_table,
                        },
                    });
                }
                Ok(out)
            }
            LivePool::Mssql(_)
            | LivePool::Clickhouse(_)
            | LivePool::Duckdb(_)
            | LivePool::Redis(_)
            | LivePool::Mongo(_)
            | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(Vec::new()),
        }
    }

    async fn introspect_table_stats(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let row = sqlx::query(
                    "SELECT \
                       coalesce(c.reltuples::bigint, 0) AS est_rows, \
                       pg_size_pretty(pg_total_relation_size(c.oid)) AS total_size, \
                       pg_size_pretty(pg_relation_size(c.oid)) AS table_size \
                     FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relname = $2 \
                     LIMIT 1",
                )
                .bind(schema)
                .bind(table)
                .fetch_optional(p)
                .await?;
                if let Some(r) = row {
                    let rows: i64 = r.try_get("est_rows").unwrap_or(0);
                    let total: String = r.try_get("total_size").unwrap_or_default();
                    let tbl: String = r.try_get("table_size").unwrap_or_default();
                    Ok(vec![
                        format!("approx_rows · {rows}"),
                        format!("table_size · {tbl}"),
                        format!("total_size · {total}"),
                    ])
                } else {
                    Ok(Vec::new())
                }
            }
            LivePool::Mysql(p) => {
                let db = database.unwrap_or(schema);
                let row = sqlx::query(
                    "SELECT TABLE_ROWS, DATA_LENGTH, INDEX_LENGTH \
                     FROM information_schema.TABLES \
                     WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? LIMIT 1",
                )
                .bind(db)
                .bind(table)
                .fetch_optional(p)
                .await?;
                if let Some(r) = row {
                    let rows: i64 = r.try_get("TABLE_ROWS").unwrap_or(0);
                    let data: i64 = r.try_get("DATA_LENGTH").unwrap_or(0);
                    let idx: i64 = r.try_get("INDEX_LENGTH").unwrap_or(0);
                    Ok(vec![
                        format!("approx_rows · {rows}"),
                        format!("data_bytes · {data}"),
                        format!("index_bytes · {idx}"),
                    ])
                } else {
                    Ok(Vec::new())
                }
            }
            LivePool::Mssql(c) => {
                let esc_s = schema.replace('\'', "''");
                let esc_t = table.replace('\'', "''");
                let qr = engines::exec_mssql(
                    c,
                    &format!(
                        "SELECT SUM(p.rows) AS approx_rows, \
                           SUM(a.total_pages) * 8 AS total_kb \
                         FROM sys.tables t \
                         JOIN sys.schemas s ON s.schema_id = t.schema_id \
                         JOIN sys.partitions p ON p.object_id = t.object_id AND p.index_id IN (0,1) \
                         JOIN sys.allocation_units a ON a.container_id = p.partition_id \
                         WHERE s.name = N'{esc_s}' AND t.name = N'{esc_t}'"
                    ),
                )
                .await?;
                if let Some(row) = qr.rows.first() {
                    let rows = row.first().cloned().unwrap_or_else(|| "?".into());
                    let kb = row.get(1).cloned().unwrap_or_else(|| "?".into());
                    Ok(vec![
                        format!("approx_rows · {rows}"),
                        format!("total_kb · {kb}"),
                    ])
                } else {
                    Ok(Vec::new())
                }
            }
            LivePool::Sqlite(p) => {
                let cnt: i64 = sqlx::query_scalar(&format!(
                    "SELECT COUNT(*) FROM \"{}\"",
                    table.replace('"', "\"\"")
                ))
                .fetch_one(p)
                .await
                .unwrap_or(0);
                Ok(vec![format!("rows · {cnt}")])
            }
            _ => Ok(Vec::new()),
        }
    }

    async fn introspect_triggers(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT t.tgname, pg_get_triggerdef(t.oid, true) AS def \
                     FROM pg_trigger t \
                     JOIN pg_class c ON c.oid = t.tgrelid \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE NOT t.tgisinternal AND n.nspname = $1 AND c.relname = $2 \
                     ORDER BY 1",
                )
                .bind(schema)
                .bind(table)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("tgname");
                        let def: String = r.get("def");
                        format!("{name} · {def}")
                    })
                    .collect())
            }
            LivePool::Mysql(p) => {
                let db = database.context("mysql triggers need active database")?;
                let rows = sqlx::query(
                    "SELECT TRIGGER_NAME, ACTION_TIMING, EVENT_MANIPULATION, ACTION_STATEMENT \
                     FROM information_schema.TRIGGERS \
                     WHERE TRIGGER_SCHEMA = ? AND EVENT_OBJECT_TABLE = ? \
                     ORDER BY 1",
                )
                .bind(db)
                .bind(table)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("TRIGGER_NAME");
                        let timing: String = r.get("ACTION_TIMING");
                        let event: String = r.get("EVENT_MANIPULATION");
                        let body: String = r.get("ACTION_STATEMENT");
                        format!("{name} · {timing} {event} · {body}")
                    })
                    .collect())
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Programmability folder: procedures · functions · triggers for Explorer.
    async fn introspect_programmability(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
    ) -> Result<DbTreeNode> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        let db_owned = database.map(|s| s.to_string());
        let mut children = Vec::new();
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT p.proname AS name, \
                            CASE p.prokind WHEN 'p' THEN 'PROCEDURE' ELSE 'FUNCTION' END AS kind \
                     FROM pg_proc p \
                     JOIN pg_namespace n ON n.oid = p.pronamespace \
                     WHERE n.nspname = $1 \
                     ORDER BY kind, name \
                     LIMIT 200",
                )
                .bind(schema)
                .fetch_all(p)
                .await?;
                for r in rows {
                    let name: String = r.get("name");
                    let kind: String = r.get("kind");
                    children.push(routine_node(
                        source_id,
                        db_owned.clone(),
                        schema,
                        &name,
                        &kind,
                    ));
                }
                let trig = sqlx::query(
                    "SELECT t.tgname AS name \
                     FROM pg_trigger t \
                     JOIN pg_class c ON c.oid = t.tgrelid \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE NOT t.tgisinternal AND n.nspname = $1 \
                     ORDER BY 1 LIMIT 100",
                )
                .bind(schema)
                .fetch_all(p)
                .await
                .unwrap_or_default();
                for r in trig {
                    let name: String = r.get("name");
                    children.push(routine_node(
                        source_id,
                        db_owned.clone(),
                        schema,
                        &name,
                        "TRIGGER",
                    ));
                }
            }
            LivePool::Mysql(p) => {
                let db = database.unwrap_or(schema);
                let rows = sqlx::query(
                    "SELECT ROUTINE_NAME AS name, ROUTINE_TYPE AS kind \
                     FROM information_schema.ROUTINES \
                     WHERE ROUTINE_SCHEMA = ? \
                     ORDER BY ROUTINE_TYPE, ROUTINE_NAME LIMIT 200",
                )
                .bind(db)
                .fetch_all(p)
                .await?;
                for r in rows {
                    let name: String = r.get("name");
                    let kind: String = r.get("kind");
                    children.push(routine_node(
                        source_id,
                        Some(db.to_string()),
                        db,
                        &name,
                        &kind,
                    ));
                }
                let trig = sqlx::query(
                    "SELECT TRIGGER_NAME AS name FROM information_schema.TRIGGERS \
                     WHERE TRIGGER_SCHEMA = ? ORDER BY 1 LIMIT 100",
                )
                .bind(db)
                .fetch_all(p)
                .await
                .unwrap_or_default();
                for r in trig {
                    let name: String = r.get("name");
                    children.push(routine_node(
                        source_id,
                        Some(db.to_string()),
                        db,
                        &name,
                        "TRIGGER",
                    ));
                }
            }
            LivePool::Mssql(c) => {
                let esc = schema.replace('\'', "''");
                let qr = engines::exec_mssql(
                    c,
                    &format!(
                        "SELECT TOP 200 o.name, \
                           CASE o.type \
                             WHEN 'P' THEN 'PROCEDURE' \
                             WHEN 'FN' THEN 'FUNCTION' \
                             WHEN 'IF' THEN 'FUNCTION' \
                             WHEN 'TF' THEN 'FUNCTION' \
                             WHEN 'TR' THEN 'TRIGGER' \
                             ELSE RTRIM(o.type) END AS kind \
                         FROM sys.objects o \
                         JOIN sys.schemas s ON s.schema_id = o.schema_id \
                         WHERE s.name = N'{esc}' \
                           AND o.type IN ('P','FN','IF','TF','TR') \
                         ORDER BY kind, o.name"
                    ),
                )
                .await?;
                for row in qr.rows {
                    let name = row.first().cloned().unwrap_or_default();
                    let kind = row.get(1).cloned().unwrap_or_else(|| "PROCEDURE".into());
                    if name.is_empty() {
                        continue;
                    }
                    children.push(routine_node(
                        source_id,
                        db_owned.clone(),
                        schema,
                        &name,
                        &kind,
                    ));
                }
            }
            _ => {}
        }
        if children.is_empty() {
            children.push(DbTreeNode {
                id: format!("{source_id}::prog::{schema}::empty"),
                label: "(no routines)".into(),
                kind: DbNodeKind::AdminQuery,
                source_id: source_id.into(),
                database: db_owned.clone(),
                schema: Some(schema.into()),
                table: None,
                query_sql: Some("-- no procedures / functions / triggers in this schema".into()),
                children: Vec::new(),
                expanded: false,
            });
        }
        Ok(DbTreeNode {
            id: format!(
                "{source_id}::prog::{}::{schema}",
                database.unwrap_or("")
            ),
            label: "Programmability".into(),
            kind: DbNodeKind::Admin,
            source_id: source_id.into(),
            database: db_owned,
            schema: Some(schema.into()),
            table: None,
            query_sql: None,
            children,
            expanded: false,
        })
    }

    async fn introspect_routines(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
    ) -> Result<Vec<String>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT p.proname, pg_catalog.pg_get_function_identity_arguments(p.oid) AS args, \
                            CASE p.prokind WHEN 'p' THEN 'PROCEDURE' ELSE 'FUNCTION' END AS kind \
                     FROM pg_proc p \
                     JOIN pg_namespace n ON n.oid = p.pronamespace \
                     WHERE n.nspname = $1 \
                     ORDER BY kind, p.proname",
                )
                .bind(schema)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("proname");
                        let args: String = r.get("args");
                        let kind: String = r.get("kind");
                        format!("{kind} {name}({args})")
                    })
                    .collect())
            }
            LivePool::Mysql(p) => {
                let db = database.unwrap_or(schema);
                let rows = sqlx::query(
                    "SELECT ROUTINE_NAME, ROUTINE_TYPE, DTD_IDENTIFIER \
                     FROM information_schema.ROUTINES \
                     WHERE ROUTINE_SCHEMA = ? \
                     ORDER BY ROUTINE_TYPE, ROUTINE_NAME",
                )
                .bind(db)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("ROUTINE_NAME");
                        let kind: String = r.get("ROUTINE_TYPE");
                        let ret: String = r.get("DTD_IDENTIFIER");
                        format!("{kind} {name} → {ret}")
                    })
                    .collect())
            }
            _ => Ok(Vec::new()),
        }
    }

    async fn introspect_grants(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT grantee, \
                            string_agg(privilege_type, ', ' ORDER BY privilege_type) AS privs, \
                            bool_or(is_grantable = 'YES') AS grantable \
                     FROM information_schema.role_table_grants \
                     WHERE table_schema = $1 AND table_name = $2 \
                     GROUP BY grantee ORDER BY 1",
                )
                .bind(schema)
                .bind(table)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let grantee: String = r.get("grantee");
                        let privs: String = r.get("privs");
                        let grantable: bool = r.try_get("grantable").unwrap_or(false);
                        let who = if grantee.eq_ignore_ascii_case("PUBLIC") {
                            "PUBLIC".to_string()
                        } else {
                            format!("\"{}\"", grantee.replace('"', "\"\""))
                        };
                        let mut stmt = format!(
                            "GRANT {privs} ON TABLE \"{schema}\".\"{table}\" TO {who}"
                        );
                        if grantable {
                            stmt.push_str(" WITH GRANT OPTION");
                        }
                        stmt.push(';');
                        stmt
                    })
                    .collect())
            }
            LivePool::Mysql(_) => Ok(vec!["-- use SHOW GRANTS in console".into()]),
            LivePool::Sqlite(_) => Ok(vec!["-- SQLite has no grants".into()]),
            LivePool::Mssql(_)
            | LivePool::Clickhouse(_)
            | LivePool::Duckdb(_)
            | LivePool::Redis(_)
            | LivePool::Mongo(_)
            | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(Vec::new()),
        }
    }

    async fn introspect_ddl(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
        cols: &[ColumnInfo],
        indexes: &[String],
    ) -> Result<String> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                // Prefer view definition when applicable.
                let view: Option<String> = sqlx::query_scalar(
                    "SELECT pg_get_viewdef(format('%I.%I', $1::text, $2::text)::regclass, true)",
                )
                .bind(schema)
                .bind(table)
                .fetch_optional(p)
                .await
                .ok()
                .flatten();
                if let Some(def) = view {
                    return Ok(format!(
                        "CREATE OR REPLACE VIEW \"{schema}\".\"{table}\" AS\n{def}"
                    ));
                }

                let mut body = Vec::new();
                for c in cols {
                    let null = if c.nullable { "" } else { " NOT NULL" };
                    body.push(format!("  \"{}\" {}{null}", c.name, c.data_type));
                }
                let pks: Vec<String> = sqlx::query(
                    "SELECT a.attname AS col \
                     FROM pg_index i \
                     JOIN pg_class t ON t.oid = i.indrelid \
                     JOIN pg_namespace n ON n.oid = t.relnamespace \
                     JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(i.indkey) \
                     WHERE i.indisprimary AND n.nspname = $1 AND t.relname = $2 \
                     ORDER BY array_position(i.indkey, a.attnum)",
                )
                .bind(schema)
                .bind(table)
                .fetch_all(p)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter_map(|r| r.try_get::<String, _>("col").ok())
                .collect();
                if !pks.is_empty() {
                    let pk = pks
                        .iter()
                        .map(|c| format!("\"{c}\""))
                        .collect::<Vec<_>>()
                        .join(", ");
                    body.push(format!("  PRIMARY KEY ({pk})"));
                }
                let mut ddl = format!(
                    "CREATE TABLE \"{schema}\".\"{table}\" (\n{}\n);",
                    body.join(",\n")
                );
                for ix in indexes {
                    // index lines look like "name · CREATE INDEX ..."
                    if let Some((_, def)) = ix.split_once(" · ") {
                        ddl.push('\n');
                        ddl.push_str(def);
                        if !def.trim_end().ends_with(';') {
                            ddl.push(';');
                        }
                    }
                }
                Ok(ddl)
            }
            LivePool::Mysql(p) => {
                let q = format!("SHOW CREATE TABLE `{schema}`.`{table}`");
                let row = sqlx::query(&q).fetch_one(p).await?;
                let ddl: String = row.try_get(1).unwrap_or_else(|_| "-- unavailable".into());
                Ok(ddl)
            }
            LivePool::Sqlite(p) => {
                let row = sqlx::query(
                    "SELECT sql FROM sqlite_master WHERE type IN ('table','view') AND name = ?",
                )
                .bind(table)
                .fetch_optional(p)
                .await?;
                Ok(row
                    .and_then(|r| r.try_get::<String, _>(0).ok())
                    .unwrap_or_else(|| "-- unavailable".into()))
            }
            LivePool::Mssql(_)
            | LivePool::Clickhouse(_)
            | LivePool::Duckdb(_)
            | LivePool::Redis(_)
            | LivePool::Mongo(_)
            | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(format!(
                "-- DDL introspection not yet implemented for this engine\n-- table {schema}.{table}"
            )),
        }
    }

    pub async fn toggle_selected(&mut self) -> Result<()> {
        let Some(entry) = self.flat.get(self.selected).cloned() else {
            return Ok(());
        };
        match entry.kind {
            DbNodeKind::Group => {
                toggle_expand(&mut self.tree, &entry.id);
                self.rebuild_flat();
            }
            DbNodeKind::Source => {
                let driver = self
                    .sources
                    .iter()
                    .find(|s| s.id == entry.source_id)
                    .map(|s| s.driver)
                    .unwrap_or_default();
                // Connect to the source's default DB first (needed to list catalogs).
                match self.ensure_connected(&entry.source_id, None).await {
                    Ok(()) => {
                        self.connection_health
                            .insert(entry.source_id.clone(), ConnHealth::ok_now());
                    }
                    Err(e) => {
                        self.connection_health.insert(
                            entry.source_id.clone(),
                            ConnHealth::Dead(format!("{e:#}")),
                        );
                        self.rebuild_flat();
                        return Err(e);
                    }
                }
                toggle_expand(&mut self.tree, &entry.id);
                let expanded = find_node(&self.tree, &entry.id)
                    .map(|n| n.expanded)
                    .unwrap_or(false);
                if expanded {
                    // Refresh catalogs each expand so Admin appears even on already-loaded trees.
                    let mut children = match driver {
                        DbDriver::Postgres
                        | DbDriver::Mysql
                        | DbDriver::Mssql
                        | DbDriver::Clickhouse
                        | DbDriver::Mongo => {
                            self.introspect_databases(&entry.source_id).await?
                        }
                        DbDriver::Sqlite | DbDriver::Duckdb => {
                            self.introspect_schemas(&entry.source_id, None).await?
                        }
                        DbDriver::Redis => self.introspect_redis_keys(&entry.source_id).await?,
                        DbDriver::Snowflake | DbDriver::Bigquery => {
                            self.cloud_explorer_nodes(&entry.source_id, driver)
                        }
                        DbDriver::Elasticsearch => {
                            self.introspect_elasticsearch(&entry.source_id).await?
                        }
                        DbDriver::Trino => self.introspect_trino_catalogs(&entry.source_id).await?,
                    };
                    children.insert(
                        0,
                        admin_folder_for(
                            &entry.source_id,
                            driver,
                            &self.history,
                            &self.bookmarks,
                            &self.queries_dir,
                        ),
                    );
                    if let Some(node) = find_node_mut(&mut self.tree, &entry.id) {
                        node.children = children;
                    }
                }
                self.rebuild_flat();
            }
            DbNodeKind::Admin => {
                toggle_expand(&mut self.tree, &entry.id);
                self.rebuild_flat();
            }
            DbNodeKind::AdminQuery | DbNodeKind::AdminAction => {
                // Enter handled by app layer (query tab or dump/restore).
            }
            DbNodeKind::Database => {
                let dbname = entry.database.clone().context("missing database name")?;
                self.ensure_connected(&entry.source_id, Some(&dbname))
                    .await?;
                toggle_expand(&mut self.tree, &entry.id);
                let load = find_node(&self.tree, &entry.id)
                    .map(|n| n.expanded && n.children.is_empty())
                    .unwrap_or(false);
                if load {
                    let driver = self
                        .sources
                        .iter()
                        .find(|s| s.id == entry.source_id)
                        .map(|s| s.driver)
                        .unwrap_or_default();
                    let children = match driver {
                        // MySQL: database node lists tables directly.
                        DbDriver::Mysql => {
                            let mut kids = self
                                .introspect_tables(&entry.source_id, Some(&dbname), &dbname)
                                .await?;
                            if let Ok(prog) = self
                                .introspect_programmability(
                                    &entry.source_id,
                                    Some(&dbname),
                                    &dbname,
                                )
                                .await
                            {
                                kids.insert(0, prog);
                            }
                            kids
                        }
                        DbDriver::Clickhouse => {
                            self.introspect_tables(&entry.source_id, Some(&dbname), &dbname)
                                .await?
                        }
                        DbDriver::Mongo => {
                            self.introspect_mongo_collections(&entry.source_id, &dbname)
                                .await?
                        }
                        DbDriver::Trino => {
                            self.introspect_trino_schemas(&entry.source_id, &dbname)
                                .await?
                        }
                        _ => {
                            self.introspect_schemas(&entry.source_id, Some(&dbname))
                                .await?
                        }
                    };
                    if let Some(node) = find_node_mut(&mut self.tree, &entry.id) {
                        node.children = children;
                    }
                }
                self.rebuild_flat();
            }
            DbNodeKind::Schema => {
                let db = entry.database.as_deref();
                self.ensure_connected(&entry.source_id, db).await?;
                toggle_expand(&mut self.tree, &entry.id);
                let load = find_node(&self.tree, &entry.id)
                    .map(|n| n.expanded && n.children.is_empty())
                    .unwrap_or(false);
                if load {
                    if let Some(schema) = &entry.schema {
                        let driver = self
                            .sources
                            .iter()
                            .find(|s| s.id == entry.source_id)
                            .map(|s| s.driver)
                            .unwrap_or_default();
                        let children = if driver == DbDriver::Trino {
                            let catalog = db.unwrap_or("memory");
                            let key = Self::pool_key(&entry.source_id, None);
                            match self.pools.get(&key) {
                                Some(LivePool::Http(h)) if h.driver == DbDriver::Trino => {
                                    http_engines::trino_tables(
                                        h,
                                        &entry.source_id,
                                        catalog,
                                        schema,
                                    )
                                    .await?
                                }
                                _ => Vec::new(),
                            }
                        } else {
                            let mut kids = self
                                .introspect_tables(&entry.source_id, db, schema)
                                .await?;
                            if matches!(
                                driver,
                                DbDriver::Postgres | DbDriver::Mysql | DbDriver::Mssql
                            ) {
                                if let Ok(prog) = self
                                    .introspect_programmability(
                                        &entry.source_id,
                                        db,
                                        schema,
                                    )
                                    .await
                                {
                                    kids.insert(0, prog);
                                }
                            }
                            kids
                        };
                        if let Some(node) = find_node_mut(&mut self.tree, &entry.id) {
                            node.children = children;
                        }
                    }
                }
                self.rebuild_flat();
            }
            DbNodeKind::Table | DbNodeKind::View => {
                // DataGrip-style: Enter opens a table data tab; columns live in Structure.
                let _ = self.open_selected_table_tab().await?;
            }
            DbNodeKind::Routine => {
                // Enter scripts routine DDL into a console tab.
            }
            DbNodeKind::Column => {}
        }
        Ok(())
    }

    pub async fn ensure_connected(
        &mut self,
        source_id: &str,
        database: Option<&str>,
    ) -> Result<()> {
        let key = Self::pool_key(source_id, database);
        if self.pools.contains_key(&key) {
            self.active_source = Some(source_id.to_string());
            self.active_database = database.map(|s| s.to_string());
            let name = self.source_name(source_id);
            self.status = match database {
                Some(db) => format!("connected: {name}/{db}"),
                None => format!("connected: {name}"),
            };
            return Ok(());
        }
        let mut src = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .cloned()
            .context("unknown data source")?
            .for_connect();
        if let Some(db) = database {
            if !db.is_empty() {
                src.database = db.to_string();
            }
        }
        // Postgres needs a database name to connect; fall back to "postgres".
        if matches!(src.driver, DbDriver::Postgres) && src.database.is_empty() {
            src.database = "postgres".into();
        }
        let local_port = if src.uses_ssh_tunnel() {
            Some(self.ensure_ssh_tunnel(&src)?)
        } else {
            None
        };
        let url = src.url_via(local_port);
        let pool = match src.driver {
            DbDriver::Postgres => {
                let p = PgPoolOptions::new()
                    .max_connections(5)
                    .connect(&url)
                    .await
                    .with_context(|| {
                        format!(
                            "connect postgres {} ({}){}",
                            src.name,
                            src.database,
                            if local_port.is_some() {
                                " via SSH tunnel"
                            } else {
                                ""
                            }
                        )
                    })?;
                LivePool::Pg(p)
            }
            DbDriver::Mysql => {
                let p = MySqlPoolOptions::new()
                    .max_connections(5)
                    .connect(&url)
                    .await
                    .with_context(|| {
                        format!(
                            "connect mysql {}{}",
                            src.name,
                            if local_port.is_some() {
                                " via SSH tunnel"
                            } else {
                                ""
                            }
                        )
                    })?;
                LivePool::Mysql(p)
            }
            DbDriver::Sqlite => {
                let p = SqlitePoolOptions::new()
                    .max_connections(3)
                    .connect(&url)
                    .await
                    .with_context(|| format!("connect sqlite {}", src.name))?;
                LivePool::Sqlite(p)
            }
            DbDriver::Mssql => {
                let c = engines::connect_mssql(&src, local_port)
                    .await
                    .with_context(|| format!("connect mssql {}", src.name))?;
                LivePool::Mssql(c)
            }
            DbDriver::Clickhouse => {
                LivePool::Clickhouse(ClickhouseHttp::from_source(&src, local_port)?)
            }
            DbDriver::Duckdb => LivePool::Duckdb(alt_engines::connect_duckdb(&src)?),
            DbDriver::Redis => {
                let r = alt_engines::connect_redis(&src, local_port)?;
                alt_engines::ping_redis(&r).await?;
                LivePool::Redis(r)
            }
            DbDriver::Mongo => {
                LivePool::Mongo(alt_engines::connect_mongo(&src, local_port).await?)
            }
            DbDriver::Snowflake => {
                if !alt_engines::snowflake_configured(&src) {
                    bail!(alt_engines::cloud_not_configured_message(DbDriver::Snowflake));
                }
                LivePool::CloudStub(DbDriver::Snowflake)
            }
            DbDriver::Bigquery => {
                if !alt_engines::bigquery_configured(&src) {
                    bail!(alt_engines::cloud_not_configured_message(DbDriver::Bigquery));
                }
                LivePool::CloudStub(DbDriver::Bigquery)
            }
            DbDriver::Elasticsearch | DbDriver::Trino => {
                LivePool::Http(http_engines::connect_http(&src)?)
            }
        };
        self.pools.insert(key, pool);
        self.active_source = Some(source_id.to_string());
        self.active_database = database
            .map(|s| s.to_string())
            .or_else(|| {
                if src.database.is_empty() {
                    None
                } else {
                    Some(src.database.clone())
                }
            });
        let via = if local_port.is_some() {
            format!(" via ssh {}", src.ssh_host)
        } else {
            String::new()
        };
        self.status = match &self.active_database {
            Some(db) => format!("connected: {}/{db}{via}", src.name),
            None => format!("connected: {}{via}", src.name),
        };
        Ok(())
    }

    /// Probe a data source with an 8s timeout (does not mutate the live workspace pools).
    pub async fn test_data_source(src: &DataSource) -> Result<String> {
        oauth::load_persisted_tokens();
        let mut ws = DbWorkspace::default();
        // Resolve env:/vault before connect (same as live pools).
        let mut src = src.for_connect();
        if src.id.is_empty() {
            src.id = uuid::Uuid::new_v4().to_string();
        }
        let id = src.id.clone();
        ws.sources.push(src);
        tokio::time::timeout(
            Duration::from_secs(8),
            ws.ensure_connected(&id, None),
        )
        .await
        .context("connection test timed out (8s)")?
        .context("connection test failed")?;
        let is_bq = ws
            .sources
            .first()
            .map(|s| s.driver == DbDriver::Bigquery)
            .unwrap_or(false);
        if is_bq {
            let db = ws.active_database.clone();
            let key = Self::pool_key(&id, db.as_deref());
            let _ = ws
                .exec_statement(&key, DbDriver::Bigquery, db.as_deref(), "SELECT 1 AS ok")
                .await
                .context("BigQuery SELECT 1")?;
        }
        Ok(ws.status.clone())
    }

    /// ASCII ER edges for the active schema (PG/MySQL/SQLite FKs).
    pub async fn er_sketch_active_schema(&mut self) -> Result<String> {
        let source_id = self
            .active_source
            .clone()
            .context("connect a source first")?;
        let schema = self
            .flat
            .get(self.selected)
            .and_then(|e| e.schema.clone())
            .or_else(|| self.last_result.as_ref().and_then(|r| r.source_schema.clone()))
            .unwrap_or_else(|| "public".into());
        let database = self.active_database.clone();
        self.er_sketch_for_schema(&source_id, database.as_deref(), &schema)
            .await
    }

    async fn er_sketch_for_schema(
        &mut self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
    ) -> Result<String> {
        self.ensure_connected(source_id, database).await?;
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        let edges: Vec<(String, String, String, String)> = match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT tc.table_name, kcu.column_name, \
                            ccu.table_name AS ref_table, ccu.column_name AS ref_column \
                     FROM information_schema.table_constraints tc \
                     JOIN information_schema.key_column_usage kcu \
                       ON tc.constraint_name = kcu.constraint_name \
                      AND tc.table_schema = kcu.table_schema \
                     JOIN information_schema.constraint_column_usage ccu \
                       ON ccu.constraint_name = tc.constraint_name \
                      AND ccu.table_schema = tc.table_schema \
                     WHERE tc.constraint_type = 'FOREIGN KEY' \
                       AND tc.table_schema = $1 \
                     ORDER BY tc.table_name, kcu.ordinal_position",
                )
                .bind(&schema)
                .fetch_all(p)
                .await?;
                rows.into_iter()
                    .filter_map(|r| {
                        Some((
                            r.try_get(0).ok()?,
                            r.try_get(1).ok()?,
                            r.try_get(2).ok()?,
                            r.try_get(3).ok()?,
                        ))
                    })
                    .collect()
            }
            LivePool::Mysql(p) => {
                let db = database.context("MySQL ER needs active database")?;
                let rows = sqlx::query(
                    "SELECT TABLE_NAME, COLUMN_NAME, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
                     FROM information_schema.KEY_COLUMN_USAGE \
                     WHERE TABLE_SCHEMA = ? AND REFERENCED_TABLE_NAME IS NOT NULL \
                     ORDER BY TABLE_NAME, ORDINAL_POSITION",
                )
                .bind(db)
                .fetch_all(p)
                .await?;
                rows.into_iter()
                    .filter_map(|r| {
                        Some((
                            r.try_get(0).ok()?,
                            r.try_get(1).ok()?,
                            r.try_get(2).ok()?,
                            r.try_get(3).ok()?,
                        ))
                    })
                    .collect()
            }
            LivePool::Sqlite(p) => {
                let tables = sqlx::query(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                )
                .fetch_all(p)
                .await?;
                let mut out = Vec::new();
                for t in tables {
                    let name: String = t.try_get(0).unwrap_or_default();
                    let fks = sqlx::query(&format!("PRAGMA foreign_key_list('{name}')"))
                        .fetch_all(p)
                        .await
                        .unwrap_or_default();
                    for r in fks {
                        let ref_table: String = r.try_get(2).unwrap_or_default();
                        let from_col: String = r.try_get(3).unwrap_or_default();
                        let ref_col: String = r.try_get(4).unwrap_or_default();
                        out.push((name.clone(), from_col, ref_table, ref_col));
                    }
                }
                out
            }
            _ => {
                return Ok(format!(
                    "# ER · {schema}\n(ER sketch not available for this engine)\n"
                ));
            }
        };
        if edges.is_empty() {
            return Ok(format!("# ER · {schema}\n(no foreign keys found)\n"));
        }
        let mut out = format!("# ER · {schema}\n");
        for (t, c, rt, rc) in edges {
            out.push_str(&format!("{t}.{c} --> {rt}.{rc}\n"));
        }
        Ok(out)
    }

    /// Start (or reuse) `ssh -L local:host:port` for this source. Returns local port.
    fn ensure_ssh_tunnel(&mut self, src: &DataSource) -> Result<u16> {
        if let Some(t) = self.ssh_tunnels.get_mut(&src.id) {
            // Reuse if child still alive.
            match t.child.try_wait() {
                Ok(None) => return Ok(t.local_port),
                Ok(Some(status)) => {
                    bail!(
                        "SSH tunnel to {} exited ({status})",
                        src.ssh_host
                    );
                }
                Err(e) => bail!("SSH tunnel status: {e}"),
            }
        }

        let local_port = {
            let listener = TcpListener::bind("127.0.0.1:0")
                .context("bind local port for SSH tunnel")?;
            listener.local_addr()?.port()
        };

        let remote_host = if src.host.trim().is_empty() {
            "127.0.0.1"
        } else {
            src.host.trim()
        };
        let forward = format!("{local_port}:{remote_host}:{}", src.port);
        let target = if src.ssh_user.trim().is_empty() {
            src.ssh_host.trim().to_string()
        } else {
            format!("{}@{}", src.ssh_user.trim(), src.ssh_host.trim())
        };

        let mut cmd = Command::new("ssh");
        cmd.args([
            "-N",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "ServerAliveInterval=30",
            "-L",
            &forward,
            "-p",
            &src.ssh_port.to_string(),
        ]);
        if !src.ssh_key.trim().is_empty() {
            cmd.args(["-i", src.ssh_key.trim()]);
        }
        cmd.arg(&target);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn ssh tunnel to {target}"))?;

        // Wait until local forward accepts connections (or child dies).
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            if std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{local_port}").parse()?,
                Duration::from_millis(200),
            )
            .is_ok()
            {
                break;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut err = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        use std::io::Read;
                        let _ = stderr.read_to_string(&mut err);
                    }
                    bail!(
                        "SSH tunnel failed ({status}): {}",
                        err.trim().lines().next().unwrap_or("see ssh -v")
                    );
                }
                Ok(None) if std::time::Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(100));
                }
                Ok(None) => bail!("SSH tunnel timed out waiting for local port {local_port}"),
                Err(e) => bail!("SSH tunnel wait: {e}"),
            }
        }

        self.ssh_tunnels.insert(
            src.id.clone(),
            SshTunnel {
                local_port,
                child,
            },
        );
        Ok(local_port)
    }

    async fn introspect_databases(&self, source_id: &str) -> Result<Vec<DbTreeNode>> {
        let key = Self::pool_key(source_id, None);
        let pool = self
            .pools
            .get(&key)
            .or_else(|| self.pools.get(source_id))
            .context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT datname FROM pg_database \
                     WHERE datistemplate = false \
                     ORDER BY 1",
                )
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("datname");
                        DbTreeNode {
                            id: format!("{source_id}::db::{name}"),
                            label: name.clone(),
                            kind: DbNodeKind::Database,
                            source_id: source_id.into(),
                            database: Some(name),
                            schema: None,
                            table: None,
                            query_sql: None,
                            children: Vec::new(),
                            expanded: false,
                        }
                    })
                    .collect())
            }
            LivePool::Mysql(p) => {
                let rows = sqlx::query("SHOW DATABASES").fetch_all(p).await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| {
                        let name: String = r.try_get(0).ok()?;
                        if matches!(
                            name.as_str(),
                            "information_schema" | "performance_schema" | "mysql" | "sys"
                        ) {
                            return None;
                        }
                        Some(DbTreeNode {
                            id: format!("{source_id}::db::{name}"),
                            label: name.clone(),
                            kind: DbNodeKind::Database,
                            source_id: source_id.into(),
                            database: Some(name),
                            schema: None,
                            table: None,
                            query_sql: None,
                            children: Vec::new(),
                            expanded: false,
                        })
                    })
                    .collect())
            }
            LivePool::Sqlite(_) => Ok(Vec::new()),
            LivePool::Mssql(c) => engines::mssql_databases(c, source_id).await,
            LivePool::Clickhouse(c) => engines::ch_databases(c, source_id).await,
            LivePool::Mongo(m) => alt_engines::mongo_databases(m, source_id).await,
            LivePool::Duckdb(_) | LivePool::Redis(_) | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(Vec::new()),
        }
    }

    async fn introspect_schemas(
        &self,
        source_id: &str,
        database: Option<&str>,
    ) -> Result<Vec<DbTreeNode>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        let db_owned = database.map(|s| s.to_string());
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT schema_name FROM information_schema.schemata \
                     WHERE schema_name NOT IN ('pg_catalog','information_schema','pg_toast') \
                     ORDER BY 1",
                )
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("schema_name");
                        let id_db = db_owned.clone().unwrap_or_default();
                        DbTreeNode {
                            id: format!("{source_id}::schema::{id_db}::{name}"),
                            label: name.clone(),
                            kind: DbNodeKind::Schema,
                            source_id: source_id.into(),
                            database: db_owned.clone(),
                            schema: Some(name),
                            table: None,
                            query_sql: None,
                            children: Vec::new(),
                            expanded: false,
                        }
                    })
                    .collect())
            }
            LivePool::Mysql(_p) => {
                // Under a MySQL database node, schemas ≈ that database; show as single schema.
                let schema = database.unwrap_or("").to_string();
                Ok(vec![DbTreeNode {
                    id: format!("{source_id}::schema::{schema}"),
                    label: schema.clone(),
                    kind: DbNodeKind::Schema,
                    source_id: source_id.into(),
                    database: Some(schema.clone()),
                    schema: Some(schema),
                    table: None,
                    query_sql: None,
                    children: Vec::new(),
                    expanded: false,
                }])
            }
            LivePool::Sqlite(_) => Ok(vec![DbTreeNode {
                id: format!("{source_id}::schema::main"),
                label: "main".into(),
                kind: DbNodeKind::Schema,
                source_id: source_id.into(),
                database: None,
                schema: Some("main".into()),
                table: None,
                query_sql: None,
                children: Vec::new(),
                expanded: false,
            }]),
            LivePool::Mssql(c) => engines::mssql_schemas(c, source_id, database).await,
            LivePool::Clickhouse(_) => {
                let schema = database.unwrap_or("default").to_string();
                Ok(vec![DbTreeNode {
                    id: format!("{source_id}::schema::{schema}"),
                    label: schema.clone(),
                    kind: DbNodeKind::Schema,
                    source_id: source_id.into(),
                    database: Some(schema.clone()),
                    schema: Some(schema),
                    table: None,
                    query_sql: None,
                    children: Vec::new(),
                    expanded: false,
                }])
            }
            LivePool::Duckdb(_) => Ok(vec![DbTreeNode {
                id: format!("{source_id}::schema::main"),
                label: "main".into(),
                kind: DbNodeKind::Schema,
                source_id: source_id.into(),
                database: None,
                schema: Some("main".into()),
                table: None,
                query_sql: None,
                children: Vec::new(),
                expanded: false,
            }]),
            LivePool::Redis(_) | LivePool::Mongo(_) | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(Vec::new()),
        }
    }

    async fn introspect_tables(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
    ) -> Result<Vec<DbTreeNode>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        let db_owned = database.map(|s| s.to_string());
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT table_name, table_type FROM information_schema.tables \
                     WHERE table_schema = $1 \
                     ORDER BY CASE WHEN table_type = 'BASE TABLE' THEN 0 ELSE 1 END, 1",
                )
                .bind(schema)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("table_name");
                        let ty: String = r.get("table_type");
                        let kind = if ty.contains("VIEW") {
                            DbNodeKind::View
                        } else {
                            DbNodeKind::Table
                        };
                        let id_db = db_owned.clone().unwrap_or_default();
                        DbTreeNode {
                            id: format!("{source_id}::table::{id_db}::{schema}::{name}"),
                            label: name.clone(),
                            kind,
                            source_id: source_id.into(),
                            database: db_owned.clone(),
                            schema: Some(schema.into()),
                            table: Some(name),
                            query_sql: None,
                            children: Vec::new(),
                            expanded: false,
                        }
                    })
                    .collect())
            }
            LivePool::Mysql(p) => {
                let q = format!("SHOW FULL TABLES FROM `{schema}`");
                let rows = sqlx::query(&q).fetch_all(p).await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.try_get(0).unwrap_or_default();
                        let ty: String = r.try_get(1).unwrap_or_default();
                        let kind = if ty.to_ascii_lowercase().contains("view") {
                            DbNodeKind::View
                        } else {
                            DbNodeKind::Table
                        };
                        DbTreeNode {
                            id: format!("{source_id}::table::{schema}::{name}"),
                            label: name.clone(),
                            kind,
                            source_id: source_id.into(),
                            database: db_owned.clone(),
                            schema: Some(schema.into()),
                            table: Some(name),
                            query_sql: None,
                            children: Vec::new(),
                            expanded: false,
                        }
                    })
                    .collect())
            }
            LivePool::Sqlite(p) => {
                let rows = sqlx::query(
                    "SELECT name, type FROM sqlite_master WHERE type IN ('table','view') ORDER BY 1",
                )
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("name");
                        let ty: String = r.get("type");
                        let kind = if ty == "view" {
                            DbNodeKind::View
                        } else {
                            DbNodeKind::Table
                        };
                        DbTreeNode {
                            id: format!("{source_id}::table::main::{name}"),
                            label: name.clone(),
                            kind,
                            source_id: source_id.into(),
                            database: None,
                            schema: Some("main".into()),
                            table: Some(name),
                            query_sql: None,
                            children: Vec::new(),
                            expanded: false,
                        }
                    })
                    .collect())
            }
            LivePool::Mssql(c) => {
                engines::mssql_tables(c, source_id, database, schema).await
            }
            LivePool::Clickhouse(c) => {
                let db = database.unwrap_or(schema);
                engines::ch_tables(c, source_id, db).await
            }
            LivePool::Duckdb(d) => alt_engines::duckdb_tables(d, source_id).await,
            LivePool::Redis(_)
            | LivePool::Mongo(_)
            | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(Vec::new()),
        }
    }

    async fn introspect_column_info(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
    ) -> Result<Vec<ColumnInfo>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT column_name, data_type, is_nullable \
                     FROM information_schema.columns \
                     WHERE table_schema=$1 AND table_name=$2 ORDER BY ordinal_position",
                )
                .bind(schema)
                .bind(table)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(|r| {
                        let name: String = r.get("column_name");
                        let ty: String = r.get("data_type");
                        let null: String = r.get("is_nullable");
                        ColumnInfo {
                            name,
                            data_type: ty,
                            nullable: null == "YES",
                        }
                    })
                    .collect())
            }
            LivePool::Mysql(p) => {
                let q = format!("SHOW COLUMNS FROM `{schema}`.`{table}`");
                let rows = sqlx::query(&q).fetch_all(p).await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| {
                        let name: String = r.try_get(0).ok()?;
                        let ty: String = r.try_get(1).unwrap_or_default();
                        let null: String = r.try_get::<String, _>(2).unwrap_or_default();
                        Some(ColumnInfo {
                            name,
                            data_type: ty,
                            nullable: null.eq_ignore_ascii_case("YES"),
                        })
                    })
                    .collect())
            }
            LivePool::Sqlite(p) => {
                let q = format!("PRAGMA table_info('{table}')");
                let rows = sqlx::query(&q).fetch_all(p).await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| {
                        let name: String = r.try_get(1).ok()?;
                        let ty: String = r.try_get(2).unwrap_or_default();
                        let notnull: i64 = r.try_get(3).unwrap_or(0);
                        Some(ColumnInfo {
                            name,
                            data_type: ty,
                            nullable: notnull == 0,
                        })
                    })
                    .collect())
            }
            LivePool::Mssql(c) => engines::mssql_columns(c, database, schema, table).await,
            LivePool::Clickhouse(c) => {
                let db = database.unwrap_or(schema);
                engines::ch_columns(c, db, table).await
            }
            LivePool::Duckdb(d) => alt_engines::duckdb_columns(d, schema, table).await,
            LivePool::Redis(_) | LivePool::Mongo(_) | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(Vec::new()),
        }
    }

    pub async fn execute_sql(&mut self) -> Result<()> {
        let sql = self.sql.trim().to_string();
        self.execute_sql_text(&sql).await
    }

    /// Run SQL for this session only (e.g. after substituting query params).
    pub async fn execute_sql_text(&mut self, sql: &str) -> Result<()> {
        let Some(source_id) = self.active_source.clone() else {
            bail!("no active data source — expand one in the Database sidebar");
        };
        let database = self.active_database.clone();
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let sql = sql.trim();
        if sql.is_empty() {
            bail!("empty SQL");
        }
        let key = Self::pool_key(&source_id, database.as_deref());
        if self.in_txn && self.txn_pool_key.as_deref() != Some(key.as_str()) {
            bail!("transaction open on another connection — commit or rollback first");
        }
        let statements = split_statements(sql);
        if statements.len() == 1 && query_paging::is_paged_query(sql) {
            self.last_page_sql = Some(sql.to_string());
        }
        let start = std::time::Instant::now();
        self.query_running = true;
        self.query_cancel.store(false, Ordering::Relaxed);
        self.status = "running query… Esc cancel".into();
        let mut last = QueryResult::empty();
        let driver = self
            .source_cfg(&source_id)
            .map(|s| s.driver)
            .unwrap_or_default();
        let page = self.result_page;
        let timeout = self.query_timeout_secs;
        let mut result_tab_n = 0usize;
        for (si, stmt) in statements.iter().enumerate() {
            if self.query_cancel.load(Ordering::Relaxed) {
                self.query_running = false;
                let _ = self.cancel_backend_best_effort(&key, driver).await;
                self.disconnect_active_pool();
                bail!("query cancelled");
            }
            self.guard_writable(&source_id, stmt)?;
            let exec_sql = if statements.len() == 1 {
                paginate_select_sql(stmt, page)
            } else {
                stmt.to_string()
            };
            let stmt_start = std::time::Instant::now();
            let cancel = self.query_cancel.clone();
            let outcome: Result<QueryResult> = if timeout > 0 {
                tokio::select! {
                    r = tokio::time::timeout(
                        Duration::from_secs(timeout),
                        self.exec_statement(&key, driver, database.as_deref(), &exec_sql),
                    ) => match r {
                        Ok(inner) => inner,
                        Err(_) => Err(anyhow::anyhow!("query timed out after {timeout}s")),
                    },
                    _ = Self::wait_query_cancel(cancel) => {
                        Err(anyhow::anyhow!("query cancelled"))
                    }
                }
            } else {
                tokio::select! {
                    r = self.exec_statement(&key, driver, database.as_deref(), &exec_sql) => r,
                    _ = Self::wait_query_cancel(cancel) => {
                        Err(anyhow::anyhow!("query cancelled"))
                    }
                }
            };
            if let Err(ref e) = outcome {
                let msg = e.to_string();
                if msg.contains("cancelled") || msg.contains("timed out") {
                    self.query_running = false;
                    let _ = self.cancel_backend_best_effort(&key, driver).await;
                    if msg.contains("cancelled") {
                        self.disconnect_active_pool();
                    }
                }
            }
            last = outcome?;
            last.page = page;
            last.has_more = last.rows.len() >= RESULT_PAGE_SIZE;
            last.elapsed_ms = stmt_start.elapsed().as_millis();
            self.history.push(QueryHistoryEntry {
                sql: stmt.clone(),
                elapsed_ms: last.elapsed_ms,
            });
            // Multi-statement: park intermediate result sets in new tabs.
            if statements.len() > 1 && si + 1 < statements.len() && !last.columns.is_empty() {
                result_tab_n += 1;
                let mut tab = SqlTab::query(self.tabs.len() + 1);
                tab.title = format!("Result {result_tab_n}");
                tab.sql = stmt.clone();
                tab.result = Some(last.clone());
                tab.source_id = Some(source_id.clone());
                tab.database = database.clone();
                self.tabs.push(tab);
            }
        }
        self.query_running = false;
        if self.history.len() > 80 {
            let drop_n = self.history.len() - 80;
            self.history.drain(0..drop_n);
        }
        self.persist_history();
        let elapsed_ms = start.elapsed().as_millis();
        let (schema, table) = parse_select_target(statements.last().map(|s| s.as_str()).unwrap_or(""));
        let pks = if let (Some(ref sch), Some(ref tbl)) = (&schema, &table) {
            self.fetch_primary_keys(&source_id, sch, tbl)
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        last = last.with_meta(schema, table, pks);
        last.elapsed_ms = elapsed_ms;
        let page_tag = if page > 0 {
            format!(" · page {} (offset {})", page + 1, page * RESULT_PAGE_SIZE)
        } else {
            String::new()
        };
        let more_tag = if last.has_more { " · more…" } else { "" };
        let txn_tag = if self.in_txn { " · TXN" } else { "" };
        self.status = format!(
            "{} cols · {} rows · {elapsed_ms} ms · {} stmt(s){}{}{}{}",
            last.columns.len(),
            last.rows.len(),
            statements.len(),
            if last.truncated { " (truncated)" } else { "" },
            page_tag,
            more_tag,
            txn_tag
        );
        self.last_result = Some(last);
        self.pending_row_deletes.clear();
        self.flush_active_tab();
        Ok(())
    }

    async fn exec_statement(
        &mut self,
        key: &str,
        driver: DbDriver,
        database: Option<&str>,
        exec_sql: &str,
    ) -> Result<QueryResult> {
        if self.in_txn {
            return self.exec_stmt_in_txn(exec_sql).await;
        }
        if driver == DbDriver::Redis {
            let pool = self.pools.get(key).context("not connected")?;
            let LivePool::Redis(r) = pool else {
                bail!("not a redis connection");
            };
            return alt_engines::exec_redis(r, exec_sql).await;
        }
        if driver == DbDriver::Mongo {
            let pool = self.pools.get(key).context("not connected")?;
            let LivePool::Mongo(m) = pool else {
                bail!("not a mongo connection");
            };
            return alt_engines::exec_mongo(m, exec_sql, database).await;
        }
        // Dedicated connection for PG/MySQL so cancel targets the real backend pid.
        if driver == DbDriver::Postgres {
            let pool = self.pools.get(key).context("not connected")?;
            let LivePool::Pg(p) = pool else {
                bail!("expected postgres pool");
            };
            let mut conn = p.acquire().await.context("acquire pg conn")?;
            let pid: i64 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *conn)
                .await
                .unwrap_or(0);
            self.query_backend_pid.store(pid, Ordering::Relaxed);
            let result = exec_pg_conn(&mut *conn, exec_sql).await;
            self.query_backend_pid.store(0, Ordering::Relaxed);
            return result;
        }
        if driver == DbDriver::Mysql {
            let pool = self.pools.get(key).context("not connected")?;
            let LivePool::Mysql(p) = pool else {
                bail!("expected mysql pool");
            };
            let mut conn = p.acquire().await.context("acquire mysql conn")?;
            let id: i64 = sqlx::query_scalar("SELECT CONNECTION_ID()")
                .fetch_one(&mut *conn)
                .await
                .unwrap_or(0);
            self.query_backend_pid.store(id, Ordering::Relaxed);
            let result = exec_mysql_conn(&mut *conn, exec_sql).await;
            self.query_backend_pid.store(0, Ordering::Relaxed);
            return result;
        }
        let pool = self.pools.get(key).context("not connected")?;
        match pool {
            LivePool::Pg(_) | LivePool::Mysql(_) => unreachable!("handled above"),
            LivePool::Sqlite(p) => exec_sqlite(p, exec_sql).await,
            LivePool::Mssql(c) => engines::exec_mssql(c, exec_sql).await,
            LivePool::Clickhouse(c) => engines::exec_clickhouse(c, exec_sql).await,
            LivePool::Duckdb(d) => alt_engines::exec_duckdb(d, exec_sql).await,
            LivePool::Http(h) => http_engines::exec_http(h, exec_sql).await,
            LivePool::CloudStub(DbDriver::Bigquery) => {
                let src = self
                    .sources
                    .iter()
                    .find(|s| {
                        self.active_source
                            .as_ref()
                            .map(|id| &s.id == id)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .context("no active BigQuery source")?
                    .for_connect();
                alt_engines::exec_bigquery(&src, exec_sql).await
            }
            LivePool::CloudStub(DbDriver::Snowflake) => {
                let src = self
                    .sources
                    .iter()
                    .find(|s| {
                        self.active_source
                            .as_ref()
                            .map(|id| &s.id == id)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .context("no active Snowflake source")?
                    .for_connect();
                alt_engines::exec_snowflake(&src, exec_sql).await
            }
            LivePool::CloudStub(d) => bail!(alt_engines::cloud_not_configured_message(*d)),
            LivePool::Redis(_) | LivePool::Mongo(_) => unreachable!(),
        }
    }

    async fn wait_query_cancel(cancel: Arc<AtomicBool>) {
        while !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }

    pub fn request_query_cancel(&self) {
        self.query_cancel.store(true, Ordering::Relaxed);
    }

    /// Cancel the dedicated in-flight backend (pid captured on query connection).
    async fn cancel_backend_best_effort(&mut self, key: &str, driver: DbDriver) -> Result<()> {
        let pid = self.query_backend_pid.load(Ordering::Relaxed);
        let pool = self.pools.get(key).context("not connected")?;
        match (driver, pool) {
            (DbDriver::Postgres, LivePool::Pg(p)) if pid > 0 => {
                let _ = sqlx::query_scalar::<_, bool>("SELECT pg_cancel_backend($1)")
                    .bind(pid as i32)
                    .fetch_one(p)
                    .await;
            }
            (DbDriver::Mysql, LivePool::Mysql(p)) if pid > 0 => {
                let _ = sqlx::query(&format!("KILL QUERY {pid}")).execute(p).await;
            }
            (DbDriver::Postgres, LivePool::Pg(p)) => {
                let _ = sqlx::query(
                    "SELECT pg_cancel_backend(pid) FROM pg_stat_activity \
                     WHERE datname = current_database() \
                       AND pid <> pg_backend_pid() \
                       AND state <> 'idle' \
                       AND usename = current_user",
                )
                .execute(p)
                .await;
            }
            _ => {}
        }
        Ok(())
    }

    async fn introspect_elasticsearch(&self, source_id: &str) -> Result<Vec<DbTreeNode>> {
        let key = Self::pool_key(source_id, None);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Http(h) if h.driver == DbDriver::Elasticsearch => {
                http_engines::elasticsearch_indices(h, source_id).await
            }
            _ => Ok(Vec::new()),
        }
    }

    fn cloud_explorer_nodes(&self, source_id: &str, driver: DbDriver) -> Vec<DbTreeNode> {
        match driver {
            DbDriver::Bigquery => {
                let project = self
                    .sources
                    .iter()
                    .find(|s| s.id == source_id)
                    .map(|s| {
                        if s.database.trim().is_empty() {
                            s.name.clone()
                        } else {
                            s.database.clone()
                        }
                    })
                    .unwrap_or_else(|| "project".into());
                vec![
                    DbTreeNode {
                        id: format!("{source_id}::bq::datasets"),
                        label: format!("datasets · {project}"),
                        kind: DbNodeKind::AdminQuery,
                        source_id: source_id.into(),
                        database: Some(project.clone()),
                        schema: None,
                        table: Some("datasets".into()),
                        query_sql: Some(
                            "SELECT schema_name FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY 1 LIMIT 100;"
                                .into(),
                        ),
                        children: Vec::new(),
                        expanded: false,
                    },
                    DbTreeNode {
                        id: format!("{source_id}::bq::jobs"),
                        label: "recent jobs".into(),
                        kind: DbNodeKind::AdminQuery,
                        source_id: source_id.into(),
                        database: Some(project.clone()),
                        schema: None,
                        table: Some("jobs".into()),
                        query_sql: Some(
                            "SELECT job_id, state, statement_type, creation_time\n\
                             FROM INFORMATION_SCHEMA.JOBS_BY_PROJECT\n\
                             ORDER BY creation_time DESC LIMIT 25;"
                                .into(),
                        ),
                        children: Vec::new(),
                        expanded: false,
                    },
                    DbTreeNode {
                        id: format!("{source_id}::bq::tables"),
                        label: "tables (INFORMATION_SCHEMA)".into(),
                        kind: DbNodeKind::AdminQuery,
                        source_id: source_id.into(),
                        database: Some(project),
                        schema: None,
                        table: Some("tables".into()),
                        query_sql: Some(
                            "SELECT table_schema, table_name, table_type, creation_time\n\
                             FROM INFORMATION_SCHEMA.TABLES\n\
                             ORDER BY creation_time DESC LIMIT 100;"
                                .into(),
                        ),
                        children: Vec::new(),
                        expanded: false,
                    },
                ]
            }
            DbDriver::Snowflake => {
                let db = self
                    .sources
                    .iter()
                    .find(|s| s.id == source_id)
                    .map(|s| s.database.clone())
                    .filter(|d| !d.is_empty());
                let show_schemas = match &db {
                    Some(d) => format!("SHOW SCHEMAS IN DATABASE {d};"),
                    None => "SHOW SCHEMAS;".into(),
                };
                let show_tables = match &db {
                    Some(d) => format!("SHOW TABLES IN DATABASE {d};"),
                    None => "SHOW TABLES;".into(),
                };
                vec![
                    DbTreeNode {
                        id: format!("{source_id}::sf::warehouses"),
                        label: "warehouses".into(),
                        kind: DbNodeKind::AdminQuery,
                        source_id: source_id.into(),
                        database: db.clone(),
                        schema: None,
                        table: Some("warehouses".into()),
                        query_sql: Some("SHOW WAREHOUSES;".into()),
                        children: Vec::new(),
                        expanded: false,
                    },
                    DbTreeNode {
                        id: format!("{source_id}::sf::databases"),
                        label: "databases".into(),
                        kind: DbNodeKind::AdminQuery,
                        source_id: source_id.into(),
                        database: db.clone(),
                        schema: None,
                        table: Some("databases".into()),
                        query_sql: Some("SHOW DATABASES;".into()),
                        children: Vec::new(),
                        expanded: false,
                    },
                    DbTreeNode {
                        id: format!("{source_id}::sf::schemas"),
                        label: "schemas".into(),
                        kind: DbNodeKind::AdminQuery,
                        source_id: source_id.into(),
                        database: db.clone(),
                        schema: None,
                        table: Some("schemas".into()),
                        query_sql: Some(show_schemas),
                        children: Vec::new(),
                        expanded: false,
                    },
                    DbTreeNode {
                        id: format!("{source_id}::sf::tables"),
                        label: "tables".into(),
                        kind: DbNodeKind::AdminQuery,
                        source_id: source_id.into(),
                        database: db.clone(),
                        schema: None,
                        table: Some("tables".into()),
                        query_sql: Some(show_tables),
                        children: Vec::new(),
                        expanded: false,
                    },
                    DbTreeNode {
                        id: format!("{source_id}::sf::get-ddl"),
                        label: "GET_DDL (script table)".into(),
                        kind: DbNodeKind::AdminQuery,
                        source_id: source_id.into(),
                        database: db,
                        schema: None,
                        table: Some("get_ddl".into()),
                        query_sql: Some(
                            "-- Edit schema.table then run:\n\
                             SELECT GET_DDL('TABLE', 'PUBLIC.MY_TABLE');\n"
                                .into(),
                        ),
                        children: Vec::new(),
                        expanded: false,
                    },
                ]
            }
            _ => Vec::new(),
        }
    }

    async fn introspect_trino_catalogs(&self, source_id: &str) -> Result<Vec<DbTreeNode>> {
        let key = Self::pool_key(source_id, None);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Http(h) if h.driver == DbDriver::Trino => {
                http_engines::trino_catalogs(h, source_id).await
            }
            _ => Ok(Vec::new()),
        }
    }

    async fn introspect_trino_schemas(
        &self,
        source_id: &str,
        catalog: &str,
    ) -> Result<Vec<DbTreeNode>> {
        let key = Self::pool_key(source_id, None);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Http(h) if h.driver == DbDriver::Trino => {
                http_engines::trino_schemas(h, source_id, catalog).await
            }
            _ => Ok(Vec::new()),
        }
    }

    pub async fn refresh_connection_health(&mut self, source_id: &str) -> Result<()> {
        match self.ensure_connected(source_id, None).await {
            Ok(()) => {
                self.connection_health
                    .insert(source_id.to_string(), ConnHealth::ok_now());
                self.status = format!("health ok · {source_id}");
                Ok(())
            }
            Err(e) => {
                self.connection_health
                    .insert(source_id.to_string(), ConnHealth::Dead(format!("{e:#}")));
                Err(e)
            }
        }
    }

    /// Mark Ok health older than `ttl_secs` as Stale (UI badge).
    pub fn mark_stale_health(&mut self, ttl_secs: u64) {
        let now = now_millis();
        let ttl_ms = (ttl_secs as u128).saturating_mul(1000);
        for h in self.connection_health.values_mut() {
            if let ConnHealth::Ok { checked_at_ms } = *h {
                if now.saturating_sub(checked_at_ms) > ttl_ms {
                    *h = ConnHealth::Stale { checked_at_ms };
                }
            }
        }
    }

    /// Re-ping sources that are Stale or missing health (best-effort, non-fatal).
    pub async fn refresh_stale_health(&mut self) {
        self.mark_stale_health(45);
        let ids: Vec<String> = self
            .sources
            .iter()
            .filter(|s| {
                matches!(
                    self.connection_health.get(&s.id),
                    None | Some(ConnHealth::Stale { .. })
                )
            })
            .map(|s| s.id.clone())
            .take(3)
            .collect();
        for id in ids {
            let _ = self.refresh_connection_health(&id).await;
        }
    }

    pub fn remember_table(&mut self, source_id: &str, schema: &str, table: &str) {
        let key = format!("{source_id}|{schema}|{table}");
        self.recent_tables.retain(|k| k != &key);
        self.recent_tables.insert(0, key);
        if self.recent_tables.len() > 20 {
            self.recent_tables.truncate(20);
        }
    }

    pub async fn er_canvas_active_schema(&mut self) -> Result<String> {
        let sketch = self.er_sketch_active_schema().await?;
        let schema = self
            .flat
            .get(self.selected)
            .and_then(|e| e.schema.clone())
            .unwrap_or_else(|| "public".into());
        Ok(render_er_canvas(&schema, &parse_er_edges(&sketch)))
    }

    pub fn undo_last_cell_edit(&mut self) -> bool {
        let Some((row, col, prev)) = self.cell_undo.pop() else {
            return false;
        };
        let Some(result) = self.last_result.as_mut() else {
            return false;
        };
        if let Some(cell) = result.rows.get_mut(row).and_then(|r| r.get_mut(col)) {
            *cell = prev;
            result.cursor_row = row;
            result.cursor_col = col;
            self.flush_active_tab();
            true
        } else {
            false
        }
    }

    /// Jump explorer selection to `schema.table` (best-effort, case-insensitive).
    pub fn jump_to_table(&mut self, schema: &str, table: &str) -> bool {
        if let Some(idx) = self.flat.iter().position(|e| {
            e.table
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case(table))
                && (schema.is_empty()
                    || e.schema
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case(schema)))
        }) {
            self.selected = idx;
            if let Some(sid) = self.flat.get(idx).map(|e| e.source_id.clone()) {
                self.active_source = Some(sid);
            }
            true
        } else if let Some(idx) = self.flat.iter().position(|e| {
            e.table
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case(table))
        }) {
            self.selected = idx;
            if let Some(sid) = self.flat.get(idx).map(|e| e.source_id.clone()) {
                self.active_source = Some(sid);
            }
            true
        } else {
            false
        }
    }

    /// Jump using a recent-tables key `source|schema|table`.
    pub fn jump_recent_key(&mut self, key: &str) -> bool {
        let parts: Vec<&str> = key.split('|').collect();
        if parts.len() < 3 {
            return false;
        }
        self.active_source = Some(parts[0].to_string());
        self.jump_to_table(parts[1], parts[2])
    }

    fn disconnect_active_pool(&mut self) {
        let Some(source_id) = self.active_source.clone() else {
            return;
        };
        let database = self.active_database.clone();
        let key = Self::pool_key(&source_id, database.as_deref());
        if let Some(pool) = self.pools.remove(&key) {
            match pool {
                LivePool::Pg(p) => {
                    let _ = p.close();
                }
                LivePool::Mysql(p) => {
                    let _ = p.close();
                }
                LivePool::Sqlite(p) => {
                    let _ = p.close();
                }
                LivePool::Duckdb(d) => {
                    let _ = d;
                }
                _ => {}
            }
        }
        self.pools.remove(&source_id);
    }

    pub async fn fetch_result_page(&mut self, page: usize) -> Result<()> {
        self.result_page = page;
        let sql = self
            .last_page_sql
            .clone()
            .unwrap_or_else(|| self.sql.trim().to_string());
        self.execute_sql_text(&sql).await
    }

    pub async fn next_result_page(&mut self) -> Result<()> {
        let has_more = self
            .last_result
            .as_ref()
            .map(|r| r.has_more)
            .unwrap_or(false);
        if !has_more {
            bail!("no next page");
        }
        self.fetch_result_page(self.result_page + 1).await
    }

    pub async fn prev_result_page(&mut self) -> Result<()> {
        if self.result_page == 0 {
            bail!("already on first page");
        }
        self.fetch_result_page(self.result_page - 1).await
    }

    pub fn chart_last_result(&mut self, col_idx: Option<usize>) -> Result<String> {
        let Some(result) = self.last_result.as_mut() else {
            bail!("no results to chart");
        };
        let chart = chart_result(result, col_idx).context("no numeric column to chart")?;
        result.chart = Some(chart.clone());
        Ok(chart)
    }

    pub async fn materialize_last_select(
        &mut self,
        name: &str,
        as_temp: bool,
    ) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            bail!("name required");
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            bail!("use letters, digits, underscore only");
        }
        let Some(source_id) = self.active_source.clone() else {
            bail!("no active data source");
        };
        let sql = self
            .last_page_sql
            .as_deref()
            .unwrap_or_else(|| self.sql.trim());
        if !query_paging::is_paged_query(sql) {
            bail!("last query is not a SELECT — run a SELECT first");
        }
        let base = sql.trim().trim_end_matches(';');
        let driver = self
            .source_cfg(&source_id)
            .map(|s| s.driver)
            .unwrap_or_default();
        self.guard_writable(&source_id, "CREATE")?;
        let ddl = match driver {
            DbDriver::Postgres | DbDriver::Duckdb => {
                if as_temp {
                    format!("CREATE TEMP TABLE \"{name}\" AS {base}")
                } else {
                    format!("CREATE VIEW \"{name}\" AS {base}")
                }
            }
            DbDriver::Sqlite => {
                if as_temp {
                    format!("CREATE TEMP TABLE \"{name}\" AS {base}")
                } else {
                    format!("CREATE VIEW \"{name}\" AS {base}")
                }
            }
            _ => bail!("save as view/temp table supported for Postgres, SQLite, DuckDB first"),
        };
        self.execute_sql_text(&ddl).await?;
        Ok(ddl)
    }

    async fn exec_stmt_in_txn(&mut self, stmt: &str) -> Result<QueryResult> {
        if matches!(self.txn_conn, Some(TxnConnection::Mssql)) {
            let key = self
                .txn_pool_key
                .clone()
                .context("mssql txn pool key missing")?;
            let pool = self.pools.get(&key).context("not connected")?;
            let LivePool::Mssql(c) = pool else {
                bail!("expected mssql pool for txn");
            };
            return engines::exec_mssql(c, stmt).await;
        }
        let conn = self
            .txn_conn
            .as_mut()
            .context("transaction connection missing")?;
        match conn {
            TxnConnection::Pg(c) => exec_pg_conn(&mut **c, stmt).await,
            TxnConnection::Mysql(c) => exec_mysql_conn(&mut **c, stmt).await,
            TxnConnection::Sqlite(c) => exec_sqlite_conn(&mut **c, stmt).await,
            TxnConnection::Mssql => unreachable!("handled above"),
        }
    }

    /// Begin a transaction (Postgres / MySQL / SQLite / MSSQL).
    pub async fn begin_transaction(&mut self) -> Result<()> {
        if self.in_txn {
            bail!("transaction already open");
        }
        let Some(source_id) = self.active_source.clone() else {
            bail!("no active data source");
        };
        let database = self.active_database.clone();
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let key = Self::pool_key(&source_id, database.as_deref());
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let mut conn = p.acquire().await?;
                sqlx::query("BEGIN").execute(&mut *conn).await?;
                self.txn_conn = Some(TxnConnection::Pg(conn));
            }
            LivePool::Mysql(p) => {
                let mut conn = p.acquire().await?;
                sqlx::query("START TRANSACTION").execute(&mut *conn).await?;
                self.txn_conn = Some(TxnConnection::Mysql(conn));
            }
            LivePool::Sqlite(p) => {
                let mut conn = p.acquire().await?;
                sqlx::query("BEGIN").execute(&mut *conn).await?;
                self.txn_conn = Some(TxnConnection::Sqlite(conn));
            }
            LivePool::Mssql(c) => {
                let _ = engines::exec_mssql(c, "BEGIN TRANSACTION").await?;
                self.txn_conn = Some(TxnConnection::Mssql);
            }
            LivePool::Clickhouse(_)
            | LivePool::Duckdb(_)
            | LivePool::Redis(_)
            | LivePool::Mongo(_)
            | LivePool::CloudStub(_)
            | LivePool::Http(_) => {
                bail!("transactions not supported for this driver yet");
            }
        }
        self.in_txn = true;
        self.txn_pool_key = Some(key);
        self.status = "transaction started".into();
        Ok(())
    }

    pub async fn commit_transaction(&mut self) -> Result<()> {
        if !self.in_txn {
            bail!("no open transaction");
        }
        match self.txn_conn.as_mut() {
            Some(TxnConnection::Pg(c)) => {
                sqlx::query("COMMIT").execute(&mut **c).await?;
            }
            Some(TxnConnection::Mysql(c)) => {
                sqlx::query("COMMIT").execute(&mut **c).await?;
            }
            Some(TxnConnection::Sqlite(c)) => {
                sqlx::query("COMMIT").execute(&mut **c).await?;
            }
            Some(TxnConnection::Mssql) => {
                let key = self
                    .txn_pool_key
                    .clone()
                    .context("mssql txn pool key missing")?;
                let pool = self.pools.get(&key).context("not connected")?;
                let LivePool::Mssql(c) = pool else {
                    bail!("expected mssql pool");
                };
                let _ = engines::exec_mssql(c, "COMMIT").await?;
            }
            None => bail!("transaction connection missing"),
        }
        self.clear_txn();
        self.status = "transaction committed".into();
        Ok(())
    }

    pub async fn rollback_transaction(&mut self) -> Result<()> {
        if !self.in_txn {
            bail!("no open transaction");
        }
        match self.txn_conn.as_mut() {
            Some(TxnConnection::Pg(c)) => {
                sqlx::query("ROLLBACK").execute(&mut **c).await?;
            }
            Some(TxnConnection::Mysql(c)) => {
                sqlx::query("ROLLBACK").execute(&mut **c).await?;
            }
            Some(TxnConnection::Sqlite(c)) => {
                sqlx::query("ROLLBACK").execute(&mut **c).await?;
            }
            Some(TxnConnection::Mssql) => {
                let key = self
                    .txn_pool_key
                    .clone()
                    .context("mssql txn pool key missing")?;
                let pool = self.pools.get(&key).context("not connected")?;
                let LivePool::Mssql(c) = pool else {
                    bail!("expected mssql pool");
                };
                let _ = engines::exec_mssql(c, "ROLLBACK").await?;
            }
            None => bail!("transaction connection missing"),
        }
        self.clear_txn();
        self.status = "transaction rolled back".into();
        Ok(())
    }

    fn clear_txn(&mut self) {
        self.in_txn = false;
        self.txn_pool_key = None;
        self.txn_conn = None;
    }

    /// Select a table row in the explorer flat list (does not expand tree parents).
    pub fn select_table_in_explorer(
        &mut self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
    ) -> bool {
        for (i, e) in self.flat.iter().enumerate() {
            if e.source_id == source_id
                && e.database.as_deref() == database
                && e.schema.as_deref() == Some(schema)
                && e.table.as_deref() == Some(table)
                && matches!(e.kind, DbNodeKind::Table | DbNodeKind::View)
            {
                self.selected = i;
                return true;
            }
        }
        false
    }

    /// Explorer selection or bound table tab → `(source_id, database, schema, table)`.
    pub fn selected_table_context(&self) -> Option<(String, Option<String>, String, String)> {
        if let Some(e) = self.flat.get(self.selected) {
            if matches!(e.kind, DbNodeKind::Table | DbNodeKind::View) {
                if let (Some(schema), Some(table)) = (e.schema.clone(), e.table.clone()) {
                    return Some((e.source_id.clone(), e.database.clone(), schema, table));
                }
            }
        }
        if let Some(tab) = self.tabs.get(self.active_tab) {
            if let Some(key) = tab.table_key.as_deref() {
                let parts: Vec<&str> = key.splitn(3, "::").collect();
                if parts.len() == 3 {
                    let source_id = parts[0].to_string();
                    let database = if parts[1].is_empty() {
                        None
                    } else {
                        Some(parts[1].to_string())
                    };
                    if let Some((schema, table)) = parts[2].split_once('.') {
                        return Some((source_id, database, schema.to_string(), table.to_string()));
                    }
                }
            }
        }
        // Fall back to last result provenance (open table data tab / import target).
        if let Some(r) = &self.last_result {
            if let Some(table) = r.source_table.clone() {
                let schema = r
                    .source_schema
                    .clone()
                    .unwrap_or_else(|| "main".into());
                let source_id = self.active_source.clone()?;
                return Some((source_id, self.active_database.clone(), schema, table));
            }
        }
        None
    }

    pub fn select_flat_index(&mut self, idx: usize) -> bool {
        if idx < self.flat.len() {
            self.selected = idx;
            true
        } else {
            false
        }
    }

    /// Run `VACUUM` / `ANALYZE` / `REINDEX` on the selected table (Postgres · SQLite).
    pub async fn run_postgres_maintenance(&mut self, op: PostgresMaintenance) -> Result<()> {
        if self.in_txn {
            bail!("maintenance cannot run inside a transaction — commit or rollback first");
        }
        let (source_id, database, schema, table) = self
            .selected_table_context()
            .context("select a table/view in Explorer or open a table data tab")?;
        let driver = self
            .source_cfg(&source_id)
            .map(|s| s.driver)
            .unwrap_or_default();
        if !matches!(driver, DbDriver::Postgres | DbDriver::Sqlite) {
            bail!("VACUUM/ANALYZE/REINDEX supported for Postgres and SQLite");
        }
        self.guard_writable_op(&source_id, "maintenance")?;
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let key = Self::pool_key(&source_id, database.as_deref());
        let pool = self.pools.get(&key).context("not connected")?;
        let (label, sql) = match (driver, op) {
            (DbDriver::Postgres, PostgresMaintenance::VacuumAnalyze) => (
                "VACUUM (ANALYZE)",
                format!(
                    "VACUUM (ANALYZE) {}.{}",
                    pg_quote_ident(&schema),
                    pg_quote_ident(&table)
                ),
            ),
            (DbDriver::Postgres, PostgresMaintenance::Analyze) => (
                "ANALYZE",
                format!(
                    "ANALYZE {}.{}",
                    pg_quote_ident(&schema),
                    pg_quote_ident(&table)
                ),
            ),
            (DbDriver::Postgres, PostgresMaintenance::Reindex) => (
                "REINDEX TABLE",
                format!(
                    "REINDEX TABLE {}.{}",
                    pg_quote_ident(&schema),
                    pg_quote_ident(&table)
                ),
            ),
            (DbDriver::Sqlite, PostgresMaintenance::VacuumAnalyze) => {
                ("VACUUM", "VACUUM;".into())
            }
            (DbDriver::Sqlite, PostgresMaintenance::Analyze) => {
                ("ANALYZE", format!("ANALYZE \"{}\";", table.replace('"', "\"\"")))
            }
            (DbDriver::Sqlite, PostgresMaintenance::Reindex) => {
                ("REINDEX", format!("REINDEX \"{}\";", table.replace('"', "\"\"")))
            }
            _ => bail!("maintenance not supported for {}", driver.label()),
        };
        let start = std::time::Instant::now();
        let result = match pool {
            LivePool::Pg(p) => exec_pg(p, &sql).await?,
            LivePool::Sqlite(p) => exec_sqlite(p, &sql).await?,
            _ => bail!("unexpected pool for maintenance"),
        };
        let elapsed_ms = start.elapsed().as_millis();
        self.status = format!(
            "{label} {schema}.{table} · {elapsed_ms} ms · affected {}",
            result.affected.unwrap_or(0)
        );
        let mut qr = QueryResult::empty();
        qr.columns = vec!["maintenance".into(), "status".into()];
        qr.rows = vec![vec![label.into(), self.status.clone()]];
        qr.affected = result.affected;
        qr.elapsed_ms = elapsed_ms;
        qr.source_schema = Some(schema);
        qr.source_table = Some(table);
        self.last_result = Some(qr);
        Ok(())
    }

    pub async fn explain_sql(&mut self) -> Result<()> {
        let stmt = split_first_statement(self.sql.trim()).to_string();
        self.explain_sql_text(&stmt).await
    }

    pub async fn explain_sql_text(&mut self, stmt: &str) -> Result<()> {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            bail!("empty SQL");
        }
        let Some(source_id) = self.active_source.clone() else {
            bail!("no active data source");
        };
        let database = self.active_database.clone();
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let src = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .context("missing source")?;
        let explain = match src.driver {
            DbDriver::Postgres => format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) {stmt}"),
            DbDriver::Mysql => format!("EXPLAIN {stmt}"),
            DbDriver::Sqlite | DbDriver::Duckdb => format!("EXPLAIN {stmt}"),
            // SHOWPLAN_ALL → tabular plan rows in Results.
            DbDriver::Mssql => {
                format!("SET SHOWPLAN_ALL ON; {stmt}; SET SHOWPLAN_ALL OFF;")
            }
            DbDriver::Clickhouse => format!("EXPLAIN PLAN {stmt}"),
            DbDriver::Redis
            | DbDriver::Mongo
            | DbDriver::Snowflake
            | DbDriver::Bigquery
            | DbDriver::Elasticsearch
            | DbDriver::Trino => {
                bail!("EXPLAIN not supported for {}", src.driver.label());
            }
        };
        let key = Self::pool_key(&source_id, database.as_deref());
        let pool = self.pools.get(&key).context("not connected")?;
        let result = match pool {
            LivePool::Pg(p) => exec_pg(p, &explain).await?,
            LivePool::Mysql(p) => exec_mysql(p, &explain).await?,
            LivePool::Sqlite(p) => exec_sqlite(p, &explain).await?,
            LivePool::Mssql(c) => engines::exec_mssql(c, &explain).await?,
            LivePool::Clickhouse(c) => engines::exec_clickhouse(c, &explain).await?,
            LivePool::Duckdb(d) => alt_engines::exec_duckdb(d, &explain).await?,
            LivePool::Redis(r) => alt_engines::exec_redis(r, &explain).await?,
            LivePool::Mongo(m) => alt_engines::exec_mongo(m, &explain, database.as_deref()).await?,
            LivePool::CloudStub(d) => bail!(alt_engines::cloud_not_configured_message(*d)),
            LivePool::Http(_) => bail!("EXPLAIN not supported for HTTP engines"),
        };
        let text = if result.columns.is_empty() {
            "(empty plan)".into()
        } else {
            let mut lines = Vec::new();
            // Header for non-PG engines so the plan reads clearly in Results.
            if !matches!(src.driver, DbDriver::Postgres) {
                lines.push(format!("── EXPLAIN · {} ──", src.driver.label()));
                if !result.columns.is_empty() {
                    lines.push(result.columns.join(" │ "));
                    lines.push("─".repeat(result.columns.join(" │ ").len().min(72)));
                }
            }
            for row in &result.rows {
                lines.push(row.join(" │ "));
            }
            lines.join("\n")
        };
        self.last_explain = Some(text.clone());
        let structured = if src.driver == DbDriver::Postgres {
            explain_format::postgres_explain_grid(&text)
        } else {
            None
        };
        let is_structured = structured.is_some();
        let mut qr = structured.unwrap_or_else(|| {
            let mut q = QueryResult::empty();
            q.columns = vec!["plan".into()];
            q.rows = text.lines().map(|l| vec![l.to_string()]).collect();
            q.elapsed_ms = result.elapsed_ms;
            q.explain = Some(text.clone());
            q
        });
        qr.elapsed_ms = result.elapsed_ms;
        if qr.explain.is_none() {
            qr.explain = Some(text.clone());
        }
        let view = if is_structured {
            "structured"
        } else {
            match src.driver {
                DbDriver::Mysql => "mysql",
                DbDriver::Sqlite => "sqlite",
                DbDriver::Duckdb => "duckdb",
                DbDriver::Mssql => "mssql",
                DbDriver::Clickhouse => "clickhouse",
                _ => "raw",
            }
        };
        self.status = format!(
            "explain · {} lines · {view}",
            text.lines().count()
        );
        self.last_result = Some(qr);
        Ok(())
    }

    pub fn insert_result_row(&mut self) -> Result<()> {
        let Some(result) = self.last_result.as_mut() else {
            bail!("no result set");
        };
        result.insert_blank_row()?;
        self.flush_active_tab();
        self.status = "row inserted · edit cells · Ctrl-S apply INSERT".into();
        Ok(())
    }

    pub fn delete_result_row(&mut self) -> Result<()> {
        let Some(result) = self.last_result.as_mut() else {
            bail!("no result set");
        };
        if result.rows.is_empty() {
            bail!("no rows to delete");
        }
        let i = result.cursor_row.min(result.rows.len() - 1);
        if i < result.original_rows.len() {
            let sql = result.delete_sql_for_original_row(i)?;
            self.pending_row_deletes.push(sql);
            result.rows.remove(i);
            result.original_rows.remove(i);
        } else {
            // Discard unsaved insert.
            result.rows.remove(i);
        }
        if result.cursor_row >= result.rows.len() && !result.rows.is_empty() {
            result.cursor_row = result.rows.len() - 1;
        } else if result.rows.is_empty() {
            result.cursor_row = 0;
        }
        self.flush_active_tab();
        self.status = format!(
            "row deleted · {} DELETE queued · Ctrl-S apply",
            self.pending_row_deletes.len()
        );
        Ok(())
    }

    pub fn toggle_result_cell_null(&mut self) -> Result<()> {
        let Some(result) = self.last_result.as_mut() else {
            bail!("no result set");
        };
        let cur = result
            .rows
            .get(result.cursor_row)
            .and_then(|r| r.get(result.cursor_col))
            .cloned()
            .unwrap_or_default();
        let prev = if cur == "NULL" {
            result.set_cell(String::new())
        } else {
            result.set_cell_null()
        };
        if let Some(p) = prev {
            self.cell_undo
                .push((result.cursor_row, result.cursor_col, p));
        }
        self.flush_active_tab();
        self.status = "cell NULL toggled · Ctrl-S apply".into();
        Ok(())
    }

    pub fn pending_mutations(&self) -> Result<Vec<String>> {
        let Some(result) = self.last_result.as_ref() else {
            bail!("no result set");
        };
        let mut stmts = result.pending_updates().unwrap_or_default();
        stmts.extend(result.pending_inserts().unwrap_or_default());
        stmts.extend(self.pending_row_deletes.clone());
        if stmts.is_empty() {
            // Surface a clearer error if UPDATEs failed due to missing PK.
            if !result.dirty_rows().is_empty() && result.primary_keys.is_empty() {
                bail!("no primary key metadata — cannot generate UPDATE/DELETE");
            }
            bail!("no pending INSERT/UPDATE/DELETE to apply");
        }
        Ok(stmts)
    }

    pub async fn apply_pending_updates(&mut self) -> Result<usize> {
        let stmts = self.pending_mutations()?;
        let Some(source_id) = self.active_source.clone() else {
            bail!("no active data source");
        };
        let database = self.active_database.clone();
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let key = Self::pool_key(&source_id, database.as_deref());
        for stmt in &stmts {
            self.guard_writable(&source_id, stmt)?;
            let pool = self.pools.get(&key).context("not connected")?;
            match pool {
                LivePool::Pg(p) => {
                    sqlx::query(stmt).execute(p).await?;
                }
                LivePool::Mysql(p) => {
                    sqlx::query(stmt).execute(p).await?;
                }
                LivePool::Sqlite(p) => {
                    sqlx::query(stmt).execute(p).await?;
                }
                LivePool::Mssql(c) => {
                    let _ = engines::exec_mssql(c, stmt).await?;
                }
                LivePool::Clickhouse(c) => {
                    let _ = engines::exec_clickhouse(c, stmt).await?;
                }
                LivePool::Duckdb(d) => {
                    let _ = alt_engines::exec_duckdb(d, stmt).await?;
                }
                LivePool::Redis(r) => {
                    let _ = alt_engines::exec_redis(r, stmt).await?;
                }
                LivePool::Mongo(m) => {
                    let _ = alt_engines::exec_mongo(m, stmt, database.as_deref()).await?;
                }
                LivePool::CloudStub(d) => {
                    bail!(alt_engines::cloud_not_configured_message(*d));
                }
                LivePool::Http(_) => bail!("row apply not supported for HTTP engines"),
            }
        }
        if let Some(res) = self.last_result.as_mut() {
            res.original_rows = res.rows.clone();
        }
        self.pending_row_deletes.clear();
        let n = stmts.len();
        self.status = format!("applied {n} mutation(s)");
        Ok(n)
    }

    pub fn export_csv(&self, path: &std::path::Path) -> Result<()> {
        let Some(result) = &self.last_result else {
            bail!("no result to export");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let filter = self.results_filter.trim();
        let body = if filter.is_empty() {
            result.to_csv()
        } else {
            result.to_csv_filtered(Some(filter))
        };
        std::fs::write(path, body)?;
        Ok(())
    }

    pub fn export_sql(&self, path: &std::path::Path) -> Result<()> {
        let Some(result) = &self.last_result else {
            bail!("no result to export");
        };
        let table = result
            .source_table
            .clone()
            .unwrap_or_else(|| "exported_rows".into());
        let schema = result.source_schema.as_deref();
        let mysql = self
            .active_source
            .as_ref()
            .and_then(|id| self.sources.iter().find(|s| &s.id == id))
            .map(|s| matches!(s.driver, DbDriver::Mysql))
            .unwrap_or(false);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let filter = self.results_filter.trim();
        let body = if filter.is_empty() {
            result.to_sql_inserts(schema, &table, mysql)
        } else {
            result.to_sql_inserts_filtered(schema, &table, mysql, Some(filter))
        };
        std::fs::write(path, body)?;
        Ok(())
    }

    pub fn export_json(&self, path: &std::path::Path) -> Result<()> {
        let Some(result) = &self.last_result else {
            bail!("no result to export");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let filter = self.results_filter.trim();
        let body = if filter.is_empty() {
            result.to_json_filtered(None)
        } else {
            result.to_json_filtered(Some(filter))
        };
        std::fs::write(path, body)?;
        Ok(())
    }

    pub fn export_markdown(&self, path: &std::path::Path) -> Result<()> {
        let Some(result) = &self.last_result else {
            bail!("no result to export");
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let filter = self.results_filter.trim();
        let body = if filter.is_empty() {
            result.to_markdown_filtered(None)
        } else {
            result.to_markdown_filtered(Some(filter))
        };
        std::fs::write(path, body)?;
        Ok(())
    }

    /// Build ALTER TABLE … ADD/DROP/RENAME COLUMN for the selected table (caller confirms).
    pub fn alter_sql_for_selection(
        &self,
        kind: AlterColumnKind,
        spec: &str,
    ) -> Result<(String, String)> {
        let (source_id, _database, schema, table) = self
            .selected_table_context()
            .context("select a table in Explorer or open a table data tab")?;
        let driver = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| s.driver)
            .unwrap_or_default();
        let spec = spec.trim();
        if spec.is_empty() {
            bail!("empty ALTER spec");
        }
        let sql = match kind {
            AlterColumnKind::Add => {
                let (col, ty) = if let Some((c, t)) = spec.split_once([' ', ',']) {
                    (c.trim(), t.trim())
                } else {
                    (spec, "TEXT")
                };
                if col.is_empty() {
                    bail!("column name required — e.g. email TEXT");
                }
                ddl_add_column(driver, &schema, &table, col, ty)
            }
            AlterColumnKind::Drop => {
                let col = spec.split_whitespace().next().unwrap_or(spec);
                ddl_drop_column(driver, &schema, &table, col)
            }
            AlterColumnKind::Rename => {
                let (old, new) = spec
                    .split_once('=')
                    .or_else(|| spec.split_once([' ', ',']))
                    .context("rename as old=new")?;
                let old = old.trim();
                let new = new.trim();
                if old.is_empty() || new.is_empty() {
                    bail!("rename as old=new");
                }
                ddl_rename_column(driver, &schema, &table, old, new)
            }
        };
        let label = match kind {
            AlterColumnKind::Add => format!("ADD COLUMN on {schema}.{table}?"),
            AlterColumnKind::Drop => format!("DROP COLUMN on {schema}.{table}? This cannot be undone."),
            AlterColumnKind::Rename => format!("RENAME COLUMN on {schema}.{table}?"),
        };
        Ok((sql, label))
    }

    /// Run active console SQL on every ★ pinned connection; summary in Results.
    pub async fn run_fan_out_pinned(&mut self) -> Result<()> {
        let sql = self.sql.trim().to_string();
        if sql.is_empty() {
            bail!("empty SQL — write a statement first");
        }
        let targets: Vec<(String, String)> = self
            .sources
            .iter()
            .filter(|s| s.pinned)
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        if targets.is_empty() {
            bail!("pin at least one connection (★) for fan-out");
        }
        let restore = self.active_source.clone();
        let restore_db = self.active_database.clone();
        let start = std::time::Instant::now();
        let mut rows: Vec<Vec<String>> = Vec::new();
        for (id, name) in &targets {
            self.active_source = Some(id.clone());
            self.active_database = self
                .sources
                .iter()
                .find(|s| s.id == *id)
                .map(|s| s.database.clone())
                .filter(|d| !d.is_empty());
            match self.execute_sql_text(&sql).await {
                Ok(()) => {
                    let detail = self.status.clone();
                    rows.push(vec![name.clone(), "ok".into(), detail]);
                }
                Err(e) => rows.push(vec![name.clone(), "err".into(), format!("{e:#}")]),
            }
        }
        self.active_source = restore;
        self.active_database = restore_db;
        let elapsed_ms = start.elapsed().as_millis();
        let ok = rows.iter().filter(|r| r.get(1).map(|s| s.as_str()) == Some("ok")).count();
        let err = rows.len() - ok;
        let mut result = QueryResult::empty();
        result.columns = vec!["connection".into(), "status".into(), "detail".into()];
        result.rows = rows;
        result.original_rows = result.rows.clone();
        result.elapsed_ms = elapsed_ms;
        self.last_result = Some(result);
        self.status = format!("fan-out · {ok} ok · {err} err · {elapsed_ms} ms");
        Ok(())
    }

    /// Pretty-print (light) the active console SQL in place.
    pub fn format_active_sql(&mut self) -> Result<()> {
        let formatted = format_sql_light(&self.sql);
        if formatted.trim().is_empty() {
            bail!("empty SQL");
        }
        self.sql = formatted;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.sql = self.sql.clone();
        }
        self.status = "SQL formatted".into();
        Ok(())
    }

    /// Build DROP or TRUNCATE for the selected table (caller must confirm).
    pub fn destructive_sql_for_selection(&self, truncate: bool) -> Result<(String, String)> {
        let entry = self
            .flat
            .get(self.selected)
            .context("nothing selected")?;
        if !matches!(entry.kind, DbNodeKind::Table) {
            bail!("select a table (not a view)");
        }
        let schema = entry.schema.as_deref().unwrap_or("public");
        let table = entry.table.as_deref().context("missing table")?;
        let driver = self
            .sources
            .iter()
            .find(|s| s.id == entry.source_id)
            .map(|s| s.driver)
            .unwrap_or_default();
        let qual = match driver {
            DbDriver::Mysql | DbDriver::Clickhouse => {
                if let Some(db) = entry.database.as_deref() {
                    format!("`{db}`.`{table}`")
                } else {
                    format!("`{schema}`.`{table}`")
                }
            }
            DbDriver::Mssql => format!("[{schema}].[{table}]"),
            DbDriver::Sqlite => format!("\"{table}\""),
            _ => format!("\"{schema}\".\"{table}\""),
        };
        let sql = if truncate {
            match driver {
                DbDriver::Sqlite => format!("DELETE FROM {qual};"),
                _ => format!("TRUNCATE TABLE {qual};"),
            }
        } else {
            match driver {
                DbDriver::Postgres => format!("DROP TABLE IF EXISTS {qual} CASCADE;"),
                _ => format!("DROP TABLE IF EXISTS {qual};"),
            }
        };
        let label = if truncate {
            format!("TRUNCATE {schema}.{table}? This cannot be undone.")
        } else {
            format!("DROP TABLE {schema}.{table}? This cannot be undone.")
        };
        Ok((sql, label))
    }

    fn next_migration_version(&self) -> u32 {
        let mut max_v = 0u32;
        if let Ok(rd) = std::fs::read_dir(&self.queries_dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(rest) = name.strip_prefix("migrate_") {
                    let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(v) = num.parse::<u32>() {
                        max_v = max_v.max(v);
                    }
                }
            }
        }
        max_v + 1
    }

    /// Save current console SQL as versioned `queries/migrate_NNNN.sql` + `.down.sql` stub.
    pub fn save_migration_file(&mut self) -> Result<PathBuf> {
        let sql = self.sql.trim();
        if sql.is_empty() {
            bail!("empty SQL — generate schema diff DDL first");
        }
        std::fs::create_dir_all(&self.queries_dir)?;
        let ver = self.next_migration_version();
        let stamp = Local::now().format("%Y%m%d_%H%M%S");
        let stem = format!("migrate_{ver:04}_{stamp}");
        let path = self.queries_dir.join(format!("{stem}.sql"));
        let down = self.queries_dir.join(format!("{stem}.down.sql"));
        let header = format!(
            "-- tui-ide migration · v{ver:04} · {stamp}\n-- review before applying\n\n"
        );
        std::fs::write(&path, format!("{header}{sql}\n"))?;
        let down_body = format!(
            "-- tui-ide rollback stub · v{ver:04}\n\
             -- Fill inverse DDL, then palette: Rollback last migration\n\
             -- Example: DROP TABLE IF EXISTS …;\n"
        );
        std::fs::write(&down, down_body)?;
        self.status = format!(
            "migration v{ver:04} saved {} (+ .down.sql stub)",
            path.display()
        );
        Ok(path)
    }

    pub fn migration_journal_path(&self) -> PathBuf {
        self.queries_dir.join(".migrate_journal.jsonl")
    }

    fn list_migration_statements(text: &str, mssql: bool) -> Vec<String> {
        split_sql_script(text, mssql)
            .into_iter()
            .map(|s| s.trim().trim_end_matches(';').trim().to_string())
            .filter(|s| !s.is_empty() && !s.lines().all(|l| {
                let t = l.trim();
                t.is_empty() || t.starts_with("--")
            }))
            .collect()
    }

    /// Dry-run a migration file: show statements in Results without executing.
    pub fn dry_run_migration_file(&mut self, path: &Path) -> Result<()> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let mssql = self
            .active_source
            .as_ref()
            .and_then(|id| self.sources.iter().find(|s| &s.id == id))
            .map(|s| matches!(s.driver, DbDriver::Mssql))
            .unwrap_or(false);
        let stmts = Self::list_migration_statements(&text, mssql);
        let mut rows: Vec<Vec<String>> = Vec::new();
        for (i, s) in stmts.iter().enumerate() {
            let preview: String = s.chars().take(200).collect();
            rows.push(vec![(i + 1).to_string(), preview]);
        }
        let n = rows.len();
        let mut result = QueryResult::empty();
        result.columns = vec!["#".into(), "statement".into()];
        result.rows = rows;
        result.original_rows = result.rows.clone();
        self.last_result = Some(result);
        self.pending_row_deletes.clear();
        self.status = format!(
            "dry-run {} · {n} statement(s) — not executed",
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("migrate.sql")
        );
        Ok(())
    }

    /// Apply a migration SQL file and append a journal line under `queries/.migrate_journal.jsonl`.
    pub async fn apply_migration_file(&mut self, path: &Path) -> Result<()> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let Some(source_id) = self.active_source.clone() else {
            bail!("no active data source");
        };
        self.guard_writable_op(&source_id, "migration apply")?;
        let mssql = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| matches!(s.driver, DbDriver::Mssql))
            .unwrap_or(false);
        let stmts = Self::list_migration_statements(&text, mssql);
        if stmts.is_empty() {
            bail!("migration file has no executable statements");
        }
        let database = self.active_database.clone();
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let start = std::time::Instant::now();
        for stmt in &stmts {
            self.execute_sql_text(stmt).await?;
        }
        let elapsed_ms = start.elapsed().as_millis();
        std::fs::create_dir_all(&self.queries_dir)?;
        let journal = self.migration_journal_path();
        let entry = serde_json::json!({
            "applied_at": Local::now().to_rfc3339(),
            "path": path.display().to_string(),
            "source_id": source_id,
            "statements": stmts.len(),
            "elapsed_ms": elapsed_ms,
        });
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal)?;
        writeln!(f, "{entry}")?;
        self.status = format!(
            "applied migration · {} stmt(s) · {elapsed_ms} ms · journal {}",
            stmts.len(),
            journal.display()
        );
        let mut qr = QueryResult::empty();
        qr.columns = vec!["migration".into(), "status".into()];
        qr.rows = vec![vec![
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("migrate.sql")
                .into(),
            self.status.clone(),
        ]];
        qr.original_rows = qr.rows.clone();
        qr.elapsed_ms = elapsed_ms;
        self.last_result = Some(qr);
        Ok(())
    }

    /// Latest `queries/migrate_*.sql` by name (excludes `.down.sql`).
    pub fn latest_migration_path(&self) -> Option<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.queries_dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.starts_with("migrate_")
                    && name.ends_with(".sql")
                    && !name.ends_with(".down.sql")
            })
            .collect();
        files.sort();
        files.pop()
    }

    fn journal_applied_paths(&self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(self.migration_journal_path()) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for line in text.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(p) = v.get("path").and_then(|x| x.as_str()) {
                    out.push(p.to_string());
                }
            }
        }
        out
    }

    /// Show migration files vs journal (applied / pending / has-down).
    pub fn migration_status(&mut self) -> Result<()> {
        let applied: std::collections::HashSet<String> =
            self.journal_applied_paths().into_iter().collect();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.queries_dir)
            .context("read queries dir")?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.starts_with("migrate_")
                    && name.ends_with(".sql")
                    && !name.ends_with(".down.sql")
            })
            .collect();
        files.sort();
        let mut rows = Vec::new();
        for p in files {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?")
                .to_string();
            let path_s = p.display().to_string();
            let status = if applied.contains(&path_s)
                || applied.iter().any(|a| a.ends_with(&name))
            {
                "applied"
            } else {
                "pending"
            };
            let down = {
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                p.with_file_name(format!("{stem}.down.sql"))
            };
            let has_down = if down.exists() { "yes" } else { "no" };
            rows.push(vec![name, status.into(), has_down.into()]);
        }
        let n = rows.len();
        let mut result = QueryResult::empty();
        result.columns = vec!["file".into(), "status".into(), "down".into()];
        result.rows = rows;
        result.original_rows = result.rows.clone();
        self.last_result = Some(result);
        self.status = format!("migration status · {n} file(s)");
        Ok(())
    }

    /// Run sibling `.down.sql` for the last journal entry and append a rollback journal line.
    pub async fn rollback_last_migration(&mut self) -> Result<()> {
        let applied = self.journal_applied_paths();
        let Some(last_path) = applied.last().cloned() else {
            bail!("migration journal empty — nothing to rollback");
        };
        let up = PathBuf::from(&last_path);
        let stem = up.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let down = up.with_file_name(format!("{stem}.down.sql"));
        if !down.exists() {
            bail!(
                "no rollback file {} — edit the .down.sql stub then retry",
                down.display()
            );
        }
        let text = std::fs::read_to_string(&down)?;
        if Self::list_migration_statements(&text, false).is_empty()
            || text.lines().all(|l| {
                let t = l.trim();
                t.is_empty() || t.starts_with("--")
            })
        {
            bail!(
                "rollback stub {} is empty — fill inverse DDL first",
                down.display()
            );
        }
        self.apply_migration_file(&down).await?;
        // Tag journal entry as rollback
        let journal = self.migration_journal_path();
        let entry = serde_json::json!({
            "applied_at": Local::now().to_rfc3339(),
            "path": down.display().to_string(),
            "rollback_of": last_path,
            "kind": "rollback",
        });
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&journal)?;
        writeln!(f, "{entry}")?;
        self.status = format!("rolled back via {}", down.display());
        Ok(())
    }

    /// Export datasources pack (keep env:/empty only — strip plaintext passwords).
    pub fn export_connection_pack(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut sources = Vec::new();
        for s in &self.sources {
            let mut c = s.clone();
            let p = c.password.trim();
            if !(p.is_empty() || p.starts_with("env:") || p == "(vault)") {
                c.password = String::new();
            }
            sources.push(c);
        }
        let file = SourcesFile {
            sources,
            active_source: self.active_source.clone(),
        };
        std::fs::write(path, toml::to_string_pretty(&file)?)?;
        Ok(())
    }

    /// Import datasources from a pack file.
    pub fn import_connection_pack(&mut self, path: &std::path::Path) -> Result<usize> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let file: SourcesFile = toml::from_str(&text).context("parse connection pack")?;
        let mut added = 0usize;
        for mut s in file.sources {
            if self.sources.iter().any(|x| x.id == s.id) {
                s.id = uuid::Uuid::new_v4().to_string();
            }
            if self
                .sources
                .iter()
                .any(|x| x.name == s.name && x.host == s.host && x.database == s.database)
            {
                continue;
            }
            let p = s.password.trim();
            if !(p.is_empty() || p.starts_with("env:")) {
                s.password = String::new();
            }
            self.sources.push(s);
            added += 1;
        }
        if added > 0 {
            self.save_sources()?;
            self.rebuild_tree_roots();
            self.rebuild_flat();
        }
        self.status = format!("imported {added} connection(s)");
        Ok(added)
    }

    /// Human preview of pending INSERT/UPDATE/DELETE (for confirm dialog).
    pub fn pending_updates_preview(&self, max_stmts: usize) -> Result<String> {
        let stmts = self.pending_mutations()?;
        let mut lines = vec![format!("{} mutation(s):", stmts.len())];
        for (i, s) in stmts.iter().take(max_stmts).enumerate() {
            let one: String = s.chars().take(120).collect();
            lines.push(format!("{}. {one}", i + 1));
        }
        if stmts.len() > max_stmts {
            lines.push(format!("… +{} more", stmts.len() - max_stmts));
        }
        Ok(lines.join("\n"))
    }

    /// Import a CSV file into the given schema.table (or selected explorer table).
    pub async fn import_csv_file(&mut self, path: &std::path::Path) -> Result<usize> {
        let (schema, table) = self.resolve_import_target()?;
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let (headers, rows) = parse_csv(&text)?;
        if headers.is_empty() {
            bail!("CSV has no header row");
        }
        if rows.is_empty() {
            bail!("CSV has no data rows");
        }
        let Some(source_id) = self.active_source.clone() else {
            bail!("no active data source");
        };
        self.guard_writable_op(&source_id, "CSV import")?;
        let database = self.active_database.clone();
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let mysql = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| matches!(s.driver, DbDriver::Mysql))
            .unwrap_or(false);
        let q = |id: &str| -> String {
            if mysql {
                format!("`{}`", id.replace('`', "``"))
            } else {
                format!("\"{}\"", id.replace('"', "\"\""))
            }
        };
        let qual = match schema.as_deref() {
            Some(s) if !s.is_empty() && s != "main" => format!("{}.{}", q(s), q(&table)),
            _ => q(&table),
        };
        let cols = headers.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
        let key = Self::pool_key(&source_id, database.as_deref());
        let mut inserted = 0usize;
        for row in &rows {
            let mut vals = Vec::with_capacity(headers.len());
            for i in 0..headers.len() {
                let v = row.get(i).map(|s| s.as_str()).unwrap_or("");
                vals.push(sql_literal(v));
            }
            let sql = format!(
                "INSERT INTO {qual} ({cols}) VALUES ({});",
                vals.join(", ")
            );
            let pool = self.pools.get(&key).context("not connected")?;
            match pool {
                LivePool::Pg(p) => {
                    sqlx::query(&sql).execute(p).await?;
                }
                LivePool::Mysql(p) => {
                    sqlx::query(&sql).execute(p).await?;
                }
                LivePool::Sqlite(p) => {
                    sqlx::query(&sql).execute(p).await?;
                }
                LivePool::Mssql(c) => {
                    let _ = engines::exec_mssql(c, &sql).await?;
                }
                LivePool::Clickhouse(c) => {
                    let _ = engines::exec_clickhouse(c, &sql).await?;
                }
                LivePool::Duckdb(d) => {
                    let _ = alt_engines::exec_duckdb(d, &sql).await?;
                }
                LivePool::Redis(_) | LivePool::Mongo(_) | LivePool::CloudStub(_) | LivePool::Http(_) => {
                    bail!("CSV import not supported for this driver");
                }
            }
            inserted += 1;
        }
        self.status = format!("imported {inserted} row(s) → {qual}");
        Ok(inserted)
    }

    /// Import a JSON array of objects (or newline-delimited objects) into the target table.
    pub async fn import_json_file(&mut self, path: &std::path::Path) -> Result<usize> {
        let (schema, table) = self.resolve_import_target()?;
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read {}", path.display()))?;
        let (headers, rows) = parse_json_table(&text)?;
        if headers.is_empty() {
            bail!("JSON has no object keys");
        }
        if rows.is_empty() {
            bail!("JSON has no data rows");
        }
        let Some(source_id) = self.active_source.clone() else {
            bail!("no active data source");
        };
        self.guard_writable_op(&source_id, "JSON import")?;
        let database = self.active_database.clone();
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let mysql = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| matches!(s.driver, DbDriver::Mysql))
            .unwrap_or(false);
        let q = |id: &str| -> String {
            if mysql {
                format!("`{}`", id.replace('`', "``"))
            } else {
                format!("\"{}\"", id.replace('"', "\"\""))
            }
        };
        let qual = match schema.as_deref() {
            Some(s) if !s.is_empty() && s != "main" => format!("{}.{}", q(s), q(&table)),
            _ => q(&table),
        };
        let cols = headers.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
        let key = Self::pool_key(&source_id, database.as_deref());
        let mut inserted = 0usize;
        for row in &rows {
            let mut vals = Vec::with_capacity(headers.len());
            for i in 0..headers.len() {
                let v = row.get(i).map(|s| s.as_str()).unwrap_or("");
                vals.push(sql_literal(v));
            }
            let sql = format!(
                "INSERT INTO {qual} ({cols}) VALUES ({});",
                vals.join(", ")
            );
            let pool = self.pools.get(&key).context("not connected")?;
            match pool {
                LivePool::Pg(p) => {
                    sqlx::query(&sql).execute(p).await?;
                }
                LivePool::Mysql(p) => {
                    sqlx::query(&sql).execute(p).await?;
                }
                LivePool::Sqlite(p) => {
                    sqlx::query(&sql).execute(p).await?;
                }
                LivePool::Mssql(c) => {
                    let _ = engines::exec_mssql(c, &sql).await?;
                }
                LivePool::Clickhouse(c) => {
                    let _ = engines::exec_clickhouse(c, &sql).await?;
                }
                LivePool::Duckdb(d) => {
                    let _ = alt_engines::exec_duckdb(d, &sql).await?;
                }
                LivePool::Redis(_) | LivePool::Mongo(_) | LivePool::CloudStub(_) | LivePool::Http(_) => {
                    bail!("JSON import not supported for this driver");
                }
            }
            inserted += 1;
        }
        self.status = format!("imported {inserted} JSON row(s) → {qual}");
        Ok(inserted)
    }

    fn resolve_import_target(&self) -> Result<(Option<String>, String)> {
        if let Some(r) = &self.last_result {
            if let Some(t) = &r.source_table {
                return Ok((r.source_schema.clone(), t.clone()));
            }
        }
        if let Some(e) = self.flat.get(self.selected) {
            if matches!(e.kind, DbNodeKind::Table | DbNodeKind::View) {
                if let Some(t) = &e.table {
                    return Ok((e.schema.clone(), t.clone()));
                }
            }
        }
        bail!("select a table (or open a table result) before import")
    }

    async fn fetch_primary_keys(
        &self,
        source_id: &str,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>> {
        let key = Self::pool_key(source_id, self.active_database.as_deref());
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT a.attname FROM pg_index i \
                     JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
                     JOIN pg_class c ON c.oid = i.indrelid \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE i.indisprimary AND n.nspname = $1 AND c.relname = $2 \
                     ORDER BY a.attnum",
                )
                .bind(schema)
                .bind(table)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| r.try_get::<String, _>(0).ok())
                    .collect())
            }
            LivePool::Mysql(p) => {
                let q = format!("SHOW KEYS FROM `{schema}`.`{table}` WHERE Key_name = 'PRIMARY'");
                let rows = sqlx::query(&q).fetch_all(p).await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| r.try_get::<String, _>("Column_name").ok())
                    .collect())
            }
            LivePool::Sqlite(p) => {
                let q = format!("PRAGMA table_info('{table}')");
                let rows = sqlx::query(&q).fetch_all(p).await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| {
                        let pk: i64 = r.try_get(5).ok()?;
                        if pk > 0 {
                            r.try_get::<String, _>(1).ok()
                        } else {
                            None
                        }
                    })
                    .collect())
            }
            LivePool::Mssql(_)
            | LivePool::Clickhouse(_)
            | LivePool::Duckdb(_)
            | LivePool::Redis(_)
            | LivePool::Mongo(_)
            | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(Vec::new()),
        }
    }
}

const MAX_ROWS: usize = RESULT_PAGE_SIZE;

async fn exec_pg(pool: &PgPool, sql: &str) -> Result<QueryResult> {
    let mut q = exec_pg_conn(pool, sql).await?;
    if q.columns.is_empty() && is_query(sql) {
        if let Ok(mut conn) = pool.acquire().await {
            if let Ok(stmt) = conn.prepare(sql).await {
                q.columns = stmt
                    .columns()
                    .iter()
                    .map(|c| c.name().to_string())
                    .collect();
                q.affected = Some(0);
            }
        }
    }
    Ok(q)
}

async fn exec_pg_conn<'c, E>(exec: E, sql: &str) -> Result<QueryResult>
where
    E: sqlx::Executor<'c, Database = Postgres>,
{
    if is_query(sql) {
        let rows = sqlx::query(sql).fetch_all(exec).await?;
        Ok(rows_from_pg(rows))
    } else {
        let r = sqlx::query(sql).execute(exec).await?;
        let mut q = QueryResult::empty();
        q.columns = vec!["affected".into()];
        q.rows = vec![vec![r.rows_affected().to_string()]];
        q.affected = Some(r.rows_affected());
        Ok(q)
    }
}

async fn exec_mysql_conn<'c, E>(exec: E, sql: &str) -> Result<QueryResult>
where
    E: sqlx::Executor<'c, Database = MySqlDb>,
{
    if is_query(sql) {
        let rows = sqlx::query(sql).fetch_all(exec).await?;
        Ok(rows_from_mysql(rows))
    } else {
        let r = sqlx::query(sql).execute(exec).await?;
        let mut q = QueryResult::empty();
        q.columns = vec!["affected".into()];
        q.rows = vec![vec![r.rows_affected().to_string()]];
        q.affected = Some(r.rows_affected());
        Ok(q)
    }
}

async fn exec_sqlite_conn<'c, E>(exec: E, sql: &str) -> Result<QueryResult>
where
    E: sqlx::Executor<'c, Database = SqliteDb>,
{
    if is_query(sql) {
        let rows = sqlx::query(sql).fetch_all(exec).await?;
        Ok(rows_from_sqlite(rows))
    } else {
        let r = sqlx::query(sql).execute(exec).await?;
        let mut q = QueryResult::empty();
        q.columns = vec!["affected".into()];
        q.rows = vec![vec![r.rows_affected().to_string()]];
        q.affected = Some(r.rows_affected());
        Ok(q)
    }
}

async fn exec_mysql(pool: &MySqlPool, sql: &str) -> Result<QueryResult> {
    if is_query(sql) {
        let rows = sqlx::query(sql).fetch_all(pool).await?;
        let mut q = rows_from_mysql(rows);
        if q.columns.is_empty() {
            if let Ok(mut conn) = pool.acquire().await {
                if let Ok(stmt) = conn.prepare(sql).await {
                    q.columns = stmt
                        .columns()
                        .iter()
                        .map(|c| c.name().to_string())
                        .collect();
                    q.affected = Some(0);
                }
            }
        }
        Ok(q)
    } else {
        let r = sqlx::query(sql).execute(pool).await?;
        let mut q = QueryResult::empty();
        q.columns = vec!["affected".into()];
        q.rows = vec![vec![r.rows_affected().to_string()]];
        q.affected = Some(r.rows_affected());
        Ok(q)
    }
}

async fn exec_sqlite(pool: &SqlitePool, sql: &str) -> Result<QueryResult> {
    if is_query(sql) {
        let rows = sqlx::query(sql).fetch_all(pool).await?;
        let mut q = rows_from_sqlite(rows);
        if q.columns.is_empty() {
            if let Ok(mut conn) = pool.acquire().await {
                if let Ok(stmt) = conn.prepare(sql).await {
                    q.columns = stmt
                        .columns()
                        .iter()
                        .map(|c| c.name().to_string())
                        .collect();
                    q.affected = Some(0);
                }
            }
        }
        Ok(q)
    } else {
        let r = sqlx::query(sql).execute(pool).await?;
        let mut q = QueryResult::empty();
        q.columns = vec!["affected".into()];
        q.rows = vec![vec![r.rows_affected().to_string()]];
        q.affected = Some(r.rows_affected());
        Ok(q)
    }
}

fn is_query(sql: &str) -> bool {
    let t = sql.trim_start().to_ascii_lowercase();
    t.starts_with("select")
        || t.starts_with("with")
        || t.starts_with("show")
        || t.starts_with("explain")
        || t.starts_with("pragma")
        || t.starts_with("describe")
        || t.starts_with("desc")
}

fn rows_from_pg(rows: Vec<PgRow>) -> QueryResult {
    let mut q = QueryResult::empty();
    if rows.is_empty() {
        q.affected = Some(0);
        return q;
    }
    let columns: Vec<String> = rows[0]
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    let ncols = columns.len();
    q.truncated = rows.len() > MAX_ROWS;
    q.rows = rows
        .into_iter()
        .take(MAX_ROWS)
        .map(|r| (0..ncols).map(|i| pg_cell(&r, i)).collect())
        .collect();
    q.columns = columns;
    q
}

fn rows_from_mysql(rows: Vec<MySqlRow>) -> QueryResult {
    let mut q = QueryResult::empty();
    if rows.is_empty() {
        q.affected = Some(0);
        return q;
    }
    q.columns = rows[0]
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    q.truncated = rows.len() > MAX_ROWS;
    let ncols = q.columns.len();
    q.rows = rows
        .into_iter()
        .take(MAX_ROWS)
        .map(|r| (0..ncols).map(|i| mysql_cell(&r, i)).collect())
        .collect();
    q
}

fn rows_from_sqlite(rows: Vec<SqliteRow>) -> QueryResult {
    let mut q = QueryResult::empty();
    if rows.is_empty() {
        q.affected = Some(0);
        return q;
    }
    q.columns = rows[0]
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    q.truncated = rows.len() > MAX_ROWS;
    let ncols = q.columns.len();
    q.rows = rows
        .into_iter()
        .take(MAX_ROWS)
        .map(|r| (0..ncols).map(|i| sqlite_cell(&r, i)).collect())
        .collect();
    q
}

fn pg_cell(row: &PgRow, idx: usize) -> String {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
    use sqlx::types::Json;
    use uuid::Uuid;

    // Null via raw value (works for every PG type).
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return "NULL".into();
        }
    }

    if let Ok(v) = row.try_get::<String, _>(idx) {
        return v;
    }
    // INT2 / INT4 / INT8 — sqlx does not widen; try each width.
    if let Ok(v) = row.try_get::<i16, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<i32, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<i64, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<f32, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<f64, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<bool, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<Uuid, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<DateTime<Utc>, _>(idx) {
        return v.to_rfc3339();
    }
    if let Ok(v) = row.try_get::<NaiveDateTime, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<NaiveDate, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<NaiveTime, _>(idx) {
        return v.to_string();
    }
    if let Ok(Json(v)) = row.try_get::<Json<serde_json::Value>, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
        return format_blob_cell(&v);
    }
    // Last resort: type name only if nothing decoded (should be rare).
    let ty = row.column(idx).type_info().name().to_string();
    format!("<{ty}>")
}

fn mysql_cell(row: &MySqlRow, idx: usize) -> String {
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return "NULL".into();
        }
    }
    if let Ok(v) = row.try_get::<String, _>(idx) {
        return v;
    }
    if let Ok(v) = row.try_get::<i64, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<f64, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<bool, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
        return format_blob_cell(&v);
    }
    "<?>".into()
}

fn sqlite_cell(row: &SqliteRow, idx: usize) -> String {
    if let Ok(raw) = row.try_get_raw(idx) {
        if raw.is_null() {
            return "NULL".into();
        }
    }
    if let Ok(v) = row.try_get::<String, _>(idx) {
        return v;
    }
    if let Ok(v) = row.try_get::<i64, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<f64, _>(idx) {
        return v.to_string();
    }
    if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
        return format_blob_cell(&v);
    }
    "<?>".into()
}

fn format_blob_cell(v: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(v) {
        if s.chars()
            .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        {
            return s.to_string();
        }
    }
    let preview: String = v
        .iter()
        .take(32)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let more = if v.len() > 32 { "…" } else { "" };
    format!("<blob {}b: {preview}{more}>", v.len())
}

fn format_wide_cell_body(cell: &str) -> String {
    if let Some(rest) = cell.strip_prefix("<blob ") {
        if let Some((len_part, hex_part)) = rest.split_once(": ") {
            let hex = hex_part.trim_end_matches('>').trim_end_matches('…');
            return format!(
                "BLOB {len_part}\nhex:\n{}\nutf8 attempt:\n{}",
                hex.replace(' ', ""),
                String::from_utf8_lossy(
                    &hex.split_whitespace()
                        .filter_map(|h| u8::from_str_radix(h, 16).ok())
                        .collect::<Vec<_>>()
                )
            );
        }
    }
    if let Some(hex) = cell.strip_prefix("\\x") {
        return format!("BYTEA/hex ({})\n{hex}", hex.len() / 2);
    }
    cell.to_string()
}

fn parse_paste_grid(text: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let cols: Vec<String> = if line.contains('\t') {
            line.split('\t').map(|s| s.to_string()).collect()
        } else if line.contains(',') {
            // light CSV: no quote nesting
            line.split(',').map(|s| s.trim().to_string()).collect()
        } else {
            vec![line.to_string()]
        };
        out.push(cols);
    }
    out
}

fn split_first_statement(sql: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (i, c) in sql.char_indices() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ';' if !in_single && !in_double => return sql[..i].trim(),
            _ => {}
        }
    }
    sql.trim()
}

fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for c in sql.chars() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
                cur.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                cur.push(c);
            }
            ';' if !in_single && !in_double => {
                let t = cur.trim().to_string();
                if !t.is_empty() {
                    out.push(t);
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim().to_string();
    if !t.is_empty() {
        out.push(t);
    }
    out
}

/// Best-effort `FROM schema.table` / `FROM table` extractor for SELECT.
fn parse_select_target(sql: &str) -> (Option<String>, Option<String>) {
    let lower = sql.to_ascii_lowercase();
    let Some(idx) = lower.find(" from ") else {
        return (None, None);
    };
    // Byte index is safe: the matched needle is ASCII and lower keeps UTF-8 layout.
    let Some(after) = sql.get(idx + 6..) else {
        return (None, None);
    };
    let after = after.trim_start();
    let token = after
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .next()
        .unwrap_or("")
        .trim_matches('"')
        .trim_matches('`');
    if token.is_empty() {
        return (None, None);
    }
    if let Some((schema, table)) = token.split_once('.') {
        (
            Some(schema.trim_matches('"').trim_matches('`').to_string()),
            Some(table.trim_matches('"').trim_matches('`').to_string()),
        )
    } else {
        (None, Some(token.to_string()))
    }
}

fn flatten(node: &DbTreeNode, depth: usize, out: &mut Vec<FlatDbEntry>) {
    out.push(FlatDbEntry {
        id: node.id.clone(),
        label: node.label.clone(),
        kind: node.kind.clone(),
        source_id: node.source_id.clone(),
        database: node.database.clone(),
        schema: node.schema.clone(),
        table: node.table.clone(),
        query_sql: node.query_sql.clone(),
        depth,
        expanded: node.expanded,
    });
    if node.expanded {
        for c in &node.children {
            flatten(c, depth + 1, out);
        }
    }
}

fn admin_query_node(
    source_id: &str,
    slug: &str,
    label: &str,
    sql: &str,
) -> DbTreeNode {
    DbTreeNode {
        id: format!("{source_id}::admin::{slug}"),
        label: label.into(),
        kind: DbNodeKind::AdminQuery,
        source_id: source_id.into(),
        database: None,
        schema: None,
        table: Some(slug.into()),
        query_sql: Some(sql.into()),
        children: Vec::new(),
        expanded: false,
    }
}

fn routine_node(
    source_id: &str,
    database: Option<String>,
    schema: &str,
    name: &str,
    kind: &str,
) -> DbTreeNode {
    let glyph = if kind.to_ascii_uppercase().contains("TRIG") {
        "trg"
    } else if kind.to_ascii_uppercase().contains("PROC") {
        "proc"
    } else {
        "fn"
    };
    DbTreeNode {
        id: format!(
            "{source_id}::routine::{}::{schema}::{kind}::{name}",
            database.as_deref().unwrap_or("")
        ),
        label: format!("{glyph} {name}"),
        kind: DbNodeKind::Routine,
        source_id: source_id.into(),
        database,
        schema: Some(schema.into()),
        table: Some(name.into()),
        query_sql: Some(kind.into()),
        children: Vec::new(),
        expanded: false,
    }
}

pub const PG_BACKUP_HISTORY: &str = "\
SELECT archived_count, failed_count,
  last_archived_wal, last_archived_time,
  last_failed_wal, last_failed_time,
  stats_reset
FROM pg_stat_archiver;";

pub const MSSQL_BACKUP_HISTORY: &str = "\
SELECT TOP 40
  bs.database_name,
  CASE bs.type
    WHEN 'D' THEN 'Full'
    WHEN 'I' THEN 'Diff'
    WHEN 'L' THEN 'Log'
    ELSE bs.type
  END AS backup_type,
  bs.backup_start_date,
  bs.backup_finish_date,
  CAST(bs.backup_size / 1024.0 / 1024.0 AS decimal(18,1)) AS size_mb,
  bmf.physical_device_name AS device,
  bs.user_name
FROM msdb.dbo.backupset bs
LEFT JOIN msdb.dbo.backupmediafamily bmf ON bmf.media_set_id = bs.media_set_id
ORDER BY bs.backup_finish_date DESC;";

fn admin_action_node(source_id: &str, slug: &str, label: &str) -> DbTreeNode {
    DbTreeNode {
        id: format!("{source_id}::admin-action::{slug}"),
        label: label.into(),
        kind: DbNodeKind::AdminAction,
        source_id: source_id.into(),
        database: None,
        schema: None,
        table: Some(slug.into()),
        query_sql: None,
        children: Vec::new(),
        expanded: false,
    }
}

fn truncate_sql_label(sql: &str, max: usize) -> String {
    let one = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        let mut out: String = one.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Lightweight SQL pretty-printer (major keywords start a new line).
pub fn format_sql_light(sql: &str) -> String {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let break_before = [
        "select", "from", "where", "join", "left", "right", "inner", "outer", "full",
        "group", "order", "having", "limit", "offset", "union", "insert", "values",
        "update", "set", "delete", "create", "alter", "drop", "truncate", "with",
        "returning", "begin", "commit", "rollback",
    ];
    let mut out = String::new();
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        let low = t.trim_end_matches([',', ';', ')']).to_ascii_lowercase();
        let starts = break_before.contains(&low.as_str());
        if starts && !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        } else if !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        out.push_str(t);
        i += 1;
    }
    if !out.ends_with(';') {
        out.push(';');
    }
    out.push('\n');
    out
}

fn history_folder(
    source_id: &str,
    history: &[QueryHistoryEntry],
    bookmarks: &[String],
) -> DbTreeNode {
    let mut children = Vec::new();
    for (i, sql) in bookmarks.iter().rev().take(20).enumerate() {
        children.push(admin_query_node(
            source_id,
            &format!("bookmark-{i}"),
            &format!("★ {}", truncate_sql_label(sql, 36)),
            sql,
        ));
    }
    for (i, entry) in history.iter().rev().take(40).enumerate() {
        children.push(admin_query_node(
            source_id,
            &format!("hist-{i}"),
            &entry.label(40),
            &entry.sql,
        ));
    }
    if children.is_empty() {
        children.push(DbTreeNode {
            id: format!("{source_id}::admin::history-empty"),
            label: "(empty — run queries first)".into(),
            kind: DbNodeKind::AdminQuery,
            source_id: source_id.into(),
            database: None,
            schema: None,
            table: None,
            query_sql: Some("-- no history yet".into()),
            children: Vec::new(),
            expanded: false,
        });
    }
    DbTreeNode {
        id: format!("{source_id}::admin::history"),
        label: "History".into(),
        kind: DbNodeKind::Admin,
        source_id: source_id.into(),
        database: None,
        schema: None,
        table: None,
        query_sql: None,
        children,
        expanded: false,
    }
}

fn saved_queries_folder(source_id: &str, dir: &Path) -> DbTreeNode {
    let mut children = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut files: Vec<_> = rd
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("sql"))
                    .unwrap_or(false)
            })
            .collect();
        files.sort_by_key(|e| e.file_name());
        for (i, e) in files.into_iter().take(40).enumerate() {
            let name = e.file_name().to_string_lossy().to_string();
            let sql = std::fs::read_to_string(e.path()).unwrap_or_default();
            if sql.trim().is_empty() {
                continue;
            }
            children.push(admin_query_node(
                source_id,
                &format!("file-{i}-{name}"),
                &name,
                &sql,
            ));
        }
    }
    if children.is_empty() {
        children.push(DbTreeNode {
            id: format!("{source_id}::admin::saved-empty"),
            label: "(empty — Save query / bookmark)".into(),
            kind: DbNodeKind::AdminQuery,
            source_id: source_id.into(),
            database: None,
            schema: None,
            table: None,
            query_sql: Some("-- save a query to populate this folder".into()),
            children: Vec::new(),
            expanded: false,
        });
    }
    DbTreeNode {
        id: format!("{source_id}::admin::saved"),
        label: "Saved queries".into(),
        kind: DbNodeKind::Admin,
        source_id: source_id.into(),
        database: None,
        schema: None,
        table: None,
        query_sql: None,
        children,
        expanded: false,
    }
}

fn health_folder(
    source_id: &str,
    children: Vec<DbTreeNode>,
) -> DbTreeNode {
    DbTreeNode {
        id: format!("{source_id}::admin::health"),
        label: "Health".into(),
        kind: DbNodeKind::Admin,
        source_id: source_id.into(),
        database: None,
        schema: None,
        table: None,
        query_sql: None,
        children,
        expanded: true,
    }
}

fn postgres_health_folder(source_id: &str) -> DbTreeNode {
    health_folder(
        source_id,
        vec![
            admin_query_node(source_id, "health-overview", "Overview", PG_HEALTH_OVERVIEW),
            admin_query_node(source_id, "health-activity", "Activity", PG_HEALTH_ACTIVITY),
            admin_query_node(source_id, "health-blocking", "Blocking", PG_HEALTH_BLOCKING),
            admin_query_node(source_id, "health-waits", "Waits", PG_HEALTH_WAITS),
            admin_query_node(source_id, "health-bloat", "Bloat", PG_HEALTH_BLOAT),
            admin_query_node(source_id, "health-indexes", "Index health", PG_HEALTH_INDEXES),
        ],
    )
}

fn mysql_health_folder(source_id: &str) -> DbTreeNode {
    health_folder(
        source_id,
        vec![
            admin_query_node(source_id, "health-overview", "Overview", MYSQL_HEALTH_OVERVIEW),
            admin_query_node(source_id, "health-activity", "Activity", MYSQL_HEALTH_ACTIVITY),
            admin_query_node(source_id, "health-locks", "Locks / waits", MYSQL_HEALTH_LOCKS),
            admin_query_node(source_id, "health-innodb", "InnoDB status", MYSQL_HEALTH_INNODB),
            admin_query_node(
                source_id,
                "health-expensive",
                "Expensive",
                MYSQL_HEALTH_EXPENSIVE,
            ),
        ],
    )
}

fn clickhouse_health_folder(source_id: &str) -> DbTreeNode {
    health_folder(
        source_id,
        vec![
            admin_query_node(source_id, "health-overview", "Overview", CH_HEALTH_OVERVIEW),
            admin_query_node(source_id, "health-activity", "Activity", CH_HEALTH_ACTIVITY),
            admin_query_node(source_id, "health-merges", "Merges", CH_HEALTH_MERGES),
            admin_query_node(source_id, "health-parts", "Parts", CH_HEALTH_PARTS),
        ],
    )
}

/// SSMS-like Activity Monitor / Health DMVs under Admin → Health.
fn mssql_health_folder(source_id: &str) -> DbTreeNode {
    health_folder(
        source_id,
        vec![
            admin_query_node(source_id, "health-overview", "Overview", MSSQL_HEALTH_OVERVIEW),
            admin_query_node(source_id, "health-activity", "Activity", MSSQL_HEALTH_ACTIVITY),
            admin_query_node(source_id, "health-blocking", "Blocking", MSSQL_HEALTH_BLOCKING),
            admin_query_node(source_id, "health-waits", "Waits", MSSQL_HEALTH_WAITS),
            admin_query_node(
                source_id,
                "health-expensive",
                "Expensive queries",
                MSSQL_HEALTH_EXPENSIVE,
            ),
            admin_query_node(source_id, "health-io", "I/O by file", MSSQL_HEALTH_IO),
            admin_query_node(source_id, "health-jobs", "Agent Jobs", MSSQL_HEALTH_JOBS),
            admin_query_node(
                source_id,
                "health-indexes",
                "Index fragmentation",
                MSSQL_HEALTH_INDEXES,
            ),
        ],
    )
}

fn health_view_for(driver: DbDriver, which: &str) -> Result<(&'static str, &'static str, &'static str)> {
    let w = which.trim().to_ascii_lowercase();
    match driver {
        DbDriver::Postgres => Ok(match w.as_str() {
            "overview" | "" => ("health-overview", "Health · Overview", PG_HEALTH_OVERVIEW),
            "activity" => ("health-activity", "Health · Activity", PG_HEALTH_ACTIVITY),
            "blocking" => ("health-blocking", "Health · Blocking", PG_HEALTH_BLOCKING),
            "waits" => ("health-waits", "Health · Waits", PG_HEALTH_WAITS),
            "bloat" => ("health-bloat", "Health · Bloat", PG_HEALTH_BLOAT),
            "indexes" | "index" => ("health-indexes", "Health · Indexes", PG_HEALTH_INDEXES),
            other => bail!("unknown Postgres health view: {other}"),
        }),
        DbDriver::Mysql => Ok(match w.as_str() {
            "overview" | "" => ("health-overview", "Health · Overview", MYSQL_HEALTH_OVERVIEW),
            "activity" => ("health-activity", "Health · Activity", MYSQL_HEALTH_ACTIVITY),
            "locks" | "blocking" | "waits" => {
                ("health-locks", "Health · Locks", MYSQL_HEALTH_LOCKS)
            }
            "innodb" => ("health-innodb", "Health · InnoDB", MYSQL_HEALTH_INNODB),
            "expensive" => (
                "health-expensive",
                "Health · Expensive",
                MYSQL_HEALTH_EXPENSIVE,
            ),
            other => bail!("unknown MySQL health view: {other}"),
        }),
        DbDriver::Clickhouse => Ok(match w.as_str() {
            "overview" | "" => ("health-overview", "Health · Overview", CH_HEALTH_OVERVIEW),
            "activity" | "queries" => ("health-activity", "Health · Activity", CH_HEALTH_ACTIVITY),
            "merges" => ("health-merges", "Health · Merges", CH_HEALTH_MERGES),
            "parts" => ("health-parts", "Health · Parts", CH_HEALTH_PARTS),
            other => bail!("unknown ClickHouse health view: {other}"),
        }),
        DbDriver::Mssql => Ok(match w.as_str() {
            "overview" | "" => ("health-overview", "Health · Overview", MSSQL_HEALTH_OVERVIEW),
            "activity" => ("health-activity", "Health · Activity", MSSQL_HEALTH_ACTIVITY),
            "blocking" => ("health-blocking", "Health · Blocking", MSSQL_HEALTH_BLOCKING),
            "waits" => ("health-waits", "Health · Waits", MSSQL_HEALTH_WAITS),
            "expensive" => (
                "health-expensive",
                "Health · Expensive",
                MSSQL_HEALTH_EXPENSIVE,
            ),
            "io" | "i/o" => ("health-io", "Health · I/O", MSSQL_HEALTH_IO),
            "jobs" | "agent" => ("health-jobs", "Health · Agent Jobs", MSSQL_HEALTH_JOBS),
            "indexes" | "index" | "frag" => {
                ("health-indexes", "Health · Indexes", MSSQL_HEALTH_INDEXES)
            }
            other => bail!("unknown MSSQL health view: {other}"),
        }),
        _ => bail!("Health views not available for {driver:?}"),
    }
}

pub const PG_HEALTH_OVERVIEW: &str = "\
SELECT metric, value FROM (
  SELECT 1 AS ord, 'databases' AS metric, count(*)::text AS value FROM pg_database WHERE NOT datistemplate
  UNION ALL
  SELECT 2, 'backends', count(*)::text FROM pg_stat_activity
  UNION ALL
  SELECT 3, 'active', count(*)::text FROM pg_stat_activity WHERE state = 'active'
  UNION ALL
  SELECT 4, 'idle_in_xact', count(*)::text FROM pg_stat_activity WHERE state = 'idle in transaction'
  UNION ALL
  SELECT 5, 'waiting', count(*)::text FROM pg_stat_activity WHERE wait_event_type IS NOT NULL AND state <> 'idle'
  UNION ALL
  SELECT 6, 'max_connections', current_setting('max_connections')
) x ORDER BY ord;";

pub const PG_HEALTH_ACTIVITY: &str = "\
SELECT pid, usename, datname, client_addr, state, wait_event_type, wait_event,
  backend_start, xact_start, query_start,
  left(query, 160) AS query
FROM pg_stat_activity
WHERE pid <> pg_backend_pid()
ORDER BY state NULLS LAST, query_start DESC NULLS LAST;";

pub const PG_HEALTH_BLOCKING: &str = "\
SELECT blocked.pid AS blocked_pid,
  blocked.usename AS blocked_user,
  blocking.pid AS blocking_pid,
  blocking.usename AS blocking_user,
  blocked.wait_event_type,
  blocked.wait_event,
  left(blocked.query, 120) AS blocked_query,
  left(blocking.query, 120) AS blocking_query
FROM pg_stat_activity blocked
JOIN pg_locks bl ON bl.pid = blocked.pid AND NOT bl.granted
JOIN pg_locks kl ON kl.locktype = bl.locktype
  AND kl.database IS NOT DISTINCT FROM bl.database
  AND kl.relation IS NOT DISTINCT FROM bl.relation
  AND kl.page IS NOT DISTINCT FROM bl.page
  AND kl.tuple IS NOT DISTINCT FROM bl.tuple
  AND kl.virtualxid IS NOT DISTINCT FROM bl.virtualxid
  AND kl.transactionid IS NOT DISTINCT FROM bl.transactionid
  AND kl.classid IS NOT DISTINCT FROM bl.classid
  AND kl.objid IS NOT DISTINCT FROM bl.objid
  AND kl.objsubid IS NOT DISTINCT FROM bl.objsubid
  AND kl.pid <> bl.pid
  AND kl.granted
JOIN pg_stat_activity blocking ON blocking.pid = kl.pid
ORDER BY blocked.pid;";

pub const PG_HEALTH_WAITS: &str = "\
SELECT coalesce(wait_event_type, '(none)') AS wait_event_type,
  coalesce(wait_event, '(none)') AS wait_event,
  count(*) AS backends,
  count(*) FILTER (WHERE state = 'active') AS active
FROM pg_stat_activity
WHERE pid <> pg_backend_pid()
GROUP BY 1, 2
ORDER BY backends DESC
LIMIT 40;";

pub const PG_HEALTH_BLOAT: &str = "\
SELECT n.nspname AS schema,
  c.relname AS relation,
  pg_size_pretty(pg_total_relation_size(c.oid)) AS total_size,
  pg_size_pretty(pg_relation_size(c.oid)) AS table_size,
  c.reltuples::bigint AS est_rows
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind = 'r'
  AND n.nspname NOT IN ('pg_catalog', 'information_schema')
ORDER BY pg_total_relation_size(c.oid) DESC
LIMIT 40;";

pub const PG_HEALTH_INDEXES: &str = "\
SELECT schemaname, relname AS table, indexrelname AS index,
  idx_scan, idx_tup_read, idx_tup_fetch,
  pg_size_pretty(pg_relation_size(indexrelid)) AS index_size
FROM pg_stat_user_indexes
ORDER BY idx_scan ASC, pg_relation_size(indexrelid) DESC
LIMIT 40;";

pub const MYSQL_HEALTH_OVERVIEW: &str = "\
SELECT 'threads_connected' AS metric, VARIABLE_VALUE AS value
FROM performance_schema.global_status WHERE VARIABLE_NAME = 'Threads_connected'
UNION ALL
SELECT 'threads_running', VARIABLE_VALUE
FROM performance_schema.global_status WHERE VARIABLE_NAME = 'Threads_running'
UNION ALL
SELECT 'uptime', VARIABLE_VALUE
FROM performance_schema.global_status WHERE VARIABLE_NAME = 'Uptime'
UNION ALL
SELECT 'questions', VARIABLE_VALUE
FROM performance_schema.global_status WHERE VARIABLE_NAME = 'Questions'
UNION ALL
SELECT 'slow_queries', VARIABLE_VALUE
FROM performance_schema.global_status WHERE VARIABLE_NAME = 'Slow_queries'
UNION ALL
SELECT 'max_connections', VARIABLE_VALUE
FROM performance_schema.global_variables WHERE VARIABLE_NAME = 'max_connections';";

pub const MYSQL_HEALTH_ACTIVITY: &str = "\
SELECT id, user, host, db, command, time, state, left(info, 160) AS info
FROM information_schema.processlist
ORDER BY time DESC;";

pub const MYSQL_HEALTH_LOCKS: &str = "\
SELECT ENGINE_LOCK_ID, ENGINE_TRANSACTION_ID, THREAD_ID, OBJECT_SCHEMA, OBJECT_NAME,
  INDEX_NAME, LOCK_TYPE, LOCK_MODE, LOCK_STATUS, LOCK_DATA
FROM performance_schema.data_locks
LIMIT 100;";

pub const MYSQL_HEALTH_INNODB: &str = "\
SELECT VARIABLE_NAME AS metric, VARIABLE_VALUE AS value
FROM performance_schema.global_status
WHERE VARIABLE_NAME IN (
  'Innodb_buffer_pool_read_requests','Innodb_buffer_pool_reads',
  'Innodb_buffer_pool_pages_dirty','Innodb_row_lock_current_waits',
  'Innodb_row_lock_time','Innodb_rows_read','Innodb_rows_inserted',
  'Innodb_rows_updated','Innodb_rows_deleted'
)
ORDER BY VARIABLE_NAME;";

pub const MYSQL_HEALTH_EXPENSIVE: &str = "\
SELECT DIGEST_TEXT AS query,
  COUNT_STAR AS exec_count,
  ROUND(SUM_TIMER_WAIT/1e12, 3) AS total_sec,
  ROUND(AVG_TIMER_WAIT/1e12, 6) AS avg_sec,
  SUM_ROWS_EXAMINED AS rows_examined
FROM performance_schema.events_statements_summary_by_digest
ORDER BY SUM_TIMER_WAIT DESC
LIMIT 25;";

pub const CH_HEALTH_OVERVIEW: &str = "\
SELECT 'processes' AS metric, toString(count()) AS value FROM system.processes
UNION ALL
SELECT 'merges', toString(count()) FROM system.merges
UNION ALL
SELECT 'mutations', toString(count()) FROM system.mutations WHERE is_done = 0
UNION ALL
SELECT 'databases', toString(count()) FROM system.databases
WHERE name NOT IN ('system','INFORMATION_SCHEMA','information_schema')";

pub const CH_HEALTH_ACTIVITY: &str = "\
SELECT query_id, user, elapsed, read_rows, written_rows, memory_usage,
  left(query, 160) AS query
FROM system.processes
ORDER BY elapsed DESC";

pub const CH_HEALTH_MERGES: &str = "\
SELECT database, table, elapsed, progress, num_parts, is_mutation,
  total_size_bytes_compressed, bytes_read_uncompressed
FROM system.merges
ORDER BY elapsed DESC
LIMIT 50";

pub const CH_HEALTH_PARTS: &str = "\
SELECT database, table, count() AS parts,
  sum(rows) AS rows,
  formatReadableSize(sum(bytes_on_disk)) AS on_disk
FROM system.parts
WHERE active
GROUP BY database, table
ORDER BY sum(bytes_on_disk) DESC
LIMIT 40";

pub const MSSQL_HEALTH_OVERVIEW: &str = "\
SELECT metric, value FROM (
  SELECT 1 AS ord, 'user_sessions' AS metric,
    CAST(COUNT(*) AS nvarchar(32)) AS value
  FROM sys.dm_exec_sessions WHERE is_user_process = 1
  UNION ALL
  SELECT 2, 'active_requests',
    CAST(COUNT(*) AS nvarchar(32))
  FROM sys.dm_exec_requests WHERE session_id > 50
  UNION ALL
  SELECT 3, 'blocked_requests',
    CAST(SUM(CASE WHEN blocking_session_id <> 0 THEN 1 ELSE 0 END) AS nvarchar(32))
  FROM sys.dm_exec_requests WHERE session_id > 50
  UNION ALL
  SELECT 4, 'batch_requests_sec',
    CAST(cntr_value AS nvarchar(32))
  FROM sys.dm_os_performance_counters
  WHERE counter_name = 'Batch Requests/sec' AND instance_name = ''
  UNION ALL
  SELECT 5, 'page_life_expectancy',
    CAST(cntr_value AS nvarchar(32))
  FROM sys.dm_os_performance_counters
  WHERE counter_name = 'Page life expectancy' AND object_name LIKE '%Buffer Manager%'
  UNION ALL
  SELECT 6, 'total_server_memory_kb',
    CAST(cntr_value AS nvarchar(32))
  FROM sys.dm_os_performance_counters
  WHERE counter_name = 'Total Server Memory (KB)'
) x ORDER BY ord;";

pub const MSSQL_HEALTH_ACTIVITY: &str = "\
SELECT
  s.session_id,
  s.login_name,
  DB_NAME(r.database_id) AS [database],
  s.host_name,
  s.program_name,
  s.status AS session_status,
  r.status AS request_status,
  r.command,
  r.wait_type,
  r.wait_time,
  r.blocking_session_id,
  r.cpu_time,
  r.total_elapsed_time,
  r.reads,
  r.writes,
  LEFT(t.text, 160) AS query
FROM sys.dm_exec_sessions s
LEFT JOIN sys.dm_exec_requests r ON r.session_id = s.session_id
OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t
WHERE s.is_user_process = 1
ORDER BY
  CASE WHEN r.session_id IS NULL THEN 1 ELSE 0 END,
  r.total_elapsed_time DESC,
  s.session_id;";

pub const MSSQL_HEALTH_BLOCKING: &str = "\
SELECT
  r.session_id,
  r.blocking_session_id,
  DB_NAME(r.database_id) AS [database],
  r.wait_type,
  r.wait_time,
  r.status,
  r.command,
  LEFT(t.text, 160) AS query,
  s.login_name,
  s.host_name
FROM sys.dm_exec_requests r
JOIN sys.dm_exec_sessions s ON s.session_id = r.session_id
OUTER APPLY sys.dm_exec_sql_text(r.sql_handle) t
WHERE r.blocking_session_id <> 0
ORDER BY r.wait_time DESC;";

pub const MSSQL_HEALTH_WAITS: &str = "\
SELECT TOP 25
  wait_type,
  waiting_tasks_count,
  wait_time_ms,
  signal_wait_time_ms,
  wait_time_ms - signal_wait_time_ms AS resource_wait_ms
FROM sys.dm_os_wait_stats
WHERE wait_type NOT LIKE 'SLEEP%'
  AND wait_type NOT LIKE 'BROKER%'
  AND wait_type NOT IN (
    'CLR_AUTO_EVENT','CLR_MANUAL_EVENT','LAZYWRITER_SLEEP',
    'DIRTY_PAGE_POLL','HADR_FILESTREAM_IOMGR_IOCOMPLETION',
    'SQLTRACE_INCREMENTAL_FLUSH_SLEEP','XE_DISPATCHER_WAIT','XE_TIMER_EVENT'
  )
ORDER BY wait_time_ms DESC;";

pub const MSSQL_HEALTH_EXPENSIVE: &str = "\
SELECT TOP 25
  qs.total_worker_time / 1000 AS total_cpu_ms,
  qs.execution_count,
  qs.total_elapsed_time / 1000 AS total_elapsed_ms,
  qs.total_logical_reads,
  qs.total_logical_writes,
  LEFT(st.text, 160) AS query
FROM sys.dm_exec_query_stats qs
CROSS APPLY sys.dm_exec_sql_text(qs.sql_handle) st
ORDER BY qs.total_worker_time DESC;";

pub const MSSQL_HEALTH_IO: &str = "\
SELECT
  DB_NAME(vfs.database_id) AS [database],
  mf.name AS [file],
  mf.type_desc,
  vfs.num_of_reads,
  vfs.num_of_writes,
  vfs.io_stall_read_ms,
  vfs.io_stall_write_ms,
  vfs.io_stall,
  vfs.size_on_disk_bytes / 1024 / 1024 AS size_mb
FROM sys.dm_io_virtual_file_stats(NULL, NULL) vfs
JOIN sys.master_files mf
  ON mf.database_id = vfs.database_id AND mf.file_id = vfs.file_id
ORDER BY vfs.io_stall DESC;";

pub const MSSQL_HEALTH_JOBS: &str = "\
SELECT
  j.name AS job_name,
  j.enabled,
  CASE jh.run_status
    WHEN 0 THEN 'Failed'
    WHEN 1 THEN 'Succeeded'
    WHEN 2 THEN 'Retry'
    WHEN 3 THEN 'Canceled'
    WHEN 4 THEN 'In progress'
    ELSE CAST(jh.run_status AS nvarchar(16))
  END AS last_run_status,
  msdb.dbo.agent_datetime(jh.run_date, jh.run_time) AS last_run_at,
  jh.run_duration AS last_run_duration,
  jh.message AS last_message
FROM msdb.dbo.sysjobs j
OUTER APPLY (
  SELECT TOP 1 *
  FROM msdb.dbo.sysjobhistory h
  WHERE h.job_id = j.job_id AND h.step_id = 0
  ORDER BY h.instance_id DESC
) jh
ORDER BY j.name;";

pub const MSSQL_HEALTH_INDEXES: &str = "\
SELECT TOP 40
  DB_NAME() AS [database],
  OBJECT_SCHEMA_NAME(ips.object_id) AS [schema],
  OBJECT_NAME(ips.object_id) AS [table],
  i.name AS index_name,
  ips.index_type_desc,
  ips.avg_fragmentation_in_percent,
  ips.page_count
FROM sys.dm_db_index_physical_stats(DB_ID(), NULL, NULL, NULL, 'LIMITED') ips
JOIN sys.indexes i
  ON i.object_id = ips.object_id AND i.index_id = ips.index_id
WHERE ips.page_count > 100
  AND ips.avg_fragmentation_in_percent > 10
ORDER BY ips.avg_fragmentation_in_percent DESC;";

fn admin_folder_for(
    source_id: &str,
    driver: DbDriver,
    history: &[QueryHistoryEntry],
    bookmarks: &[String],
    queries_dir: &Path,
) -> DbTreeNode {
    let mut children = match driver {
        DbDriver::Postgres => {
            let mut v = vec![
                admin_query_node(
                    source_id,
                    "sessions",
                    "Sessions (legacy)",
                    "SELECT pid, usename, datname, client_addr, state, wait_event_type, wait_event,\n\
                     \tbackend_start, xact_start, query_start, left(query, 120) AS query\n\
                     FROM pg_stat_activity\n\
                     ORDER BY state NULLS LAST, query_start DESC NULLS LAST;",
                ),
                admin_query_node(
                    source_id,
                    "locks",
                    "Locks",
                    "SELECT l.locktype, l.mode, l.granted, l.pid, a.usename, a.datname,\n\
                     \tc.relname AS relation, left(a.query, 100) AS query\n\
                     FROM pg_locks l\n\
                     LEFT JOIN pg_stat_activity a ON a.pid = l.pid\n\
                     LEFT JOIN pg_class c ON c.oid = l.relation\n\
                     ORDER BY l.granted, l.pid;",
                ),
                admin_query_node(
                    source_id,
                    "roles",
                    "Roles",
                    "SELECT rolname, rolsuper, rolinherit, rolcreaterole, rolcreatedb,\n\
                     \trolcanlogin, rolreplication, rolconnlimit\n\
                     FROM pg_roles\n\
                     ORDER BY 1;",
                ),
                admin_query_node(
                    source_id,
                    "databases",
                    "Databases",
                    "SELECT d.datname, pg_catalog.pg_get_userbyid(d.datdba) AS owner,\n\
                     \tpg_catalog.pg_encoding_to_char(d.encoding) AS encoding,\n\
                     \tpg_catalog.pg_size_pretty(pg_catalog.pg_database_size(d.datname)) AS size,\n\
                     \td.datcollate, d.datctype\n\
                     FROM pg_catalog.pg_database d\n\
                     WHERE d.datistemplate = false\n\
                     ORDER BY 1;",
                ),
                admin_query_node(
                    source_id,
                    "settings",
                    "Settings",
                    "SELECT name, setting, unit, short_desc\n\
                     FROM pg_settings\n\
                     WHERE name IN (\n\
                     \t'max_connections','shared_buffers','work_mem','maintenance_work_mem',\n\
                     \t'effective_cache_size','checkpoint_completion_target','wal_buffers',\n\
                     \t'default_statistics_target','random_page_cost','effective_io_concurrency',\n\
                     \t'max_worker_processes','max_parallel_workers','listen_addresses','port',\n\
                     \t'timezone','log_min_duration_statement'\n\
                     )\n\
                     ORDER BY name;",
                ),
                admin_query_node(
                    source_id,
                    "prepared",
                    "Prepared xacts",
                    "SELECT gid, prepared, owner, database\n\
                     FROM pg_prepared_xacts\n\
                     ORDER BY prepared;",
                ),
                admin_query_node(
                    source_id,
                    "backup-history",
                    "Backup history (archiver)",
                    PG_BACKUP_HISTORY,
                ),
                admin_action_node(source_id, "vacuum-analyze", "Vacuum (analyze) table…"),
                admin_action_node(source_id, "analyze-table", "Analyze table…"),
                admin_action_node(source_id, "reindex-table", "Reindex table…"),
            ];
            v.insert(0, postgres_health_folder(source_id));
            v
        }
        DbDriver::Mysql => {
            let mut v = vec![
                admin_query_node(
                    source_id,
                    "processlist",
                    "Processlist (legacy)",
                    "SELECT id, user, host, db, command, time, state, left(info, 120) AS info\n\
                     FROM information_schema.processlist\n\
                     ORDER BY time DESC;",
                ),
                admin_query_node(
                    source_id,
                    "variables",
                    "Variables",
                    "SHOW VARIABLES WHERE Variable_name IN (\n\
                     \t'max_connections','innodb_buffer_pool_size','innodb_log_file_size',\n\
                     \t'tmp_table_size','max_allowed_packet','version','version_comment'\n\
                     );",
                ),
                admin_query_node(
                    source_id,
                    "status",
                    "Status",
                    "SHOW GLOBAL STATUS WHERE Variable_name IN (\n\
                     \t'Threads_connected','Threads_running','Questions','Uptime',\n\
                     \t'Slow_queries','Innodb_buffer_pool_read_requests','Innodb_buffer_pool_reads'\n\
                     );",
                ),
                admin_query_node(
                    source_id,
                    "engines",
                    "Engines",
                    "SHOW ENGINES;",
                ),
            ];
            v.insert(0, mysql_health_folder(source_id));
            v
        }
        DbDriver::Sqlite => vec![
            admin_query_node(
                source_id,
                "database_list",
                "Database list",
                "PRAGMA database_list;",
            ),
            admin_query_node(
                source_id,
                "compile_options",
                "Compile options",
                "PRAGMA compile_options;",
            ),
            admin_query_node(
                source_id,
                "foreign_keys",
                "Foreign keys",
                "PRAGMA foreign_keys;",
            ),
            admin_query_node(
                source_id,
                "schema",
                "sqlite_master",
                "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name;",
            ),
        ],
        DbDriver::Mssql => {
            let mut v = vec![
                admin_query_node(
                    source_id,
                    "sessions",
                    "Sessions (legacy)",
                    "SELECT session_id, login_name, host_name, status, cpu_time, total_elapsed_time,
                     	LEFT(text, 120) AS query
                     FROM sys.dm_exec_requests r
                     CROSS APPLY sys.dm_exec_sql_text(r.sql_handle)
                     ORDER BY total_elapsed_time DESC;",
                ),
                admin_query_node(
                    source_id,
                    "databases",
                    "Databases",
                    "SELECT name, state_desc, recovery_model_desc, compatibility_level
                     FROM sys.databases ORDER BY name;",
                ),
                admin_query_node(
                    source_id,
                    "tables",
                    "Top tables",
                    "SELECT TOP 50 s.name AS [schema], t.name AS [table], t.create_date
                     FROM sys.tables t JOIN sys.schemas s ON s.schema_id = t.schema_id
                     ORDER BY t.create_date DESC;",
                ),
                admin_query_node(
                    source_id,
                    "backup-history",
                    "Backup history",
                    MSSQL_BACKUP_HISTORY,
                ),
            ];
            v.insert(0, mssql_health_folder(source_id));
            v
        }
        DbDriver::Clickhouse => {
            let mut v = vec![
                admin_query_node(
                    source_id,
                    "databases",
                    "Databases",
                    "SELECT name, engine FROM system.databases ORDER BY name",
                ),
                admin_query_node(
                    source_id,
                    "processes",
                    "Processes (legacy)",
                    "SELECT query_id, user, elapsed, read_rows, written_rows, left(query, 120) AS query
                     FROM system.processes ORDER BY elapsed DESC",
                ),
                admin_query_node(
                    source_id,
                    "tables",
                    "Tables",
                    "SELECT database, name, engine, total_rows FROM system.tables
                     WHERE database NOT IN ('system','INFORMATION_SCHEMA','information_schema')
                     ORDER BY total_rows DESC NULLS LAST LIMIT 50",
                ),
                admin_query_node(
                    source_id,
                    "metrics",
                    "Metrics",
                    "SELECT metric, value FROM system.metrics ORDER BY metric LIMIT 80",
                ),
                admin_query_node(
                    source_id,
                    "parts",
                    "Parts (system.parts)",
                    "SELECT database, table, partition, rows, bytes_on_disk \
                     FROM system.parts WHERE active ORDER BY bytes_on_disk DESC LIMIT 50",
                ),
            ];
            v.insert(0, clickhouse_health_folder(source_id));
            v
        }
        DbDriver::Duckdb => vec![admin_query_node(
            source_id,
            "tables",
            "Tables",
            "SELECT table_schema, table_name, table_type FROM information_schema.tables \
             WHERE table_schema NOT IN ('information_schema','pg_catalog') ORDER BY 1, 2;",
        )],
        DbDriver::Redis => vec![
            admin_query_node(source_id, "scan", "SCAN keys", "SCAN"),
            admin_query_node(source_id, "dbsize", "DBSIZE", "DBSIZE"),
            admin_query_node(source_id, "info", "INFO server", "INFO server"),
            admin_query_node(
                source_id,
                "ping",
                "PING",
                "PING",
            ),
        ],
        DbDriver::Mongo => vec![
            admin_query_node(
                source_id,
                "dbs",
                "List databases",
                "-- expand source to list databases; then: count <coll> · indexes <coll> · find <coll>",
            ),
            admin_query_node(
                source_id,
                "count-hint",
                "count <collection>",
                "count my_collection",
            ),
            admin_query_node(
                source_id,
                "indexes-hint",
                "indexes <collection>",
                "indexes my_collection",
            ),
        ],
        DbDriver::Snowflake | DbDriver::Bigquery => Vec::new(),
        DbDriver::Elasticsearch => vec![admin_query_node(
            source_id,
            "indices",
            "Cat indices",
            "GET /_cat/indices?format=json",
        )],
        DbDriver::Trino => Vec::new(),
    };
    children.push(admin_action_node(
        source_id,
        "edit-connection",
        "Edit connection…",
    ));
    children.push(admin_action_node(
        source_id,
        "duplicate-connection",
        "Duplicate connection…",
    ));
    children.push(admin_action_node(
        source_id,
        "delete-connection",
        "Delete connection…",
    ));
    children.push(admin_action_node(source_id, "dump", "Dump database"));
    children.push(admin_action_node(source_id, "restore", "Restore dump…"));
    children.push(admin_action_node(source_id, "schema-diff", "Schema diff…"));
    children.push(admin_action_node(
        source_id,
        "schema-diff-ddl",
        "Schema diff → DDL…",
    ));
    children.push(admin_action_node(source_id, "data-compare", "Data compare…"));
    children.push(admin_action_node(
        source_id,
        "txn-begin",
        "Begin transaction",
    ));
    children.push(admin_action_node(
        source_id,
        "txn-commit",
        "Commit transaction",
    ));
    children.push(admin_action_node(
        source_id,
        "txn-rollback",
        "Rollback transaction",
    ));
    children.push(admin_action_node(source_id, "chart-results", "Chart results…"));
    children.push(admin_action_node(
        source_id,
        "save-result-temp",
        "Save result as temp table…",
    ));
    children.push(admin_action_node(
        source_id,
        "save-result-view",
        "Save result as view…",
    ));
    children.push(admin_action_node(source_id, "save-query", "Save current SQL…"));
    children.push(admin_action_node(source_id, "sync-push", "Sync queries push"));
    children.push(admin_action_node(source_id, "sync-pull", "Sync queries pull"));
    children.push(saved_queries_folder(source_id, queries_dir));
    children.push(history_folder(source_id, history, bookmarks));
    DbTreeNode {
        id: format!("{source_id}::admin"),
        label: "Admin".into(),
        kind: DbNodeKind::Admin,
        source_id: source_id.into(),
        database: None,
        schema: None,
        table: None,
        query_sql: None,
        children,
        expanded: false,
    }
}

fn toggle_expand(nodes: &mut [DbTreeNode], id: &str) -> bool {
    for n in nodes {
        if n.id == id {
            n.expanded = !n.expanded;
            return true;
        }
        if toggle_expand(&mut n.children, id) {
            return true;
        }
    }
    false
}

fn find_node_mut<'a>(nodes: &'a mut [DbTreeNode], id: &str) -> Option<&'a mut DbTreeNode> {
    for n in nodes {
        if n.id == id {
            return Some(n);
        }
        if let Some(f) = find_node_mut(&mut n.children, id) {
            return Some(f);
        }
    }
    None
}

fn find_node<'a>(nodes: &'a [DbTreeNode], id: &str) -> Option<&'a DbTreeNode> {
    for n in nodes {
        if n.id == id {
            return Some(n);
        }
        if let Some(f) = find_node(&n.children, id) {
            return Some(f);
        }
    }
    None
}

fn sources_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("tui-ide");
    p.push("datasources.toml");
    Some(p)
}

fn history_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("tui-ide");
    p.push("query_history.json");
    Some(p)
}

fn default_queries_dir() -> PathBuf {
    dirs::config_dir()
        .map(|mut p| {
            p.push("tui-ide");
            p.push("queries");
            p
        })
        .unwrap_or_else(|| PathBuf::from("queries"))
}

fn git_toplevel_for(path: &Path) -> Result<PathBuf> {
    let start = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .context("git rev-parse (is git on PATH?)")?;
    if !out.status.success() {
        bail!(
            "queries dir is not inside a git repo — init one or open a workspace with GitHub/GitLab remote"
        );
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

fn pathdiff_queries(root: &Path, queries: &Path) -> String {
    queries
        .strip_prefix(root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "queries".into())
}

fn run_git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {err}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct QueryMemoryFile {
    #[serde(default)]
    history: Vec<QueryHistoryEntry>,
    #[serde(default)]
    bookmarks: Vec<String>,
}

fn load_query_history() -> Vec<QueryHistoryEntry> {
    let Some(path) = history_path() else {
        return Vec::new();
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    if let Ok(file) = serde_json::from_str::<QueryMemoryFile>(&text) {
        return file.history;
    }
    #[derive(Deserialize)]
    struct LegacyFile {
        #[serde(default)]
        history: Vec<String>,
    }
    serde_json::from_str::<LegacyFile>(&text)
        .ok()
        .map(|f| {
            f.history
                .into_iter()
                .map(|sql| QueryHistoryEntry {
                    sql,
                    elapsed_ms: 0,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn load_query_bookmarks() -> Vec<String> {
    let Some(path) = history_path() else {
        return Vec::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<QueryMemoryFile>(&t).ok())
        .map(|f| f.bookmarks)
        .unwrap_or_default()
}

impl DbWorkspace {
    fn persist_history(&self) {
        let Some(path) = history_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = QueryMemoryFile {
            history: self.history.clone(),
            bookmarks: self.bookmarks.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&file) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn set_queries_dir(&mut self, dir: PathBuf) {
        self.queries_dir = dir;
        let _ = std::fs::create_dir_all(&self.queries_dir);
    }

    pub fn bookmark_current_sql(&mut self) {
        let sql = self.sql.trim().to_string();
        if sql.is_empty() {
            self.status = "nothing to bookmark".into();
            return;
        }
        if !self.bookmarks.iter().any(|b| b == &sql) {
            self.bookmarks.push(sql.clone());
            if self.bookmarks.len() > 40 {
                let drop_n = self.bookmarks.len() - 40;
                self.bookmarks.drain(0..drop_n);
            }
            self.persist_history();
        }
        // Also persist as a file so it can sync via GitHub/GitLab.
        let _ = self.save_query_file(None, &sql);
        self.status = "bookmarked + saved under queries/".into();
    }

    /// Save SQL to `queries_dir/{name}.sql`. Auto-names if `name` is None.
    pub fn save_query_file(&mut self, name: Option<&str>, sql: &str) -> Result<PathBuf> {
        let sql = sql.trim();
        if sql.is_empty() {
            bail!("empty SQL");
        }
        std::fs::create_dir_all(&self.queries_dir)?;
        let stem = name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                let base = truncate_sql_label(sql, 32)
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() {
                            c.to_ascii_lowercase()
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>();
                let stamp = Local::now().format("%H%M%S");
                format!("{base}_{stamp}")
            });
        let stem = stem.trim_end_matches(".sql");
        let path = self.queries_dir.join(format!("{stem}.sql"));
        std::fs::write(&path, format!("{sql}\n"))?;
        self.status = format!("saved {}", path.display());
        Ok(path)
    }

    /// `git add queries/ && commit && push` from the queries parent git root.
    pub fn sync_queries_push(&self) -> Result<String> {
        let root = git_toplevel_for(&self.queries_dir)?;
        let rel = pathdiff_queries(&root, &self.queries_dir);
        std::fs::create_dir_all(&self.queries_dir)?;
        run_git(&root, &["add", "-A", &rel])?;
        let status = run_git(&root, &["status", "--porcelain", "--", &rel])?;
        if status.trim().is_empty() {
            // Still try push in case remote is behind locally committed work.
            let out = run_git(&root, &["push"])?;
            return Ok(if out.trim().is_empty() {
                "queries up to date (pushed)".into()
            } else {
                out
            });
        }
        let msg = format!("queries: sync {}", Local::now().format("%Y-%m-%d %H:%M"));
        let _ = run_git(&root, &["commit", "-m", &msg]);
        let out = run_git(&root, &["push"])?;
        Ok(if out.trim().is_empty() {
            format!("pushed {rel}")
        } else {
            out
        })
    }

    pub fn sync_queries_pull(&self) -> Result<String> {
        let root = git_toplevel_for(&self.queries_dir)?;
        let out = run_git(&root, &["pull", "--ff-only"])?;
        Ok(if out.trim().is_empty() {
            "pulled (ff-only)".into()
        } else {
            out
        })
    }

    async fn dump_mssql(
        &mut self,
        src: &DataSource,
        source_id: &str,
        db: &str,
        out_dir: &Path,
        stamp: &str,
    ) -> Result<PathBuf> {
        if db.is_empty() {
            bail!("select an MSSQL database first");
        }
        let out = out_dir.join(format!("{db}-{stamp}.sql"));
        let local_port = if src.ssh_host.is_empty() {
            None
        } else {
            Some(self.ensure_ssh_tunnel(src)?)
        };
        let host = if local_port.is_some() {
            "127.0.0.1"
        } else if src.host.is_empty() {
            "127.0.0.1"
        } else {
            &src.host
        };
        let port = local_port.unwrap_or(if src.port == 0 { 1433 } else { src.port });
        let server = format!("{host},{port}");

        if tool_on_path("sqlcmd") {
            let script_q = "EXEC sp_MSforeachtable 'SELECT ''-- TABLE: ?'' AS [--]; SELECT * FROM ?'";
            let mut cmd = Command::new("sqlcmd");
            cmd.args(["-S", &server, "-d", db, "-I", "-b", "-r", "1", "-y", "0"]);
            if !src.user.is_empty() {
                cmd.args(["-U", &src.user]);
            }
            if !src.password.is_empty() {
                cmd.args(["-P", &src.password]);
            }
            cmd.args([
                "-o",
                out.to_str().context("path utf8")?,
                "-Q",
                script_q,
            ]);
            let status = cmd.status().context("spawn sqlcmd (is it on PATH?)")?;
            if status.success() && out.exists() && out.metadata()?.len() > 0 {
                return Ok(out);
            }
        }

        if tool_on_path("mssql-scripter") {
            let mut cmd = Command::new("mssql-scripter");
            cmd.args(["-S", &server, "-d", db, "--file-path", out.to_str().context("path utf8")?]);
            if !src.user.is_empty() {
                cmd.args(["-U", &src.user]);
            }
            if !src.password.is_empty() {
                cmd.args(["-P", &src.password]);
            }
            let status = cmd
                .status()
                .context("spawn mssql-scripter (is it on PATH?)")?;
            if status.success() {
                return Ok(out);
            }
        }

        self.ensure_connected(source_id, Some(db)).await?;
        let key = Self::pool_key(source_id, Some(db));
        let pool = self.pools.get(&key).context("not connected")?;
        let LivePool::Mssql(c) = pool else {
            bail!("expected MSSQL pool");
        };

        let mut body = format!(
            "-- tui-ide MSSQL export (sqlcmd/mssql-scripter not used or failed)\n\
             -- database: {db}\n\
             -- For large datasets prefer: sqlcmd or mssql-scripter on PATH\n\n"
        );
        let schemas = engines::mssql_schemas(c, source_id, Some(db)).await?;
        for sch_node in schemas {
            let Some(schema) = sch_node.schema else {
                continue;
            };
            if matches!(
                schema.as_str(),
                "sys" | "INFORMATION_SCHEMA" | "guest"
            ) {
                continue;
            }
            let tables = engines::mssql_tables(c, source_id, Some(db), &schema).await?;
            for t in tables {
                let Some(table) = t.table else {
                    continue;
                };
                if t.kind == DbNodeKind::View {
                    if let Ok(def) =
                        engines::mssql_show_create(c, Some(db), &schema, &table).await
                    {
                        if !def.is_empty() {
                            body.push_str(&format!("-- view {schema}.{table}\n{def};\n\n"));
                            continue;
                        }
                    }
                }
                let cols = engines::mssql_columns(c, Some(db), &schema, &table).await?;
                body.push_str(&format!(
                    "-- table {schema}.{table}\n{}\n\n",
                    engines::build_mssql_create_table(&schema, &table, &cols)
                ));
                let qual = engines::mssql_qual(&schema, &table);
                let qr = engines::exec_mssql(c, &format!("SELECT * FROM {qual}")).await?;
                if !qr.rows.is_empty() {
                    body.push_str(&mssql_inserts_from_result(&qr, &schema, &table));
                    body.push('\n');
                }
            }
        }
        std::fs::write(&out, body)?;
        Ok(out)
    }

    async fn dump_clickhouse(
        &mut self,
        src: &DataSource,
        source_id: &str,
        out_dir: &Path,
        stamp: &str,
    ) -> Result<PathBuf> {
        let db = self
            .active_database
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if src.database.is_empty() {
                    "default".into()
                } else {
                    src.database.clone()
                }
            });
        let out = out_dir.join(format!("{db}-{stamp}.sql"));
        self.ensure_connected(source_id, Some(&db)).await?;
        let key = Self::pool_key(source_id, Some(&db));
        let pool = self.pools.get(&key).context("not connected")?;
        let LivePool::Clickhouse(ch) = pool else {
            bail!("expected ClickHouse pool");
        };
        let ch = ch.with_database(&db);
        let mut body = format!(
            "-- tui-ide ClickHouse export\n-- database: {db}\n\n"
        );
        let tables = engines::ch_tables(&ch, source_id, &db).await?;
        for t in tables {
            let Some(table) = t.table else {
                continue;
            };
            let ddl = engines::ch_show_create(&ch, &db, &table).await?;
            body.push_str(&format!("-- table {db}.{table}\n{ddl};\n\n"));
            let inserts = engines::ch_query_format(
                &ch,
                &format!("SELECT * FROM `{}`.`{}`", db.replace('`', "``"), table.replace('`', "``")),
                "SQLInsert",
            )
            .await
            .unwrap_or_else(|e| format!("-- data export failed: {e:#}\n"));
            if !inserts.trim().is_empty() {
                body.push_str(&inserts);
                if !inserts.ends_with('\n') {
                    body.push('\n');
                }
                body.push('\n');
            }
        }
        std::fs::write(&out, body)?;
        Ok(out)
    }

    /// Dump active database with native tools into `out_dir`. Returns output path.
    pub async fn dump_database(&mut self, out_dir: &Path) -> Result<PathBuf> {
        let Some(source_id) = &self.active_source else {
            bail!("no active source");
        };
        let source_id = source_id.clone();
        let src = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .context("missing source")?
            .clone();
        std::fs::create_dir_all(out_dir)?;
        let stamp = Local::now().format("%Y%m%d-%H%M%S");
        match src.driver {
            DbDriver::Postgres => {
                let db = self
                    .active_database
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        if src.database.is_empty() {
                            "postgres".into()
                        } else {
                            src.database.clone()
                        }
                    });
                let out = out_dir.join(format!("{db}-{stamp}.sql"));
                let mut cmd = Command::new("pg_dump");
                cmd.env("PGPASSWORD", &src.password)
                    .args([
                        "-h",
                        &src.host,
                        "-p",
                        &src.port.to_string(),
                        "-U",
                        &src.user,
                        "-d",
                        &db,
                        "-f",
                        out.to_str().context("path utf8")?,
                        "--no-owner",
                        "--no-acl",
                    ]);
                let status = cmd.status().context("spawn pg_dump (is it on PATH?)")?;
                if !status.success() {
                    bail!("pg_dump failed with {status}");
                }
                Ok(out)
            }
            DbDriver::Mysql => {
                let db = self
                    .active_database
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| src.database.clone());
                if db.is_empty() {
                    bail!("select a MySQL database first");
                }
                let out = out_dir.join(format!("{db}-{stamp}.sql"));
                let mut cmd = Command::new("mysqldump");
                cmd.args([
                    "-h",
                    &src.host,
                    "-P",
                    &src.port.to_string(),
                    "-u",
                    &src.user,
                ]);
                if !src.password.is_empty() {
                    cmd.arg(format!("-p{}", src.password));
                }
                cmd.arg(&db)
                    .stdout(std::fs::File::create(&out).context("create dump file")?);
                let status = cmd.status().context("spawn mysqldump (is it on PATH?)")?;
                if !status.success() {
                    bail!("mysqldump failed with {status}");
                }
                Ok(out)
            }
            DbDriver::Sqlite => {
                let file = if src.file.is_empty() {
                    bail!("sqlite source has no file path")
                } else {
                    src.file.clone()
                };
                let out = out_dir.join(format!("sqlite-{stamp}.sql"));
                let output = Command::new("sqlite3")
                    .args([&file, ".dump"])
                    .output()
                    .context("spawn sqlite3 (is it on PATH?)")?;
                if !output.status.success() {
                    bail!(
                        "sqlite3 .dump failed: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
                std::fs::write(&out, output.stdout)?;
                Ok(out)
            }
            DbDriver::Mssql => {
                let db = self
                    .active_database
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| src.database.clone());
                self.dump_mssql(&src, &source_id, &db, out_dir, &stamp.to_string())
                    .await
            }
            DbDriver::Clickhouse => {
                self.dump_clickhouse(&src, &source_id, out_dir, &stamp.to_string())
                    .await
            }
            DbDriver::Duckdb | DbDriver::Redis | DbDriver::Mongo | DbDriver::Elasticsearch | DbDriver::Trino => {
                bail!("dump not supported for {} yet", src.driver.label());
            }
            DbDriver::Snowflake | DbDriver::Bigquery => {
                bail!("{}", alt_engines::cloud_not_configured_message(src.driver));
            }
        }
    }

    /// Restore a SQL dump into the active database using native clients.
    pub async fn restore_database(&mut self, dump_path: &Path) -> Result<()> {
        if !dump_path.exists() {
            bail!("dump not found: {}", dump_path.display());
        }
        let Some(source_id) = &self.active_source else {
            bail!("no active source");
        };
        let src = self
            .sources
            .iter()
            .find(|s| &s.id == source_id)
            .context("missing source")?;
        self.guard_writable_op(source_id, "restore dump")?;
        match src.driver {
            DbDriver::Postgres => {
                let db = self
                    .active_database
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        if src.database.is_empty() {
                            "postgres".into()
                        } else {
                            src.database.clone()
                        }
                    });
                let status = Command::new("psql")
                    .env("PGPASSWORD", &src.password)
                    .args([
                        "-h",
                        &src.host,
                        "-p",
                        &src.port.to_string(),
                        "-U",
                        &src.user,
                        "-d",
                        &db,
                        "-v",
                        "ON_ERROR_STOP=1",
                        "-f",
                        dump_path.to_str().context("path utf8")?,
                    ])
                    .status()
                    .context("spawn psql (is it on PATH?)")?;
                if !status.success() {
                    bail!("psql restore failed with {status}");
                }
                Ok(())
            }
            DbDriver::Mysql => {
                let db = self
                    .active_database
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| src.database.clone());
                if db.is_empty() {
                    bail!("select a MySQL database first");
                }
                let mut cmd = Command::new("mysql");
                cmd.args([
                    "-h",
                    &src.host,
                    "-P",
                    &src.port.to_string(),
                    "-u",
                    &src.user,
                ]);
                if !src.password.is_empty() {
                    cmd.arg(format!("-p{}", src.password));
                }
                cmd.arg(&db)
                    .stdin(std::fs::File::open(dump_path).context("open dump")?);
                let status = cmd.status().context("spawn mysql (is it on PATH?)")?;
                if !status.success() {
                    bail!("mysql restore failed with {status}");
                }
                Ok(())
            }
            DbDriver::Sqlite => {
                let file = if src.file.is_empty() {
                    bail!("sqlite source has no file path")
                } else {
                    src.file.clone()
                };
                let status = Command::new("sqlite3")
                    .arg(&file)
                    .stdin(std::fs::File::open(dump_path).context("open dump")?)
                    .status()
                    .context("spawn sqlite3")?;
                if !status.success() {
                    bail!("sqlite3 restore failed with {status}");
                }
                Ok(())
            }
            DbDriver::Mssql => {
                let source_id = source_id.clone();
                let active_db = self.active_database.clone();
                self.ensure_connected(&source_id, active_db.as_deref())
                    .await?;
                let key = Self::pool_key(&source_id, active_db.as_deref());
                let pool = self.pools.get(&key).context("not connected")?;
                let LivePool::Mssql(c) = pool else {
                    bail!("expected MSSQL pool");
                };
                let text = std::fs::read_to_string(dump_path).context("read dump")?;
                for stmt in split_sql_script(&text, true) {
                    engines::exec_mssql(c, &stmt).await?;
                }
                Ok(())
            }
            DbDriver::Clickhouse => {
                let source_id = source_id.clone();
                let active_db = self.active_database.clone();
                self.ensure_connected(&source_id, active_db.as_deref())
                    .await?;
                let key = Self::pool_key(&source_id, active_db.as_deref());
                let pool = self.pools.get(&key).context("not connected")?;
                let LivePool::Clickhouse(ch) = pool else {
                    bail!("expected ClickHouse pool");
                };
                let text = std::fs::read_to_string(dump_path).context("read dump")?;
                for stmt in split_sql_script(&text, false) {
                    engines::ch_exec_script(ch, &stmt).await?;
                }
                Ok(())
            }
            DbDriver::Duckdb | DbDriver::Redis | DbDriver::Mongo | DbDriver::Elasticsearch | DbDriver::Trino => {
                bail!("restore not supported for {} yet", src.driver.label());
            }
            DbDriver::Snowflake | DbDriver::Bigquery => {
                bail!("{}", alt_engines::cloud_not_configured_message(src.driver));
            }
        }
    }

    /// Kill the session/process identified by the current results cursor row.
    pub async fn kill_session_at_cursor(&mut self) -> Result<()> {
        let Some(result) = &self.last_result else {
            bail!("open Admin → Sessions/Processlist first");
        };
        let pid_col = result
            .columns
            .iter()
            .position(|c| {
                let l = c.to_ascii_lowercase();
                l == "pid"
                    || l == "id"
                    || l == "session_id"
                    || l == "spid"
                    || l == "query_id"
            })
            .context("no pid/session_id column in results")?;
        let pid = result
            .rows
            .get(result.cursor_row)
            .and_then(|r| r.get(pid_col))
            .cloned()
            .context("no row selected")?;
        let pid = pid.trim().to_string();
        if pid.is_empty() || pid == "NULL" {
            bail!("invalid pid");
        }
        let Some(source_id) = self.active_source.clone() else {
            bail!("no active source");
        };
        let driver = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| s.driver)
            .unwrap_or_default();
        let database = self.active_database.clone();
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let key = Self::pool_key(&source_id, database.as_deref());
        let pool = self.pools.get(&key).context("not connected")?;
        match (driver, pool) {
            (DbDriver::Postgres, LivePool::Pg(p)) => {
                let pid_i64: i64 = pid.parse().context("pid must be integer")?;
                let ok: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
                    .bind(pid_i64)
                    .fetch_one(p)
                    .await?;
                if !ok {
                    bail!("pg_terminate_backend returned false for pid {pid}");
                }
                self.status = format!("terminated pid {pid}");
            }
            (DbDriver::Mysql, LivePool::Mysql(p)) => {
                let sql = format!("KILL {pid}");
                sqlx::query(&sql).execute(p).await?;
                self.status = format!("killed connection {pid}");
            }
            (DbDriver::Sqlite, _) => bail!("SQLite has no server sessions to kill"),
            (DbDriver::Mssql, LivePool::Mssql(c)) => {
                let sql = format!("KILL {pid}");
                let _ = engines::exec_mssql(c, &sql).await?;
                self.status = format!("killed session {pid}");
            }
            (DbDriver::Clickhouse, LivePool::Clickhouse(c)) => {
                let sql = format!("KILL QUERY WHERE query_id = '{pid}'");
                let _ = engines::exec_clickhouse(c, &sql).await?;
                self.status = format!("killed query {pid}");
            }
            _ => bail!("driver/pool mismatch"),
        }
        Ok(())
    }

    pub fn selected_admin_action(&self) -> Option<String> {
        let e = self.flat.get(self.selected)?;
        if matches!(e.kind, DbNodeKind::AdminAction) {
            e.table.clone()
        } else {
            None
        }
    }

    /// Default prompt seed for schema diff (`left..` or `db.schema..`).
    pub fn schema_diff_prompt_seed(&self) -> String {
        if let Some(e) = self.flat.get(self.selected) {
            if matches!(e.kind, DbNodeKind::Schema) {
                if let Some(sch) = e.schema.as_deref() {
                    return match e.database.as_deref() {
                        Some(db) if !db.is_empty() => format!("{db}.{sch}.."),
                        _ => format!("{sch}.."),
                    };
                }
            }
            if let Some(sch) = e.schema.as_deref() {
                return match e.database.as_deref().or(self.active_database.as_deref()) {
                    Some(db) if !db.is_empty() => format!("{db}.{sch}.."),
                    _ => format!("{sch}.."),
                };
            }
        }
        "public..".into()
    }

    /// Compare two schemas: `left..right` where each side is `schema` or `database.schema`.
    pub async fn run_schema_diff(&mut self, spec: &str) -> Result<()> {
        let Some(source_id) = self.active_source.clone().or_else(|| {
            self.flat
                .get(self.selected)
                .map(|e| e.source_id.clone())
                .filter(|s| !s.is_empty())
        }) else {
            bail!("no active data source");
        };
        self.active_source = Some(source_id.clone());

        let (left, right) = parse_schema_diff_spec(spec, self.active_database.as_deref())?;
        let start = std::time::Instant::now();

        self.ensure_connected(&source_id, left.database.as_deref())
            .await?;
        self.ensure_connected(&source_id, right.database.as_deref())
            .await?;

        let left_tables = self
            .introspect_tables(&source_id, left.database.as_deref(), &left.schema)
            .await?;
        let right_tables = self
            .introspect_tables(&source_id, right.database.as_deref(), &right.schema)
            .await?;

        let left_names: Vec<String> = left_tables
            .iter()
            .filter_map(|t| t.table.clone())
            .collect();
        let right_names: Vec<String> = right_tables
            .iter()
            .filter_map(|t| t.table.clone())
            .collect();
        let left_set: std::collections::BTreeSet<_> = left_names.iter().cloned().collect();
        let right_set: std::collections::BTreeSet<_> = right_names.iter().cloned().collect();

        let mut rows: Vec<Vec<String>> = Vec::new();
        let left_label = side_label(&left);
        let right_label = side_label(&right);

        for t in left_set.difference(&right_set) {
            rows.push(vec![
                "-".into(),
                format!("table {t}"),
                left_label.clone(),
                String::new(),
            ]);
        }
        for t in right_set.difference(&left_set) {
            rows.push(vec![
                "+".into(),
                format!("table {t}"),
                String::new(),
                right_label.clone(),
            ]);
        }

        for t in left_set.intersection(&right_set) {
            let lcols = self
                .introspect_column_info(&source_id, left.database.as_deref(), &left.schema, t)
                .await
                .unwrap_or_default();
            let rcols = self
                .introspect_column_info(&source_id, right.database.as_deref(), &right.schema, t)
                .await
                .unwrap_or_default();
            let lmap: std::collections::BTreeMap<_, _> =
                lcols.into_iter().map(|c| (c.name.clone(), c)).collect();
            let rmap: std::collections::BTreeMap<_, _> =
                rcols.into_iter().map(|c| (c.name.clone(), c)).collect();
            let lkeys: std::collections::BTreeSet<_> = lmap.keys().cloned().collect();
            let rkeys: std::collections::BTreeSet<_> = rmap.keys().cloned().collect();

            for col in lkeys.difference(&rkeys) {
                let c = &lmap[col];
                rows.push(vec![
                    "-".into(),
                    format!("{t}.{col}"),
                    format!("{}{}", c.data_type, if c.nullable { "?" } else { "" }),
                    String::new(),
                ]);
            }
            for col in rkeys.difference(&lkeys) {
                let c = &rmap[col];
                rows.push(vec![
                    "+".into(),
                    format!("{t}.{col}"),
                    String::new(),
                    format!("{}{}", c.data_type, if c.nullable { "?" } else { "" }),
                ]);
            }
            for col in lkeys.intersection(&rkeys) {
                let lc = &lmap[col];
                let rc = &rmap[col];
                let lt = norm_type(&lc.data_type);
                let rt = norm_type(&rc.data_type);
                if lt != rt || lc.nullable != rc.nullable {
                    rows.push(vec![
                        "~".into(),
                        format!("{t}.{col}"),
                        format!("{}{}", lc.data_type, if lc.nullable { "?" } else { "" }),
                        format!("{}{}", rc.data_type, if rc.nullable { "?" } else { "" }),
                    ]);
                }
            }
        }

        let elapsed_ms = start.elapsed().as_millis();
        let n = rows.len();
        let mut result = QueryResult::empty();
        result.columns = vec![
            "change".into(),
            "object".into(),
            "left".into(),
            "right".into(),
        ];
        result.rows = rows;
        result.original_rows = result.rows.clone();
        result.elapsed_ms = elapsed_ms;
        result.source_schema = Some(left.schema.clone());
        result.source_table = Some(format!("diff→{}", right.schema));

        let title = format!("diff {left_label}..{right_label}");
        let tab_key = format!("schema-diff::{source_id}::{left_label}::{right_label}");
        self.flush_active_tab();
        if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_key) {
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.title = title.clone();
                tab.sql = format!("-- schema diff {left_label} .. {right_label}\n");
                tab.result = Some(result.clone());
                tab.source_id = Some(source_id.clone());
                tab.database = left.database.clone().or(right.database.clone());
            }
        } else {
            self.tabs.push(SqlTab {
                id: tab_key,
                title: title.clone(),
                sql: format!("-- schema diff {left_label} .. {right_label}\n"),
                result: Some(result.clone()),
                table_key: None,
                source_id: Some(source_id.clone()),
                database: left.database.clone().or(right.database.clone()),
                pinned: false,
            });
            self.active_tab = self.tabs.len() - 1;
        }
        self.sql = format!("-- schema diff {left_label} .. {right_label}\n");
        self.last_result = Some(result);
        self.structure_title = title;
        self.structure_lines = vec![
            StructureLine::section("SCHEMA DIFF"),
            StructureLine::item(format!("left  {left_label}")),
            StructureLine::item(format!("right {right_label}")),
            StructureLine::ddl("+ only right · - only left · ~ type/null drift"),
        ];
        self.structure_scroll = 0;
        self.status = if n == 0 {
            format!("schema diff · identical · {elapsed_ms} ms")
        } else {
            format!("schema diff · {n} change(s) · {elapsed_ms} ms")
        };
        Ok(())
    }

    /// Build CREATE/ALTER-ish SQL from the last schema-diff result (does not execute).
    pub async fn generate_schema_diff_ddl(&mut self) -> Result<()> {
        let result = self
            .last_result
            .clone()
            .filter(|r| {
                r.columns.len() == 4
                    && r.columns[0] == "change"
                    && r.columns[1] == "object"
                    && r.columns[2] == "left"
                    && r.columns[3] == "right"
            })
            .context("run schema diff first (Results with change/object/left/right)")?;
        let tab = self
            .tabs
            .get(self.active_tab)
            .context("no active tab")?;
        let (left_label, right_label) = parse_diff_tab_labels(&tab.title)?;
        let (left, right) = parse_schema_diff_spec(
            &format!("{left_label}..{right_label}"),
            self.active_database.as_deref(),
        )?;
        let source_id = tab
            .source_id
            .clone()
            .or_else(|| self.active_source.clone())
            .context("no source on diff tab")?;
        let driver = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| s.driver)
            .unwrap_or_default();

        let mut ddl = format!(
            "-- DDL patch: {left_label} → {right_label}\n\
             -- Generated by tui-ide (review before running)\n\n"
        );
        for row in &result.rows {
            let change = row.first().map(|s| s.as_str()).unwrap_or("");
            let object = row.get(1).map(|s| s.as_str()).unwrap_or("");
            let left_ty = row.get(2).map(|s| s.as_str()).unwrap_or("");
            let right_ty = row.get(3).map(|s| s.as_str()).unwrap_or("");
            match change {
                "+" if object.starts_with("table ") => {
                    let t = object.trim_start_matches("table ").trim();
                    let cols = self
                        .introspect_column_info(
                            &source_id,
                            right.database.as_deref(),
                            &right.schema,
                            t,
                        )
                        .await
                        .unwrap_or_default();
                    ddl.push_str(&format!("-- table only on right: {t}\n"));
                    if cols.is_empty() {
                        ddl.push_str(&format!("-- TODO: CREATE TABLE {t} (...);\n\n"));
                    } else {
                        ddl.push_str(&ddl_create_table(driver, &right.schema, t, &cols));
                        ddl.push_str("\n\n");
                    }
                }
                "-" if object.starts_with("table ") => {
                    let t = object.trim_start_matches("table ").trim();
                    ddl.push_str(&ddl_drop_table(driver, &left.schema, t));
                    ddl.push_str("\n\n");
                }
                "+" if object.contains('.') => {
                    let (t, col) = object.split_once('.').unwrap_or((object, ""));
                    ddl.push_str(&ddl_add_column(driver, &right.schema, t, col, right_ty));
                    ddl.push_str("\n\n");
                }
                "-" if object.contains('.') => {
                    let (t, col) = object.split_once('.').unwrap_or((object, ""));
                    ddl.push_str(&ddl_drop_column(driver, &left.schema, t, col));
                    ddl.push_str("\n\n");
                }
                "~" if object.contains('.') => {
                    let (t, col) = object.split_once('.').unwrap_or((object, ""));
                    ddl.push_str(&ddl_alter_column(
                        driver,
                        &left.schema,
                        t,
                        col,
                        left_ty,
                        right_ty,
                    ));
                    ddl.push_str("\n\n");
                }
                _ => {}
            }
        }

        let title = format!("ddl {left_label}→{right_label}");
        let tab_key = format!("schema-diff-ddl::{source_id}::{left_label}::{right_label}");
        self.flush_active_tab();
        if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_key) {
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.title = title.clone();
                tab.sql = ddl.clone();
                tab.result = None;
            }
        } else {
            self.tabs.push(SqlTab {
                id: tab_key,
                title: title.clone(),
                sql: ddl.clone(),
                result: None,
                table_key: None,
                source_id: Some(source_id),
                database: left.database.clone().or(right.database.clone()),
                pinned: false,
            });
            self.active_tab = self.tabs.len() - 1;
        }
        let line_count = ddl.lines().count();
        self.sql = ddl;
        self.status = format!("schema diff DDL · {line_count} line(s) — not executed");
        Ok(())
    }

    pub fn data_compare_prompt_seed(&self) -> String {
        if let Some(e) = self.flat.get(self.selected) {
            if let (Some(sch), Some(tbl)) = (e.schema.as_deref(), e.table.as_deref()) {
                return match e.database.as_deref().or(self.active_database.as_deref()) {
                    Some(db) if !db.is_empty() => format!("{db}.{sch}.{tbl}.."),
                    _ => format!("{sch}.{tbl}.."),
                };
            }
        }
        "public.my_table..".into()
    }

    /// Row-level compare between two tables (`left..right`, up to 500 rows each).
    pub async fn run_data_compare(&mut self, spec: &str) -> Result<()> {
        const LIMIT: usize = 500;
        let Some(source_id) = self.active_source.clone().or_else(|| {
            self.flat
                .get(self.selected)
                .map(|e| e.source_id.clone())
                .filter(|s| !s.is_empty())
        }) else {
            bail!("no active data source");
        };
        self.active_source = Some(source_id.clone());
        let (left, right) = parse_table_compare_spec(spec, self.active_database.as_deref())?;
        let driver = self
            .sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| s.driver)
            .unwrap_or_default();

        let start = std::time::Instant::now();
        self.ensure_connected(&source_id, left.database.as_deref())
            .await?;
        self.ensure_connected(&source_id, right.database.as_deref())
            .await?;

        let left_pack = self
            .fetch_table_sample(
                &source_id,
                driver,
                &left,
                LIMIT,
            )
            .await?;
        let right_pack = self
            .fetch_table_sample(
                &source_id,
                driver,
                &right,
                LIMIT,
            )
            .await?;

        let mut rows: Vec<Vec<String>> = Vec::new();
        let left_label = table_side_label(&left);
        let right_label = table_side_label(&right);

        let mut left_map: std::collections::BTreeMap<String, (String, String)> =
            std::collections::BTreeMap::new();
        for (key, digest, preview) in left_pack {
            left_map.insert(key, (digest, preview));
        }
        let mut right_map: std::collections::BTreeMap<String, (String, String)> =
            std::collections::BTreeMap::new();
        for (key, digest, preview) in right_pack {
            right_map.insert(key, (digest, preview));
        }

        for k in left_map.keys().filter(|k| !right_map.contains_key(*k)) {
            let (_, preview) = &left_map[k];
            rows.push(vec![
                "-".into(),
                k.clone(),
                preview.clone(),
                String::new(),
            ]);
        }
        for k in right_map.keys().filter(|k| !left_map.contains_key(*k)) {
            let (_, preview) = &right_map[k];
            rows.push(vec![
                "+".into(),
                k.clone(),
                String::new(),
                preview.clone(),
            ]);
        }
        for k in left_map.keys().filter(|k| right_map.contains_key(*k)) {
            let (ld, lp) = &left_map[k];
            let (rd, rp) = &right_map[k];
            if ld != rd {
                rows.push(vec![
                    "~".into(),
                    k.clone(),
                    lp.clone(),
                    rp.clone(),
                ]);
            }
        }

        let elapsed_ms = start.elapsed().as_millis();
        let n = rows.len();
        let mut result = QueryResult::empty();
        result.columns = vec![
            "change".into(),
            "key".into(),
            "left".into(),
            "right".into(),
        ];
        result.rows = rows;
        result.original_rows = result.rows.clone();
        result.elapsed_ms = elapsed_ms;

        let title = format!("data {left_label}..{right_label}");
        let tab_key = format!("data-compare::{source_id}::{left_label}::{right_label}");
        self.flush_active_tab();
        if let Some(idx) = self.tabs.iter().position(|t| t.id == tab_key) {
            self.active_tab = idx;
            if let Some(tab) = self.tabs.get_mut(idx) {
                tab.title = title.clone();
                tab.sql = format!(
                    "-- data compare {left_label} .. {right_label} (≤{LIMIT} rows/side)\n"
                );
                tab.result = Some(result.clone());
            }
        } else {
            self.tabs.push(SqlTab {
                id: tab_key,
                title,
                sql: format!(
                    "-- data compare {left_label} .. {right_label} (≤{LIMIT} rows/side)\n"
                ),
                result: Some(result.clone()),
                table_key: None,
                source_id: Some(source_id.clone()),
                database: left.database.clone().or(right.database.clone()),
                pinned: false,
            });
            self.active_tab = self.tabs.len() - 1;
        }
        self.last_result = Some(result);
        self.status = if n == 0 {
            format!("data compare · identical (sample) · {elapsed_ms} ms")
        } else {
            format!("data compare · {n} row diff(s) · {elapsed_ms} ms")
        };
        Ok(())
    }

    async fn fetch_table_sample(
        &self,
        source_id: &str,
        driver: DbDriver,
        side: &TableSide,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>> {
        let key = Self::pool_key(source_id, side.database.as_deref());
        let pool = self.pools.get(&key).context("not connected")?;
        let pks = self
            .table_primary_keys(source_id, side.database.as_deref(), &side.schema, &side.table)
            .await?;
        let qual = table_qual(driver, &side.schema, &side.table);
        let sql = format!("SELECT * FROM {qual} LIMIT {limit}");
        let qr = match pool {
            LivePool::Pg(p) => exec_pg(p, &sql).await?,
            LivePool::Mysql(p) => {
                let q = format!("SELECT * FROM {qual} LIMIT {limit}");
                exec_mysql(p, &q).await?
            }
            LivePool::Sqlite(p) => {
                let q = format!("SELECT * FROM {qual} LIMIT {limit}");
                exec_sqlite(p, &q).await?
            }
            LivePool::Mssql(c) => {
                if let Some(db) = side.database.as_deref() {
                    let _ = engines::exec_mssql(
                        c,
                        &format!("USE [{}]", db.replace(']', "]]")),
                    )
                    .await;
                }
                let q = format!("SELECT TOP ({limit}) * FROM {qual}");
                engines::exec_mssql(c, &q).await?
            }
            LivePool::Clickhouse(ch) => {
                let db = side.database.as_deref().unwrap_or(&side.schema);
                let q = format!(
                    "SELECT * FROM `{}`.`{}` LIMIT {limit}",
                    db.replace('`', "``"),
                    side.table.replace('`', "``")
                );
                engines::exec_clickhouse(&ch.with_database(db), &q).await?
            }
            LivePool::Duckdb(d) => alt_engines::exec_duckdb(d, &sql).await?,
            LivePool::Redis(_) | LivePool::Mongo(_) | LivePool::CloudStub(_) | LivePool::Http(_) => {
                bail!("data compare not supported for this driver");
            }
        };
        let mut out = Vec::new();
        for row in &qr.rows {
            let digest = row_digest(row);
            let preview = row_preview(row, &qr.columns);
            let key = if !pks.is_empty() {
                let idxs: Vec<usize> = pks
                    .iter()
                    .filter_map(|pk| qr.columns.iter().position(|c| c == pk))
                    .collect();
                if idxs.is_empty() {
                    digest.clone()
                } else {
                    idxs.iter()
                        .map(|i| row.get(*i).cloned().unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join("|")
                }
            } else {
                digest.clone()
            };
            out.push((key, digest, preview));
        }
        Ok(out)
    }

    async fn table_primary_keys(
        &self,
        source_id: &str,
        database: Option<&str>,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>> {
        let key = Self::pool_key(source_id, database);
        let pool = self.pools.get(&key).context("not connected")?;
        match pool {
            LivePool::Pg(p) => {
                let rows = sqlx::query(
                    "SELECT a.attname FROM pg_index i \
                     JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
                     JOIN pg_class c ON c.oid = i.indrelid \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE i.indisprimary AND n.nspname = $1 AND c.relname = $2 \
                     ORDER BY a.attnum",
                )
                .bind(schema)
                .bind(table)
                .fetch_all(p)
                .await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| r.try_get::<String, _>(0).ok())
                    .collect())
            }
            LivePool::Mysql(p) => {
                let q = format!("SHOW KEYS FROM `{schema}`.`{table}` WHERE Key_name = 'PRIMARY'");
                let rows = sqlx::query(&q).fetch_all(p).await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| r.try_get::<String, _>("Column_name").ok())
                    .collect())
            }
            LivePool::Sqlite(p) => {
                let q = format!("PRAGMA table_info('{table}')");
                let rows = sqlx::query(&q).fetch_all(p).await?;
                Ok(rows
                    .into_iter()
                    .filter_map(|r| {
                        let pk: i64 = r.try_get(5).ok()?;
                        if pk > 0 {
                            r.try_get::<String, _>(1).ok()
                        } else {
                            None
                        }
                    })
                    .collect())
            }
            LivePool::Mssql(c) => {
                engines::mssql_primary_keys(c, database, schema, table).await
            }
            LivePool::Clickhouse(_)
            | LivePool::Duckdb(_)
            | LivePool::Redis(_)
            | LivePool::Mongo(_)
            | LivePool::CloudStub(_) | LivePool::Http(_) => Ok(Vec::new()),
        }
    }
}

#[derive(Debug, Clone)]
struct DiffSide {
    database: Option<String>,
    schema: String,
}

fn side_label(s: &DiffSide) -> String {
    match &s.database {
        Some(db) if !db.is_empty() => format!("{}.{}", db, s.schema),
        _ => s.schema.clone(),
    }
}

fn norm_type(t: &str) -> String {
    t.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn parse_diff_side(raw: &str, default_db: Option<&str>) -> Result<DiffSide> {
    let s = raw.trim();
    if s.is_empty() {
        bail!("empty schema side");
    }
    if let Some((db, sch)) = s.split_once('.') {
        let db = db.trim();
        let sch = sch.trim();
        if db.is_empty() || sch.is_empty() {
            bail!("expected database.schema");
        }
        Ok(DiffSide {
            database: Some(db.into()),
            schema: sch.into(),
        })
    } else {
        Ok(DiffSide {
            database: default_db.map(|d| d.to_string()),
            schema: s.into(),
        })
    }
}

fn parse_schema_diff_spec(spec: &str, default_db: Option<&str>) -> Result<(DiffSide, DiffSide)> {
    let spec = spec.trim();
    let (left_raw, right_raw) = if let Some((a, b)) = spec.split_once("..") {
        (a.trim(), b.trim())
    } else if let Some((a, b)) = spec.split_once("->") {
        (a.trim(), b.trim())
    } else if let Some((a, b)) = spec.split_once(" vs ") {
        (a.trim(), b.trim())
    } else {
        bail!("use left..right (schema or database.schema)");
    };
    if left_raw.is_empty() || right_raw.is_empty() {
        bail!("both sides required: left..right");
    }
    Ok((
        parse_diff_side(left_raw, default_db)?,
        parse_diff_side(right_raw, default_db)?,
    ))
}

#[derive(Debug, Clone)]
struct TableSide {
    database: Option<String>,
    schema: String,
    table: String,
}

fn table_side_label(s: &TableSide) -> String {
    match &s.database {
        Some(db) if !db.is_empty() => format!("{}.{}.{}", db, s.schema, s.table),
        _ => format!("{}.{}", s.schema, s.table),
    }
}

fn parse_table_side(raw: &str, default_db: Option<&str>) -> Result<TableSide> {
    let s = raw.trim();
    if s.is_empty() {
        bail!("empty table side");
    }
    let parts: Vec<&str> = s.split('.').map(str::trim).filter(|p| !p.is_empty()).collect();
    match parts.len() {
        3 => Ok(TableSide {
            database: Some(parts[0].into()),
            schema: parts[1].into(),
            table: parts[2].into(),
        }),
        2 => Ok(TableSide {
            database: default_db.map(|d| d.to_string()),
            schema: parts[0].into(),
            table: parts[1].into(),
        }),
        1 => bail!("use schema.table or database.schema.table"),
        _ => bail!("invalid table spec: {s}"),
    }
}

fn parse_table_compare_spec(spec: &str, default_db: Option<&str>) -> Result<(TableSide, TableSide)> {
    let spec = spec.trim();
    let (left_raw, right_raw) = if let Some((a, b)) = spec.split_once("..") {
        (a.trim(), b.trim())
    } else if let Some((a, b)) = spec.split_once("->") {
        (a.trim(), b.trim())
    } else {
        bail!("use left_table..right_table (schema.table or database.schema.table)");
    };
    if left_raw.is_empty() || right_raw.is_empty() {
        bail!("both tables required: left..right");
    }
    Ok((
        parse_table_side(left_raw, default_db)?,
        parse_table_side(right_raw, default_db)?,
    ))
}

fn parse_diff_tab_labels(title: &str) -> Result<(String, String)> {
    let t = title.strip_prefix("diff ").unwrap_or(title);
    let (a, b) = t
        .split_once("..")
        .context("tab title must be diff left..right")?;
    Ok((a.trim().into(), b.trim().into()))
}

fn tool_on_path(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn split_sql_script(text: &str, mssql: bool) -> Vec<String> {
    let mut out = Vec::new();
    if mssql {
        let mut buf = String::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.eq_ignore_ascii_case("GO") {
                let stmt = buf.trim().trim_end_matches(';').trim();
                if !stmt.is_empty() {
                    out.push(stmt.to_string());
                }
                buf.clear();
            } else {
                buf.push_str(line);
                buf.push('\n');
            }
        }
        let stmt = buf.trim().trim_end_matches(';').trim();
        if !stmt.is_empty() {
            out.push(stmt.to_string());
        }
        return out;
    }
    let mut current = String::new();
    let mut in_string = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' && !in_string {
            in_string = true;
            current.push(c);
        } else if c == '\'' && in_string {
            if chars.peek() == Some(&'\'') {
                current.push('\'');
                chars.next();
            } else {
                in_string = false;
                current.push(c);
            }
        } else if c == ';' && !in_string {
            let stmt = current.trim();
            if !stmt.is_empty() {
                out.push(stmt.to_string());
            }
            current.clear();
        } else {
            current.push(c);
        }
    }
    let stmt = current.trim();
    if !stmt.is_empty() {
        out.push(stmt.to_string());
    }
    out
}

fn mssql_inserts_from_result(qr: &QueryResult, schema: &str, table: &str) -> String {
    let qual = engines::mssql_qual(schema, table);
    let cols = qr
        .columns
        .iter()
        .map(|c| format!("[{}]", c.replace(']', "]]")))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = String::new();
    for row in &qr.rows {
        let vals = row.iter().map(|v| sql_literal(v)).collect::<Vec<_>>().join(", ");
        out.push_str(&format!("INSERT INTO {qual} ({cols}) VALUES ({vals});\n"));
    }
    out
}

fn ident_quote(driver: DbDriver, id: &str) -> String {
    match driver {
        DbDriver::Mysql | DbDriver::Clickhouse => format!("`{}`", id.replace('`', "``")),
        DbDriver::Mssql => format!("[{}]", id.replace(']', "]]")),
        _ => format!("\"{}\"", id.replace('"', "\"\"")),
    }
}

fn table_qual(driver: DbDriver, schema: &str, table: &str) -> String {
    if matches!(driver, DbDriver::Clickhouse) {
        format!("{}.{}", ident_quote(driver, schema), ident_quote(driver, table))
    } else if schema.is_empty() || schema == "main" {
        ident_quote(driver, table)
    } else {
        format!(
            "{}.{}",
            ident_quote(driver, schema),
            ident_quote(driver, table)
        )
    }
}

fn ddl_create_table(driver: DbDriver, schema: &str, table: &str, cols: &[ColumnInfo]) -> String {
    let qual = table_qual(driver, schema, table);
    let body: Vec<String> = cols
        .iter()
        .map(|c| {
            let null = if c.nullable { "" } else { " NOT NULL" };
            format!(
                "  {} {}{null}",
                ident_quote(driver, &c.name),
                c.data_type
            )
        })
        .collect();
    format!("CREATE TABLE {qual} (\n{}\n);", body.join(",\n"))
}

fn ddl_drop_table(driver: DbDriver, schema: &str, table: &str) -> String {
    let qual = table_qual(driver, schema, table);
    match driver {
        DbDriver::Mssql => format!("DROP TABLE IF EXISTS {qual};"),
        DbDriver::Mysql | DbDriver::Clickhouse => format!("DROP TABLE IF EXISTS {qual};"),
        _ => format!("DROP TABLE IF EXISTS {qual} CASCADE;"),
    }
}

fn ddl_add_column(driver: DbDriver, schema: &str, table: &str, col: &str, ty: &str) -> String {
    let qual = table_qual(driver, schema, table);
    let ty = ty.trim();
    let ty = if ty.is_empty() { "text" } else { ty };
    format!(
        "ALTER TABLE {qual} ADD COLUMN {} {ty};",
        ident_quote(driver, col)
    )
}

fn ddl_drop_column(driver: DbDriver, schema: &str, table: &str, col: &str) -> String {
    let qual = table_qual(driver, schema, table);
    format!(
        "ALTER TABLE {qual} DROP COLUMN {};",
        ident_quote(driver, col)
    )
}

fn ddl_rename_column(
    driver: DbDriver,
    schema: &str,
    table: &str,
    old: &str,
    new: &str,
) -> String {
    let qual = table_qual(driver, schema, table);
    match driver {
        DbDriver::Postgres | DbDriver::Sqlite | DbDriver::Duckdb => format!(
            "ALTER TABLE {qual} RENAME COLUMN {} TO {};",
            ident_quote(driver, old),
            ident_quote(driver, new)
        ),
        DbDriver::Mysql => format!(
            "ALTER TABLE {qual} RENAME COLUMN {} TO {};",
            ident_quote(driver, old),
            ident_quote(driver, new)
        ),
        DbDriver::Mssql => format!(
            "EXEC sp_rename N'{}.{}', N'{}', N'COLUMN';",
            schema.replace('\'', "''"),
            old.replace('\'', "''"),
            new.replace('\'', "''")
        ),
        DbDriver::Clickhouse => format!(
            "ALTER TABLE {qual} RENAME COLUMN {} TO {};",
            ident_quote(driver, old),
            ident_quote(driver, new)
        ),
        _ => format!("-- RENAME COLUMN not supported for {}", driver.label()),
    }
}

fn ddl_alter_column(
    driver: DbDriver,
    schema: &str,
    table: &str,
    col: &str,
    _from: &str,
    to: &str,
) -> String {
    let qual = table_qual(driver, schema, table);
    let to = to.trim();
    let ty = if to.is_empty() { "text" } else { to.trim_end_matches('?') };
    match driver {
        DbDriver::Postgres => format!(
            "ALTER TABLE {qual} ALTER COLUMN {} TYPE {ty};",
            ident_quote(driver, col)
        ),
        DbDriver::Mysql | DbDriver::Clickhouse => format!(
            "ALTER TABLE {qual} MODIFY COLUMN {} {ty};",
            ident_quote(driver, col)
        ),
        DbDriver::Mssql => format!(
            "ALTER TABLE {qual} ALTER COLUMN {} {ty};",
            ident_quote(driver, col)
        ),
        DbDriver::Sqlite | DbDriver::Duckdb => {
            format!("-- SQLite/DuckDB: recreate table to change {} to {ty}", col)
        }
        DbDriver::Redis
        | DbDriver::Mongo
        | DbDriver::Snowflake
        | DbDriver::Bigquery
        | DbDriver::Elasticsearch
        | DbDriver::Trino => {
            format!("-- DDL not supported for {}", driver.label())
        }
    }
}

fn row_digest(row: &[String]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    for v in row {
        v.hash(&mut h);
    }
    format!("{:x}", h.finish())
}

fn row_preview(row: &[String], cols: &[String]) -> String {
    let mut parts = Vec::new();
    for (i, v) in row.iter().enumerate() {
        let name = cols.get(i).map(|s| s.as_str()).unwrap_or("?");
        parts.push(format!("{name}={v}"));
    }
    let s = parts.join(", ");
    if s.len() > 120 {
        format!("{}…", &s[..117])
    } else {
        s
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct SourcesFile {
    #[serde(default)]
    sources: Vec<DataSource>,
    /// Last active connection id (restored on boot).
    #[serde(default)]
    active_source: Option<String>,
}

pub fn default_brain_source() -> DataSource {
    let mut s = DataSource::postgres_local("brain", "brain");
    s.user = std::env::var("USER").unwrap_or_else(|_| "postgres".into());
    // Common local DSN used in this environment
    if let Ok(dsn) = std::env::var("BRAIN_DSN") {
        if let Some(parsed) = parse_postgres_url(&dsn) {
            return parsed;
        }
    }
    s.name = format!("brain ({})", Local::now().format("%H:%M"));
    s
}

fn parse_postgres_url(url: &str) -> Option<DataSource> {
    // postgres://user@host:port/db
    let url = url.strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))?;
    let (auth_host, database) = url.split_once('/')?;
    let database = database.split('?').next()?.to_string();
    let (user, hostport) = if let Some((u, rest)) = auth_host.split_once('@') {
        let user = u.split(':').next()?.to_string();
        (user, rest)
    } else {
        ("postgres".into(), auth_host)
    };
    let (host, port) = if let Some((h, p)) = hostport.split_once(':') {
        (h.to_string(), p.parse().unwrap_or(5432))
    } else {
        (hostport.to_string(), 5432)
    };
    Some(DataSource {
        id: uuid::Uuid::new_v4().to_string(),
        name: database.clone(),
        driver: DbDriver::Postgres,
        host,
        port,
        user,
        password: String::new(),
        database,
        file: String::new(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_port: 22,
        ssh_key: String::new(),
        ssl_mode: default_ssl_mode(),
        ssl_trust_cert: true,
        ssl_ca: String::new(),
        read_only: false,
        role: SourceRole::Admin,
        group: String::new(),
        accent: String::new(),
        pinned: false,
    })
}

fn looks_like_ddl(sql: &str) -> bool {
    let head = strip_sql_comments(sql)
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        head.as_str(),
        "create" | "alter" | "drop" | "truncate" | "grant" | "revoke" | "comment"
    )
}

pub fn parse_er_edges(sketch: &str) -> Vec<(String, String, String, String)> {
    let mut edges = Vec::new();
    for line in sketch.lines() {
        if let Some((left, right)) = line.split_once("-->") {
            let lb: Vec<&str> = left.trim().split('.').collect();
            let rb: Vec<&str> = right.trim().split('.').collect();
            if lb.len() >= 2 && rb.len() >= 2 {
                edges.push((
                    lb[0].to_string(),
                    lb[1].to_string(),
                    rb[rb.len() - 2].to_string(),
                    rb[rb.len() - 1].to_string(),
                ));
            }
        }
    }
    edges
}

/// Row visible under Results `/` filter (case-insensitive substring).
pub fn row_matches_filter(row: &[String], filter: &str) -> bool {
    let f = filter.trim();
    if f.is_empty() {
        return true;
    }
    let f = f.to_ascii_lowercase();
    row.iter()
        .any(|c| c.to_ascii_lowercase().contains(&f))
}

/// True when SQL is safe on read-only connections (SELECT/SHOW/EXPLAIN/WITH…).
pub fn is_read_only_sql(sql: &str) -> bool {
    let stripped = strip_sql_comments(sql);
    let s = stripped.trim();
    if s.is_empty() {
        return true;
    }
    let head = s
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match head.as_str() {
        "select" | "show" | "explain" | "describe" | "desc" | "with" | "values" => true,
        "pragma" => !s.to_ascii_lowercase().contains('='),
        _ => false,
    }
}

fn strip_sql_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '-' && chars.peek() == Some(&'-') {
            while chars.next().is_some_and(|ch| ch != '\n') {}
            out.push('\n');
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(ch) = chars.next() {
                if ch == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}
