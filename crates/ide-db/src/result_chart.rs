//! ASCII / Unicode charts from numeric query columns.

use crate::QueryResult;

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn chart_result(result: &QueryResult, col_idx: Option<usize>) -> Option<String> {
    if result.rows.is_empty() || result.columns.is_empty() {
        return None;
    }
    let ci = col_idx.unwrap_or_else(|| first_numeric_column(result));
    let col_name = result.columns.get(ci)?;
    let values = parse_numeric_column(result, ci)?;
    if values.is_empty() {
        return None;
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).abs().max(f64::EPSILON);
    let sparkline: String = values
        .iter()
        .map(|v| {
            let t = ((v - min) / span).clamp(0.0, 1.0);
            let idx = (t * (SPARK.len() - 1) as f64).round() as usize;
            SPARK[idx]
        })
        .collect();
    let bar_w = 24usize;
    let mut bars = String::new();
    for (i, v) in values.iter().take(12).enumerate() {
        let t = ((v - min) / span).clamp(0.0, 1.0);
        let n = (t * bar_w as f64).round() as usize;
        let bar: String = std::iter::repeat('█').take(n).collect();
        bars.push_str(&format!(" {:>2} │{:<width$} {:.4}\n", i + 1, bar, v, width = bar_w));
    }
    Some(format!(
        "Chart · {col_name} (n={}) min={min:.4} max={max:.4}\n\
         Spark: {sparkline}\n\
         Bars (first {} rows):\n{bars}",
        values.len(),
        values.len().min(12)
    ))
}

fn first_numeric_column(result: &QueryResult) -> usize {
    for ci in 0..result.columns.len() {
        if parse_numeric_column(result, ci).map(|v| !v.is_empty()).unwrap_or(false) {
            return ci;
        }
    }
    0
}

fn parse_numeric_column(result: &QueryResult, ci: usize) -> Option<Vec<f64>> {
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
    if out.is_empty() { None } else { Some(out) }
}
