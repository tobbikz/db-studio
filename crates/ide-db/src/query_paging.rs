//! Result paging: LIMIT/OFFSET wrapper for SELECT queries.

pub const RESULT_PAGE_SIZE: usize = 500;

/// Append LIMIT/OFFSET when the statement looks like a plain SELECT/WITH (no existing LIMIT).
pub fn paginate_select_sql(sql: &str, page: usize) -> String {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return sql.to_string();
    }
    if !is_paged_query(trimmed) || has_limit_clause(trimmed) {
        return sql.to_string();
    }
    let offset = page.saturating_mul(RESULT_PAGE_SIZE);
    format!("{trimmed} LIMIT {RESULT_PAGE_SIZE} OFFSET {offset}")
}

pub fn is_paged_query(sql: &str) -> bool {
    let t = sql.trim_start().to_ascii_lowercase();
    t.starts_with("select") || t.starts_with("with")
}

fn has_limit_clause(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    // Naive: user-authored LIMIT disables auto-paging.
    lower.contains(" limit ") || lower.ends_with(" limit")
}
