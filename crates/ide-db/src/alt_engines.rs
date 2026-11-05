//! DuckDB, Redis, MongoDB, and cloud SQL (BigQuery / Snowflake).

use anyhow::{Context, Result, bail};
use duckdb::types::ValueRef;
use duckdb::Row;
use mongodb::bson::{Bson, Document};
use redis::AsyncCommands;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{ColumnInfo, DataSource, DbDriver, DbNodeKind, DbTreeNode, QueryResult};

pub type DuckDbConn = Arc<Mutex<duckdb::Connection>>;

pub struct RedisConn {
    pub client: redis::Client,
    pub db_index: u8,
}

pub struct MongoConn {
    pub client: mongodb::Client,
    pub default_db: String,
}

pub fn connect_duckdb(src: &DataSource) -> Result<DuckDbConn> {
    let conn = if src.file.is_empty() || src.file == ":memory:" {
        duckdb::Connection::open_in_memory().context("duckdb :memory:")?
    } else {
        duckdb::Connection::open(Path::new(&src.file))
            .with_context(|| format!("duckdb open {}", src.file))?
    };
    Ok(Arc::new(Mutex::new(conn)))
}

pub fn connect_redis(src: &DataSource, local_port: Option<u16>) -> Result<RedisConn> {
    let host = match local_port {
        Some(_) => "127.0.0.1",
        None => {
            if src.host.is_empty() {
                "127.0.0.1"
            } else {
                src.host.as_str()
            }
        }
    };
    let port = local_port.unwrap_or(if src.port == 0 { 6379 } else { src.port });
    let db_index: u8 = src.database.trim().parse().unwrap_or(0);
    let url = if src.password.is_empty() {
        format!("redis://{host}:{port}/{db_index}")
    } else {
        format!(
            "redis://:{}@{}:{}/{}",
            urlencoding_lite(&src.password),
            host,
            port,
            db_index
        )
    };
    let client = redis::Client::open(url.as_str()).context("redis client")?;
    Ok(RedisConn { client, db_index })
}

pub async fn ping_redis(r: &RedisConn) -> Result<()> {
    let mut conn = r
        .client
        .get_multiplexed_async_connection()
        .await
        .context("redis connect")?;
    let _: String = redis::cmd("PING").query_async(&mut conn).await?;
    Ok(())
}

pub async fn connect_mongo(src: &DataSource, local_port: Option<u16>) -> Result<MongoConn> {
    let host = match local_port {
        Some(_) => "127.0.0.1".to_string(),
        None => {
            if src.host.is_empty() {
                "127.0.0.1".into()
            } else {
                src.host.clone()
            }
        }
    };
    let port = local_port.unwrap_or(if src.port == 0 { 27017 } else { src.port });
    let user_pass = if src.user.is_empty() && src.password.is_empty() {
        String::new()
    } else if src.password.is_empty() {
        format!("{}@", urlencoding_lite(&src.user))
    } else {
        format!(
            "{}:{}@",
            urlencoding_lite(&src.user),
            urlencoding_lite(&src.password)
        )
    };
    let uri = format!("mongodb://{user_pass}{host}:{port}/");
    let client = mongodb::Client::with_uri_str(&uri)
        .await
        .context("mongodb connect")?;
    let default_db = if src.database.is_empty() {
        "test".into()
    } else {
        src.database.clone()
    };
    Ok(MongoConn {
        client,
        default_db,
    })
}

pub fn snowflake_configured(src: &DataSource) -> bool {
    !src.host.trim().is_empty()
        && !src.user.trim().is_empty()
        && std::env::var("SNOWFLAKE_ACCOUNT")
            .map(|s| !s.is_empty())
            .unwrap_or(false)
}

pub fn bigquery_configured(src: &DataSource) -> bool {
    std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
        .map(|p| !p.is_empty() && Path::new(&p).exists())
        .unwrap_or(false)
        || (!src.database.trim().is_empty()
            && std::env::var("BIGQUERY_PROJECT")
                .map(|s| !s.is_empty())
                .unwrap_or(false))
}

