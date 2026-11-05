use ide_core::LanguageId;
use ide_syntax::Highlighter;

#[test]
fn rust_highlight_has_spans() {
    let src = "fn main() {\n    let x = 42;\n}\n";
    let hl = Highlighter::new();
    let lines = hl.highlight_lines(LanguageId::Rust, src, 0, 3);
    assert_eq!(lines.len(), 3);
    let total: usize = lines.iter().map(|l| l.len()).sum();
    assert!(total > 0, "expected highlight spans");
}

#[test]
fn markdown_highlight_runs() {
    let src = "# Title\n\nHello **world**\n";
    let hl = Highlighter::new();
    let lines = hl.highlight_lines(LanguageId::Markdown, src, 0, 3);
    assert_eq!(lines.len(), 3);
}
