//! Multi-engine Create DDL + capability matrix for `n New`.

use crate::create_object_form::{
    ColSub, ColumnDraft, CreateObjectForm, CreateObjectKind, GrantPreset,
};
use ide_db::DbDriver;

/// Which Create kinds the active driver exposes in the `n` menu.
pub fn supports_kind(driver: DbDriver, kind: CreateObjectKind) -> bool {
    match driver {
        DbDriver::Postgres => true, // all CreateObjectKind variants
        DbDriver::Mysql => matches!(
            kind,
            CreateObjectKind::Database
                | CreateObjectKind::Schema
                | CreateObjectKind::Table
                | CreateObjectKind::View
                | CreateObjectKind::Index
                | CreateObjectKind::Role
                | CreateObjectKind::Function
        ),
        DbDriver::Mssql => matches!(
            kind,
            CreateObjectKind::Database
                | CreateObjectKind::Schema
                | CreateObjectKind::Table
                | CreateObjectKind::View
                | CreateObjectKind::Index
                | CreateObjectKind::Sequence
                | CreateObjectKind::Role
                | CreateObjectKind::Function
        ),
        DbDriver::Sqlite => matches!(
            kind,
            CreateObjectKind::Table | CreateObjectKind::View | CreateObjectKind::Index
        ),
        DbDriver::Duckdb => matches!(
            kind,
            CreateObjectKind::Schema
                | CreateObjectKind::Table
                | CreateObjectKind::View
                | CreateObjectKind::Index
                | CreateObjectKind::Sequence
                | CreateObjectKind::Function
        ),
        DbDriver::Clickhouse => matches!(
            kind,
            CreateObjectKind::Database
                | CreateObjectKind::Schema
                | CreateObjectKind::Table
                | CreateObjectKind::View
                | CreateObjectKind::Function
                | CreateObjectKind::Role
        ),
        DbDriver::Snowflake | DbDriver::Bigquery | DbDriver::Trino => matches!(
            kind,
            CreateObjectKind::Schema
                | CreateObjectKind::Table
                | CreateObjectKind::View
                | CreateObjectKind::Database
                | CreateObjectKind::Function
        ),
        // Document / KV / search: no relational Create wizards.
        DbDriver::Redis
        | DbDriver::Mongo
        | DbDriver::Elasticsearch => false,
    }
}

pub fn create_menu_ids(driver: DbDriver) -> Vec<(&'static str, &'static str)> {
    let all = [
        ("Schema", "create_schema"),
        ("Table", "create_table"),
        ("View", "create_view"),
        ("Materialized view", "create_matview"),
        ("Index", "create_index"),
        ("Sequence", "create_sequence"),
        ("Extension", "create_extension"),
        ("Role / User", "create_role"),
        ("RLS Policy", "create_policy"),
        ("Function", "create_function"),
        ("Publication", "create_publication"),
        ("Tablespace", "create_tablespace"),
        ("Database", "create_database"),
    ];
    all.into_iter()
        .filter(|(_, id)| {
            CreateObjectKind::from_id(id)
                .map(|k| supports_kind(driver, k))
                .unwrap_or(false)
        })
        .collect()
}

pub fn default_schema(driver: DbDriver) -> &'static str {
    match driver {
        DbDriver::Mysql | DbDriver::Clickhouse => "",
        DbDriver::Sqlite => "main",
        DbDriver::Mssql => "dbo",
        DbDriver::Bigquery => "",
        _ => "public",
    }
}

pub fn column_types(driver: DbDriver) -> &'static [&'static str] {
    match driver {
        DbDriver::Mysql => &[
            "BIGINT",
            "INT",
            "SMALLINT",
            "TINYINT",
            "VARCHAR(255)",
            "TEXT",
            "BOOLEAN",
            "DATETIME",
            "TIMESTAMP",
            "DATE",
            "DECIMAL(18,2)",
            "JSON",
            "BLOB",
        ],
        DbDriver::Mssql => &[
            "bigint",
            "int",
            "smallint",
            "nvarchar(255)",
            "nvarchar(max)",
            "bit",
            "datetime2",
            "date",
            "uniqueidentifier",
            "decimal(18,2)",
            "varbinary(max)",
        ],
        DbDriver::Sqlite | DbDriver::Duckdb => &[
            "INTEGER",
            "BIGINT",
            "TEXT",
            "REAL",
            "DOUBLE",
            "BLOB",
            "BOOLEAN",
            "TIMESTAMP",
            "DATE",
            "UUID",
            "JSON",
        ],
        DbDriver::Clickhouse => &[
            "UInt64",
            "Int64",
            "UInt32",
            "String",
            "FixedString(64)",
            "Bool",
            "DateTime",
            "Date",
            "UUID",
            "Float64",
            "Decimal(18, 2)",
            "JSON",
        ],
        DbDriver::Snowflake => &[
            "NUMBER",
            "INTEGER",
            "VARCHAR",
            "TEXT",
            "BOOLEAN",
            "TIMESTAMP_NTZ",
            "DATE",
            "VARIANT",
            "BINARY",
        ],
        DbDriver::Bigquery => &[
            "INT64",
            "STRING",
            "BOOL",
            "TIMESTAMP",
            "DATE",
            "FLOAT64",
            "NUMERIC",
            "BYTES",
            "JSON",
        ],
        DbDriver::Trino => &[
            "bigint",
            "integer",
            "varchar",
            "boolean",
            "timestamp",
            "date",
            "double",
            "decimal(18,2)",
            "varbinary",
            "json",
        ],
        // Postgres + fallback
        _ => &[
            "bigint",
            "integer",
            "smallint",
            "text",
            "varchar(255)",
            "boolean",
            "timestamptz",
            "timestamp",
            "date",
            "uuid",
            "numeric",
            "jsonb",
            "bytea",
        ],
    }
}