pub fn cloud_not_configured_message(driver: DbDriver) -> String {
    match driver {
        DbDriver::Snowflake => {
            "Snowflake: set SNOWFLAKE_ACCOUNT (+ host/user) and install `snowsql`; password via form / SNOWFLAKE_PASSWORD / env:VAR. Explorer: SHOW warehouses·databases·schemas·tables".into()
        }
        DbDriver::Bigquery => {
            "BigQuery: database=project + BIGQUERY_ACCESS_TOKEN (or `gcloud auth print-access-token`); optional GOOGLE_APPLICATION_CREDENTIALS / `bq`. Explorer: datasets + recent jobs".into()
        }
        _ => "cloud driver not configured".into(),
    }
}

fn bq_project(src: &DataSource) -> Result<String> {
    let project = if !src.database.trim().is_empty() {
        src.database.trim().to_string()
    } else {
        std::env::var("BIGQUERY_PROJECT").unwrap_or_default()
    };
    if project.is_empty() {
        bail!("BigQuery needs database=project or BIGQUERY_PROJECT");
    }
    Ok(project)
}

async fn bigquery_access_token() -> Result<String> {
    if let Ok(t) = std::env::var("BIGQUERY_ACCESS_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    if let Ok(t) = std::env::var("GOOGLE_OAUTH_ACCESS_TOKEN") {
        if !t.is_empty() {
            return Ok(t);
        }
    }
    let output = tokio::process::Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()
        .await
        .context("gcloud auth print-access-token")?;
    if !output.status.success() {
        bail!(
            "no BIGQUERY_ACCESS_TOKEN and gcloud failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// BigQuery via jobs.query REST; falls back to `bq` CLI.
pub async fn exec_bigquery(src: &DataSource, sql: &str) -> Result<QueryResult> {
    let project = bq_project(src)?;
    let started = std::time::Instant::now();
    match exec_bigquery_rest(&project, sql).await {
        Ok(mut q) => {
            q.elapsed_ms = started.elapsed().as_millis();
            return Ok(q);
        }
        Err(rest_err) => {
            match exec_bigquery_cli(src, &project, sql).await {
                Ok(mut q) => {
                    q.elapsed_ms = started.elapsed().as_millis();
                    Ok(q)
                }
                Err(cli_err) => Err(rest_err).context(format!("REST failed; bq also failed: {cli_err:#}")),
            }
        }
    }
}

async fn exec_bigquery_rest(project: &str, sql: &str) -> Result<QueryResult> {
    let token = bigquery_access_token().await?;
    let url = format!(
        "https://bigquery.googleapis.com/bigquery/v2/projects/{}/queries",
        urlencoding_lite(project)
    );
    let body = serde_json::json!({
        "query": sql,
        "useLegacySql": false,
        "maxResults": 500,
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .context("BigQuery HTTP")?;
    let status = resp.status();
    let text = resp.text().await.context("BigQuery body")?;
    if !status.is_success() {
        bail!("BigQuery HTTP {status}: {text}");
    }
    let v: serde_json::Value = serde_json::from_str(&text).context("BigQuery JSON")?;
    let schema_fields = v
        .pointer("/schema/fields")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    let columns: Vec<String> = schema_fields
        .iter()
        .filter_map(|f| f.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
        .collect();
    let rows_json = v
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let mut rows = Vec::new();
    for row in rows_json {
        let cells = row
            .get("f")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|c| {
                        c.get("v")
                            .map(|v| match v {
                                serde_json::Value::Null => "NULL".into(),
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .unwrap_or_else(|| "NULL".into())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        rows.push(cells);
    }
    let mut q = QueryResult::empty();
    q.columns = if columns.is_empty() {
        vec!["(empty)".into()]
    } else {
        columns
    };
    q.rows = rows;
    q.original_rows = q.rows.clone();
    if q.rows.len() >= 500 {
        q.truncated = true;
    }
    Ok(q)
}

async fn exec_bigquery_cli(src: &DataSource, project: &str, sql: &str) -> Result<QueryResult> {
    let mut cmd = tokio::process::Command::new("bq");
    cmd.args([
        "query",
        "--format=json",
        "--use_legacy_sql=false",
        "--max_rows=500",
        &format!("--project_id={project}"),
        sql,
    ]);
    if let Ok(creds) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        if !creds.is_empty() {
            cmd.env("GOOGLE_APPLICATION_CREDENTIALS", creds);
        }
    }
    let _ = src;
    let output = cmd.output().await.context("run bq query")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    let trimmed = String::from_utf8_lossy(&output.stdout);
    let trimmed = trimmed.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        let mut q = QueryResult::empty();
        q.columns = vec!["(empty)".into()];
        return Ok(q);
    }
    let rows_json: Vec<serde_json::Value> =
        serde_json::from_str(trimmed).context("parse bq JSON")?;
    let mut col_set = std::collections::BTreeSet::new();
    for row in &rows_json {
        if let Some(obj) = row.as_object() {
            for k in obj.keys() {
                col_set.insert(k.clone());
            }
        }
    }
    let columns: Vec<String> = col_set.into_iter().collect();
    let mut rows = Vec::new();
    for row in rows_json {
        let obj = row.as_object();
        let mut cells = Vec::new();
        for c in &columns {
            let cell = obj
                .and_then(|o| o.get(c))
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => "NULL".into(),
                    other => other.to_string(),
                })
                .unwrap_or_else(|| "NULL".into());
            cells.push(cell);
        }
        rows.push(cells);
    }
    let mut q = QueryResult::empty();
    q.columns = columns;
    q.rows = rows;
    q.original_rows = q.rows.clone();
    if q.rows.len() >= 500 {
        q.truncated = true;
    }
    Ok(q)
}

/// Snowflake via `snowsql` CLI (MVP).
pub async fn exec_snowflake(src: &DataSource, sql: &str) -> Result<QueryResult> {
    let account = std::env::var("SNOWFLAKE_ACCOUNT").unwrap_or_default();
    if account.is_empty() && src.host.trim().is_empty() {
        bail!(cloud_not_configured_message(DbDriver::Snowflake));
    }
    let account = if account.is_empty() {
        src.host.clone()
    } else {
        account
    };
    let user = if src.user.is_empty() {
        std::env::var("SNOWFLAKE_USER").unwrap_or_default()
    } else {
        src.user.clone()
    };
    if user.is_empty() {
        bail!("Snowflake needs user");
    }
    let pass = src.resolved_password();
    let pass = if pass.is_empty() {
        std::env::var("SNOWFLAKE_PASSWORD").unwrap_or_default()
    } else {
        pass
    };
    let started = std::time::Instant::now();
    let mut cmd = tokio::process::Command::new("snowsql");
    cmd.args([
        "-a",
        &account,
        "-u",
        &user,
        "-o",
        "output_format=json",
        "-o",
        "friendly=false",
        "-o",
        "header=true",
        "-q",
        sql,
    ]);
    if !pass.is_empty() {
        cmd.env("SNOWSQL_PWD", pass);
    }
    if !src.database.is_empty() {
        cmd.args(["-d", &src.database]);
    }
    let output = cmd
        .output()
        .await
        .context("run snowsql (install Snowflake CLI)")?;
    if !output.status.success() {
        bail!(
            "snowsql failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    let mut q = QueryResult::empty();
    q.elapsed_ms = started.elapsed().as_millis();
    if trimmed.is_empty() {
        q.columns = vec!["status".into()];
        q.rows = vec![vec!["OK".into()]];
        q.original_rows = q.rows.clone();
        return Ok(q);
    }
    // snowsql json may be array of objects or nested
    if let Ok(rows_json) = serde_json::from_str::<Vec<serde_json::Value>>(trimmed) {
        let mut col_set = std::collections::BTreeSet::new();
        for row in &rows_json {
            if let Some(obj) = row.as_object() {
                for k in obj.keys() {
                    col_set.insert(k.clone());
                }
            }
        }
        let columns: Vec<String> = col_set.into_iter().collect();
        let mut rows = Vec::new();
        for row in rows_json {
            let obj = row.as_object();
            let mut cells = Vec::new();
            for c in &columns {
                let cell = obj
                    .and_then(|o| o.get(c))
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => "NULL".into(),
                        other => other.to_string(),
                    })
                    .unwrap_or_else(|| "NULL".into());
                cells.push(cell);
            }
            rows.push(cells);
        }
        q.columns = columns;
        q.rows = rows;
    } else {
        q.columns = vec!["output".into()];
        q.rows = trimmed.lines().map(|l| vec![l.to_string()]).collect();
    }
    q.original_rows = q.rows.clone();
    Ok(q)
}

pub async fn exec_duckdb(conn: &DuckDbConn, sql: &str) -> Result<QueryResult> {
    let sql = sql.to_string();
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || exec_duckdb_sync(&conn, &sql))
        .await
        .context("duckdb task")?
}

fn exec_duckdb_sync(conn: &DuckDbConn, sql: &str) -> Result<QueryResult> {
    let sql = sql.trim();
    if sql.is_empty() {
        bail!("empty SQL");
    }
    let guard = conn.blocking_lock();
    let head = sql
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        head.as_str(),
        "select" | "with" | "show" | "describe" | "desc" | "explain" | "pragma" | "values"
    ) {
        let mut stmt = guard.prepare(sql).context("duckdb prepare")?;
        let col_count = stmt.column_count();
        let columns: Vec<String> = (0..col_count)
            .map(|i| {
                stmt.column_name(i)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| "?".into())
            })
            .collect();
        let mut rows = stmt.query(duckdb::params![]).context("duckdb query")?;
        let mut out_rows = Vec::new();
        while let Some(row) = rows.next().context("duckdb row")? {
            out_rows.push(duckdb_row_cells(row, col_count));
        }
        let mut q = QueryResult::empty();
        q.columns = columns;
        q.rows = out_rows.clone();
        q.original_rows = out_rows;
        Ok(q)
    } else {
        guard.execute_batch(sql).context("duckdb execute")?;
        let mut q = QueryResult::empty();
        q.columns = vec!["status".into()];
        q.rows = vec![vec!["ok".into()]];
        q.original_rows = q.rows.clone();
        q.affected = Some(1);
        Ok(q)
    }
}

fn duckdb_row_cells(row: &Row<'_>, col_count: usize) -> Vec<String> {
    (0..col_count)
        .map(|i| match row.get_ref(i) {
            Ok(ValueRef::Null) => "NULL".into(),
            Ok(ValueRef::Boolean(b)) => b.to_string(),
            Ok(ValueRef::TinyInt(v)) => v.to_string(),
            Ok(ValueRef::SmallInt(v)) => v.to_string(),
            Ok(ValueRef::Int(v)) => v.to_string(),
            Ok(ValueRef::BigInt(v)) => v.to_string(),
            Ok(ValueRef::HugeInt(v)) => v.to_string(),
            Ok(ValueRef::UTinyInt(v)) => v.to_string(),
            Ok(ValueRef::USmallInt(v)) => v.to_string(),
            Ok(ValueRef::UInt(v)) => v.to_string(),
            Ok(ValueRef::UBigInt(v)) => v.to_string(),
            Ok(ValueRef::Float(v)) => v.to_string(),
            Ok(ValueRef::Double(v)) => v.to_string(),
            Ok(ValueRef::Text(s)) => String::from_utf8_lossy(s).into_owned(),
            Ok(ValueRef::Blob(b)) => {
                let preview: String = b
                    .iter()
                    .take(32)
                    .map(|x| format!("{x:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let more = if b.len() > 32 { "…" } else { "" };
                format!("<blob {}b: {preview}{more}>", b.len())
            }
            Ok(other) => format!("{other:?}"),
            Err(_) => "NULL".into(),
        })
        .collect()
}

pub async fn exec_redis(r: &RedisConn, line: &str) -> Result<QueryResult> {
    let line = line.trim();
    if line.is_empty() {
        bail!("empty command");
    }
    let mut parts: Vec<&str> = line.split_whitespace().collect();
    let cmd = parts.remove(0).to_ascii_uppercase();
    let mut conn = r
        .client
        .get_multiplexed_async_connection()
        .await
        .context("redis connect")?;
    match cmd.as_str() {
        "GET" => {
            let key = parts.first().context("GET needs key")?;
            let val: Option<String> = conn.get(*key).await.context("redis GET")?;
            Ok(single_col_row("value", val.unwrap_or_else(|| "(nil)".into())))
        }
        "SET" => {
            let key = parts.first().context("SET needs key")?;
            let val = parts.get(1).context("SET needs value")?;
            let _: () = conn.set(*key, *val).await.context("redis SET")?;
            Ok(single_col_row("status", "OK".into()))
        }
        "KEYS" => {
            let pattern = *parts.first().unwrap_or(&"*");
            let keys: Vec<String> = conn.keys(pattern).await.context("redis KEYS")?;
            list_keys_result(keys)
        }
        "SCAN" => {
            let mut cursor: u64 = 0;
            let mut all = Vec::new();
            loop {
                let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("COUNT")
                    .arg(100)
                    .query_async(&mut conn)
                    .await
                    .context("redis SCAN")?;
                all.extend(batch);
                cursor = next;
                if cursor == 0 || all.len() >= 500 {
                    break;
                }
            }
            list_keys_result(all)
        }
        "PING" => Ok(single_col_row("pong", "PONG".into())),
        "TYPE" => {
            let key = parts.first().context("TYPE needs key")?;
            let ty: String = redis::cmd("TYPE")
                .arg(*key)
                .query_async(&mut conn)
                .await
                .context("redis TYPE")?;
            Ok(single_col_row("type", ty))
        }
        "TTL" => {
            let key = parts.first().context("TTL needs key")?;
            let ttl: i64 = conn.ttl(*key).await.context("redis TTL")?;
            Ok(single_col_row("ttl", ttl.to_string()))
        }
        "EXPIRE" => {
            let key = parts.first().context("EXPIRE needs key")?;
            let secs: i64 = parts
                .get(1)
                .and_then(|s| s.parse().ok())
                .context("EXPIRE needs seconds")?;
            let ok: bool = redis::cmd("EXPIRE")
                .arg(*key)
                .arg(secs)
                .query_async(&mut conn)
                .await
                .context("redis EXPIRE")?;
            Ok(single_col_row("expire", if ok { "1" } else { "0" }.into()))
        }
        "DEL" | "UNLINK" => {
            let key = parts.first().context("DEL needs key")?;
            let n: i64 = redis::cmd(cmd.as_str())
                .arg(*key)
                .query_async(&mut conn)
                .await
                .context("redis DEL")?;
            Ok(single_col_row("deleted", n.to_string()))
        }
        "DBSIZE" => {
            let n: i64 = redis::cmd("DBSIZE")
                .query_async(&mut conn)
                .await
                .context("redis DBSIZE")?;
            Ok(single_col_row("keys", n.to_string()))
        }
        "INFO" => {
            let section = parts.first().copied().unwrap_or("server");
            let info: String = redis::cmd("INFO")
                .arg(section)
                .query_async(&mut conn)
                .await
                .context("redis INFO")?;
            let mut q = QueryResult::empty();
            q.columns = vec!["info".into()];
            q.rows = info.lines().map(|l| vec![l.to_string()]).collect();
            q.original_rows = q.rows.clone();
            Ok(q)
        }
        "HGETALL" => {
            let key = parts.first().context("HGETALL needs key")?;
            let map: std::collections::HashMap<String, String> =
                conn.hgetall(*key).await.context("redis HGETALL")?;
            let mut q = QueryResult::empty();
            q.columns = vec!["field".into(), "value".into()];
            q.rows = map.into_iter().map(|(k, v)| vec![k, v]).collect();
            q.original_rows = q.rows.clone();
            Ok(q)
        }
        "LRANGE" => {
            let key = parts.first().context("LRANGE needs key")?;
            let start: isize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let stop: isize = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(99);
            let vals: Vec<String> = conn.lrange(*key, start, stop).await.context("redis LRANGE")?;
            let mut q = QueryResult::empty();
            q.columns = vec!["index".into(), "value".into()];
            q.rows = vals
                .into_iter()
                .enumerate()
                .map(|(i, v)| vec![i.to_string(), v])
                .collect();
            q.original_rows = q.rows.clone();
            Ok(q)
        }
        "SMEMBERS" => {
            let key = parts.first().context("SMEMBERS needs key")?;
            let vals: Vec<String> = conn.smembers(*key).await.context("redis SMEMBERS")?;
            list_keys_result(vals)
        }
        "ZRANGE" => {
            let key = parts.first().context("ZRANGE needs key")?;
            let start: isize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let stop: isize = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(99);
            let vals: Vec<String> = conn
                .zrange(*key, start, stop)
                .await
                .context("redis ZRANGE")?;
            let mut q = QueryResult::empty();
            q.columns = vec!["member".into()];
            q.rows = vals.into_iter().map(|v| vec![v]).collect();
            q.original_rows = q.rows.clone();
            Ok(q)
        }
        _ => bail!(
            "Redis: GET SET KEYS SCAN PING TYPE TTL EXPIRE DEL DBSIZE INFO HGETALL LRANGE SMEMBERS ZRANGE (not {cmd})"
        ),
    }
}

fn single_col_row(col: &str, val: String) -> QueryResult {
    let mut q = QueryResult::empty();
    q.columns = vec![col.into()];
    q.rows = vec![vec![val]];
    q.original_rows = q.rows.clone();
    q
}

fn list_keys_result(keys: Vec<String>) -> Result<QueryResult> {
    let mut q = QueryResult::empty();
    q.columns = vec!["key".into()];
    q.rows = keys.into_iter().map(|k| vec![k]).collect();
    q.original_rows = q.rows.clone();
    if q.rows.len() >= 500 {
        q.truncated = true;
    }
    Ok(q)
}

pub fn is_redis_read_only(line: &str) -> bool {
    let head = line
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(
        head.as_str(),
        "GET" | "KEYS" | "SCAN" | "PING" | "TYPE" | "TTL" | "DBSIZE" | "INFO" | "HGETALL"
            | "LRANGE" | "SMEMBERS"
            | "ZRANGE"
    )
}

pub async fn exec_mongo(m: &MongoConn, line: &str, active_db: Option<&str>) -> Result<QueryResult> {
    let line = line.trim();
    if line.is_empty() {
        bail!("empty command");
    }
    let db_name = active_db
        .filter(|s| !s.is_empty())
        .unwrap_or(m.default_db.as_str());
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("find ") {
        let rest = line[5..].trim();
        let (coll, filter, limit) = parse_mongo_find(rest)?;
        return mongo_find(m, db_name, &coll, filter, limit).await;
    }
    if lower.starts_with("count ") {
        let coll = line[6..].trim();
        if coll.is_empty() {
            bail!("count <collection>");
        }
        let db = m.client.database(db_name);
        let n = db
            .collection::<Document>(coll)
            .count_documents(Document::new())
            .await
            .context("mongo count")?;
        return Ok(single_col_row("count", n.to_string()));
    }
    if lower.starts_with("indexes ") {
        let coll = line[8..].trim();
        if coll.is_empty() {
            bail!("indexes <collection>");
        }
        let db = m.client.database(db_name);
        let mut cursor = db
            .collection::<Document>(coll)
            .list_indexes()
            .await
            .context("mongo list_indexes")?;
        use futures_util::StreamExt;
        let mut rows = Vec::new();
        while let Some(ix) = cursor.next().await {
            let ix = ix.context("index doc")?;
            // IndexModel fields are crate-private — Debug is the stable TUI preview.
            rows.push(vec![format!("{ix:?}")]);
        }
        let mut q = QueryResult::empty();
        q.columns = vec!["index".into()];
        q.rows = rows;
        q.original_rows = q.rows.clone();
        return Ok(q);
    }
    if lower.starts_with("select") {
        bail!("MongoDB: use `find collection` (not SQL SELECT)");
    }
    bail!("MongoDB: `find|count|indexes <collection> …`");
}

fn parse_mongo_find(rest: &str) -> Result<(String, Document, i64)> {
    let rest = rest.trim();
    if rest.is_empty() {
        bail!("find <collection>");
    }
    let mut coll = String::new();
    let mut i = 0;
    let bytes = rest.as_bytes();
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'{' {
        coll.push(bytes[i] as char);
        i += 1;
    }
    if coll.is_empty() {
        bail!("find <collection>");
    }
    let rest = rest[i..].trim_start();
    let mut filter = Document::new();
    let mut limit: i64 = 100;
    if rest.starts_with('{') {
        let end = find_json_object_end(rest).context("unclosed JSON filter")?;
        let json = &rest[..=end];
        let v: serde_json::Value = serde_json::from_str(json).context("mongo filter JSON")?;
        filter = json_value_to_doc(&v)?;
        let after = rest[end + 1..].trim_start();
        if let Some(lim) = parse_limit_suffix(after) {
            limit = lim;
        }
    } else if let Some(lim) = parse_limit_suffix(rest) {
        limit = lim;
    }
    Ok((coll, filter, limit))
}

fn find_json_object_end(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_limit_suffix(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("limit ") {
        return rest.trim().parse().ok();
    }
    s.parse().ok()
}

fn json_value_to_doc(v: &serde_json::Value) -> Result<Document> {
    let obj = v
        .as_object()
        .context("mongo filter must be a JSON object")?;
    let mut doc = Document::new();
    for (k, val) in obj {
        doc.insert(k, json_to_bson(val.clone()));
    }
    Ok(doc)
}

fn json_to_bson(v: serde_json::Value) -> Bson {
    match v {
        serde_json::Value::Null => Bson::Null,
        serde_json::Value::Bool(b) => Bson::Boolean(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Bson::Int64(i)
            } else if let Some(f) = n.as_f64() {
                Bson::Double(f)
            } else {
                Bson::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => Bson::String(s),
        serde_json::Value::Array(a) => Bson::Array(a.into_iter().map(json_to_bson).collect()),
        serde_json::Value::Object(o) => {
            let mut d = Document::new();
            for (k, v) in o {
                d.insert(k, json_to_bson(v));
            }
            Bson::Document(d)
        }
    }
}

pub async fn mongo_find(
    m: &MongoConn,
    db_name: &str,
    collection: &str,
    filter: Document,
    limit: i64,
) -> Result<QueryResult> {
    let db = m.client.database(db_name);
    let coll = db.collection::<Document>(collection);
    let mut cursor = coll
        .find(filter)
        .limit(limit)
        .await
        .context("mongo find")?;
    let mut docs = Vec::new();
    use futures_util::StreamExt;
    while let Some(doc) = cursor.next().await {
        docs.push(doc.context("mongo doc")?);
    }
    if docs.is_empty() {
        let mut q = QueryResult::empty();
        q.columns = vec!["_id".into()];
        return Ok(q);
    }
    let mut col_set = std::collections::BTreeSet::new();
    for d in &docs {
        for k in d.keys() {
            col_set.insert(k.clone());
        }
    }
    let columns: Vec<String> = col_set.into_iter().collect();
    let rows: Vec<Vec<String>> = docs
        .iter()
        .map(|d| {
            columns
                .iter()
                .map(|c| bson_to_display(d.get(c)))
                .collect()
        })
        .collect();
    let mut q = QueryResult::empty();
    q.columns = columns;
    q.rows = rows.clone();
    q.original_rows = rows;
    if docs.len() as i64 >= limit {
        q.truncated = true;
    }
    Ok(q)
}

fn bson_to_display(v: Option<&Bson>) -> String {
    match v {
        None => "NULL".into(),
        Some(Bson::Null) => "NULL".into(),
        Some(Bson::String(s)) => s.clone(),
        Some(Bson::Int32(n)) => n.to_string(),
        Some(Bson::Int64(n)) => n.to_string(),
        Some(Bson::Double(n)) => n.to_string(),
        Some(Bson::Boolean(b)) => b.to_string(),
        Some(Bson::ObjectId(oid)) => oid.to_hex(),
        Some(Bson::Document(d)) => serde_json::to_string(d).unwrap_or_else(|_| "{...}".into()),
        Some(other) => other.to_string(),
    }
}

pub async fn duckdb_tables(conn: &DuckDbConn, source_id: &str) -> Result<Vec<DbTreeNode>> {
    let qr = exec_duckdb(
        conn,
        "SELECT table_schema, table_name, table_type FROM information_schema.tables \
         WHERE table_schema NOT IN ('information_schema','pg_catalog') \
         ORDER BY 1, 2",
    )
    .await?;
    Ok(qr
        .rows
        .into_iter()
        .filter_map(|r| {
            let schema = r.first()?.clone();
            let name = r.get(1)?.clone();
            let ty = r.get(2).cloned().unwrap_or_default();
            let kind = if ty.to_ascii_lowercase().contains("view") {
                DbNodeKind::View
            } else {
                DbNodeKind::Table
            };
            Some(DbTreeNode {
                id: format!("{source_id}::table::{schema}::{name}"),
                label: name.clone(),
                kind,
                source_id: source_id.into(),
                database: None,
                schema: Some(schema),
                table: Some(name),
                query_sql: None,
                children: Vec::new(),
                expanded: false,
            })
        })
        .collect())
}

pub async fn redis_key_nodes(r: &RedisConn, source_id: &str) -> Result<Vec<DbTreeNode>> {
    let qr = exec_redis(r, "SCAN").await?;
    let mut conn = r
        .client
        .get_multiplexed_async_connection()
        .await
        .context("redis connect")?;
    let mut out = Vec::new();
    for row in qr.rows.into_iter().take(120) {
        let Some(key) = row.first().cloned() else {
            continue;
        };
        let ty: String = redis::cmd("TYPE")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or_else(|_| "?".into());
        let ttl: i64 = conn.ttl(&key).await.unwrap_or(-2);
        let ttl_s = if ttl < 0 {
            "∞".into()
        } else {
            ttl.to_string()
        };
        out.push(DbTreeNode {
            id: format!("{source_id}::key::{key}"),
            label: format!("{key}  [{ty} ttl={ttl_s}]"),
            kind: DbNodeKind::Table,
            source_id: source_id.into(),
            database: None,
            schema: Some("keys".into()),
            table: Some(key),
            query_sql: None,
            children: Vec::new(),
            expanded: false,
        });
    }
    Ok(out)
}

pub async fn mongo_databases(m: &MongoConn, source_id: &str) -> Result<Vec<DbTreeNode>> {
    let names = m
        .client
        .list_database_names()
        .await
        .context("mongo list dbs")?;
    Ok(names
        .into_iter()
        .filter(|n| n != "admin" && n != "local")
        .map(|name| DbTreeNode {
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
        .collect())
}

pub async fn mongo_collections(
    m: &MongoConn,
    source_id: &str,
    database: &str,
) -> Result<Vec<DbTreeNode>> {
    let db = m.client.database(database);
    let names = db
        .list_collection_names()
        .await
        .context("mongo collections")?;
    Ok(names
        .into_iter()
        .map(|name| DbTreeNode {
            id: format!("{source_id}::coll::{database}::{name}"),
            label: name.clone(),
            kind: DbNodeKind::Table,
            source_id: source_id.into(),
            database: Some(database.into()),
            schema: Some(database.into()),
            table: Some(name),
            query_sql: None,
            children: Vec::new(),
            expanded: false,
        })
        .collect())
}

pub async fn duckdb_columns(
    conn: &DuckDbConn,
    schema: &str,
    table: &str,
) -> Result<Vec<ColumnInfo>> {
    let qr = exec_duckdb(
        conn,
        &format!(
            "SELECT column_name, data_type, is_nullable FROM information_schema.columns \
             WHERE table_schema = '{}' AND table_name = '{}' ORDER BY ordinal_position",
            schema.replace('\'', "''"),
            table.replace('\'', "''")
        ),
    )
    .await?;
    Ok(qr
        .rows
        .into_iter()
        .filter_map(|r| {
            Some(ColumnInfo {
                name: r.first()?.clone(),
                data_type: r.get(1).cloned().unwrap_or_default(),
                nullable: r
                    .get(2)
                    .map(|s| s.eq_ignore_ascii_case("YES"))
                    .unwrap_or(true),
            })
        })
        .collect())
}

fn urlencoding_lite(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}
