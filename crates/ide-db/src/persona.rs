//! Persona 100% helpers: recipes, packs, quality, PII, dbt/BI bridges, audit.

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::{DbDriver, QueryResult};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersonaConfig {
    #[serde(default)]
    pub superset_url: String,
    #[serde(default)]
    pub metabase_url: String,
    #[serde(default)]
    pub airflow_url: String,
    #[serde(default)]
    pub model_registry_url: String,
    #[serde(default)]
    pub bi_export_dir: String,
    #[serde(default)]
    pub health_webhook_url: String,
    #[serde(default)]
    pub mask_pii: bool,
    #[serde(default)]
    pub confirm_destructive_ddl: bool,
    #[serde(default)]
    pub onboarding_tips: bool,
}

impl PersonaConfig {
    pub fn load() -> Self {
        let path = config_path();
        let Ok(text) = fs::read_to_string(&path) else {
            let cfg = Self {
                onboarding_tips: true,
                confirm_destructive_ddl: true,
                ..Default::default()
            };
            let _ = cfg.save();
            return cfg;
        };
        toml::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tui-ide")
        .join("persona.toml")
}

pub fn audit_log_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tui-ide")
        .join("audit.log")
}

/// Append one DDL/DML audit line (local file).
pub fn audit_append(source: &str, driver: &str, sql: &str) -> Result<()> {
    let path = audit_log_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    let one_line = sql.replace('\n', " ").chars().take(500).collect::<String>();
    writeln!(f, "{ts}\t{driver}\t{source}\t{one_line}")?;
    Ok(())
}

pub fn audit_export_csv(out: &Path) -> Result<()> {
    let path = audit_log_path();
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut csv = String::from("ts,driver,source,sql\n");
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        if parts.len() == 4 {
            csv.push_str(&format!(
                "{},{},{},\"{}\"\n",
                parts[0],
                parts[1],
                parts[2],
                parts[3].replace('"', "\"\"")
            ));
        }
    }
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, csv)?;
    Ok(())
}

pub fn is_destructive_ddl(sql: &str) -> bool {
    let s = sql.trim_start().to_ascii_uppercase();
    s.starts_with("DROP ")
        || s.starts_with("TRUNCATE ")
        || s.starts_with("ALTER TABLE") && s.contains(" DROP ")
        || s.starts_with("DELETE ") && !s.contains(" WHERE ")
}

/// Mask emails / phones / card-like digits in a cell.
pub fn mask_pii_cell(s: &str) -> String {
    if s == "NULL" || s.is_empty() {
        return s.to_string();
    }
    let only_cardish = s
        .chars()
        .all(|c| c.is_ascii_digit() || c == ' ' || c == '-' || c == '+');
    let digit_n = s.chars().filter(|c| c.is_ascii_digit()).count();
    if only_cardish && (13..=19).contains(&digit_n) {
        return "[card]".into();
    }
    let mut out = lazy_regex_email()
        .replace_all(s, "[email]")
        .to_string();
    out = lazy_regex_card().replace_all(&out, "[card]").to_string();
    out = lazy_regex_phone().replace_all(&out, "[phone]").to_string();
    out
}

fn lazy_regex_email() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}").unwrap())
}
fn lazy_regex_phone() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:\+?\d[\d\-\s().]{7,}\d)").unwrap())
}
fn lazy_regex_card() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(?:\d[ -]*?){13,19}\b").unwrap())
}

pub fn mask_result(result: &QueryResult) -> QueryResult {
    let mut out = result.clone();
    for row in &mut out.rows {
        for cell in row.iter_mut() {
            *cell = mask_pii_cell(cell);
        }
    }
    out
}

