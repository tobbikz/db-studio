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

/// ASCII EXPLAIN visuals: timeline bars + nested boxes (icicle-ish) from plan depth.
pub fn explain_visual_ascii(text: &str) -> String {
    let Some(grid) = postgres_explain_grid(text) else {
        return format!("══ EXPLAIN (raw) ══\n{text}");
    };
    let mut out = String::from("══ EXPLAIN visual · boxes + timeline ══\n\n");
    let mut timed: Vec<(usize, String, f64)> = Vec::new();
    for row in &grid.rows {
        let depth: usize = row.first().and_then(|s| s.parse().ok()).unwrap_or(0);
        let node = row.get(1).cloned().unwrap_or_default();
        let t = row
            .get(4)
            .and_then(|s| parse_time_ms(s))
            .or_else(|| row.get(2).and_then(|s| parse_cost_end(s)))
            .unwrap_or(0.0);
        timed.push((depth, node, t));
    }
    let max_t = timed
        .iter()
        .map(|(_, _, t)| *t)
        .fold(0.0_f64, f64::max)
        .max(f64::EPSILON);
    out.push_str("Timeline (relative actual/cost):\n");
    for (depth, node, t) in &timed {
        let bar_n = ((*t / max_t) * 28.0).round() as usize;
        let bar: String = std::iter::repeat('█').take(bar_n).collect();
        let pad = "│ ".repeat(*depth);
        out.push_str(&format!(" {pad}├─ {node:<32} {bar:<28} {t:.3}\n"));
    }
    out.push('\n');
    out.push_str("Icicle / nested boxes:\n");
    for (depth, node, t) in &timed {
        let w = (40usize.saturating_sub(depth * 2)).max(12);
        let label = truncate(&format!("{node} · {t:.2}"), w.saturating_sub(2));
        let indent = "  ".repeat(*depth);
        let bar = "─".repeat(w);
        out.push_str(&format!("{indent}┌{bar}┐\n"));
        out.push_str(&format!("{indent}│ {label:<width$} │\n", width = w - 1));
        out.push_str(&format!("{indent}└{bar}┘\n"));
    }
    out.push('\n');
    out.push_str("Funnel (depth → count):\n");
    let mut by_depth = std::collections::BTreeMap::<usize, usize>::new();
    for (d, _, _) in &timed {
        *by_depth.entry(*d).or_default() += 1;
    }
    let max_c = by_depth.values().copied().max().unwrap_or(1).max(1);
    for (d, c) in by_depth {
        let n = ((c as f64 / max_c as f64) * 24.0).round() as usize;
        let wedge: String = std::iter::repeat('▓').take(n.max(1)).collect();
        out.push_str(&format!("  d{d}: {wedge} ({c})\n"));
    }
    out
}

fn parse_time_ms(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((_, end)) = s.split_once("..") {
        end.parse().ok()
    } else {
        s.parse().ok()
    }
}

fn parse_cost_end(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((_, end)) = s.split_once("..") {
        end.parse().ok()
    } else {
        s.parse().ok()
    }
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else if max <= 1 {
        "…".into()
    } else {
        chars[..max - 1].iter().collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_from_analyze_text() {
        let plan = "\
Seq Scan on t  (cost=0.00..10.00 rows=100 width=4) (actual time=0.010..0.050 rows=100 loops=1)\n\
  Filter: (id > 1)\n\
Planning Time: 0.1 ms\n\
Execution Time: 0.2 ms\n";
        let v = explain_visual_ascii(plan);
        assert!(v.contains("EXPLAIN"));
    }

    #[test]
    fn visual_boxes_when_structured() {
        let plan = "\
Nested Loop  (cost=0.00..20.00 rows=10 width=8) (actual time=0.100..1.000 rows=10 loops=1)\n\
  ->  Seq Scan on a  (cost=0.00..5.00 rows=10 width=4) (actual time=0.010..0.050 rows=10 loops=1)\n\
  ->  Index Scan on b  (cost=0.00..1.00 rows=1 width=4) (actual time=0.005..0.008 rows=1 loops=10)\n";
        let v = explain_visual_ascii(plan);
        assert!(v.contains("Timeline") || v.contains("boxes") || v.contains("EXPLAIN"));
    }
}
