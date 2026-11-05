use ide_db::{DataSource, DbDriver, DbWorkspace};

#[tokio::test]
async fn duckdb_memory_query_and_introspect() {
    let mut ws = DbWorkspace::default();
    let src = DataSource {
        id: "local-duck".into(),
        name: "duck".into(),
        driver: DbDriver::Duckdb,
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
    ws.add_source(src);
    ws.ensure_connected(&id, None).await.unwrap();
    ws.active_source = Some(id);
    ws.sql = "CREATE TABLE t (id INTEGER, name VARCHAR);".into();
    ws.execute_sql().await.unwrap();
    ws.sql = "INSERT INTO t VALUES (1, 'duck');".into();
    ws.execute_sql().await.unwrap();
    ws.sql = "SELECT id, name FROM t;".into();
    ws.execute_sql().await.unwrap();
    let res = ws.last_result.as_ref().unwrap();
    assert_eq!(res.columns, vec!["id", "name"]);
    assert_eq!(res.rows.len(), 1);
    assert_eq!(res.rows[0][1], "duck");
}
