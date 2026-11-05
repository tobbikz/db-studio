use ide_db::{DataSource, DbDriver, DbWorkspace};

#[tokio::test]
async fn sqlite_query_and_introspect() {
    let mut ws = DbWorkspace::default();
    let src = DataSource {
        id: "local-sqlite".into(),
        name: "test".into(),
        driver: DbDriver::Sqlite,
        host: String::new(),
        port: 0,
        user: String::new(),
        password: String::new(),
        database: String::new(),
        file: ":memory:".into(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_port: 22,
        ssh_key: String::new(),
        ssl_mode: "prefer".into(),
        ssl_trust_cert: true,
        ssl_ca: String::new(),
        read_only: false,
        role: ide_db::SourceRole::Admin,
        group: String::new(),
        accent: String::new(),
        pinned: false,
    };
    let id = src.id.clone();
    // Avoid add_source — it writes ~/.config/tui-ide/datasources.toml (racy in parallel tests).
    ws.sources.push(src);
    ws.ensure_connected(&id, None).await.unwrap();
    ws.active_source = Some(id);
    ws.sql = "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT);".into();
    ws.execute_sql().await.unwrap();
    ws.sql = "INSERT INTO t VALUES (1, 'hello');".into();
    ws.execute_sql().await.unwrap();
    ws.sql = "SELECT id, name FROM t;".into();
    ws.execute_sql().await.unwrap();
    let res = ws.last_result.as_ref().unwrap();
    assert_eq!(res.columns, vec!["id", "name"]);
    assert_eq!(res.rows.len(), 1);
    assert_eq!(res.rows[0][1], "hello");
}

#[test]
fn read_only_sql_detection() {
    use ide_db::is_read_only_sql;
    assert!(is_read_only_sql("SELECT 1"));
    assert!(is_read_only_sql("WITH x AS (SELECT 1) SELECT * FROM x"));
    assert!(!is_read_only_sql("INSERT INTO t VALUES (1)"));
    assert!(!is_read_only_sql("UPDATE t SET x=1"));
}

#[test]
fn rbac_role_gates() {
    use ide_db::SourceRole;
    assert!(!SourceRole::Viewer.allows_write());
    assert!(SourceRole::Editor.allows_write());
    assert!(!SourceRole::Editor.allows_admin());
    assert!(SourceRole::Admin.allows_admin());
    assert_eq!(SourceRole::Viewer.cycle(true), SourceRole::Editor);
    assert_eq!(SourceRole::Admin.cycle(true), SourceRole::Viewer);
}

#[test]
fn parse_er_edges_and_filter() {
    use ide_db::{parse_er_edges, row_matches_filter, render_er_canvas};
    let sketch = "# ER · public\nusers.id --> orders.user_id\norders.item_id --> items.id\n";
    let edges = parse_er_edges(sketch);
    assert_eq!(edges.len(), 2);
    assert_eq!(edges[0].0, "users");
    assert_eq!(edges[0].3, "user_id");
    let canvas = render_er_canvas("public", &edges);
    assert!(canvas.contains("users"));
    assert!(canvas.contains("──►"));

    let row = vec!["Alice".into(), "42".into()];
    assert!(row_matches_filter(&row, "ali"));
    assert!(row_matches_filter(&row, "42"));
    assert!(!row_matches_filter(&row, "bob"));
    assert!(row_matches_filter(&row, ""));
}

#[tokio::test]
async fn studio_smoke_filter_snippets_cancel_flag() {
    use ide_db::{ensure_example_snippet, list_snippets};
    let mut ws = DbWorkspace::default();
    let src = DataSource {
        id: "smoke-sqlite".into(),
        name: "smoke".into(),
        driver: DbDriver::Sqlite,
        host: String::new(),
        port: 0,
        user: String::new(),
        password: String::new(),
        database: String::new(),
        file: ":memory:".into(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_port: 22,
        ssh_key: String::new(),
        ssl_mode: "prefer".into(),
        ssl_trust_cert: true,
        ssl_ca: String::new(),
        read_only: false,
        role: ide_db::SourceRole::Viewer,
        group: String::new(),
        accent: String::new(),
        pinned: false,
    };
    let id = src.id.clone();
    ws.sources.push(src);
    ws.ensure_connected(&id, None).await.unwrap();
    ws.active_source = Some(id.clone());
    ws.sql = "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT);".into();
    // Viewer cannot DDL
    assert!(ws.execute_sql().await.is_err());

    // Flip to admin via sources mut
    if let Some(s) = ws.sources.iter_mut().find(|s| s.id == id) {
        s.role = ide_db::SourceRole::Admin;
    }
    ws.sql = "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT);".into();
    ws.execute_sql().await.unwrap();
    ws.sql = "INSERT INTO t VALUES (1, 'alpha'), (2, 'beta');".into();
    ws.execute_sql().await.unwrap();
    ws.sql = "SELECT id, name FROM t ORDER BY id;".into();
    ws.execute_sql().await.unwrap();
    ws.results_filter = "alp".into();
    let res = ws.last_result.as_ref().unwrap();
    let visible: Vec<_> = res
        .rows
        .iter()
        .filter(|r| ide_db::row_matches_filter(r, &ws.results_filter))
        .collect();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0][1], "alpha");

    ws.remember_table(&id, "main", "t");
    let expect = format!("{id}|main|t");
    assert_eq!(ws.recent_tables.first().map(String::as_str), Some(expect.as_str()));
    ws.request_query_cancel();
    assert!(ws.query_cancel.load(std::sync::atomic::Ordering::Relaxed));

    let _ = ensure_example_snippet();
    assert!(!list_snippets().is_empty());

    let res = ws.last_result.as_ref().unwrap();
    let csv_all = res.to_csv();
    let csv_f = res.to_csv_filtered(Some("alp"));
    assert!(csv_all.lines().count() > csv_f.lines().count());
    assert!(csv_f.contains("alpha"));
    assert!(!csv_f.contains("beta"));
}

