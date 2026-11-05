//! Elasticsearch + Trino over HTTP.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{DataSource, DbDriver, DbNodeKind, DbTreeNode, QueryResult};

#[derive(Clone)]
pub struct HttpSqlConn {
    pub driver: DbDriver,
    pub base: String,
    pub user: String,
    pub password: String,
    pub database: String,
}

pub fn connect_http(src: &DataSource) -> Result<HttpSqlConn> {
    let scheme = if src.tls_required() { "https" } else { "http" };
    let host = if src.host.is_empty() {
        "127.0.0.1"
    } else {
        src.host.as_str()
    };
    let port = if src.port == 0 {
        match src.driver {
            DbDriver::Elasticsearch => 9200,
            DbDriver::Trino => 8080,
            _ => 8080,
        }
    } else {
        src.port
    };
    Ok(HttpSqlConn {
        driver: src.driver,
        base: format!("{scheme}://{host}:{port}"),
        user: src.user.clone(),
        password: src.password.clone(),
        database: src.database.clone(),
    })
}

pub async fn exec_http(conn: &HttpSqlConn, sql: &str) -> Result<QueryResult> {
    match conn.driver {
        DbDriver::Elasticsearch => exec_elasticsearch(conn, sql).await,
        DbDriver::Trino => exec_trino(conn, sql).await,
        _ => bail!("not an HTTP SQL engine"),
    }
}

