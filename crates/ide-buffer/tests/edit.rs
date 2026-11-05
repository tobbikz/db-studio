use ide_buffer::Buffer;

#[test]
fn insert_undo_save_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sample.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();

    let mut buf = Buffer::from_path(&path).unwrap();
    buf.cursor.row = 0;
    buf.cursor.col = 3; // before 'm' of main
    buf.insert_str("foo_");
    assert!(
        buf.text().contains("fn foo_main"),
        "got {:?}",
        buf.text()
    );
    buf.undo();
    assert_eq!(buf.text(), "fn main() {}\n");
    buf.cursor.row = 0;
    buf.cursor.col = 0;
    buf.insert_str("// edited\n");
    buf.save().unwrap();
    let disk = std::fs::read_to_string(&path).unwrap();
    assert!(disk.starts_with("// edited\n"));
    assert!(!buf.dirty);
}
