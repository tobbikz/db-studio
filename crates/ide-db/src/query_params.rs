//! Detect `:name` / `@name` / `$n` placeholders and substitute SQL-literal values for one run.

use anyhow::{Context, Result, bail};
use std::collections::{BTreeSet, HashMap};

/// Escape a cell value as a SQL string literal (or NULL).
pub fn sql_string_literal(v: &str) -> String {
    if v.eq_ignore_ascii_case("NULL") {
        "NULL".into()
    } else {
        format!("'{}'", v.replace('\'', "''"))
    }
}

/// Return sorted unique placeholder keys: `:name`/`@name` without sigil, and `"1"`, `"2"` for `$n`.
pub fn detect_query_params(sql: &str) -> Vec<String> {
    let mut named = BTreeSet::new();
    let mut positional = BTreeSet::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '\'' && !in_double {
            if in_single && bytes.get(i + 1) == Some(&b'\'') {
                i += 2;
                continue;
            }
            in_single = !in_single;
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if in_single || in_double {
            i += 1;
            continue;
        }
        if (c == ':' || c == '@') && i + 1 < bytes.len() {
            let next = bytes[i + 1] as char;
            // Skip `::` (PG cast) and `@@` (MSSQL globals).
            if next == c {
                i += 2;
                continue;
            }
            if next.is_ascii_alphabetic() || next == '_' {
                let start = i + 1;
                i += 2;
                while i < bytes.len() {
                    let ch = bytes[i] as char;
                    if ch.is_ascii_alphanumeric() || ch == '_' {
                        i += 1;
                    } else {
                        break;
                    }
                }
                let name = &sql[start..i];
                named.insert(name.to_string());
                continue;
            }
        }
        if c == '$' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let n = &sql[start..i];
            positional.insert(n.parse::<u32>().unwrap_or(0));
            continue;
        }
        i += 1;
    }
    let mut out: Vec<String> = named.into_iter().collect();
    for n in positional {
        if n > 0 {
            out.push(n.to_string());
        }
    }
    out.sort_by(|a, b| {
        let a_pos = a.parse::<u32>().ok();
        let b_pos = b.parse::<u32>().ok();
        match (a_pos, b_pos) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.cmp(b),
        }
    });
    out
}

pub fn sql_has_query_params(sql: &str) -> bool {
    !detect_query_params(sql).is_empty()
}

/// Seed text for the query-params prompt (`name=,1=` style).
pub fn query_param_prompt_seed(sql: &str) -> String {
    let params = detect_query_params(sql);
    params
        .iter()
        .map(|p| format!("{p}="))
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_param_map(input: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for part in input.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, v)) = part.split_once('=') {
            let k = k.trim();
            if !k.is_empty() {
                map.insert(k.to_string(), v.trim().to_string());
            }
        }
    }
    map
}

/// Substitute placeholders using `name=foo,1=42` (values SQL-escaped; use NULL for null).
pub fn substitute_query_params(sql: &str, param_input: &str) -> Result<String> {
    let required = detect_query_params(sql);
    if required.is_empty() {
        return Ok(sql.to_string());
    }
    let values = parse_param_map(param_input);
    for key in &required {
        if !values.contains_key(key) {
            bail!("missing value for query param `{key}` — use `{key}=...` in the prompt");
        }
    }

    let mut out = sql.to_string();
    // Positional $n — highest first so $10 is not broken by $1.
    let mut pos: Vec<u32> = required
        .iter()
        .filter_map(|k| k.parse().ok())
        .collect();
    pos.sort_by(|a, b| b.cmp(a));
    for n in pos {
        let key = n.to_string();
        let lit = sql_string_literal(values.get(&key).context("param")?);
        let needle = format!("${n}");
        if !out.contains(&needle) {
            bail!("SQL no longer contains placeholder ${n}");
        }
        out = out.replace(&needle, &lit);
    }
    // Named :params / @params — longest names first to avoid prefix clashes.
    let mut names: Vec<String> = required
        .iter()
        .filter(|k| k.parse::<u32>().is_err())
        .cloned()
        .collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    for name in names {
        let lit = sql_string_literal(values.get(&name).context("param")?);
        let colon = format!(":{name}");
        let at = format!("@{name}");
        if out.contains(&colon) {
            out = out.replace(&colon, &lit);
        } else if out.contains(&at) {
            out = out.replace(&at, &lit);
        } else {
            bail!("SQL no longer contains placeholder :{name} or @{name}");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_mixed_params() {
        let sql = "SELECT * FROM t WHERE id = $1 AND name = :name AND x = :other AND y = @y;";
        let p = detect_query_params(sql);
        assert!(p.contains(&"1".to_string()));
        assert!(p.contains(&"name".to_string()));
        assert!(p.contains(&"other".to_string()));
        assert!(p.contains(&"y".to_string()));
    }

    #[test]
    fn substitutes_literals() {
        let sql = "SELECT * FROM u WHERE id = $1 AND slug = :slug AND n = @n;";
        let out = substitute_query_params(sql, "1=42,slug=O'Brien,n=hi").unwrap();
        assert!(out.contains("id = 42"));
        assert!(out.contains("slug = 'O''Brien'"));
        assert!(out.contains("n = 'hi'"));
        assert!(!out.contains('$'));
        assert!(!out.contains(":slug"));
        assert!(!out.contains("@n"));
    }
}