pub fn cycle_column_type(driver: DbDriver, typ: &mut String, forward: bool) {
    let types = column_types(driver);
    let idx = types
        .iter()
        .position(|t| t.eq_ignore_ascii_case(typ.as_str()))
        .unwrap_or(0);
    let next = if forward {
        (idx + 1) % types.len()
    } else {
        (idx + types.len() - 1) % types.len()
    };
    *typ = types[next].to_string();
}

pub fn seed_table_columns(driver: DbDriver) -> Vec<ColumnDraft> {
    let types = column_types(driver);
    let id_ty = types[0];
    let name_ty = types
        .iter()
        .find(|t| {
            let u = t.to_ascii_lowercase();
            u.contains("text") || u.contains("varchar") || u.contains("string") || u == "nvarchar(255)"
        })
        .copied()
        .unwrap_or(types.get(1).copied().unwrap_or(id_ty));
    let mut id = ColumnDraft::new("id", id_ty);
    id.primary_key = true;
    id.identity = true;
    id.nullable = false;
    let mut name = ColumnDraft::new("name", name_ty);
    name.nullable = false;
    vec![id, name]
}

fn q(driver: DbDriver, id: &str) -> String {
    match driver {
        DbDriver::Mysql | DbDriver::Clickhouse => format!("`{}`", id.replace('`', "``")),
        DbDriver::Mssql => format!("[{}]", id.replace(']', "]]")),
        DbDriver::Bigquery => format!("`{}`", id.replace('`', "``")),
        _ => {
            // Postgres / SQLite / DuckDB / Snowflake / Trino — double-quote if needed
            if id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && id
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_lowercase() || c == '_')
                    .unwrap_or(false)
            {
                id.to_string()
            } else {
                format!("\"{}\"", id.replace('"', "\"\""))
            }
        }
    }
}

fn qual(driver: DbDriver, schema: &str, name: &str) -> String {
    let schema = schema.trim();
    let name_q = q(driver, name);
    if schema.is_empty() {
        return name_q;
    }
    format!("{}.{}", q(driver, schema), name_q)
}