/// null% / distinct / min / max quality strip for Results.
pub fn quality_strip(result: &QueryResult) -> String {
    if result.columns.is_empty() {
        return "quality: (no columns)".into();
    }
    let n = result.rows.len().max(1) as f64;
    let mut lines = vec![format!("══ Quality strip · {} rows ══", result.rows.len())];
    for (ci, name) in result.columns.iter().enumerate() {
        let mut nulls = 0usize;
        let mut distinct = std::collections::BTreeSet::new();
        let mut nums = Vec::new();
        for row in &result.rows {
            let cell = row.get(ci).map(|s| s.as_str()).unwrap_or("NULL");
            if cell == "NULL" || cell.is_empty() {
                nulls += 1;
            } else {
                distinct.insert(cell.to_string());
                if let Ok(v) = cell.parse::<f64>() {
                    nums.push(v);
                }
            }
        }
        let null_pct = (nulls as f64 / n) * 100.0;
        let mut line = format!(
            "  {name}: null={null_pct:.1}% distinct={}",
            distinct.len()
        );
        if let (Some(min), Some(max)) = (
            nums.iter().copied().reduce(f64::min),
            nums.iter().copied().reduce(f64::max),
        ) {
            line.push_str(&format!(" min={min} max={max}"));
        }
        lines.push(line);
    }
    lines.join("\n")
}

pub fn chart_typed(result: &QueryResult, kind: &str, value_col: Option<usize>, label_col: Option<usize>) -> Result<String> {
    let kind = kind.to_ascii_lowercase();
    let vci = value_col.unwrap_or_else(|| first_numeric(result).unwrap_or(0));
    let values = parse_nums(result, vci).context("no numeric column")?;
    let labels: Vec<String> = (0..values.len())
        .map(|i| {
            if let Some(lci) = label_col {
                result
                    .rows
                    .get(i)
                    .and_then(|r| r.get(lci))
                    .cloned()
                    .unwrap_or_else(|| (i + 1).to_string())
            } else {
                (i + 1).to_string()
            }
        })
        .collect();
    let col = result.columns.get(vci).map(|s| s.as_str()).unwrap_or("?");
    match kind.as_str() {
        "pie" => Ok(render_pie(col, &labels, &values)),
        "line" => Ok(render_line(col, &labels, &values)),
        _ => Ok(render_bar(col, &labels, &values)),
    }
}

fn first_numeric(result: &QueryResult) -> Option<usize> {
    (0..result.columns.len()).find(|&ci| parse_nums(result, ci).is_some())
}

