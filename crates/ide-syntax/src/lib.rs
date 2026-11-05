//! Tree-sitter syntax highlighting for the visible viewport.

use ide_core::LanguageId;
use parking_lot::Mutex;
use std::collections::HashMap;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    Type,
    Function,
    String,
    Comment,
    Number,
    Property,
    Variable,
    Constant,
    Operator,
    Punctuation,
    Default,
}

#[derive(Debug, Clone, Copy)]
pub struct HighlightSpan {
    /// Column within the line (char index).
    pub start_col: usize,
    pub end_col: usize,
    pub kind: HighlightKind,
}

struct LangEngine {
    parser: Parser,
    query: Query,
}

pub struct Highlighter {
    engines: Mutex<HashMap<LanguageId, LangEngine>>,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter {
    pub fn new() -> Self {
        Self {
            engines: Mutex::new(HashMap::new()),
        }
    }

    fn ensure_engine(map: &mut HashMap<LanguageId, LangEngine>, lang: LanguageId) -> Option<()> {
        if map.contains_key(&lang) {
            return Some(());
        }
        let (language, query_src) = match lang {
            LanguageId::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::HIGHLIGHTS_QUERY,
            ),
            LanguageId::Markdown => (
                tree_sitter_md::LANGUAGE.into(),
                r#"
                (atx_heading (inline) @keyword)
                (setext_heading) @keyword
                (link) @property
                (emphasis) @variable
                (strong_emphasis) @constant
                (code_span) @string
                (fenced_code_block) @string
                (indented_code_block) @string
                "#,
            ),
            LanguageId::Python => (
                tree_sitter_python::LANGUAGE.into(),
                tree_sitter_python::HIGHLIGHTS_QUERY,
            ),
            LanguageId::Go => (
                tree_sitter_go::LANGUAGE.into(),
                tree_sitter_go::HIGHLIGHTS_QUERY,
            ),
            LanguageId::JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                tree_sitter_javascript::HIGHLIGHT_QUERY,
            ),
            LanguageId::TypeScript => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ),
            LanguageId::Json => (
                tree_sitter_json::LANGUAGE.into(),
                tree_sitter_json::HIGHLIGHTS_QUERY,
            ),
            LanguageId::Shell => (
                tree_sitter_bash::LANGUAGE.into(),
                tree_sitter_bash::HIGHLIGHT_QUERY,
            ),
            _ => return None,
        };
        let mut parser = Parser::new();
        parser.set_language(&language).ok()?;
        let query = Query::new(&language, query_src).ok()?;
        map.insert(lang, LangEngine { parser, query });
        Some(())
    }

    /// Highlight lines `[start_row, end_row)` returning spans per line index (relative to start_row).
    pub fn highlight_lines(
        &self,
        language: LanguageId,
        source: &str,
        start_row: usize,
        end_row: usize,
    ) -> Vec<Vec<HighlightSpan>> {
        let line_count = end_row.saturating_sub(start_row);
        let mut result = vec![Vec::new(); line_count];

        // SQL: lightweight highlighter (no tree-sitter grammar wired yet).
        if language == LanguageId::Sql {
            return highlight_sql_lines(source, start_row, end_row);
        }

        let mut map = self.engines.lock();
        if Self::ensure_engine(&mut map, language).is_none() {
            return result;
        }
        let engine = map.get_mut(&language).unwrap();

        let tree = match engine.parser.parse(source, None) {
            Some(t) => t,
            None => return result,
        };

        // Build line start byte offsets
        let mut line_starts = Vec::new();
        line_starts.push(0usize);
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        if line_starts.last().copied() != Some(source.len()) && !source.is_empty() {
            // ok
        }

        let start_byte = line_byte_start(&line_starts, start_row, source.len());
        let end_byte = if end_row >= line_starts.len() {
            source.len()
        } else {
            line_byte_start(&line_starts, end_row, source.len())
        };

        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start_byte..end_byte);
        let mut matches = cursor.matches(&engine.query, tree.root_node(), source.as_bytes());

        while let Some(m) = matches.next() {
            for cap in m.captures {
                let name = engine.query.capture_names()[cap.index as usize];
                let kind = capture_kind(name);
                let node = cap.node;
                let s = node.start_byte();
                let e = node.end_byte();
                let start_pos = node.start_position();
                let end_pos = node.end_position();

                for row in start_pos.row..end_pos.row.saturating_add(1) {
                    if row < start_row || row >= end_row {
                        continue;
                    }
                    let rel = row - start_row;
                    let line_start = line_byte_start(&line_starts, row, source.len());
                    let line_end = if row + 1 < line_starts.len() {
                        line_starts[row + 1].saturating_sub(1)
                    } else {
                        source.len()
                    };
                    let seg_start = s.max(line_start);
                    let seg_end = e.min(line_end);
                    if seg_start >= seg_end {
                        continue;
                    }
                    let start_col = byte_to_col(source, line_start, seg_start);
                    let end_col = byte_to_col(source, line_start, seg_end);
                    if start_col < end_col {
                        result[rel].push(HighlightSpan {
                            start_col,
                            end_col,
                            kind,
                        });
                    }
                }
            }
        }

        for line in &mut result {
            line.sort_by_key(|s| s.start_col);
        }
        result
    }
}