fn ident_ok(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn ext_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn parse_conn_limit(raw: &str) -> Result<Option<i32>, String> {
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    let n: i32 = t
        .parse()
        .map_err(|_| "connection limit: integer (−1 unlimited)".to_string())?;
    if n < -1 {
        return Err("connection limit: ≥ −1".into());
    }
    Ok(Some(n))
}

fn col_fragment(driver: DbDriver, c: &ColumnDraft) -> Result<String, String> {
    let n = c.name.trim();
    if n.is_empty() {
        return Err("column name required".into());
    }
    if !ident_ok(n) {
        return Err(format!("bad column name: {n}"));
    }
    let t = c.typ.trim();
    if t.is_empty() {
        return Err("column type required".into());
    }
    let nq = q(driver, n);
    match driver {
        DbDriver::Postgres | DbDriver::Snowflake | DbDriver::Trino => {
            let mut s = format!("{nq} {t}");
            if c.identity {
                s.push_str(match driver {
                    DbDriver::Snowflake => " IDENTITY",
                    _ => " GENERATED BY DEFAULT AS IDENTITY",
                });
            }
            if c.primary_key {
                s.push_str(" PRIMARY KEY");
            } else if !c.nullable {
                s.push_str(" NOT NULL");
            }
            Ok(s)
        }
        DbDriver::Mysql => {
            let mut s = format!("{nq} {t}");
            if c.identity {
                s.push_str(" AUTO_INCREMENT");
            }
            if !c.nullable {
                s.push_str(" NOT NULL");
            }
            if c.primary_key {
                s.push_str(" PRIMARY KEY");
            }
            Ok(s)
        }
        DbDriver::Mssql => {
            let mut s = format!("{nq} {t}");
            if c.identity {
                s.push_str(" IDENTITY(1,1)");
            }
            if !c.nullable {
                s.push_str(" NOT NULL");
            }
            if c.primary_key {
                s.push_str(" PRIMARY KEY");
            }
            Ok(s)
        }
        DbDriver::Sqlite | DbDriver::Duckdb => {
            if c.identity && c.primary_key && driver == DbDriver::Sqlite {
                return Ok(format!("{nq} INTEGER PRIMARY KEY AUTOINCREMENT"));
            }
            let mut s = format!("{nq} {t}");
            if c.identity && driver == DbDriver::Duckdb {
                s.push_str(" GENERATED BY DEFAULT AS IDENTITY");
            }
            if c.primary_key {
                s.push_str(" PRIMARY KEY");
            } else if !c.nullable {
                s.push_str(" NOT NULL");
            }
            Ok(s)
        }
        DbDriver::Clickhouse => {
            // Nullable via Nullable(T); PK is ORDER BY, not column constraint.
            let ty = if c.nullable && !t.to_ascii_lowercase().starts_with("nullable(") {
                format!("Nullable({t})")
            } else {
                t.to_string()
            };
            Ok(format!("{nq} {ty}"))
        }
        DbDriver::Bigquery => {
            let mode = if c.nullable { "NULLABLE" } else { "REQUIRED" };
            let _ = mode; // standard SQL CREATE TABLE uses NOT NULL
            let mut s = format!("{nq} {t}");
            if !c.nullable {
                s.push_str(" NOT NULL");
            }
            Ok(s)
        }
        _ => {
            let mut s = format!("{nq} {t}");
            if !c.nullable {
                s.push_str(" NOT NULL");
            }
            if c.primary_key {
                s.push_str(" PRIMARY KEY");
            }
            Ok(s)
        }
    }
}

/// Emit engine-correct DDL for the create form.
pub fn emit(form: &CreateObjectForm) -> Result<String, String> {
    let driver = form.driver;
    if !supports_kind(driver, form.kind) {
        return Err(format!(
            "{} not supported on {}",
            form.kind.title(),
            driver.label()
        ));
    }
    let name = form.name.trim();
    if name.is_empty() {
        return Err("name is required".into());
    }
    if form.kind != CreateObjectKind::Extension && !ident_ok(name) {
        return Err("name: letters, digits, underscore only".into());
    }
    if form.kind == CreateObjectKind::Extension && !ext_name_ok(name) {
        return Err("extension name invalid".into());
    }
    let schema = form.schema.trim();
    let owner = form.owner.trim();
    if !owner.is_empty() && !ident_ok(owner) {
        return Err("owner: letters, digits, underscore only".into());
    }
    let grantee = form.grant_role.trim();
    if !grantee.is_empty() && !ident_ok(grantee) {
        return Err("grant to: letters, digits, underscore only".into());
    }
    if form.grant_preset != GrantPreset::None && grantee.is_empty() {
        return Err("grant to: role required when grants ≠ none".into());
    }

    match form.kind {
        CreateObjectKind::Database => emit_database(driver, form, name, owner),
        CreateObjectKind::Schema => emit_schema(driver, form, name, owner, grantee),
        CreateObjectKind::Table => emit_table(driver, form, name, schema, owner, grantee),
        CreateObjectKind::View => emit_view(driver, form, name, schema, owner, grantee, false),
        CreateObjectKind::MaterializedView => {
            emit_view(driver, form, name, schema, owner, grantee, true)
        }
        CreateObjectKind::Index => emit_index(driver, form, name, schema),
        CreateObjectKind::Sequence => emit_sequence(driver, form, name, schema, owner),
        CreateObjectKind::Extension => emit_extension(driver, name),
        CreateObjectKind::Role => emit_role(driver, form, name),
        CreateObjectKind::Policy => emit_policy(driver, form, name),
        CreateObjectKind::Function => emit_function(driver, form, name, schema),
        CreateObjectKind::Publication => emit_publication(driver, form, name),
        CreateObjectKind::Tablespace => emit_tablespace(driver, form, name),
    }
}

fn emit_policy(driver: DbDriver, form: &CreateObjectForm, name: &str) -> Result<String, String> {
    if driver != DbDriver::Postgres {
        return Err("RLS policies are PostgreSQL-only in Create".into());
    }
    let on = form.schema.trim(); // schema.table
    let (sch, tbl) = on
        .split_once('.')
        .ok_or_else(|| "On table: use schema.table".to_string())?;
    if !ident_ok(sch) || !ident_ok(tbl) {
        return Err("On table: bad schema.table".into());
    }
    let cmd = if form.password.trim().is_empty() {
        "ALL"
    } else {
        form.password.trim()
    };
    let using = form.body.trim();
    if using.is_empty() {
        return Err("USING expression required (e.g. true or tenant_id = current_setting('app.tenant')::uuid)".into());
    }
    Ok(format!(
        "ALTER TABLE {sch}.{tbl} ENABLE ROW LEVEL SECURITY;\n\
         CREATE POLICY {name} ON {sch}.{tbl}\n\
           FOR {cmd}\n\
           USING ({using});"
    ))
}

fn emit_function(
    driver: DbDriver,
    form: &CreateObjectForm,
    name: &str,
    schema: &str,
) -> Result<String, String> {
    let body = form.body.trim().trim_end_matches(';');
    if body.is_empty() {
        return Err("function body required".into());
    }
    match driver {
        DbDriver::Postgres | DbDriver::Duckdb => {
            let sch = if schema.is_empty() { "public" } else { schema };
            if !ident_ok(sch) {
                return Err("schema invalid".into());
            }
            // body may be full "RETURNS … AS $$ … $$" or just expression
            if body.to_ascii_uppercase().contains("RETURNS")
                || body.to_ascii_uppercase().contains("LANGUAGE")
            {
                Ok(format!("CREATE OR REPLACE FUNCTION {sch}.{name}()\n{body};"))
            } else {
                Ok(format!(
                    "CREATE OR REPLACE FUNCTION {sch}.{name}()\nRETURNS integer\nLANGUAGE sql\nAS $$\n{body}\n$$;"
                ))
            }
        }
        DbDriver::Mysql => Ok(format!(
            "DELIMITER //\nCREATE FUNCTION {}() RETURNS INT DETERMINISTIC\nBEGIN\n  {body};\nEND //\nDELIMITER ;",
            q(driver, name)
        )),
        DbDriver::Mssql => Ok(format!(
            "CREATE OR ALTER FUNCTION {}()\nRETURNS int\nAS\nBEGIN\n  RETURN 1;\nEND;",
            if schema.is_empty() {
                q(driver, name)
            } else {
                qual(driver, schema, name)
            }
        )),
        DbDriver::Clickhouse | DbDriver::Snowflake | DbDriver::Bigquery | DbDriver::Trino => {
            Ok(format!(
                "-- Adjust signature for {}\nCREATE FUNCTION {} AS {};",
                driver.label(),
                q(driver, name),
                body
            ))
        }
        _ => Err(format!(
            "CREATE FUNCTION not supported on {}",
            driver.label()
        )),
    }
}

fn emit_publication(
    driver: DbDriver,
    form: &CreateObjectForm,
    name: &str,
) -> Result<String, String> {
    if driver != DbDriver::Postgres {
        return Err("publications are PostgreSQL-only".into());
    }
    let tables = form.body.trim();
    if tables.is_empty() {
        return Err("publication tables required (e.g. TABLE public.t1, TABLE public.t2)".into());
    }
    Ok(format!("CREATE PUBLICATION {name} FOR {tables};"))
}

fn emit_tablespace(
    driver: DbDriver,
    form: &CreateObjectForm,
    name: &str,
) -> Result<String, String> {
    if driver != DbDriver::Postgres {
        return Err("tablespaces are PostgreSQL-only in Create".into());
    }
    let loc = form.body.trim();
    if loc.is_empty() {
        return Err("tablespace location path required".into());
    }
    let esc = loc.replace('\'', "''");
    Ok(format!("CREATE TABLESPACE {name} LOCATION '{esc}';"))
}

fn emit_database(
    driver: DbDriver,
    form: &CreateObjectForm,
    name: &str,
    owner: &str,
) -> Result<String, String> {
    match driver {
        DbDriver::Postgres => {
            let mut sql = format!("CREATE DATABASE {}", q(driver, name));
            if !owner.is_empty() {
                sql.push_str(&format!(" OWNER {}", q(driver, owner)));
            }
            let tmpl = form.template.trim();
            if !tmpl.is_empty() {
                if !ident_ok(tmpl) {
                    return Err("template: invalid".into());
                }
                sql.push_str(&format!(" TEMPLATE {}", q(driver, tmpl)));
            }
            let enc = form.encoding.trim();
            if !enc.is_empty() {
                sql.push_str(&format!(" ENCODING '{}'", enc.replace('\'', "''")));
            }
            if let Some(lim) = parse_conn_limit(&form.conn_limit)? {
                sql.push_str(&format!(" CONNECTION LIMIT {lim}"));
            }
            sql.push(';');
            Ok(sql)
        }
        DbDriver::Mysql | DbDriver::Clickhouse => {
            let mut sql = format!("CREATE DATABASE IF NOT EXISTS {}", q(driver, name));
            let enc = form.encoding.trim();
            if !enc.is_empty() && driver == DbDriver::Mysql {
                sql.push_str(&format!(" CHARACTER SET {}", enc.replace('\'', "")));
            }
            sql.push(';');
            Ok(sql)
        }
        DbDriver::Mssql => Ok(format!("CREATE DATABASE {};", q(driver, name))),
        DbDriver::Snowflake | DbDriver::Bigquery | DbDriver::Trino => {
            Ok(format!("CREATE DATABASE IF NOT EXISTS {};", q(driver, name)))
        }
        _ => Err(format!("CREATE DATABASE not supported on {}", driver.label())),
    }
}

fn emit_schema(
    driver: DbDriver,
    form: &CreateObjectForm,
    name: &str,
    owner: &str,
    grantee: &str,
) -> Result<String, String> {
    match driver {
        DbDriver::Postgres => {
            let mut sql = if owner.is_empty() {
                format!("CREATE SCHEMA {};", q(driver, name))
            } else {
                format!(
                    "CREATE SCHEMA {} AUTHORIZATION {};",
                    q(driver, name),
                    q(driver, owner)
                )
            };
            sql.push_str(&pg_schema_grants(name, form.grant_preset, grantee)?);
            Ok(sql)
        }
        DbDriver::Mysql => {
            // MySQL "schema" == database
            Ok(format!("CREATE DATABASE IF NOT EXISTS {};", q(driver, name)))
        }
        DbDriver::Mssql | DbDriver::Duckdb | DbDriver::Snowflake | DbDriver::Trino => {
            Ok(format!("CREATE SCHEMA {};", q(driver, name)))
        }
        DbDriver::Clickhouse => Ok(format!(
            "CREATE DATABASE IF NOT EXISTS {};",
            q(driver, name)
        )),
        DbDriver::Bigquery => Ok(format!(
            "CREATE SCHEMA IF NOT EXISTS {};",
            q(driver, name)
        )),
        _ => Err(format!("CREATE SCHEMA not supported on {}", driver.label())),
    }
}

fn emit_table(
    driver: DbDriver,
    form: &CreateObjectForm,
    name: &str,
    schema: &str,
    owner: &str,
    grantee: &str,
) -> Result<String, String> {
    if form.columns.is_empty() {
        return Err("add at least one column".into());
    }
    let needs_schema = !matches!(
        driver,
        DbDriver::Sqlite | DbDriver::Mysql | DbDriver::Clickhouse | DbDriver::Bigquery
    );
    // MySQL/CH may use schema as database qualifier when set.
    if needs_schema && (schema.is_empty() || !ident_ok(schema)) {
        if !matches!(driver, DbDriver::Sqlite) {
            return Err("schema is required".into());
        }
    }
    let mut parts = Vec::new();
    for c in &form.columns {
        parts.push(col_fragment(driver, c)?);
    }
    let fq = match driver {
        DbDriver::Sqlite => q(driver, name),
        _ => {
            if schema.is_empty() {
                q(driver, name)
            } else {
                qual(driver, schema, name)
            }
        }
    };

    match driver {
        DbDriver::Postgres => {
            let mut sql = String::new();
            if !schema.is_empty() {
                if owner.is_empty() {
                    sql.push_str(&format!(
                        "CREATE SCHEMA IF NOT EXISTS {};\n",
                        q(driver, schema)
                    ));
                } else {
                    sql.push_str(&format!(
                        "CREATE SCHEMA IF NOT EXISTS {} AUTHORIZATION {};\n",
                        q(driver, schema),
                        q(driver, owner)
                    ));
                }
            }
            sql.push_str(&format!(
                "CREATE TABLE {fq} (\n  {}\n);\n",
                parts.join(",\n  ")
            ));
            sql.push_str(&format!("REVOKE ALL ON TABLE {fq} FROM PUBLIC;\n"));
            if !owner.is_empty() {
                sql.push_str(&format!("ALTER TABLE {fq} OWNER TO {};\n", q(driver, owner)));
            }
            sql.push_str(&pg_relation_grants(schema, name, form.grant_preset, grantee)?);
            Ok(sql)
        }
        DbDriver::Mysql => {
            let mut sql = format!(
                "CREATE TABLE {fq} (\n  {}\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;\n",
                parts.join(",\n  ")
            );
            if form.grant_preset != GrantPreset::None && !grantee.is_empty() {
                let privs = match form.grant_preset {
                    GrantPreset::Readonly => "SELECT",
                    GrantPreset::Readwrite => "SELECT, INSERT, UPDATE, DELETE",
                    GrantPreset::None => "",
                };
                sql.push_str(&format!("GRANT {privs} ON {fq} TO {}@'%';\n", q(driver, grantee)));
            }
            Ok(sql)
        }
        DbDriver::Mssql => {
            let mut sql = String::new();
            if !schema.is_empty() {
                sql.push_str(&format!(
                    "IF NOT EXISTS (SELECT 1 FROM sys.schemas WHERE name = N'{}') EXEC('CREATE SCHEMA {}');\n",
                    schema.replace('\'', "''"),
                    q(driver, schema)
                ));
            }
            sql.push_str(&format!(
                "CREATE TABLE {fq} (\n  {}\n);\n",
                parts.join(",\n  ")
            ));
            if !owner.is_empty() {
                sql.push_str(&format!(
                    "ALTER AUTHORIZATION ON OBJECT::{fq} TO {};\n",
                    q(driver, owner)
                ));
            }
            if form.grant_preset != GrantPreset::None && !grantee.is_empty() {
                let privs = match form.grant_preset {
                    GrantPreset::Readonly => "SELECT",
                    GrantPreset::Readwrite => "SELECT, INSERT, UPDATE, DELETE",
                    GrantPreset::None => "",
                };
                sql.push_str(&format!(
                    "GRANT {privs} ON {fq} TO {};\n",
                    q(driver, grantee)
                ));
            }
            Ok(sql)
        }
        DbDriver::Sqlite | DbDriver::Duckdb => {
            let mut sql = String::new();
            if driver == DbDriver::Duckdb && !schema.is_empty() && schema != "main" {
                sql.push_str(&format!(
                    "CREATE SCHEMA IF NOT EXISTS {};\n",
                    q(driver, schema)
                ));
            }
            sql.push_str(&format!(
                "CREATE TABLE {fq} (\n  {}\n);\n",
                parts.join(",\n  ")
            ));
            Ok(sql)
        }
        DbDriver::Clickhouse => {
            let order = form
                .columns
                .iter()
                .find(|c| c.primary_key)
                .map(|c| c.name.as_str())
                .unwrap_or("tuple()");
            let order_sql = if order == "tuple()" {
                "tuple()".into()
            } else {
                q(driver, order)
            };
            let mut sql = String::new();
            if !schema.is_empty() {
                sql.push_str(&format!(
                    "CREATE DATABASE IF NOT EXISTS {};\n",
                    q(driver, schema)
                ));
            }
            sql.push_str(&format!(
                "CREATE TABLE {fq} (\n  {}\n) ENGINE = MergeTree ORDER BY {order_sql};\n",
                parts.join(",\n  ")
            ));
            if form.grant_preset != GrantPreset::None && !grantee.is_empty() {
                let privs = match form.grant_preset {
                    GrantPreset::Readonly => "SELECT",
                    GrantPreset::Readwrite => "SELECT, INSERT, ALTER, DELETE",
                    GrantPreset::None => "",
                };
                sql.push_str(&format!(
                    "GRANT {privs} ON {fq} TO {};\n",
                    q(driver, grantee)
                ));
            }
            Ok(sql)
        }
        DbDriver::Snowflake | DbDriver::Bigquery | DbDriver::Trino => {
            let mut sql = String::new();
            if !schema.is_empty()
                && matches!(driver, DbDriver::Snowflake | DbDriver::Trino)
            {
                sql.push_str(&format!(
                    "CREATE SCHEMA IF NOT EXISTS {};\n",
                    q(driver, schema)
                ));
            }
            if driver == DbDriver::Bigquery && !schema.is_empty() {
                sql.push_str(&format!(
                    "CREATE SCHEMA IF NOT EXISTS {};\n",
                    q(driver, schema)
                ));
            }
            sql.push_str(&format!(
                "CREATE TABLE IF NOT EXISTS {fq} (\n  {}\n);\n",
                parts.join(",\n  ")
            ));
            if form.grant_preset != GrantPreset::None && !grantee.is_empty() {
                let privs = match form.grant_preset {
                    GrantPreset::Readonly => "SELECT",
                    GrantPreset::Readwrite => "SELECT, INSERT, UPDATE, DELETE",
                    GrantPreset::None => "",
                };
                sql.push_str(&format!(
                    "-- GRANT {privs} ON {fq} TO ROLE {}; -- run in warehouse IAM\n",
                    grantee
                ));
            }
            Ok(sql)
        }
        _ => Err(format!("CREATE TABLE not supported on {}", driver.label())),
    }
}

fn emit_view(
    driver: DbDriver,
    form: &CreateObjectForm,
    name: &str,
    schema: &str,
    owner: &str,
    grantee: &str,
    materialized: bool,
) -> Result<String, String> {
    if materialized && !matches!(driver, DbDriver::Postgres) {
        return Err("materialized views only on PostgreSQL in Create".into());
    }
    let sel = form.body.trim().trim_end_matches(';');
    if sel.is_empty() {
        return Err("SELECT body is required".into());
    }
    let fq = if schema.is_empty() {
        q(driver, name)
    } else {
        qual(driver, schema, name)
    };
    match driver {
        DbDriver::Postgres => {
            let mut sql = String::new();
            if !schema.is_empty() {
                if owner.is_empty() {
                    sql.push_str(&format!(
                        "CREATE SCHEMA IF NOT EXISTS {};\n",
                        q(driver, schema)
                    ));
                } else {
                    sql.push_str(&format!(
                        "CREATE SCHEMA IF NOT EXISTS {} AUTHORIZATION {};\n",
                        q(driver, schema),
                        q(driver, owner)
                    ));
                }
            }
            if materialized {
                sql.push_str(&format!(
                    "CREATE MATERIALIZED VIEW {fq} AS\n{sel}\nWITH DATA;\n"
                ));
            } else {
                sql.push_str(&format!("CREATE VIEW {fq} AS\n{sel};\n"));
            }
            sql.push_str(&format!("REVOKE ALL ON TABLE {fq} FROM PUBLIC;\n"));
            if !owner.is_empty() {
                let kind = if materialized {
                    "MATERIALIZED VIEW"
                } else {
                    "VIEW"
                };
                sql.push_str(&format!(
                    "ALTER {kind} {fq} OWNER TO {};\n",
                    q(driver, owner)
                ));
            }
            sql.push_str(&pg_relation_grants(schema, name, form.grant_preset, grantee)?);
            Ok(sql)
        }
        DbDriver::Mysql | DbDriver::Sqlite | DbDriver::Duckdb | DbDriver::Mssql
        | DbDriver::Clickhouse | DbDriver::Snowflake | DbDriver::Bigquery | DbDriver::Trino => {
            Ok(format!("CREATE VIEW {fq} AS\n{sel};"))
        }
        _ => Err(format!("CREATE VIEW not supported on {}", driver.label())),
    }
}

fn emit_index(
    driver: DbDriver,
    form: &CreateObjectForm,
    name: &str,
    on: &str,
) -> Result<String, String> {
    let cols = form.body.trim();
    if cols.is_empty() {
        return Err("index columns required".into());
    }
    let (sch, tbl) = on.split_once('.').unwrap_or(("", on));
    if tbl.is_empty() || !ident_ok(tbl) {
        return Err("On: use schema.table (or table)".into());
    }
    if !sch.is_empty() && !ident_ok(sch) {
        return Err("On: bad schema".into());
    }
    let table = if sch.is_empty() {
        q(driver, tbl)
    } else {
        qual(driver, sch, tbl)
    };
    let uniq = if form.flag { "UNIQUE " } else { "" };
    match driver {
        DbDriver::Postgres => {
            let conc = if form.concurrent {
                "CONCURRENTLY "
            } else {
                ""
            };
            Ok(format!(
                "CREATE {uniq}INDEX {conc}{} ON {table} ({cols});",
                q(driver, name)
            ))
        }
        DbDriver::Mysql => Ok(format!(
            "CREATE {uniq}INDEX {} ON {table} ({cols});",
            q(driver, name)
        )),
        DbDriver::Mssql => {
            // ONLINE = ON roughly concurrent
            let online = if form.concurrent {
                " WITH (ONLINE = ON)"
            } else {
                ""
            };
            Ok(format!(
                "CREATE {uniq}INDEX {} ON {table} ({cols}){online};",
                q(driver, name)
            ))
        }
        DbDriver::Sqlite | DbDriver::Duckdb => Ok(format!(
            "CREATE {uniq}INDEX IF NOT EXISTS {} ON {table} ({cols});",
            q(driver, name)
        )),
        _ => Err(format!("CREATE INDEX not supported on {}", driver.label())),
    }
}

fn emit_sequence(
    driver: DbDriver,
    form: &CreateObjectForm,
    name: &str,
    schema: &str,
    owner: &str,
) -> Result<String, String> {
    let start = form.body.trim();
    let start_n: i64 = if start.is_empty() {
        1
    } else {
        start
            .parse()
            .map_err(|_| "start: integer required".to_string())?
    };
    match driver {
        DbDriver::Postgres | DbDriver::Duckdb => {
            if schema.is_empty() || !ident_ok(schema) {
                return Err("schema is required".into());
            }
            let fq = qual(driver, schema, name);
            let mut sql = format!("CREATE SEQUENCE {fq} START WITH {start_n};\n");
            if driver == DbDriver::Postgres {
                sql.push_str(&format!("REVOKE ALL ON SEQUENCE {fq} FROM PUBLIC;\n"));
                if !owner.is_empty() {
                    sql.push_str(&format!(
                        "ALTER SEQUENCE {fq} OWNER TO {};\n",
                        q(driver, owner)
                    ));
                }
            }
            Ok(sql)
        }
        DbDriver::Mssql => {
            let fq = if schema.is_empty() {
                q(driver, name)
            } else {
                qual(driver, schema, name)
            };
            Ok(format!(
                "CREATE SEQUENCE {fq} AS bigint START WITH {start_n} INCREMENT BY 1;"
            ))
        }
        _ => Err(format!(
            "CREATE SEQUENCE not supported on {}",
            driver.label()
        )),
    }
}

fn emit_extension(driver: DbDriver, name: &str) -> Result<String, String> {
    if driver != DbDriver::Postgres {
        return Err("extensions are PostgreSQL-only".into());
    }
    let quoted = if ident_ok(name) {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    };
    Ok(format!("CREATE EXTENSION IF NOT EXISTS {quoted};"))
}

fn emit_role(driver: DbDriver, form: &CreateObjectForm, name: &str) -> Result<String, String> {
    match driver {
        DbDriver::Postgres => {
            let mut sql = format!("CREATE ROLE {}", q(driver, name));
            sql.push_str(if form.flag { " LOGIN" } else { " NOLOGIN" });
            sql.push_str(if form.inherit {
                " INHERIT"
            } else {
                " NOINHERIT"
            });
            sql.push_str(if form.can_create_db {
                " CREATEDB"
            } else {
                " NOCREATEDB"
            });
            sql.push_str(if form.can_create_role {
                " CREATEROLE"
            } else {
                " NOCREATEROLE"
            });
            sql.push_str(if form.superuser {
                " SUPERUSER"
            } else {
                " NOSUPERUSER"
            });
            sql.push_str(if form.replication {
                " REPLICATION"
            } else {
                " NOREPLICATION"
            });
            sql.push_str(if form.bypass_rls {
                " BYPASSRLS"
            } else {
                " NOBYPASSRLS"
            });
            if let Some(lim) = parse_conn_limit(&form.conn_limit)? {
                sql.push_str(&format!(" CONNECTION LIMIT {lim}"));
            }
            let vu = form.valid_until.trim();
            if !vu.is_empty() {
                sql.push_str(&format!(" VALID UNTIL '{}'", vu.replace('\'', "''")));
            }
            let pw = form.password.trim();
            if form.flag && !pw.is_empty() {
                sql.push_str(&format!(" PASSWORD '{}'", pw.replace('\'', "''")));
            }
            sql.push(';');
            for m in form.body.split([',', ' ', ';']) {
                let m = m.trim();
                if m.is_empty() {
                    continue;
                }
                if !ident_ok(m) {
                    return Err(format!("member of: bad role {m}"));
                }
                sql.push_str(&format!(
                    "\nGRANT {} TO {};",
                    q(driver, m),
                    q(driver, name)
                ));
            }
            Ok(sql)
        }
        DbDriver::Mysql => {
            let host = form.template.trim(); // reuse template field as host (default %)
            let host = if host.is_empty() { "%" } else { host };
            let user = format!("{}@'{}'", q(driver, name), host.replace('\'', "''"));
            let mut sql = format!("CREATE USER {user}");
            let pw = form.password.trim();
            if !pw.is_empty() {
                sql.push_str(&format!(" IDENTIFIED BY '{}'", pw.replace('\'', "''")));
            }
            sql.push(';');
            if form.superuser {
                sql.push_str(&format!("\nGRANT ALL PRIVILEGES ON *.* TO {user} WITH GRANT OPTION;"));
            } else if form.can_create_db {
                sql.push_str(&format!("\nGRANT CREATE ON *.* TO {user};"));
            }
            for m in form.body.split([',', ' ', ';']) {
                let m = m.trim();
                if m.is_empty() {
                    continue;
                }
                // member_of → GRANT role TO user (MySQL 8 roles)
                if !ident_ok(m) {
                    return Err(format!("member of: bad role {m}"));
                }
                sql.push_str(&format!("\nGRANT {} TO {user};", q(driver, m)));
            }
            Ok(sql)
        }
        DbDriver::Mssql => {
            let pw = form.password.trim();
            if pw.is_empty() {
                return Err("password required for MSSQL login".into());
            }
            let mut sql = format!(
                "CREATE LOGIN {} WITH PASSWORD = N'{}';",
                q(driver, name),
                pw.replace('\'', "''")
            );
            sql.push_str(&format!(
                "\nCREATE USER {} FOR LOGIN {};",
                q(driver, name),
                q(driver, name)
            ));
            if form.superuser {
                sql.push_str(&format!(
                    "\nALTER SERVER ROLE sysadmin ADD MEMBER {};",
                    q(driver, name)
                ));
            }
            Ok(sql)
        }
        DbDriver::Clickhouse => {
            let mut sql = format!("CREATE USER IF NOT EXISTS {} IDENTIFIED WITH no_password;", q(driver, name));
            let pw = form.password.trim();
            if !pw.is_empty() {
                sql = format!(
                    "CREATE USER IF NOT EXISTS {} IDENTIFIED WITH sha256_password BY '{}';",
                    q(driver, name),
                    pw.replace('\'', "''")
                );
            }
            if form.superuser {
                sql.push_str(&format!(
                    "\nGRANT ALL ON *.* TO {} WITH GRANT OPTION;",
                    q(driver, name)
                ));
            } else if form.can_create_db {
                sql.push_str(&format!("\nGRANT CREATE DATABASE ON *.* TO {};", q(driver, name)));
            }
            Ok(sql)
        }
        _ => Err(format!(
            "CREATE ROLE/USER not supported on {}",
            driver.label()
        )),
    }
}

fn pg_schema_grants(schema: &str, preset: GrantPreset, grantee: &str) -> Result<String, String> {
    if preset == GrantPreset::None || grantee.is_empty() {
        return Ok(String::new());
    }
    let mut s = format!(
        "\nGRANT USAGE ON SCHEMA {} TO {};",
        q(DbDriver::Postgres, schema),
        q(DbDriver::Postgres, grantee)
    );
    if preset == GrantPreset::Readwrite {
        s.push_str(&format!(
            "\nGRANT CREATE ON SCHEMA {} TO {};",
            q(DbDriver::Postgres, schema),
            q(DbDriver::Postgres, grantee)
        ));
    }
    Ok(s)
}

fn pg_relation_grants(
    schema: &str,
    name: &str,
    preset: GrantPreset,
    grantee: &str,
) -> Result<String, String> {
    if preset == GrantPreset::None || grantee.is_empty() {
        return Ok(String::new());
    }
    let fq = if schema.is_empty() {
        q(DbDriver::Postgres, name)
    } else {
        qual(DbDriver::Postgres, schema, name)
    };
    let g = q(DbDriver::Postgres, grantee);
    let mut s = if !schema.is_empty() {
        format!(
            "GRANT USAGE ON SCHEMA {} TO {g};\n",
            q(DbDriver::Postgres, schema)
        )
    } else {
        String::new()
    };
    match preset {
        GrantPreset::Readonly => {
            s.push_str(&format!("GRANT SELECT ON TABLE {fq} TO {g};\n"));
        }
        GrantPreset::Readwrite => {
            s.push_str(&format!(
                "GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE {fq} TO {g};\n"
            ));
        }
        GrantPreset::None => {}
    }
    Ok(s)
}

/// Role form: MySQL reuses `template` as host (default %).
pub fn role_host_label(driver: DbDriver) -> Option<&'static str> {
    match driver {
        DbDriver::Mysql => Some("Host"),
        _ => None,
    }
}

