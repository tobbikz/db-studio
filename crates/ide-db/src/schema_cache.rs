//! Cached table/column/schema names for SQL autocomplete.

use crate::{DbDriver, DbWorkspace, LivePool};
use anyhow::{Context, Result};
use sqlx::Row;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct SchemaNameCache {
    /// `source_id` or `source_id#database`
    pub cache_key: String,
    pub schema: String,
    pub schemas: Vec<String>,
    pub tables: Vec<String>,
    pub columns: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct SqlCompletionItem {
    pub label: String,
    pub insert_text: String,
    pub replace_start_col: usize,
    pub detail: Option<String>,
}

/// Identifier fragment immediately before `col` (0-based char index).
pub fn sql_identifier_prefix(line: &str, col: usize) -> (usize, String) {
    let col = col.min(line.chars().count());
    let before: String = line.chars().take(col).collect();
    let mut start = before.chars().count();
    let mut start_byte = before.len();
    for (i, c) in before.char_indices().rev() {
        if c.is_ascii_alphanumeric() || c == '_' {
            start_byte = i;
            start = before[..i].chars().count();
        } else {
            break;
        }
    }
    let prefix: String = before[start_byte..].to_string();
    let _ = start;
    (before[..start_byte].chars().count(), prefix)
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Previous SQL word before the identifier being typed (skipping the prefix).
fn prev_sql_word(line: &str, ident_start: usize) -> String {
    let before: String = line.chars().take(ident_start).collect();
    let trimmed = before.trim_end();
    let mut word = String::new();
    for c in trimmed.chars().rev() {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else if !word.is_empty() {
            break;
        } else if c.is_whitespace() {
            continue;
        } else {
            break;
        }
    }
    word.chars().rev().collect::<String>().to_ascii_lowercase()
}

/// True when the cursor is in a spot that expects a relation (table/schema).
pub fn expecting_relation(line: &str, col: usize) -> bool {
    let (start, _prefix) = sql_identifier_prefix(line, col);
    let prev = prev_sql_word(line, start);
    matches!(
        prev.as_str(),
        "from"
            | "join"
            | "into"
            | "update"
            | "table"
            | "only" // FROM ONLY …
            | "lateral"
    )
}

/// After `table.` or `schema.table.` — returns (optional_schema, table) for column suggest.
fn dotted_relation(line: &str, col: usize) -> Option<(Option<String>, String)> {
    let col = col.min(line.chars().count());
    let before: String = line.chars().take(col).collect();
    // … schema.table.prefix|  or  table.prefix|
    let bytes = before.as_bytes();
    let mut i = bytes.len();
    // skip current ident
    while i > 0 {
        let c = bytes[i - 1] as char;
        if is_ident_char(c) {
            i -= 1;
            continue;
        }
        break;
    }
    if i == 0 || bytes[i - 1] as char != '.' {
        return None;
    }
    // left of dot
    let left = &before[..i - 1];
    let (t_start, table_or_schema) = sql_identifier_prefix(left, left.chars().count());
    if table_or_schema.is_empty() {
        return None;
    }
    // another dotted qual to the left? schema.table.
    let further = left.chars().take(t_start).collect::<String>();
    let further_trim = further.trim_end();
    if further_trim.ends_with('.') {
        let parent = &further_trim[..further_trim.len() - 1];
        let (_, schema) = sql_identifier_prefix(parent, parent.chars().count());
        if !schema.is_empty() {
            return Some((Some(schema), table_or_schema));
        }
    }
    // single qual: could be schema. (expecting table) if expecting_relation,
    // or table. (expecting column).
    Some((None, table_or_schema))
}

/// `schema.` with nothing / prefix after the last dot → suggest tables in schema.
fn dotted_schema_only(line: &str, col: usize) -> Option<(String, usize, String)> {
    let col = col.min(line.chars().count());
    let before: String = line.chars().take(col).collect();
    let (start, prefix) = sql_identifier_prefix(line, col);
    let left: String = before.chars().take(start).collect();
    let left_trim = left.trim_end();
    if !left_trim.ends_with('.') {
        return None;
    }
    let parent = &left_trim[..left_trim.len() - 1];
    let (_, schema) = sql_identifier_prefix(parent, parent.chars().count());
    if schema.is_empty() {
        return None;
    }
    // If there's yet another dot before schema, this is schema.table.col — not schema-only.
    let before_schema: String = parent
        .chars()
        .take(parent.chars().count().saturating_sub(schema.chars().count()))
        .collect();
    if before_schema.trim_end().ends_with('.') {
        return None;
    }
    if !expecting_relation(line, col) && !expecting_relation(line, start) {
        // Still allow schema.table after FROM when prefix being typed after schema.
        let prev = prev_sql_word(line, before_schema.chars().count());
        if !matches!(
            prev.as_str(),
            "from" | "join" | "into" | "update" | "table" | "only" | "lateral" | ""
        ) && !expecting_relation(line, col)
        {
            // Could be SELECT schema. — rare; still treat as schema.table if schema known.
        }
    }
    Some((schema, start, prefix))
}

impl DbWorkspace {
    pub fn schema_cache_key(&self, source_id: &str, database: Option<&str>, schema: &str) -> String {
        format!("{}#{}#{}", source_id, database.unwrap_or(""), schema)
    }

    pub fn default_schema_for_driver(&self, source_id: &str) -> String {
        self.sources
            .iter()
            .find(|s| s.id == source_id)
            .map(|s| match s.driver {
                DbDriver::Postgres => "public".into(),
                DbDriver::Mysql => s.database.clone(),
                DbDriver::Sqlite => "main".into(),
                DbDriver::Mssql => "dbo".into(),
                DbDriver::Clickhouse => "default".into(),
                DbDriver::Duckdb => "main".into(),
                DbDriver::Redis
                | DbDriver::Mongo
                | DbDriver::Snowflake
                | DbDriver::Bigquery
                | DbDriver::Elasticsearch
                | DbDriver::Trino => s.database.clone(),
            })
            .unwrap_or_else(|| "public".into())
    }

    pub fn active_schema_name(&self) -> Option<String> {
        // Console selector (DataGrip-style) wins over Explorer selection.
        if let Some(s) = &self.active_schema {
            if !s.is_empty() {
                return Some(s.clone());
            }
        }
        if let Some(e) = self.flat.get(self.selected) {
            if let Some(s) = &e.schema {
                return Some(s.clone());
            }
        }
        self.active_source
            .as_ref()
            .map(|id| self.default_schema_for_driver(id))
    }

    pub async fn refresh_schema_cache(&mut self) -> Result<()> {
        let Some(source_id) = self.active_source.clone().or_else(|| {
            self.flat
                .get(self.selected)
                .map(|e| e.source_id.clone())
        }) else {
            return Ok(());
        };
        if self.active_source.is_none() {
            self.active_source = Some(source_id.clone());
        }
        let database = self.active_database.clone().or_else(|| {
            self.flat
                .get(self.selected)
                .and_then(|e| e.database.clone())
        });
        let schema = self
            .active_schema_name()
            .unwrap_or_else(|| self.default_schema_for_driver(&source_id));
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let key = self.schema_cache_key(&source_id, database.as_deref(), &schema);
        if self.schema_cache.cache_key == key
            && !self.schema_cache.tables.is_empty()
            && !self.schema_cache.schemas.is_empty()
        {
            return Ok(());
        }
        let key_pool = Self::pool_key(&source_id, database.as_deref());
        let pool = self.pools.get(&key_pool).context("not connected")?;
        let schemas = fetch_schema_list(pool, database.as_deref()).await.unwrap_or_default();
        let (tables, columns) = fetch_schema_names(pool, &schema, database.as_deref()).await?;
        self.schema_cache = SchemaNameCache {
            cache_key: key,
            schema,
            schemas,
            tables,
            columns,
        };
        Ok(())
    }

    /// Completions for the current SQL line. Relation context (FROM/JOIN) prioritizes
    /// schemas + tables; `table.` / `schema.table.` prioritizes columns.
    pub fn sql_completions(&self, line: &str, col: usize) -> Vec<SqlCompletionItem> {
        let (start, prefix) = sql_identifier_prefix(line, col);
        let prefix_lower = prefix.to_ascii_lowercase();
        let mut out = Vec::new();
        let rel = expecting_relation(line, col);

        // schema.table.col or table.col
        if let Some((schema_opt, table)) = dotted_relation(line, col) {
            // If we're clearly after FROM and only one dotted name, prefer tables-in-schema.
            if rel && schema_opt.is_none() {
                if let Some((schema, start, prefix)) = dotted_schema_only(line, col) {
                    let pl = prefix.to_ascii_lowercase();
                    // Suggest tables; if this schema is the cached one, use cache.
                    if schema.eq_ignore_ascii_case(&self.schema_cache.schema) {
                        for t in &self.schema_cache.tables {
                            if pl.is_empty() || t.to_ascii_lowercase().starts_with(&pl) {
                                out.push(SqlCompletionItem {
                                    label: t.clone(),
                                    insert_text: t.clone(),
                                    replace_start_col: start,
                                    detail: Some(format!("table · {schema}")),
                                });
                            }
                        }
                    } else {
                        // Unknown schema in cache — still offer qualified name from known tables
                        // as schema.table when schema matches a known schema name.
                        if self
                            .schema_cache
                            .schemas
                            .iter()
                            .any(|s| s.eq_ignore_ascii_case(&schema))
                        {
                            out.push(SqlCompletionItem {
                                label: format!("{schema}.*"),
                                insert_text: String::new(),
                                replace_start_col: start,
                                detail: Some("expand schema in Explorer · Enter table".into()),
                            });
                        }
                    }
                    out.sort_by(|a, b| a.label.cmp(&b.label));
                    out.truncate(50);
                    return out;
                }
            }

            let table_key = table;
            let cols = self
                .schema_cache
                .columns
                .get(&table_key)
                .or_else(|| {
                    self.schema_cache
                        .columns
                        .iter()
                        .find(|(t, _)| t.eq_ignore_ascii_case(&table_key))
                        .map(|(_, c)| c)
                });
            if let Some(cols) = cols {
                for c in cols {
                    if prefix.is_empty() || c.to_ascii_lowercase().starts_with(&prefix_lower) {
                        out.push(SqlCompletionItem {
                            label: c.clone(),
                            insert_text: c.clone(),
                            replace_start_col: start,
                            detail: Some(format!(
                                "column · {}.{}",
                                schema_opt.as_deref().unwrap_or(self.schema_cache.schema.as_str()),
                                table_key
                            )),
                        });
                    }
                }
            }
            out.sort_by(|a, b| a.label.cmp(&b.label));
            out.truncate(50);
            return out;
        }

        // FROM / JOIN / INTO … → schemas + tables (and keywords only if prefix matches)
        if rel {
            for s in &self.schema_cache.schemas {
                if prefix.is_empty() || s.to_ascii_lowercase().starts_with(&prefix_lower) {
                    out.push(SqlCompletionItem {
                        label: s.clone(),
                        insert_text: format!("{s}."),
                        replace_start_col: start,
                        detail: Some("schema".into()),
                    });
                }
            }
            for t in &self.schema_cache.tables {
                if prefix.is_empty() || t.to_ascii_lowercase().starts_with(&prefix_lower) {
                    let qual = if self.schema_cache.schema.is_empty() {
                        t.clone()
                    } else {
                        format!("{}.{}", self.schema_cache.schema, t)
                    };
                    out.push(SqlCompletionItem {
                        label: qual.clone(),
                        insert_text: qual,
                        replace_start_col: start,
                        detail: Some("table".into()),
                    });
                    if !prefix.is_empty() && t.to_ascii_lowercase().starts_with(&prefix_lower) {
                        // also bare table name
                        if !out.iter().any(|x| x.label == *t) {
                            out.push(SqlCompletionItem {
                                label: t.clone(),
                                insert_text: t.clone(),
                                replace_start_col: start,
                                detail: Some(format!("table · {}", self.schema_cache.schema)),
                            });
                        }
                    }
                }
            }
            out.sort_by(|a, b| {
                completion_rank(a)
                    .cmp(&completion_rank(b))
                    .then_with(|| a.label.cmp(&b.label))
            });
            out.truncate(50);
            return out;
        }

        // General / SELECT-list: columns first, then tables, keywords last.
        for (_table, cols) in &self.schema_cache.columns {
            for c in cols {
                if prefix.is_empty() || c.to_ascii_lowercase().starts_with(&prefix_lower) {
                    if out.iter().any(|x| x.label == *c) {
                        continue;
                    }
                    out.push(SqlCompletionItem {
                        label: c.clone(),
                        insert_text: c.clone(),
                        replace_start_col: start,
                        detail: Some("column".into()),
                    });
                }
            }
        }
        for t in &self.schema_cache.tables {
            if prefix.is_empty() || t.to_ascii_lowercase().starts_with(&prefix_lower) {
                if out.iter().any(|x| x.label == *t) {
                    continue;
                }
                out.push(SqlCompletionItem {
                    label: t.clone(),
                    insert_text: t.clone(),
                    replace_start_col: start,
                    detail: Some(format!("table · {}", self.schema_cache.schema)),
                });
            }
        }
        if !prefix.is_empty() {
            for kw in SQL_KEYWORDS {
                if kw.to_ascii_lowercase().starts_with(&prefix_lower) {
                    out.push(SqlCompletionItem {
                        label: (*kw).to_string(),
                        insert_text: (*kw).to_string(),
                        replace_start_col: start,
                        detail: Some("keyword".into()),
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            completion_rank(a).cmp(&completion_rank(b)).then_with(|| a.label.cmp(&b.label))
        });
        out.truncate(50);
        out
    }
}

fn completion_rank(item: &SqlCompletionItem) -> u8 {
    let d = item.detail.as_deref().unwrap_or("");
    if d == "column" || d.starts_with("column ·") {
        0
    } else if d == "table" || d.starts_with("table ·") {
        1
    } else if d == "schema" {
        2
    } else if d == "keyword" {
        3
    } else {
        4
    }
}

/// Common SQL tokens for Console autocomplete (`sel` → SELECT).
const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "IS", "NULL", "AS", "ON", "JOIN",
    "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "GROUP", "BY", "ORDER", "HAVING",
    "LIMIT", "OFFSET", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
    "ALTER", "DROP", "TABLE", "VIEW", "INDEX", "SCHEMA", "DATABASE", "WITH", "DISTINCT",
    "COUNT", "SUM", "AVG", "MIN", "MAX", "CASE", "WHEN", "THEN", "ELSE", "END", "LIKE",
    "ILIKE", "BETWEEN", "EXISTS", "UNION", "ALL", "RETURNING", "TRUE", "FALSE", "COALESCE",
    "CAST", "EXPLAIN", "ANALYZE", "VACUUM", "BEGIN", "COMMIT", "ROLLBACK", "TRUNCATE",
];

async fn fetch_schema_list(pool: &LivePool, database: Option<&str>) -> Result<Vec<String>> {
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
                .filter_map(|r| r.try_get::<String, _>("schema_name").ok())
                .collect())
        }
        LivePool::Mysql(p) => {
            let rows = sqlx::query("SHOW DATABASES").fetch_all(p).await?;
            Ok(rows
                .into_iter()
                .filter_map(|r| r.try_get::<String, _>(0).ok())
                .filter(|s| {
                    !matches!(
                        s.as_str(),
                        "information_schema" | "performance_schema" | "mysql" | "sys"
                    )
                })
                .collect())
        }
        LivePool::Sqlite(_) => Ok(vec!["main".into()]),
        LivePool::Mssql(_) => Ok(vec![database.unwrap_or("dbo").into()]),
        _ => Ok(Vec::new()),
    }
}

async fn fetch_schema_names(
    pool: &LivePool,
    schema: &str,
    database: Option<&str>,
) -> Result<(Vec<String>, HashMap<String, Vec<String>>)> {
    match pool {
        LivePool::Pg(p) => {
            let tables = sqlx::query(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = $1 AND table_type IN ('BASE TABLE','VIEW') \
                 ORDER BY 1",
            )
            .bind(schema)
            .fetch_all(p)
            .await?;
            let mut table_names = Vec::new();
            let mut columns = HashMap::new();
            for r in tables {
                let t: String = r.get("table_name");
                table_names.push(t.clone());
                let cols = sqlx::query(
                    "SELECT column_name FROM information_schema.columns \
                     WHERE table_schema = $1 AND table_name = $2 ORDER BY ordinal_position",
                )
                .bind(schema)
                .bind(&t)
                .fetch_all(p)
                .await?;
                columns.insert(
                    t,
                    cols.into_iter()
                        .filter_map(|c| c.try_get::<String, _>("column_name").ok())
                        .collect(),
                );
            }
            Ok((table_names, columns))
        }
        LivePool::Mysql(p) => {
            let db = database.unwrap_or(schema);
            let tables = sqlx::query(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = ? ORDER BY 1",
            )
            .bind(db)
            .fetch_all(p)
            .await?;
            let mut table_names = Vec::new();
            let mut columns = HashMap::new();
            for r in tables {
                let t: String = r.get("table_name");
                table_names.push(t.clone());
                let cols = sqlx::query(
                    "SELECT column_name FROM information_schema.columns \
                     WHERE table_schema = ? AND table_name = ? ORDER BY ordinal_position",
                )
                .bind(db)
                .bind(&t)
                .fetch_all(p)
                .await?;
                columns.insert(
                    t,
                    cols.into_iter()
                        .filter_map(|c| c.try_get::<String, _>("column_name").ok())
                        .collect(),
                );
            }
            Ok((table_names, columns))
        }
        LivePool::Sqlite(p) => {
            let tables = sqlx::query(
                "SELECT name FROM sqlite_master WHERE type IN ('table','view') \
                 AND name NOT LIKE 'sqlite_%' ORDER BY 1",
            )
            .fetch_all(p)
            .await?;
            let mut table_names = Vec::new();
            let mut columns = HashMap::new();
            for r in tables {
                let t: String = r.get("name");
                table_names.push(t.clone());
                let q = format!("PRAGMA table_info('{t}')");
                let cols = sqlx::query(&q).fetch_all(p).await?;
                columns.insert(
                    t,
                    cols.into_iter()
                        .filter_map(|c| c.try_get::<String, _>(1).ok())
                        .collect(),
                );
            }
            Ok((table_names, columns))
        }
        LivePool::Mssql(_)
        | LivePool::Clickhouse(_)
        | LivePool::Duckdb(_)
        | LivePool::Redis(_)
        | LivePool::Mongo(_)
        | LivePool::CloudStub(_)
        | LivePool::Http(_) => Ok((Vec::new(), HashMap::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_space_expects_relation() {
        assert!(expecting_relation("SELECT * FROM ", 14));
        assert!(expecting_relation("SELECT * FROM foo", 16));
        assert!(!expecting_relation("SELECT ", 7));
    }

    #[test]
    fn prev_word_after_from() {
        let line = "SELECT * FROM ";
        let (start, prefix) = sql_identifier_prefix(line, line.len());
        assert_eq!(prefix, "");
        assert_eq!(prev_sql_word(line, start), "from");
    }
}