fn parse_nums(result: &QueryResult, ci: usize) -> Option<Vec<f64>> {
    let mut out = Vec::new();
    for row in &result.rows {
        let cell = row.get(ci)?;
        if cell == "NULL" {
            continue;
        }
        if let Ok(v) = cell.parse::<f64>() {
            out.push(v);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn render_bar(col: &str, labels: &[String], values: &[f64]) -> String {
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).abs().max(f64::EPSILON);
    let mut out = format!("Bar chart · {col}\n");
    for (i, v) in values.iter().take(16).enumerate() {
        let n = (((v - min) / span) * 28.0).round() as usize;
        let bar: String = std::iter::repeat('█').take(n).collect();
        let lab = labels.get(i).map(|s| s.as_str()).unwrap_or("?");
        out.push_str(&format!(" {lab:>12} │{bar:<28} {v:.4}\n"));
    }
    out
}

fn render_line(col: &str, labels: &[String], values: &[f64]) -> String {
    let spark = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).abs().max(f64::EPSILON);
    let line: String = values
        .iter()
        .map(|v| {
            let t = ((v - min) / span).clamp(0.0, 1.0);
            spark[(t * (spark.len() - 1) as f64).round() as usize]
        })
        .collect();
    format!(
        "Line chart · {col}\n  {}\n  labels: {}\n  min={min:.4} max={max:.4}\n",
        line,
        labels.iter().take(12).cloned().collect::<Vec<_>>().join(", ")
    )
}

fn render_pie(col: &str, labels: &[String], values: &[f64]) -> String {
    let sum: f64 = values.iter().sum::<f64>().max(f64::EPSILON);
    let mut out = format!("Pie chart · {col} (share %)\n");
    for (i, v) in values.iter().take(12).enumerate() {
        let pct = (v / sum) * 100.0;
        let n = (pct / 5.0).round() as usize;
        let wedge: String = std::iter::repeat('●').take(n).collect();
        let lab = labels.get(i).map(|s| s.as_str()).unwrap_or("?");
        out.push_str(&format!(" {lab:>12} {pct:5.1}% {wedge}\n"));
    }
    out
}

pub fn export_result_pack(result: &QueryResult, sql: &str, out_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(out_dir)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let base = out_dir.join(format!("explore-{stamp}"));
    fs::create_dir_all(&base)?;
    fs::write(base.join("query.sql"), sql)?;
    let mut csv = result.columns.join(",");
    csv.push('\n');
    for row in &result.rows {
        let line = row
            .iter()
            .map(|c| {
                if c.contains(',') || c.contains('"') {
                    format!("\"{}\"", c.replace('"', "\"\""))
                } else {
                    c.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        csv.push_str(&line);
        csv.push('\n');
    }
    fs::write(base.join("result.csv"), csv)?;
    let meta = format!(
        "elapsed_ms={}\nrows={}\ncols={}\nchart={}\n",
        result.elapsed_ms,
        result.rows.len(),
        result.columns.len(),
        result.chart.as_deref().unwrap_or("(none)")
    );
    fs::write(base.join("meta.txt"), meta)?;
    Ok(base)
}

pub fn recipe_gallery() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "top_customers",
            "-- recipe: top customers by orders\n\
             SELECT customer_id, COUNT(*) AS orders\n\
             FROM orders\n\
             GROUP BY 1 ORDER BY 2 DESC LIMIT 20;",
        ),
        (
            "revenue_by_day",
            "-- recipe: revenue by day\n\
             SELECT DATE(created_at) AS day, SUM(amount) AS revenue\n\
             FROM payments\n\
             GROUP BY 1 ORDER BY 1 DESC LIMIT 30;",
        ),
        (
            "null_audit",
            "-- recipe: null audit on selected table (edit schema.table)\n\
             SELECT column_name,\n\
                    (SELECT COUNT(*) FROM schema.table t WHERE t.col IS NULL) AS nulls\n\
             FROM information_schema.columns\n\
             WHERE table_schema = 'schema' AND table_name = 'table';",
        ),
        (
            "stratified_sample",
            "-- recipe: stratified sample ~1%\n\
             SELECT * FROM schema.table TABLESAMPLE SYSTEM (1);",
        ),
        (
            "embedding_similarity",
            "-- recipe: pgvector similarity (requires vector extension)\n\
             SELECT id, 1 - (embedding <=> $1) AS score\n\
             FROM embeddings ORDER BY embedding <=> $1 LIMIT 10;",
        ),
        (
            "feature_table",
            "-- recipe: feature table template\n\
             CREATE TABLE IF NOT EXISTS features.user_daily (\n\
               user_id BIGINT NOT NULL,\n\
               day DATE NOT NULL,\n\
               features JSONB NOT NULL,\n\
               PRIMARY KEY (user_id, day)\n\
             );",
        ),
        (
            "batch_score",
            "-- recipe: batch scoring stub\n\
             SELECT id, /* model_score(features) */ 0.0 AS score\n\
             FROM features.user_daily WHERE day = CURRENT_DATE;",
        ),
        (
            "warehouse_bytes",
            "-- recipe: inspect EXPLAIN for bytes (BQ/Snowflake)\n\
             -- Run EXPLAIN then palette → Quant: cost from explain",
        ),
    ]
}

pub fn ensure_recipe_snippets() -> Result<usize> {
    let dir = crate::snippets::snippets_dir().context("no config dir")?;
    fs::create_dir_all(&dir)?;
    let mut n = 0;
    for (name, body) in recipe_gallery() {
        let path = dir.join(format!("recipe_{name}.sql"));
        if !path.exists() {
            fs::write(&path, body)?;
            n += 1;
        }
    }
    Ok(n)
}

pub fn bi_deep_link(cfg: &PersonaConfig, schema: &str, table: &str) -> Result<String> {
    if !cfg.superset_url.is_empty() {
        let base = cfg.superset_url.trim_end_matches('/');
        return Ok(format!(
            "{base}/explore/?datasource_type=table&schema={schema}&table={table}"
        ));
    }
    if !cfg.metabase_url.is_empty() {
        let base = cfg.metabase_url.trim_end_matches('/');
        return Ok(format!("{base}/question#?schema={schema}&table={table}"));
    }
    bail!("set superset_url or metabase_url in ~/.config/tui-ide/persona.toml");
}

pub fn airflow_deep_link(cfg: &PersonaConfig) -> Result<String> {
    if cfg.airflow_url.is_empty() {
        bail!("set airflow_url in ~/.config/tui-ide/persona.toml");
    }
    Ok(format!("{}/dags", cfg.airflow_url.trim_end_matches('/')))
}

pub fn model_registry_link(cfg: &PersonaConfig) -> Result<String> {
    if cfg.model_registry_url.is_empty() {
        bail!("set model_registry_url in ~/.config/tui-ide/persona.toml");
    }
    Ok(cfg.model_registry_url.trim_end_matches('/').to_string())
}

pub fn detect_dbt_project(root: &Path) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for _ in 0..6 {
        let yml = cur.join("dbt_project.yml");
        if yml.exists() {
            return Some(cur);
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

pub fn dbt_command(project: &Path, sub: &str, model: Option<&str>) -> String {
    let mut cmd = format!("cd {} && dbt {sub}", project.display());
    if let Some(m) = model {
        if !m.is_empty() {
            cmd.push_str(&format!(" --select {m}"));
        }
    }
    cmd
}

pub fn parse_dbt_schema_yml(path: &Path) -> Result<String> {
    let text = fs::read_to_string(path)?;
    let mut out = String::from("══ dbt schema.yml stubs ══\n");
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("- name:") || t.starts_with("name:") {
            out.push_str(line);
            out.push('\n');
        }
        if t.starts_with("- unique")
            || t.starts_with("- not_null")
            || t.starts_with("unique:")
            || t.starts_with("not_null:")
            || t.contains("accepted_values")
            || t.starts_with("- name:") && out.contains("metrics")
        {
            out.push_str("  ");
            out.push_str(t);
            out.push('\n');
        }
        if t.starts_with("description:") {
            out.push_str(line);
            out.push('\n');
        }
    }
    if out.lines().count() < 2 {
        out.push_str("(no models/tests found — open models/schema.yml)\n");
    }
    Ok(out)
}

pub fn codegen_struct(driver: DbDriver, lang: &str, table: &str, cols: &[(String, String)]) -> String {
    let name = to_pascal(table);
    match lang {
        "ts" | "typescript" => {
            let mut s = format!("export interface {name} {{\n");
            for (c, ty) in cols {
                s.push_str(&format!("  {c}: {};\n", ts_type(ty)));
            }
            s.push_str("}\n");
            s
        }
        _ => {
            let mut s = format!("#[derive(Debug, Clone)]\npub struct {name} {{\n");
            for (c, ty) in cols {
                s.push_str(&format!("    pub {}: {},\n", rust_field(c), rust_type(driver, ty)));
            }
            s.push_str("}\n");
            s
        }
    }
}

fn to_pascal(s: &str) -> String {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect()
}

fn rust_field(s: &str) -> String {
    let f = s.to_ascii_lowercase();
    if matches!(
        f.as_str(),
        "type" | "match" | "ref" | "self" | "where" | "async"
    ) {
        format!("r#{f}")
    } else {
        f
    }
}

fn rust_type(_driver: DbDriver, ty: &str) -> &'static str {
    let t = ty.to_ascii_lowercase();
    if t.contains("int") || t.contains("serial") {
        "i64"
    } else if t.contains("bool") {
        "bool"
    } else if t.contains("float") || t.contains("double") || t.contains("numeric") || t.contains("decimal") {
        "f64"
    } else if t.contains("json") {
        "serde_json::Value"
    } else if t.contains("timestamp") || t.contains("datetime") {
        "String"
    } else {
        "String"
    }
}

fn ts_type(ty: &str) -> &'static str {
    let t = ty.to_ascii_lowercase();
    if t.contains("int") || t.contains("float") || t.contains("numeric") || t.contains("decimal") {
        "number"
    } else if t.contains("bool") {
        "boolean"
    } else {
        "string"
    }
}