/// Whether Superuser / Replication / BypassRLS / etc. apply.
pub fn shows_pg_role_attrs(driver: DbDriver) -> bool {
    matches!(driver, DbDriver::Postgres)
}

pub fn shows_mssql_role_attrs(driver: DbDriver) -> bool {
    matches!(driver, DbDriver::Mssql)
}

pub fn shows_mysql_role_attrs(driver: DbDriver) -> bool {
    matches!(driver, DbDriver::Mysql)
}

/// Hint under create modal.
pub fn footer_hint(form: &CreateObjectForm) -> &'static str {
    match form.driver {
        DbDriver::Postgres if form.kind == CreateObjectKind::Role && form.superuser => {
            "  ⚠ SUPERUSER on · full cluster power "
        }
        DbDriver::Mysql if form.kind == CreateObjectKind::Role && form.superuser => {
            "  ⚠ ALL PRIVILEGES *.* WITH GRANT OPTION "
        }
        DbDriver::Mssql if form.kind == CreateObjectKind::Role && form.superuser => {
            "  ⚠ sysadmin server role "
        }
        DbDriver::Clickhouse if form.kind == CreateObjectKind::Role && form.superuser => {
            "  ⚠ GRANT ALL ON *.* WITH GRANT OPTION "
        }
        DbDriver::Clickhouse if form.kind == CreateObjectKind::Table => {
            "  MergeTree · ORDER BY = PK column "
        }
        DbDriver::Sqlite if form.kind == CreateObjectKind::Table => {
            "  INTEGER PRIMARY KEY AUTOINCREMENT for id "
        }
        DbDriver::Snowflake | DbDriver::Bigquery if form.kind == CreateObjectKind::Table => {
            "  warehouse naming: logical Title → physical snake_case "
        }
        _ if form.kind == CreateObjectKind::Table => {
            "  + / a add · x del · ←→ type/flags · snake_case physical names "
        }
        _ => "  dialect follows active connection driver · Tab · Esc ",
    }
}

/// Used by column type cycling from the form.
pub fn nudge_column_type(form: &mut CreateObjectForm, forward: bool) {
    if form.kind == CreateObjectKind::Table && form.col_sub == ColSub::Type {
        if let Some(col) = form.columns.get_mut(form.col_idx) {
            cycle_column_type(form.driver, &mut col.typ, forward);
        }
    }
}
