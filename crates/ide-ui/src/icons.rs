//! Nerd Font glyphs for the Database studio (datarig-style chrome).
//!
//! Every glyph has a plain-ASCII fallback used when `[theme] icons = false`, so the
//! workspace stays readable on terminals without a Nerd Font. Code points follow Nerd
//! Fonts v3; the app never draws emoji.

use crate::theme;
use ide_db::{DbDriver, DbNodeKind};

pub const CONNECTIONS: &str = "\u{f048d}";
pub const SERVER: &str = "\u{f233}";
pub const DATABASE: &str = "\u{f1c0}";
pub const SCHEMA: &str = "\u{f12e3}";
pub const TABLE: &str = "\u{f04eb}";
pub const VIEW: &str = "\u{f1094}";
pub const COLUMN: &str = "\u{f0835}";
pub const ADMIN: &str = "\u{f013}";
pub const QUERY: &str = "\u{f120}";
pub const ACTION: &str = "\u{f04b}";
pub const ROUTINE: &str = "\u{f121}";
pub const GROUP: &str = "\u{f07b}";
pub const SCRIPT: &str = "\u{f1c9}";
pub const PIN: &str = "\u{f08d}";
pub const KEY: &str = "\u{f084}";
pub const LINK: &str = "\u{f0c1}";
pub const PAGE_PREV: &str = "\u{f0d9}";
pub const PAGE_NEXT: &str = "\u{f0da}";
pub const RUN: &str = "\u{f04b}";
pub const EXPLAIN: &str = "\u{f0eb}";
pub const EXPORT: &str = "\u{f019}";
pub const APPLY: &str = "\u{f00c}";

pub const CHEVRON_DOWN: &str = "\u{25be}";
pub const CHEVRON_RIGHT: &str = "\u{25b8}";

/// A glyph with its ASCII fallback.
fn pick(glyph: &'static str, ascii: &'static str) -> &'static str {
    if theme::icons_on() {
        glyph
    } else {
        ascii
    }
}

pub fn folder(expanded: bool) -> &'static str {
    if !theme::icons_on() {
        if expanded {
            "[-]"
        } else {
            "[+]"
        }
    } else if expanded {
        CHEVRON_DOWN
    } else {
        CHEVRON_RIGHT
    }
}

/// Expand/collapse twist; leaves keep the same width.
pub fn twist(expandable: bool, expanded: bool) -> &'static str {
    if !expandable {
        " "
    } else if expanded {
        CHEVRON_DOWN
    } else {
        CHEVRON_RIGHT
    }
}

/// The explorer glyph of a node, before its label.
pub fn node(kind: &DbNodeKind) -> &'static str {
    match kind {
        DbNodeKind::Group => pick(GROUP, "[]"),
        DbNodeKind::Source => pick(SERVER, "DB"),
        DbNodeKind::Database => pick(DATABASE, "db"),
        DbNodeKind::Schema => pick(SCHEMA, "sc"),
        DbNodeKind::Table => pick(TABLE, "tb"),
        DbNodeKind::View => pick(VIEW, "vw"),
        DbNodeKind::Column => pick(COLUMN, "cl"),
        DbNodeKind::Admin => pick(ADMIN, "ad"),
        DbNodeKind::AdminQuery => pick(QUERY, ">_"),
        DbNodeKind::AdminAction => pick(ACTION, ">"),
        DbNodeKind::Routine => pick(ROUTINE, "fn"),
    }
}

/// The driver's glyph (fallback: `DB`).
pub fn driver(driver: DbDriver) -> &'static str {
    if !theme::icons_on() {
        return "DB";
    }
    match driver {
        DbDriver::Postgres => "\u{e76e}",
        DbDriver::Mysql => "\u{e704}",
        DbDriver::Sqlite => "\u{e7c4}",
        DbDriver::Redis => "\u{e76d}",
        DbDriver::Mongo => "\u{e7a4}",
        DbDriver::Elasticsearch => "\u{e7ca}",
        DbDriver::Mssql => "\u{e82e}",
        DbDriver::Clickhouse | DbDriver::Duckdb | DbDriver::Snowflake | DbDriver::Bigquery | DbDriver::Trino => {
            DATABASE
        }
    }
}

/// The glyph of a Structure section, from its heading (`COLUMNS (12)` → table_column).
pub fn structure_section(text: &str) -> &'static str {
    if !theme::icons_on() {
        return "";
    }
    let head = text.trim_start().to_ascii_uppercase();
    if head.starts_with("COLUMNS") {
        COLUMN
    } else if head.starts_with("INDEXES") {
        "\u{f027b}"
    } else if head.starts_with("FOREIGN KEYS") {
        LINK
    } else if head.starts_with("TRIGGERS") {
        "\u{f140b}"
    } else if head.starts_with("ROUTINES") {
        ROUTINE
    } else if head.starts_with("GRANTS") {
        "\u{f132}"
    } else if head.starts_with("STATS") {
        "\u{f080}"
    } else if head.starts_with("ER") {
        "\u{f0e8}"
    } else if head.starts_with("DDL") || head.starts_with("SCHEMA DIFF") {
        "\u{f121}"
    } else if head.starts_with("ADMIN QUERY") {
        QUERY
    } else {
        ""
    }
}

pub fn connections() -> &'static str {
    pick(CONNECTIONS, "DB")
}

pub fn page_arrows() -> (&'static str, &'static str) {
    if theme::icons_on() {
        (PAGE_PREV, PAGE_NEXT)
    } else {
        ("<", ">")
    }
}

pub fn pin() -> &'static str {
    pick(PIN, "*")
}

pub fn key() -> &'static str {
    pick(KEY, "PK")
}

pub fn link() -> &'static str {
    pick(LINK, "FK")
}

pub fn script() -> &'static str {
    pick(SCRIPT, "sql")
}
