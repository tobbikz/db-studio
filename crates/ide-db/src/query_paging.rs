//! Result paging: LIMIT/OFFSET wrapper for SELECT queries.

/// Default auto-LIMIT when the user omits LIMIT (config override via `select_limit`).
pub const RESULT_PAGE_SIZE: usize = 50;

/// Hard cap when reading rows from the driver (manual LIMIT can be larger than page size).
pub const MAX_FETCH_ROWS: usize = 5_000;

/// Presets cycled by Alt-L in Studio.
pub const SELECT_LIMIT_PRESETS: &[usize] = &[50, 100, 200, 500, 0];

/// Append LIMIT/OFFSET when the statement looks like a plain SELECT/WITH (no existing LIMIT).
/// `page_size == 0` disables auto-paging (run as written).
pub fn paginate_select_sql(sql: &str, page: usize, page_size: usize) -> String {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return sql.to_string();
    }
    if page_size == 0 || !is_paged_query(trimmed) || has_limit_clause(trimmed) {
        return sql.to_string();
    }
    let offset = page.saturating_mul(page_size);
    format!("{trimmed} LIMIT {page_size} OFFSET {offset}")
}

pub fn is_paged_query(sql: &str) -> bool {
    let t = sql.trim_start().to_ascii_lowercase();
    t.starts_with("select") || t.starts_with("with")
}

pub fn has_limit_clause(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    // User-authored LIMIT disables auto-paging (word boundary-ish).
    lower.contains(" limit ")
        || lower.contains("\nlimit ")
        || lower.contains("\tlimit ")
        || lower.ends_with(" limit")
        || lower.contains(" limit\n")
        || lower.contains(" limit\t")
}

/// Next preset after `current` (wraps). `0` means unlimited / no auto LIMIT.
pub fn next_select_limit_preset(current: usize) -> usize {
    if let Some(i) = SELECT_LIMIT_PRESETS.iter().position(|&p| p == current) {
        return SELECT_LIMIT_PRESETS[(i + 1) % SELECT_LIMIT_PRESETS.len()];
    }
    // Snap unknown values onto the cycle starting at 50.
    50
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_page_size_is_50() {
        assert_eq!(RESULT_PAGE_SIZE, 50);
        assert_eq!(SELECT_LIMIT_PRESETS, &[50, 100, 200, 500, 0]);
    }

    #[test]
    fn auto_limit_on_bare_select() {
        let out = paginate_select_sql("SELECT * FROM brain.session_notes", 0, 50);
        assert_eq!(out, "SELECT * FROM brain.session_notes LIMIT 50 OFFSET 0");
    }

    #[test]
    fn page_two_uses_offset() {
        let out = paginate_select_sql("select 1", 2, 50);
        assert_eq!(out, "select 1 LIMIT 50 OFFSET 100");
    }

    #[test]
    fn explicit_limit_untouched() {
        let sql = "SELECT * FROM t LIMIT 10";
        assert_eq!(paginate_select_sql(sql, 0, 50), sql);
        assert!(has_limit_clause(sql));
    }

    #[test]
    fn page_size_zero_disables() {
        let sql = "SELECT * FROM t";
        assert_eq!(paginate_select_sql(sql, 0, 0), sql);
    }

    #[test]
    fn with_cte_is_paged() {
        let out = paginate_select_sql("WITH x AS (SELECT 1) SELECT * FROM x", 0, 50);
        assert!(out.ends_with("LIMIT 50 OFFSET 0"));
    }

    #[test]
    fn non_select_untouched() {
        let sql = "UPDATE t SET a = 1";
        assert_eq!(paginate_select_sql(sql, 0, 50), sql);
    }

    #[test]
    fn cycle_presets_wrap() {
        assert_eq!(next_select_limit_preset(50), 100);
        assert_eq!(next_select_limit_preset(100), 200);
        assert_eq!(next_select_limit_preset(200), 500);
        assert_eq!(next_select_limit_preset(500), 0);
        assert_eq!(next_select_limit_preset(0), 50);
        assert_eq!(next_select_limit_preset(77), 50);
    }
}