pub fn golden_diff(result: &QueryResult, golden_csv: &Path) -> Result<String> {
    let text = fs::read_to_string(golden_csv)?;
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("").to_string();
    let golden_cols: Vec<&str> = header.split(',').map(|s| s.trim()).collect();
    let mut mismatches = Vec::new();
    let mut gi = 0usize;
    for (ri, row) in result.rows.iter().enumerate() {
        let Some(gline) = lines.next() else {
            mismatches.push(format!("row {ri}: extra in result"));
            continue;
        };
        let gcells: Vec<&str> = gline.split(',').collect();
        for (ci, col) in result.columns.iter().enumerate() {
            let got = row.get(ci).map(|s| s.as_str()).unwrap_or("");
            let exp = gcells.get(ci).copied().unwrap_or("");
            if got.trim() != exp.trim() {
                mismatches.push(format!("row {ri} col {col}: got={got:?} expected={exp:?}"));
            }
        }
        gi += 1;
        let _ = golden_cols; // silence
    }
    for _ in lines {
        mismatches.push(format!("row {gi}: missing in result"));
        gi += 1;
    }
    if mismatches.is_empty() {
        Ok(format!(
            "GOLDEN PASS · {} rows match {}",
            result.rows.len(),
            golden_csv.display()
        ))
    } else {
        Ok(format!(
            "GOLDEN FAIL · {} mismatches\n{}",
            mismatches.len(),
            mismatches.into_iter().take(40).collect::<Vec<_>>().join("\n")
        ))
    }
}