/// Elasticsearch: `GET index/_search` JSON body, or `GET /_cat/indices`.
async fn exec_elasticsearch(conn: &HttpSqlConn, line: &str) -> Result<QueryResult> {
    let line = line.trim();
    let client = reqwest::Client::new();
    let mut req = if line.to_ascii_lowercase().starts_with("get ")
        || line.to_ascii_lowercase().starts_with("post ")
    {
        let (method, path) = line.split_once(char::is_whitespace).unwrap_or(("GET", "/"));
        let url = format!("{}{}", conn.base, if path.starts_with('/') { path.to_string() } else { format!("/{path}") });
        match method.to_ascii_uppercase().as_str() {
            "POST" => client.post(url),
            _ => client.get(url),
        }
    } else if line.starts_with('{') {
        // treat as _search body against database=index
        let index = if conn.database.is_empty() { "_all" } else { conn.database.as_str() };
        client
            .post(format!("{}/{}/_search", conn.base, index))
            .header("Content-Type", "application/json")
            .body(line.to_string())
    } else {
        // shorthand: index name → _search match_all
        let index = if line.is_empty() {
            if conn.database.is_empty() { "_all" } else { conn.database.as_str() }
        } else {
            line
        };
        client
            .post(format!("{}/{}/_search", conn.base, index))
            .header("Content-Type", "application/json")
            .body(r#"{"size":100,"query":{"match_all":{}}}"#)
    };
    if !conn.user.is_empty() {
        req = req.basic_auth(&conn.user, Some(&conn.password));
    }
    let resp = req.send().await.context("elasticsearch HTTP")?;
    let status = resp.status();
    let text = resp.text().await.context("elasticsearch body")?;
    if !status.is_success() {
        let preview: String = text.chars().take(240).collect();
        bail!("Elasticsearch HTTP {status}: {preview}");
    }
    json_to_result(&text)
}

async fn exec_trino(conn: &HttpSqlConn, sql: &str) -> Result<QueryResult> {
    let client = reqwest::Client::new();
    let catalog = if conn.database.is_empty() {
        "memory"
    } else {
        conn.database.as_str()
    };
    let mut req = client
        .post(format!("{}/v1/statement", conn.base))
        .header("X-Trino-Catalog", catalog)
        .header("X-Trino-Schema", "default")
        .header("X-Trino-User", if conn.user.is_empty() { "tui-ide" } else { &conn.user })
        .header("Content-Type", "text/plain")
        .body(sql.to_string());
    if !conn.password.is_empty() {
        req = req.basic_auth(&conn.user, Some(&conn.password));
    }
    let mut next = Some(req.send().await.context("trino statement")?);
    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    while let Some(resp) = next.take() {
        let status = resp.status();
        let text = resp.text().await.context("trino body")?;
        if !status.is_success() {
            let preview: String = text.chars().take(240).collect();
            bail!("Trino HTTP {status}: {preview}");
        }
        let v: Value = serde_json::from_str(&text).context("trino JSON")?;
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or_else(|| "unknown");
            bail!("Trino error: {msg}");
        }
        if columns.is_empty() {
            if let Some(cols) = v.get("columns").and_then(|c| c.as_array()) {
                columns = cols
                    .iter()
                    .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                    .collect();
            }
        }
        if let Some(data) = v.get("data").and_then(|d| d.as_array()) {
            for row in data {
                let cells = row
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|c| match c {
                                Value::Null => "NULL".into(),
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                rows.push(cells);
                if rows.len() >= 500 {
                    break;
                }
            }
        }
        if let Some(n) = v.get("nextUri").and_then(|u| u.as_str()) {
            if rows.len() < 500 {
                let mut r = client.get(n);
                if !conn.user.is_empty() {
                    r = r.basic_auth(&conn.user, Some(&conn.password));
                }
                next = Some(r.send().await.context("trino nextUri")?);
            }
        }
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

fn json_to_result(text: &str) -> Result<QueryResult> {
    let v: Value = serde_json::from_str(text).context("parse JSON")?;
    // ES hits
    if let Some(hits) = v.pointer("/hits/hits").and_then(|h| h.as_array()) {
        let mut rows = Vec::new();
        for h in hits.iter().take(500) {
            let id = h.get("_id").map(|x| x.to_string()).unwrap_or_default();
            let src = h.get("_source").cloned().unwrap_or(Value::Null);
            rows.push(vec![id, src.to_string()]);
        }
        let mut q = QueryResult::empty();
        q.columns = vec!["_id".into(), "_source".into()];
        q.rows = rows;
        q.original_rows = q.rows.clone();
        return Ok(q);
    }
    // generic object/array
    let mut q = QueryResult::empty();
    q.columns = vec!["json".into()];
    q.rows = vec![vec![text.to_string()]];
    q.original_rows = q.rows.clone();
    Ok(q)
}

pub async fn elasticsearch_indices(conn: &HttpSqlConn, source_id: &str) -> Result<Vec<DbTreeNode>> {
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{}/_cat/indices?format=json", conn.base));
    if !conn.user.is_empty() {
        req = req.basic_auth(&conn.user, Some(&conn.password));
    }
    let resp = req.send().await.context("ES _cat/indices")?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        let preview: String = text.chars().take(240).collect();
        bail!("Elasticsearch indices HTTP {status}: {preview}");
    }
    let arr: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
    Ok(arr
        .into_iter()
        .filter_map(|v| {
            let name = v.get("index")?.as_str()?.to_string();
            Some(DbTreeNode {
                id: format!("{source_id}::index::{name}"),
                label: name.clone(),
                kind: DbNodeKind::Table,
                source_id: source_id.into(),
                database: Some(name.clone()),
                schema: Some("indices".into()),
                table: Some(name),
                query_sql: None,
                children: Vec::new(),
                expanded: false,
            })
        })
        .collect())
}

/// Trino: list catalogs as Database nodes (expand → schemas via SHOW SCHEMAS).
pub async fn trino_catalogs(conn: &HttpSqlConn, source_id: &str) -> Result<Vec<DbTreeNode>> {
    let result = exec_trino(conn, "SHOW CATALOGS").await?;
    let mut out = Vec::new();
    for row in result.rows {
        let Some(name) = row.first().cloned() else {
            continue;
        };
        if name.eq_ignore_ascii_case("system") {
            continue;
        }
        out.push(DbTreeNode {
            id: format!("{source_id}::catalog::{name}"),
            label: name.clone(),
            kind: DbNodeKind::Database,
            source_id: source_id.into(),
            database: Some(name),
            schema: None,
            table: None,
            query_sql: None,
            children: Vec::new(),
            expanded: false,
        });
    }
    Ok(out)
}

pub async fn trino_schemas(
    conn: &HttpSqlConn,
    source_id: &str,
    catalog: &str,
) -> Result<Vec<DbTreeNode>> {
    let sql = format!("SHOW SCHEMAS FROM \"{catalog}\"");
    let result = exec_trino(conn, &sql).await?;
    let mut out = Vec::new();
    for row in result.rows {
        let Some(name) = row.first().cloned() else {
            continue;
        };
        out.push(DbTreeNode {
            id: format!("{source_id}::schema::{catalog}::{name}"),
            label: name.clone(),
            kind: DbNodeKind::Schema,
            source_id: source_id.into(),
            database: Some(catalog.into()),
            schema: Some(name),
            table: None,
            query_sql: None,
            children: Vec::new(),
            expanded: false,
        });
    }
    Ok(out)
}

pub async fn trino_tables(
    conn: &HttpSqlConn,
    source_id: &str,
    catalog: &str,
    schema: &str,
) -> Result<Vec<DbTreeNode>> {
    let sql = format!("SHOW TABLES FROM \"{catalog}\".\"{schema}\"");
    let result = exec_trino(conn, &sql).await?;
    let mut out = Vec::new();
    for row in result.rows {
        let Some(name) = row.first().cloned() else {
            continue;
        };
        out.push(DbTreeNode {
            id: format!("{source_id}::table::{catalog}::{schema}::{name}"),
            label: name.clone(),
            kind: DbNodeKind::Table,
            source_id: source_id.into(),
            database: Some(catalog.into()),
            schema: Some(schema.into()),
            table: Some(name),
            query_sql: None,
            children: Vec::new(),
            expanded: false,
        });
    }
    Ok(out)
}
