//! Optional Brain RAG ask via CLI or Postgres `brain.consult`.

use anyhow::{Context, Result, bail};
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;

/// Run a Brain consult for the given natural-language query.
pub async fn brain_ask(query: &str) -> Result<String> {
    let q = query.trim();
    if q.is_empty() {
        bail!("empty question");
    }
    if let Ok(out) = brain_cli_ask(q).await {
        if !out.trim().is_empty() {
            return Ok(out);
        }
    }
    brain_sql_consult(q).await
}

async fn brain_cli_ask(query: &str) -> Result<String> {
    if let Ok(p) = std::env::var("BRAIN_CLI") {
        if !p.is_empty() {
            return run_brain_bin(&p, query).await;
        }
    }
    for name in ["brain", "rag"] {
        match run_brain_bin(name, query).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("No such file") || msg.contains("not found") {
                    continue;
                }
                return Err(e);
            }
        }
    }
    bail!("brain/rag CLI not on PATH (set BRAIN_CLI or use BRAIN_DSN SQL)");
}

async fn run_brain_bin(bin: &str, query: &str) -> Result<String> {
    let output = tokio::process::Command::new(bin)
        .args(["ask", query])
        .output()
        .await
        .with_context(|| format!("run {bin} ask"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("{bin} ask failed: {err}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn brain_sql_consult(query: &str) -> Result<String> {
    let dsn = std::env::var("BRAIN_DSN").unwrap_or_else(|_| {
        "postgresql://tomy.berrios@127.0.0.1:5432/brain".into()
    });
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&dsn)
        .await
        .context("connect BRAIN_DSN for brain.consult")?;
    let rows = sqlx::query(
        "SELECT chunk_text, path, score FROM brain.consult($1::text, 12) \
         ORDER BY score DESC NULLS LAST",
    )
    .bind(query)
    .fetch_all(&pool)
    .await
    .context("brain.consult SQL")?;
    if rows.is_empty() {
        return Ok("(no hits from brain.consult)".into());
    }
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        let text: String = row.try_get("chunk_text").unwrap_or_default();
        let path: String = row.try_get("path").unwrap_or_default();
        let score: f32 = row.try_get("score").unwrap_or(0.0);
        if i > 0 {
            out.push_str("\n\n---\n\n");
        }
        if !path.is_empty() {
            out.push_str(&format!("[{path} · {score:.3}]\n"));
        }
        out.push_str(&text);
    }
    Ok(out)
}