pub fn fanout_rowcount_assert(counts: &[(String, usize)]) -> String {
    if counts.is_empty() {
        return "fan-out assert: no pinned results".into();
    }
    let first = counts[0].1;
    let mut ok = true;
    let mut lines = vec!["══ Fan-out rowcount assert ══".into()];
    for (name, n) in counts {
        let mark = if *n == first { "OK" } else { "DIFF" };
        if *n != first {
            ok = false;
        }
        lines.push(format!("  {name}: {n} {mark}"));
    }
    lines.push(if ok {
        "PASS — all equal".into()
    } else {
        "FAIL — rowcounts differ".into()
    });
    lines.join("\n")
}

pub fn explain_cost_hint(explain_text: &str) -> String {
    let mut hits = Vec::new();
    for line in explain_text.lines() {
        let l = line.to_ascii_lowercase();
        if l.contains("bytes")
            || l.contains("slot")
            || l.contains("cost=")
            || l.contains("estimated")
            || l.contains("billing")
        {
            hits.push(line.to_string());
        }
    }
    if hits.is_empty() {
        "No bytes/slot/cost lines found in EXPLAIN — warehouse may omit them in text format."
            .into()
    } else {
        format!("══ Cost / bytes from EXPLAIN ══\n{}", hits.join("\n"))
    }
}