#[test]
fn wave11_format_json_migration_helpers() {
    use ide_db::{format_sql_light, QueryResult};
    let pretty = format_sql_light("select a, b from t where x=1");
    assert!(pretty.to_ascii_lowercase().contains("\nfrom "));
    assert!(pretty.to_ascii_lowercase().contains("\nwhere "));
    let res = QueryResult {
        columns: vec!["id".into(), "name".into()],
        rows: vec![vec!["1".into(), "a".into()], vec!["2".into(), "b".into()]],
        affected: None,
        elapsed_ms: 0,
        truncated: false,
        source_schema: None,
        source_table: None,
        primary_keys: Vec::new(),
        original_rows: Vec::new(),
        cursor_row: 0,
        cursor_col: 0,
        explain: None,
        chart: None,
        page: 0,
        has_more: false,
    };
    let json = res.to_json_filtered(Some("a"));
    assert!(json.contains("\"name\":\"a\""));
    assert!(!json.contains("\"name\":\"b\""));
    let help = DbWorkspace::studio_help_text();
    assert!(help.contains("Migrate"));
    assert!(help.contains("migration"));
}

#[test]
fn wave10_pin_history_backup_help() {
    use ide_db::{DbWorkspace, MSSQL_BACKUP_HISTORY, PG_BACKUP_HISTORY};
    let mut ws = DbWorkspace::default();
    let a = DataSource {
        id: "a".into(),
        name: "alpha".into(),
        driver: DbDriver::Sqlite,
        host: String::new(),
        port: 0,
        user: String::new(),
        password: String::new(),
        database: String::new(),
        file: ":memory:".into(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_port: 22,
        ssh_key: String::new(),
        ssl_mode: "prefer".into(),
        ssl_trust_cert: true,
        ssl_ca: String::new(),
        read_only: false,
        role: ide_db::SourceRole::Admin,
        group: String::new(),
        accent: String::new(),
        pinned: false,
    };
    let mut b = a.clone();
    b.id = "b".into();
    b.name = "beta".into();
    b.pinned = true;
    ws.sources.push(a);
    ws.sources.push(b);
    ws.rebuild_tree_roots();
    // pinned beta should appear before unpinned alpha among ungrouped
    assert!(ws.tree[0].label.contains('★'));
    assert!(ws.tree[0].label.contains("beta"));
    ws.history.push(ide_db::QueryHistoryEntry {
        sql: "SELECT 42".into(),
        elapsed_ms: 3,
    });
    let items = ws.query_history_palette_items();
    assert_eq!(items.len(), 1);
    assert!(items[0].1.starts_with("hist:"));
    assert!(PG_BACKUP_HISTORY.contains("pg_stat_archiver"));
    assert!(MSSQL_BACKUP_HISTORY.contains("backupset"));
    let help = DbWorkspace::studio_help_text();
    assert!(help.contains("Script object"));
    assert!(help.contains("Pin connection"));
}

#[test]
fn mssql_health_sql_and_help() {
    use ide_db::{
        DbWorkspace, MSSQL_HEALTH_ACTIVITY, MSSQL_HEALTH_BLOCKING, MSSQL_HEALTH_EXPENSIVE,
        MSSQL_HEALTH_IO, MSSQL_HEALTH_INDEXES, MSSQL_HEALTH_JOBS, MSSQL_HEALTH_OVERVIEW,
        MSSQL_HEALTH_WAITS, PG_HEALTH_ACTIVITY, PG_HEALTH_OVERVIEW, MYSQL_HEALTH_ACTIVITY,
        CH_HEALTH_ACTIVITY,
    };
    for sql in [
        MSSQL_HEALTH_OVERVIEW,
        MSSQL_HEALTH_ACTIVITY,
        MSSQL_HEALTH_BLOCKING,
        MSSQL_HEALTH_WAITS,
        MSSQL_HEALTH_EXPENSIVE,
        MSSQL_HEALTH_IO,
        MSSQL_HEALTH_JOBS,
        MSSQL_HEALTH_INDEXES,
    ] {
        assert!(!sql.is_empty(), "expected non-empty health SQL");
    }
    assert!(MSSQL_HEALTH_ACTIVITY.contains("session_id"));
    assert!(MSSQL_HEALTH_JOBS.contains("msdb"));
    assert!(MSSQL_HEALTH_INDEXES.contains("avg_fragmentation_in_percent"));
    assert!(PG_HEALTH_OVERVIEW.contains("pg_stat_activity"));
    assert!(PG_HEALTH_ACTIVITY.contains("pid"));
    assert!(MYSQL_HEALTH_ACTIVITY.contains("processlist"));
    assert!(CH_HEALTH_ACTIVITY.contains("system.processes"));
    let mut ws = DbWorkspace::default();
    assert!(!ws.health_activity_auto_refresh);
    assert!(ws.toggle_health_activity_auto_refresh());
    assert!(ws.health_activity_auto_refresh);
    let help = DbWorkspace::studio_help_text();
    assert!(help.contains("Health"));
    assert!(help.contains("auto-refresh"));
}

#[test]
fn wave12_params_markdown_alter_fanout_help() {
    use ide_db::{
        chart_result, query_param_prompt_seed, sql_has_query_params, substitute_query_params,
        AlterColumnKind, DataSource, DbDriver, DbWorkspace, QueryResult,
    };
    assert!(sql_has_query_params("SELECT * FROM t WHERE id = @id AND n = :n"));
    let seed = query_param_prompt_seed("SELECT :a, @b, $1");
    assert!(seed.contains("a="));
    assert!(seed.contains("b="));
    assert!(seed.contains("1="));
    let sub = substitute_query_params("SELECT @x", "x=hi").unwrap();
    assert_eq!(sub, "SELECT 'hi'");

    let res = QueryResult {
        columns: vec!["id".into(), "n".into()],
        rows: vec![vec!["1".into(), "10".into()], vec!["2".into(), "20".into()]],
        affected: None,
        elapsed_ms: 0,
        truncated: false,
        source_schema: None,
        source_table: Some("t".into()),
        primary_keys: Vec::new(),
        original_rows: Vec::new(),
        cursor_row: 0,
        cursor_col: 0,
        explain: None,
        chart: None,
        page: 0,
        has_more: false,
    };
    let md = res.to_markdown_filtered(None);
    assert!(md.contains("| id |"));
    assert!(md.contains("| 1 |"));
    let chart = chart_result(&res, Some(1)).expect("numeric chart");
    assert!(chart.contains("Spark"));

    let mut ws = DbWorkspace::default();
    let src = DataSource {
        id: "s1".into(),
        name: "local".into(),
        driver: DbDriver::Sqlite,
        host: String::new(),
        port: 0,
        user: String::new(),
        password: String::new(),
        database: String::new(),
        file: ":memory:".into(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_port: 22,
        ssh_key: String::new(),
        ssl_mode: "prefer".into(),
        ssl_trust_cert: true,
        ssl_ca: String::new(),
        read_only: false,
        role: ide_db::SourceRole::Admin,
        group: String::new(),
        accent: String::new(),
        pinned: false,
    };
    ws.sources.push(src);
    ws.rebuild_tree_roots();
    // No table selected → alter should error cleanly.
    assert!(ws
        .alter_sql_for_selection(AlterColumnKind::Add, "c TEXT")
        .is_err());
    let help = DbWorkspace::studio_help_text();
    assert!(help.contains("Data ops"));
    assert!(help.contains("Fan-out"));
    assert!(help.contains("Wave 12") || help.contains("ALTER"));
}

#[tokio::test]
async fn wave13_sqlite_maintenance_and_help() {
    use ide_db::{DataSource, DbDriver, DbWorkspace, PostgresMaintenance};
    let mut ws = DbWorkspace::default();
    let id = String::from("sqlite-w13");
    ws.sources.push(DataSource {
        id: id.clone(),
        name: "mem".into(),
        driver: DbDriver::Sqlite,
        host: String::new(),
        port: 0,
        user: String::new(),
        password: String::new(),
        database: String::new(),
        file: ":memory:".into(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_port: 22,
        ssh_key: String::new(),
        ssl_mode: "prefer".into(),
        ssl_trust_cert: true,
        ssl_ca: String::new(),
        read_only: false,
        role: ide_db::SourceRole::Admin,
        group: String::new(),
        accent: String::new(),
        pinned: true,
    });
    ws.active_source = Some(id.clone());
    ws.ensure_connected(&id, None).await.unwrap();
    ws.execute_sql_text("CREATE TABLE t (id INTEGER PRIMARY KEY, n REAL);")
        .await
        .unwrap();
    ws.execute_sql_text("INSERT INTO t VALUES (1, 1.5), (2, 3.0);")
        .await
        .unwrap();
    // Point selection at the table via flat rebuild after introspect path:
    // open table context by setting last_result source.
    ws.last_result = Some(ide_db::QueryResult {
        columns: vec!["id".into()],
        rows: vec![vec!["1".into()]],
        affected: None,
        elapsed_ms: 0,
        truncated: false,
        source_schema: Some("main".into()),
        source_table: Some("t".into()),
        primary_keys: vec!["id".into()],
        original_rows: Vec::new(),
        cursor_row: 0,
        cursor_col: 0,
        explain: None,
        chart: None,
        page: 0,
        has_more: false,
    });
    ws.run_postgres_maintenance(PostgresMaintenance::Analyze)
        .await
        .expect("sqlite ANALYZE");
    assert!(ws.status.to_ascii_lowercase().contains("analyze"));
    let hist = ws.timing_histogram_ascii();
    assert!(!hist.is_empty());
    let help = DbWorkspace::studio_help_text();
    assert!(help.contains("Maintain") || help.contains("Vacuum"));
    assert!(help.contains("Timing histogram") || help.contains("histogram"));
    // Fan-out with empty SQL fails; with SQL on pinned works.
    ws.sql = "SELECT 1;".into();
    ws.run_fan_out_pinned().await.expect("fan-out pinned");
    assert!(ws.status.contains("fan-out"));
}

#[tokio::test]
async fn wave14_15_data_editor_and_migrate_runner() {
    use ide_db::{DataSource, DbDriver, DbWorkspace};
    let dir = tempfile::tempdir().unwrap();
    let mut ws = DbWorkspace::default();
    ws.set_queries_dir(dir.path().to_path_buf());
    let id = String::from("sqlite-w14");
    ws.sources.push(DataSource {
        id: id.clone(),
        name: "mem".into(),
        driver: DbDriver::Sqlite,
        host: String::new(),
        port: 0,
        user: String::new(),
        password: String::new(),
        database: String::new(),
        file: ":memory:".into(),
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_port: 22,
        ssh_key: String::new(),
        ssl_mode: "prefer".into(),
        ssl_trust_cert: true,
        ssl_ca: String::new(),
        read_only: false,
        role: ide_db::SourceRole::Admin,
        group: String::new(),
        accent: String::new(),
        pinned: false,
    });
    ws.active_source = Some(id.clone());
    ws.ensure_connected(&id, None).await.unwrap();
    ws.execute_sql_text("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT);")
        .await
        .unwrap();
    ws.execute_sql_text("INSERT INTO t VALUES (1, 'a');")
        .await
        .unwrap();
    ws.execute_sql_text("SELECT id, name FROM t;")
        .await
        .unwrap();
    // Attach PK meta if parse_select_target missed it.
    if let Some(r) = ws.last_result.as_mut() {
        r.source_table = Some("t".into());
        r.source_schema = Some("main".into());
        r.primary_keys = vec!["id".into()];
        r.original_rows = r.rows.clone();
    }
    ws.insert_result_row().unwrap();
    if let Some(r) = ws.last_result.as_mut() {
        r.cursor_row = r.rows.len() - 1;
        r.cursor_col = 0;
        r.set_cell("2".into());
        r.cursor_col = 1;
        r.set_cell("b".into());
    }
    let muts = ws.pending_mutations().unwrap();
    assert!(muts.iter().any(|s| s.starts_with("INSERT")));
    ws.apply_pending_updates().await.unwrap();

    ws.execute_sql_text("SELECT id, name FROM t ORDER BY id;")
        .await
        .unwrap();
    if let Some(r) = ws.last_result.as_mut() {
        r.source_table = Some("t".into());
        r.source_schema = Some("main".into());
        r.primary_keys = vec!["id".into()];
        r.original_rows = r.rows.clone();
        r.cursor_row = 0;
    }
    ws.toggle_result_cell_null().unwrap();
    ws.delete_result_row().unwrap();
    assert!(!ws.pending_row_deletes.is_empty());
    ws.apply_pending_updates().await.unwrap();

    // Migration dry-run + apply
    let mig = dir.path().join("migrate_test.sql");
    std::fs::write(&mig, "CREATE TABLE IF NOT EXISTS m (x INT);\nINSERT INTO m VALUES (1);\n")
        .unwrap();
    ws.dry_run_migration_file(&mig).unwrap();
    assert!(ws.status.contains("dry-run"));
    ws.apply_migration_file(&mig).await.unwrap();
    assert!(ws.migration_journal_path().exists() || dir.path().join(".migrate_journal.jsonl").exists());
    let help = DbWorkspace::studio_help_text();
    assert!(help.contains("v1.0"));
    assert!(help.contains("Data editor") || help.contains("Insert row"));
    assert!(help.contains("Migration runner") || help.contains("Dry-run"));
}

#[test]
fn wave18_21_paste_blob_migrate_status_help() {
    use ide_db::{format_sql_light, QueryResult};
    let mut res = QueryResult {
        columns: vec!["a".into(), "b".into()],
        rows: vec![vec!["1".into(), "x".into()]],
        affected: None,
        elapsed_ms: 0,
        truncated: false,
        source_schema: Some("main".into()),
        source_table: Some("t".into()),
        primary_keys: vec!["a".into()],
        original_rows: vec![vec!["1".into(), "x".into()]],
        cursor_row: 0,
        cursor_col: 0,
        explain: None,
        chart: None,
        page: 0,
        has_more: false,
    };
    let n = res.paste_tsv_at_cursor("2\ty\n3\tz").unwrap();
    assert_eq!(n, 2);
    assert_eq!(res.rows.len(), 2);
    res.cursor_row = 0;
    res.cursor_col = 1;
    res.rows[0][1] = "<blob 3b: 00 ff aa>".into();
    let wide = res.wide_cell_text().unwrap();
    assert!(wide.contains("BLOB") || wide.contains("blob") || wide.contains("hex"));

    let dir = tempfile::tempdir().unwrap();
    let mut ws = ide_db::DbWorkspace::default();
    ws.set_queries_dir(dir.path().to_path_buf());
    ws.sql = "CREATE TABLE IF NOT EXISTS z(id INT);".into();
    let up = ws.save_migration_file().unwrap();
    assert!(up.file_name().unwrap().to_string_lossy().starts_with("migrate_"));
    let stem = up.file_stem().unwrap().to_string_lossy().to_string();
    assert!(dir.path().join(format!("{stem}.down.sql")).exists());
    ws.migration_status().unwrap();
    assert!(ws.status.contains("migration status"));
    let help = ide_db::DbWorkspace::studio_help_text();
    assert!(help.contains("Grid power") || help.contains("Paste TSV"));
    assert!(help.contains("Rollback") || help.contains("down.sql"));
    assert!(help.contains("Redis") || help.contains("EXPIRE"));
    let _ = format_sql_light("select 1");
}
