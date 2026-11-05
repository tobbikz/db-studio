//! Cached table/column names for SQL autocomplete.

use crate::{DbDriver, DbWorkspace, LivePool};
use anyhow::{Context, Result};
use sqlx::Row;
use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct SchemaNameCache {
    /// `source_id` or `source_id#database`
    pub cache_key: String,
    pub schema: String,
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
    let mut start = before.len();
    for (i, c) in before.char_indices().rev() {
        if c.is_ascii_alphanumeric() || c == '_' {
            start = i;
        } else {
            break;
        }
    }
    let prefix: String = before.chars().skip(start).collect();
    (start, prefix)
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// After `table.` suggest columns for that table.
fn table_prefix(line: &str, col: usize) -> Option<(String, usize)> {
    let col = col.min(line.chars().count());
    let before: String = line.chars().take(col).collect();
    let bytes = before.as_bytes();
    let mut i = bytes.len();
    while i > 0 {
        let c = bytes[i - 1] as char;
        if is_ident_char(c) {
            i -= 1;
            continue;
        }
        if c == '.' {
            let table_part = &before[..i.saturating_sub(1)];
            let (start, name) = sql_identifier_prefix(table_part, table_part.chars().count());
            if !name.is_empty() {
                return Some((name, start));
            }
            return None;
        }
        break;
    }
    None
}

impl DbWorkspace {
    pub fn schema_cache_key(&self, source_id: &str, database: Option<&str>, schema: &str) -> String {
        format!(
            "{}#{}#{}",
            source_id,
            database.unwrap_or(""),
            schema
        )
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
                DbDriver::Redis | DbDriver::Mongo | DbDriver::Snowflake | DbDriver::Bigquery | DbDriver::Elasticsearch | DbDriver::Trino => {
                    s.database.clone()
                }
            })
            .unwrap_or_else(|| "public".into())
    }

    pub fn active_schema_name(&self) -> Option<String> {
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
        let Some(source_id) = self.active_source.clone() else {
            return Ok(());
        };
        let database = self.active_database.clone();
        let schema = self
            .active_schema_name()
            .unwrap_or_else(|| self.default_schema_for_driver(&source_id));
        self.ensure_connected(&source_id, database.as_deref())
            .await?;
        let key = self.schema_cache_key(&source_id, database.as_deref(), &schema);
        if self.schema_cache.cache_key == key && !self.schema_cache.tables.is_empty() {
            return Ok(());
        }
        let key_pool = Self::pool_key(&source_id, database.as_deref());
        let pool = self.pools.get(&key_pool).context("not connected")?;
        let (tables, columns) = fetch_schema_names(pool, &schema, database.as_deref()).await?;
        self.schema_cache = SchemaNameCache {
            cache_key: key,
            schema,
            tables,
            columns,
        };
        Ok(())
    }

    pub fn sql_completions(&self, line: &str, col: usize) -> Vec<SqlCompletionItem> {
        let (start, prefix) = sql_identifier_prefix(line, col);
        let prefix_lower = prefix.to_ascii_lowercase();
        let mut out = Vec::new();

        if let Some((table, _table_start)) = table_prefix(line, col) {
            let cols = self
                .schema_cache
                .columns
                .get(&table)
                .or_else(|| {
                    self.schema_cache
                        .columns
                        .iter()
                        .find(|(t, _)| t.eq_ignore_ascii_case(&table))
                        .map(|(_, c)| c)
                });
            if let Some(cols) = cols {
                for c in cols {
                    if prefix.is_empty() || c.to_ascii_lowercase().starts_with(&prefix_lower) {
                        out.push(SqlCompletionItem {
                            label: c.clone(),
                            insert_text: c.clone(),
                            replace_start_col: start,
                            detail: Some(format!("column · {}", self.schema_cache.schema)),
                        });
                    }
                }
            }
            out.sort_by(|a, b| a.label.cmp(&b.label));
            out.truncate(50);
            return out;
        }

        for t in &self.schema_cache.tables {
            if prefix.is_empty() || t.to_ascii_lowercase().starts_with(&prefix_lower) {
                out.push(SqlCompletionItem {
                    label: t.clone(),
                    insert_text: t.clone(),
                    replace_start_col: start,
                    detail: Some(format!("table · {}", self.schema_cache.schema)),
                });
            }
        }
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
        out.sort_by(|a, b| a.label.cmp(&b.label));
        out.truncate(50);
        out
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
        | LivePool::CloudStub(_) | LivePool::Http(_) => Ok((Vec::new(), HashMap::new())),
    }
}