pub fn cdc_lag_sql(driver: DbDriver) -> Result<&'static str> {
    match driver {
        DbDriver::Postgres => Ok(
            "SELECT client_addr, state, sent_lsn, write_lsn, flush_lsn, replay_lsn,\n\
             pg_wal_lsn_diff(sent_lsn, replay_lsn) AS replay_lag_bytes\n\
             FROM pg_stat_replication;",
        ),
        DbDriver::Mysql => Ok(
            "SHOW SLAVE STATUS; -- or SHOW REPLICA STATUS (MySQL 8.0.22+)",
        ),
        _ => bail!("CDC/replication lag panel for Postgres/MySQL only"),
    }
}

pub fn privilege_audit_sql(driver: DbDriver, role: &str) -> Result<String> {
    let role = role.replace('\'', "''");
    match driver {
        DbDriver::Postgres => Ok(format!(
            "SELECT table_schema, table_name, privilege_type\n\
             FROM information_schema.role_table_grants\n\
             WHERE grantee = '{role}'\n\
             ORDER BY 1,2,3;"
        )),
        DbDriver::Mysql => Ok(format!(
            "SHOW GRANTS FOR '{role}'@'%';"
        )),
        DbDriver::Mssql => Ok(format!(
            "SELECT dp.name AS principal, o.name AS object, p.permission_name, p.state_desc\n\
             FROM sys.database_permissions p\n\
             JOIN sys.database_principals dp ON p.grantee_principal_id = dp.principal_id\n\
             LEFT JOIN sys.objects o ON p.major_id = o.object_id\n\
             WHERE dp.name = N'{role}';"
        )),
        DbDriver::Clickhouse => Ok(format!(
            "SELECT * FROM system.grants WHERE user_name = '{role}' OR role_name = '{role}';"
        )),
        _ => bail!("privilege audit SQL not defined for {}", driver.label()),
    }
}

pub fn lineage_from_er(edges: &[(String, String, String, String)], focus: &str) -> String {
    let mut out = format!("══ Column lineage sketch · {focus} ══\n");
    let mut n = 0;
    for (t, c, rt, rc) in edges {
        if t == focus || rt == focus || focus.is_empty() {
            out.push_str(&format!("  {t}.{c} → {rt}.{rc}  (FK · N:1)\n"));
            n += 1;
        }
    }
    if n == 0 {
        out.push_str("  (no FK edges — run ER sketch first)\n");
    }
    out.push_str("\nBest-effort only; query AST lineage not fully parsed.\n");
    out
}

pub fn naming_hint(driver: DbDriver, logical: &str) -> String {
    let phys = logical
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    match driver {
        DbDriver::Bigquery => format!("logical `{logical}` → physical `{phys}` (BQ: dataset.table)"),
        DbDriver::Snowflake => format!("logical `{logical}` → physical \"{phys}\" (SF: SCHEMA.TABLE)"),
        DbDriver::Postgres => format!("logical `{logical}` → physical \"{phys}\" (snake_case)"),
        _ => format!("logical `{logical}` → physical `{phys}`"),
    }
}

pub fn onboarding_tips() -> &'static str {
    "══ Studio tips ══\n\
     · Ctrl-P palette · F5 run · / filter results\n\
     · n New object · recipes in snippets/recipe_*.sql\n\
     · SourceRole=viewer hides DDL create menu\n\
     · persona.toml: BI / Airflow / webhook URLs\n\
     · Studio help for persona workflows\n"
}

