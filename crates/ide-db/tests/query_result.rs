use ide_db::QueryResult;

#[test]
fn dirty_rows_and_pending_updates() {
    let mut q = QueryResult::empty();
    q.columns = vec!["id".into(), "name".into()];
    q.rows = vec![vec!["1".into(), "alice".into()]];
    q = q.with_meta(None, Some("users".into()), vec!["id".into()]);
    q.cursor_col = 1;
    q.set_cell("bob".into());
    assert_eq!(q.dirty_rows(), vec![0]);
    let stmts = q.pending_updates().expect("updates");
    assert_eq!(stmts.len(), 1);
    assert!(stmts[0].contains("UPDATE"));
    assert!(stmts[0].contains("name"));
}
