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

/// Sparkline from latency history (ms).
pub fn latency_sparkline(ms: &[u128], width: usize) -> String {
    if ms.is_empty() {
        return "▁".repeat(width.max(1));
    }
    let slice = if ms.len() > width {
        &ms[ms.len() - width..]
    } else {
        ms
    };
    let min = *slice.iter().min().unwrap_or(&0) as f64;
    let max = *slice.iter().max().unwrap_or(&1) as f64;
    let span = (max - min).abs().max(1.0);
    slice
        .iter()
        .map(|v| {
            let t = ((*v as f64 - min) / span).clamp(0.0, 1.0);
            SPARK[(t * (SPARK.len() - 1) as f64).round() as usize]
        })
        .collect()
}

/// Selection stats for numeric cells in the current column (or whole grid if col None).
pub fn selection_stats(result: &QueryResult, col: Option<usize>) -> String {
    let cols: Vec<usize> = match col {
        Some(c) if c < result.columns.len() => vec![c],
        _ => (0..result.columns.len()).collect(),
    };
    let mut nums = Vec::new();
    for ci in cols {
        if let Some(vs) = parse_numeric_column(result, ci) {
            nums.extend(vs);
        }
    }
    if nums.is_empty() {
        return format!("n={}", result.rows.len());
    }
    let sum: f64 = nums.iter().sum();
    let avg = sum / nums.len() as f64;
    let min = nums.iter().copied().fold(f64::INFINITY, f64::min);
    let max = nums.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    format!(
        "n={} sum={sum:.4} avg={avg:.4} min={min:.4} max={max:.4}",
        nums.len()
    )
}

pub fn chart_kind(result: &QueryResult, kind: &str, col_idx: Option<usize>) -> Option<String> {
    let ci = col_idx.unwrap_or_else(|| first_numeric_column(result));
    let col_name = result.columns.get(ci)?;
    let values = parse_numeric_column(result, ci)?;
    if values.is_empty() {
        return None;
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).abs().max(f64::EPSILON);
    match kind {
        "line" => {
            let spark: String = values
                .iter()
                .map(|v| {
                    let t = ((v - min) / span).clamp(0.0, 1.0);
                    SPARK[(t * (SPARK.len() - 1) as f64).round() as usize]
                })
                .collect();
            Some(format!(
                "Line · {col_name}\n  {spark}\n  min={min:.4} max={max:.4} n={}\n",
                values.len()
            ))
        }
        "pie" => {
            let sum: f64 = values.iter().sum::<f64>().max(f64::EPSILON);
            let mut out = format!("Pie · {col_name}\n");
            for (i, v) in values.iter().take(12).enumerate() {
                let pct = (v / sum) * 100.0;
                let n = (pct / 5.0).round() as usize;
                let wedge: String = std::iter::repeat('●').take(n).collect();
                out.push_str(&format!("  {:>2} {pct:5.1}% {wedge}\n", i + 1));
            }
            Some(out)
        }
        _ => chart_result(result, Some(ci)),
    }
}