fn line_byte_start(line_starts: &[usize], row: usize, len: usize) -> usize {
    line_starts.get(row).copied().unwrap_or(len)
}

fn byte_to_col(source: &str, line_start: usize, byte: usize) -> usize {
    source
        .get(line_start..byte)
        .map(|s| s.chars().count())
        .unwrap_or(0)
}

fn capture_kind(name: &str) -> HighlightKind {
    let base = name.split('.').next().unwrap_or(name);
    match base {
        "keyword" | "repeat" | "conditional" | "include" => HighlightKind::Keyword,
        "type" | "type.builtin" | "constructor" => HighlightKind::Type,
        "function" | "method" | "function.builtin" => HighlightKind::Function,
        "string" | "string.special" | "character" => HighlightKind::String,
        "comment" => HighlightKind::Comment,
        "number" | "float" => HighlightKind::Number,
        "property" | "attribute" | "tag" => HighlightKind::Property,
        "variable" | "parameter" | "field" => HighlightKind::Variable,
        "constant" | "boolean" | "constant.builtin" => HighlightKind::Constant,
        "operator" => HighlightKind::Operator,
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => {
            HighlightKind::Punctuation
        }
        _ => HighlightKind::Default,
    }
}

const SQL_KEYWORDS: &[&str] = &[
    "select", "from", "where", "and", "or", "not", "in", "is", "null", "as", "on",
    "join", "left", "right", "inner", "outer", "full", "cross", "group", "by",
    "order", "having", "limit", "offset", "insert", "into", "values", "update",
    "set", "delete", "create", "alter", "drop", "table", "view", "index",
    "primary", "key", "foreign", "references", "unique", "constraint", "default",
    "with", "recursive", "union", "all", "distinct", "case", "when", "then",
    "else", "end", "exists", "between", "like", "ilike", "returning", "asc",
    "desc", "true", "false", "explain", "analyze", "buffers", "format",
];

fn highlight_sql_lines(
    source: &str,
    start_row: usize,
    end_row: usize,
) -> Vec<Vec<HighlightSpan>> {
    let lines: Vec<&str> = source.lines().collect();
    let line_count = end_row.saturating_sub(start_row);
    let mut result = vec![Vec::new(); line_count];
    for row in start_row..end_row {
        let Some(line) = lines.get(row) else {
            break;
        };
        let rel = row - start_row;
        result[rel] = highlight_sql_line(line);
    }
    result
}

fn highlight_sql_line(line: &str) -> Vec<HighlightSpan> {
    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        // line comment
        if chars[i] == '-' && chars.get(i + 1) == Some(&'-') {
            spans.push(HighlightSpan {
                start_col: i,
                end_col: chars.len(),
                kind: HighlightKind::Comment,
            });
            break;
        }
        // string
        if chars[i] == '\'' || chars[i] == '"' {
            let quote = chars[i];
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(HighlightSpan {
                start_col: start,
                end_col: i,
                kind: HighlightKind::String,
            });
            continue;
        }
        // number
        if chars[i].is_ascii_digit() {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            spans.push(HighlightSpan {
                start_col: start,
                end_col: i,
                kind: HighlightKind::Number,
            });
            continue;
        }
        // ident / keyword
        if chars[i].is_ascii_alphabetic() || chars[i] == '_' {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '_')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let kind = if SQL_KEYWORDS
                .iter()
                .any(|k| k.eq_ignore_ascii_case(word.as_str()))
            {
                HighlightKind::Keyword
            } else {
                HighlightKind::Variable
            };
            spans.push(HighlightSpan {
                start_col: start,
                end_col: i,
                kind,
            });
            continue;
        }
        // operators / punctuation
        if "(),.;=*<>!+-/%".contains(chars[i]) {
            spans.push(HighlightSpan {
                start_col: i,
                end_col: i + 1,
                kind: HighlightKind::Operator,
            });
        }
        i += 1;
    }
    spans
}