pub fn studio_help_personas() -> &'static str {
    "\n\
     ══ Personas (100%) ══\n\
     Analyst   save/bookmark queries · Chart bar/line/pie · Export result pack\n\
     Architect ER canvas PK/FK · schema diff · naming hints · schema ADR note\n\
     DE        dbt detect/run · Airflow link · lineage · quality strip · CDC lag\n\
     AE        dbt compile/test · schema.yml stubs · model docs\n\
     BI        persona.toml Superset/Metabase URL · deep-link · CSV/Parquet path\n\
     DS        vector recipes · sample SQL · Agents Jupyter/python presets\n\
     MLOps     Agents mlflow/serve presets · batch score recipe · registry URL\n\
     Dev       seed fixtures · Rust/TS codegen · migrate dry-run/apply\n\
     DevOps    audit.log DDL/DML · health webhook · vault docs · signed pack\n\
     Sec       mask PII · privilege audit · destructive DDL confirm · audit CSV\n\
     Support   diagnose pack · read-only messaging · one-file export\n\
     Product   recipe gallery · ask→SQL Agents · viewer hides DDL · tips\n\
     QA        golden CSV diff · testdata fixtures · fan-out rowcount assert\n\
     Quant     EXPLAIN cost/bytes · warehouse Create · history cost · help\n\
     \n\
     Config: ~/.config/tui-ide/persona.toml · audit: ~/.config/tui-ide/audit.log\n\
     Vault: password env:VAR or secrets.toml — never plaintext in packs\n\
     Fixtures: workspace/testdata/*.sql · golden: testdata/golden/*.csv\n"
}

pub fn post_health_webhook(url: &str, body: &str) -> Result<()> {
    if url.is_empty() {
        bail!("health_webhook_url empty");
    }
    // Best-effort blocking curl — keeps deps light.
    let status = std::process::Command::new("curl")
        .args([
            "-sS",
            "-X",
            "POST",
            "-H",
            "Content-Type: text/plain",
            "--data-binary",
            body,
            url,
        ])
        .status()
        .context("curl webhook")?;
    if !status.success() {
        bail!("webhook POST failed: {status}");
    }
    Ok(())
}

pub fn signed_pack_export(sources_toml: &str, out: &Path) -> Result<()> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    sources_toml.hash(&mut h);
    let sig = format!("{:x}", h.finish());
    let body = format!("# tui-ide connection pack · sig={sig}\n{sources_toml}");
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, body)?;
    Ok(())
}

pub fn diagnose_pack_template(schema: &str, table: &str, explain: &str, stats: &str) -> String {
    format!(
        "══ Support diagnose pack ══\n\
         object: {schema}.{table}\n\
         generated: {}\n\
         \n\
         --- EXPLAIN ---\n{explain}\n\
         \n\
         --- TABLE STATS ---\n{stats}\n\
         \n\
         --- NOTES ---\n\
         Attach this file to the support ticket. Strip PII if needed (palette Mask PII).\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_email_phone() {
        assert!(mask_pii_cell("a@b.com").contains("[email]"));
        assert!(mask_pii_cell("4111 1111 1111 1111").contains("[card]"));
    }

    #[test]
    fn quality_and_charts() {
        let qr = QueryResult {
            columns: vec!["g".into(), "n".into()],
            rows: vec![
                vec!["a".into(), "1".into()],
                vec!["b".into(), "3".into()],
                vec!["NULL".into(), "2".into()],
            ],
            ..QueryResult::empty()
        };
        let q = quality_strip(&qr);
        assert!(q.contains("null="));
        assert!(chart_typed(&qr, "pie", Some(1), Some(0)).unwrap().contains("Pie"));
        assert!(chart_typed(&qr, "bar", Some(1), Some(0)).unwrap().contains("Bar"));
        assert!(chart_typed(&qr, "line", Some(1), None).unwrap().contains("Line"));
    }

    #[test]
    fn codegen_rust_ts() {
        let cols = vec![("id".into(), "bigint".into()), ("email".into(), "text".into())];
        assert!(codegen_struct(DbDriver::Postgres, "rust", "users", &cols).contains("struct Users"));
        assert!(codegen_struct(DbDriver::Postgres, "ts", "users", &cols).contains("interface Users"));
    }

    #[test]
    fn fanout_assert() {
        let s = fanout_rowcount_assert(&[("a".into(), 3), ("b".into(), 3)]);
        assert!(s.contains("PASS"));
        let f = fanout_rowcount_assert(&[("a".into(), 3), ("b".into(), 4)]);
        assert!(f.contains("FAIL"));
    }
}
