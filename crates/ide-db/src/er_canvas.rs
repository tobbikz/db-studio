//! TUI box-drawing ER canvas from FK edge list.

/// Build a simple canvas: one box per table, edges as lines underneath.
pub fn render_er_canvas(schema: &str, edges: &[(String, String, String, String)]) -> String {
    // edges: (from_table, from_col, to_table, to_col)
    let mut tables: Vec<String> = Vec::new();
    for (t, _, rt, _) in edges {
        if !tables.iter().any(|x| x == t) {
            tables.push(t.clone());
        }
        if !tables.iter().any(|x| x == rt) {
            tables.push(rt.clone());
        }
    }
    tables.sort();
    if tables.is_empty() {
        return format!("┌─ ER canvas · {schema} ─┐\n│ (no tables / FKs)     │\n└──────────────────────┘\n");
    }

    let mut out = format!("══ ER canvas · {schema} ══\n\n");
    for t in &tables {
        let cols: Vec<String> = edges
            .iter()
            .filter(|(ft, _, _, _)| ft == t)
            .map(|(_, c, rt, rc)| format!("  {c} → {rt}.{rc}"))
            .collect();
        let width = t.len().max(18).max(cols.iter().map(|s| s.len()).max().unwrap_or(0)) + 2;
        let bar = "─".repeat(width);
        out.push_str(&format!("┌{bar}┐\n"));
        out.push_str(&format!("│ {t:<width$} │\n", width = width - 1));
        out.push_str(&format!("├{bar}┤\n"));
        if cols.is_empty() {
            out.push_str(&format!("│ {:<width$} │\n", "(no outbound FK)", width = width - 1));
        } else {
            for c in cols {
                out.push_str(&format!("│ {c:<width$} │\n", width = width - 1));
            }
        }
        out.push_str(&format!("└{bar}┘\n\n"));
    }
    out.push_str("Edges:\n");
    for (t, c, rt, rc) in edges {
        out.push_str(&format!("  {t}.{c} ──► {rt}.{rc}\n"));
    }
    out
}
