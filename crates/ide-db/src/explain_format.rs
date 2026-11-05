//! Parse Postgres `EXPLAIN (ANALYZE, …)` text into tabular rows when possible.

use crate::QueryResult;

/// If `text` looks like a Postgres EXPLAIN ANALYZE plan, return a structured grid; else None.
pub fn postgres_explain_grid(text: &str) -> Option<QueryResult> {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut parsed_any = false;
    for line in &lines {
        if let Some(row) = parse_explain_line(line) {
            parsed_any = true;
            rows.push(row);
        } else if line.contains("(cost=") || line.contains("actual time=") {
            // Plan-ish line we failed to parse — abort structured view.
            return None;
        }
    }
    if !parsed_any {
        return None;
    }
    Some(QueryResult {
        columns: vec![
            "depth".into(),
            "node".into(),
            "cost".into(),
            "rows".into(),
            "actual_time".into(),
        ],
        rows,
        affected: None,
        elapsed_ms: 0,
        truncated: false,
        source_schema: None,
        source_table: None,
        primary_keys: Vec::new(),
        original_rows: Vec::new(),
        cursor_row: 0,
        cursor_col: 0,
        explain: Some(text.to_string()),
        chart: None,
        page: 0,
        has_more: false,
    })
}

fn parse_explain_line(line: &str) -> Option<Vec<String>> {
    let lead = line.len() - line.trim_start().len();
    let depth = (lead / 2).to_string();
    let trimmed = line.trim_start();
    let cost = extract_paren_field(trimmed, "cost=")?;
    let rows_est = extract_paren_field(trimmed, "rows=").unwrap_or_else(|| "—".into());
    let actual = extract_paren_field(trimmed, "actual time=").unwrap_or_else(|| "—".into());
    let node = node_label(trimmed);
    if node.is_empty() {
        return None;
    }
    Some(vec![depth, node, cost, rows_est, actual])
}

fn node_label(trimmed: &str) -> String {
    if let Some(idx) = trimmed.find(" (cost=") {
        return trimmed[..idx].trim().to_string();
    }
    if let Some(idx) = trimmed.find(" (actual") {
        return trimmed[..idx].trim().to_string();
    }
    trimmed.to_string()
}

fn extract_paren_field(s: &str, key: &str) -> Option<String> {
    let start = s.find(key)? + key.len();
    let rest = &s[start..];
    let end = rest
        .find(|c: char| c == ')' || c == ' ')
        .unwrap_or(rest.len());
    let val = rest[..end].trim();
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}
