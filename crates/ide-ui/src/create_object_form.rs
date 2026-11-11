//! Modal forms for `n New` — create DB objects without dumping SQL into Console.
//! DBA-complete: Owner/grants, role attributes (incl. opt-in SUPERUSER),
//! Extension / Sequence / Materialized view, Index CONCURRENTLY, Database opts.

use crate::create_dialect;
use crate::theme;
use ide_db::DbDriver;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateObjectKind {
    Database,
    Schema,
    Table,
    View,
    MaterializedView,
    Index,
    Sequence,
    Extension,
    Role,
    Policy,
    Function,
    Publication,
    Tablespace,
}

impl CreateObjectKind {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "create_database" => Some(Self::Database),
            "create_schema" => Some(Self::Schema),
            "create_table" => Some(Self::Table),
            "create_view" => Some(Self::View),
            "create_matview" => Some(Self::MaterializedView),
            "create_index" => Some(Self::Index),
            "create_sequence" => Some(Self::Sequence),
            "create_extension" => Some(Self::Extension),
            "create_role" => Some(Self::Role),
            "create_policy" => Some(Self::Policy),
            "create_function" => Some(Self::Function),
            "create_publication" => Some(Self::Publication),
            "create_tablespace" => Some(Self::Tablespace),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Database => "New database",
            Self::Schema => "New schema",
            Self::Table => "New table",
            Self::View => "New view",
            Self::MaterializedView => "New materialized view",
            Self::Index => "New index",
            Self::Sequence => "New sequence",
            Self::Extension => "New extension",
            Self::Role => "New role / user",
            Self::Policy => "New RLS policy",
            Self::Function => "New function",
            Self::Publication => "New publication",
            Self::Tablespace => "New tablespace",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Database => "create_database",
            Self::Schema => "create_schema",
            Self::Table => "create_table",
            Self::View => "create_view",
            Self::MaterializedView => "create_matview",
            Self::Index => "create_index",
            Self::Sequence => "create_sequence",
            Self::Extension => "create_extension",
            Self::Role => "create_role",
            Self::Policy => "create_policy",
            Self::Function => "create_function",
            Self::Publication => "create_publication",
            Self::Tablespace => "create_tablespace",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantPreset {
    None,
    Readonly,
    Readwrite,
}

impl GrantPreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Readonly => "readonly (SELECT)",
            Self::Readwrite => "readwrite (DML)",
        }
    }

    pub fn cycle(self, forward: bool) -> Self {
        let all = [Self::None, Self::Readonly, Self::Readwrite];
        let i = all.iter().position(|p| *p == self).unwrap_or(0);
        let next = if forward {
            (i + 1) % all.len()
        } else {
            (i + all.len() - 1) % all.len()
        };
        all[next]
    }
}

#[derive(Debug, Clone)]
pub struct ColumnDraft {
    pub name: String,
    pub typ: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub identity: bool,
}

impl ColumnDraft {
    pub fn new(name: &str, typ: &str) -> Self {
        Self {
            name: name.into(),
            typ: typ.into(),
            nullable: false,
            primary_key: false,
            identity: false,
        }
    }

}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColSub {
    Name = 0,
    Type = 1,
    Null = 2,
    Pk = 3,
    Ident = 4,
}

impl ColSub {
    fn from_usize(i: usize) -> Self {
        match i % 5 {
            0 => Self::Name,
            1 => Self::Type,
            2 => Self::Null,
            3 => Self::Pk,
            _ => Self::Ident,
        }
    }

    fn next(self) -> Self {
        Self::from_usize(self as usize + 1)
    }

    fn prev(self) -> Self {
        Self::from_usize(self as usize + 4)
    }
}

#[derive(Debug, Clone)]
pub struct CreateObjectForm {
    pub kind: CreateObjectKind,
    pub driver: DbDriver,
    pub field: usize,
    pub name: String,
    pub schema: String,
    /// View/Matview SELECT · Index columns · Role member_of · Sequence START · Extension schema.
    pub body: String,
    /// Role LOGIN · Index UNIQUE.
    pub flag: bool,
    pub password: String,
    pub owner: String,
    pub grant_role: String,
    pub grant_preset: GrantPreset,
    pub inherit: bool,
    pub can_create_db: bool,
    pub can_create_role: bool,
    /// Opt-in dangerous privilege (default false).
    pub superuser: bool,
    pub replication: bool,
    pub bypass_rls: bool,
    /// Role/Database CONNECTION LIMIT (−1 or empty = unlimited).
    pub conn_limit: String,
    /// Role VALID UNTIL timestamp string (empty = none).
    pub valid_until: String,
    /// Index CREATE INDEX CONCURRENTLY.
    pub concurrent: bool,
    /// Database TEMPLATE (empty = template1 default).
    pub template: String,
    /// Database ENCODING (default UTF8).
    pub encoding: String,
    pub columns: Vec<ColumnDraft>,
    pub col_idx: usize,
    pub col_sub: ColSub,
    pub feedback: Option<(String, bool)>,
}

impl CreateObjectForm {
    pub fn fresh(kind: CreateObjectKind, default_schema: &str) -> Self {
        Self::fresh_with_owner(kind, default_schema, "", DbDriver::Postgres)
    }

    pub fn fresh_with_owner(
        kind: CreateObjectKind,
        default_schema: &str,
        default_owner: &str,
        driver: DbDriver,
    ) -> Self {
        let fallback = create_dialect::default_schema(driver);
        let schema = if default_schema.is_empty() {
            fallback.to_string()
        } else {
            default_schema.to_string()
        };
        let (name, body, flag) = match kind {
            CreateObjectKind::Database => ("new_database".into(), String::new(), false),
            CreateObjectKind::Schema => ("new_schema".into(), String::new(), false),
            CreateObjectKind::Table => ("new_table".into(), String::new(), false),
            CreateObjectKind::View => ("new_view".into(), "SELECT 1 AS id".into(), false),
            CreateObjectKind::MaterializedView => {
                ("new_matview".into(), "SELECT 1 AS id".into(), false)
            }
            CreateObjectKind::Index => ("new_index".into(), "id".into(), false),
            CreateObjectKind::Sequence => ("new_seq".into(), "1".into(), false),
            CreateObjectKind::Extension => ("pgcrypto".into(), String::new(), false),
            CreateObjectKind::Role => ("new_role".into(), String::new(), true),
            CreateObjectKind::Policy => ("new_policy".into(), "true".into(), false),
            CreateObjectKind::Function => (
                "new_fn".into(),
                "RETURNS integer
LANGUAGE sql
AS $$ SELECT 1; $$".into(),
                false,
            ),
            CreateObjectKind::Publication => ("new_pub".into(), "TABLE public.t".into(), false),
            CreateObjectKind::Tablespace => ("new_tspace".into(), "/var/lib/postgresql/tspace".into(), false),
        };
        let schema = match kind {
            CreateObjectKind::Index | CreateObjectKind::Policy => {
                format!("{schema}.new_table")
            }
            CreateObjectKind::Extension | CreateObjectKind::Tablespace => String::new(),
            _ => schema,
        };
        let columns = if kind == CreateObjectKind::Table {
            create_dialect::seed_table_columns(driver)
        } else {
            Vec::new()
        };
        // MySQL role: template field = host (default %)
        let template = if kind == CreateObjectKind::Role && driver == DbDriver::Mysql {
            "%".into()
        } else {
            String::new()
        };
        let encoding = match driver {
            DbDriver::Mysql => "utf8mb4".into(),
            _ => "UTF8".into(),
        };
        Self {
            kind,
            driver,
            field: 0,
            name,
            schema,
            body,
            flag,
            password: if kind == CreateObjectKind::Policy {
                "ALL".into()
            } else {
                String::new()
            },
            owner: default_owner.trim().to_string(),
            grant_role: String::new(),
            grant_preset: GrantPreset::None,
            inherit: true,
            can_create_db: false,
            can_create_role: false,
            superuser: false,
            replication: false,
            bypass_rls: false,
            conn_limit: String::new(),
            valid_until: String::new(),
            concurrent: false,
            template,
            encoding,
            columns,
            col_idx: 0,
            col_sub: ColSub::Name,
            feedback: None,
        }
    }

    pub fn field_count(&self) -> usize {
        match self.kind {
            CreateObjectKind::Database => 5, // name owner template encoding conn_limit
            CreateObjectKind::Schema => 4,
            CreateObjectKind::View | CreateObjectKind::MaterializedView => 6,
            CreateObjectKind::Index => 5, // on name cols unique concurrently
            CreateObjectKind::Sequence => 4, // schema name owner start
            CreateObjectKind::Extension => 1,
            CreateObjectKind::Policy => 4, // name, table, cmd(flag via cycle), using(body)
            CreateObjectKind::Function => 3, // schema, name, body
            CreateObjectKind::Publication => 2, // name, tables body
            CreateObjectKind::Tablespace => 2, // name, location body
            CreateObjectKind::Role => match self.driver {
                DbDriver::Mysql => 6,
                DbDriver::Mssql => 3,
                _ => 12, // Postgres full attrs
            },
            CreateObjectKind::Table => 5 + self.columns.len().max(1) + 1,
        }
    }

    fn table_zone(&self) -> TableZone {
        if self.kind != CreateObjectKind::Table {
            return TableZone::Header;
        }
        if self.field <= 1 {
            TableZone::Header
        } else if self.field <= 4 {
            TableZone::Acl
        } else if self.field >= 5 + self.columns.len() {
            TableZone::AddBtn
        } else {
            TableZone::Column
        }
    }

    pub fn next_field(&mut self) {
        if self.kind == CreateObjectKind::Table {
            match self.table_zone() {
                TableZone::Header => {
                    self.field = if self.field == 0 { 1 } else { 2 };
                }
                TableZone::Acl => {
                    if self.field < 4 {
                        self.field += 1;
                    } else {
                        self.field = 5;
                        self.col_idx = 0;
                        self.col_sub = ColSub::Name;
                        self.sync_table_field();
                    }
                }
                TableZone::Column => {
                    if self.col_sub != ColSub::Ident {
                        self.col_sub = self.col_sub.next();
                    } else if self.col_idx + 1 < self.columns.len() {
                        self.col_idx += 1;
                        self.col_sub = ColSub::Name;
                    } else {
                        self.field = 5 + self.columns.len();
                    }
                    self.sync_table_field();
                }
                TableZone::AddBtn => {
                    self.field = 0;
                    self.col_idx = 0;
                    self.col_sub = ColSub::Name;
                }
            }
            return;
        }
        let n = self.field_count();
        self.field = (self.field + 1) % n;
    }

    pub fn prev_field(&mut self) {
        if self.kind == CreateObjectKind::Table {
            match self.table_zone() {
                TableZone::Header => {
                    if self.field == 1 {
                        self.field = 0;
                    } else {
                        self.field = 5 + self.columns.len();
                    }
                }
                TableZone::Acl => {
                    self.field -= 1;
                }
                TableZone::Column => {
                    if self.col_sub != ColSub::Name {
                        self.col_sub = self.col_sub.prev();
                    } else if self.col_idx > 0 {
                        self.col_idx -= 1;
                        self.col_sub = ColSub::Ident;
                    } else {
                        self.field = 4;
                    }
                    self.sync_table_field();
                }
                TableZone::AddBtn => {
                    if self.columns.is_empty() {
                        self.field = 4;
                    } else {
                        self.col_idx = self.columns.len() - 1;
                        self.col_sub = ColSub::Ident;
                        self.sync_table_field();
                    }
                }
            }
            return;
        }
        let n = self.field_count();
        self.field = if self.field == 0 { n - 1 } else { self.field - 1 };
    }

    fn sync_table_field(&mut self) {
        if self.kind == CreateObjectKind::Table && self.table_zone() == TableZone::Column {
            self.field = 5 + self.col_idx;
        }
    }

    pub fn toggle_flag(&mut self) {
        self.nudge_flag(true);
    }

    pub fn nudge_flag(&mut self, forward: bool) {
        match self.kind {
            CreateObjectKind::Index => match self.field {
                3 => self.flag = !self.flag,
                4 => self.concurrent = !self.concurrent,
                _ => {}
            },
            CreateObjectKind::Role => match self.driver {
                DbDriver::Mysql => match self.field {
                    3 => self.superuser = !self.superuser,
                    4 => self.can_create_db = !self.can_create_db,
                    _ => {}
                },
                DbDriver::Mssql => {
                    if self.field == 2 {
                        self.superuser = !self.superuser;
                    }
                }
                _ => match self.field {
                    1 => self.flag = !self.flag,
                    3 => self.inherit = !self.inherit,
                    4 => self.can_create_db = !self.can_create_db,
                    5 => self.can_create_role = !self.can_create_role,
                    6 => self.superuser = !self.superuser,
                    7 => self.replication = !self.replication,
                    8 => self.bypass_rls = !self.bypass_rls,
                    _ => {}
                },
            },
            CreateObjectKind::Schema if self.field == 2 => {
                self.grant_preset = self.grant_preset.cycle(forward);
            }
            CreateObjectKind::Policy if self.field == 2 => {
                cycle_policy_cmd(&mut self.password, forward);
            }
            CreateObjectKind::View | CreateObjectKind::MaterializedView if self.field == 3 => {
                self.grant_preset = self.grant_preset.cycle(forward);
            }
            CreateObjectKind::Table => match self.table_zone() {
                TableZone::Acl if self.field == 3 => {
                    self.grant_preset = self.grant_preset.cycle(forward);
                }
                TableZone::Column => {
                    if let Some(col) = self.columns.get_mut(self.col_idx) {
                        match self.col_sub {
                            ColSub::Type => create_dialect::cycle_column_type(self.driver, &mut col.typ, forward),
                            ColSub::Null => col.nullable = !col.nullable,
                            ColSub::Pk => {
                                col.primary_key = !col.primary_key;
                                if col.primary_key {
                                    col.nullable = false;
                                }
                            }
                            ColSub::Ident => col.identity = !col.identity,
                            ColSub::Name => {}
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn add_column(&mut self) {
        if self.kind != CreateObjectKind::Table {
            return;
        }
        let n = self.columns.len() + 1;
        self.columns.push(ColumnDraft::new(&format!("col{n}"), "text"));
        self.col_idx = self.columns.len() - 1;
        self.col_sub = ColSub::Name;
        self.sync_table_field();
    }

    pub fn remove_column(&mut self) {
        if self.kind != CreateObjectKind::Table || self.columns.len() <= 1 {
            return;
        }
        if self.table_zone() != TableZone::Column {
            return;
        }
        self.columns.remove(self.col_idx);
        if self.col_idx >= self.columns.len() {
            self.col_idx = self.columns.len().saturating_sub(1);
        }
        self.col_sub = ColSub::Name;
        self.sync_table_field();
    }

    pub fn active_mut(&mut self) -> Option<&mut String> {
        match self.kind {
            CreateObjectKind::Database => match self.field {
                0 => Some(&mut self.name),
                1 => Some(&mut self.owner),
                2 => Some(&mut self.template),
                3 => Some(&mut self.encoding),
                4 => Some(&mut self.conn_limit),
                _ => None,
            },
            CreateObjectKind::Schema => match self.field {
                0 => Some(&mut self.name),
                1 => Some(&mut self.owner),
                3 => Some(&mut self.grant_role),
                _ => None,
            },
            CreateObjectKind::View | CreateObjectKind::MaterializedView => match self.field {
                0 => Some(&mut self.schema),
                1 => Some(&mut self.name),
                2 => Some(&mut self.owner),
                4 => Some(&mut self.grant_role),
                5 => Some(&mut self.body),
                _ => None,
            },
            CreateObjectKind::Index => match self.field {
                0 => Some(&mut self.schema),
                1 => Some(&mut self.name),
                2 => Some(&mut self.body),
                _ => None,
            },
            CreateObjectKind::Sequence => match self.field {
                0 => Some(&mut self.schema),
                1 => Some(&mut self.name),
                2 => Some(&mut self.owner),
                3 => Some(&mut self.body),
                _ => None,
            },
            CreateObjectKind::Extension => {
                if self.field == 0 {
                    Some(&mut self.name)
                } else {
                    None
                }
            }
            CreateObjectKind::Policy => match self.field {
                0 => Some(&mut self.name),
                1 => Some(&mut self.schema), // table qual
                3 => Some(&mut self.body),   // USING expr
                _ => None,
            },
            CreateObjectKind::Function => match self.field {
                0 => Some(&mut self.schema),
                1 => Some(&mut self.name),
                2 => Some(&mut self.body),
                _ => None,
            },
            CreateObjectKind::Publication => match self.field {
                0 => Some(&mut self.name),
                1 => Some(&mut self.body),
                _ => None,
            },
            CreateObjectKind::Tablespace => match self.field {
                0 => Some(&mut self.name),
                1 => Some(&mut self.body),
                _ => None,
            },
            CreateObjectKind::Role => match self.driver {
                DbDriver::Mysql => match self.field {
                    0 => Some(&mut self.name),
                    1 => Some(&mut self.template), // host
                    2 => Some(&mut self.password),
                    5 => Some(&mut self.body),
                    _ => None,
                },
                DbDriver::Mssql => match self.field {
                    0 => Some(&mut self.name),
                    1 => Some(&mut self.password),
                    _ => None,
                },
                _ => match self.field {
                    0 => Some(&mut self.name),
                    2 => Some(&mut self.password),
                    9 => Some(&mut self.conn_limit),
                    10 => Some(&mut self.valid_until),
                    11 => Some(&mut self.body),
                    _ => None,
                },
            },
            CreateObjectKind::Table => match self.table_zone() {
                TableZone::Header => {
                    if self.field == 0 {
                        Some(&mut self.schema)
                    } else {
                        Some(&mut self.name)
                    }
                }
                TableZone::Acl => match self.field {
                    2 => Some(&mut self.owner),
                    4 => Some(&mut self.grant_role),
                    _ => None,
                },
                TableZone::Column => {
                    let idx = self.col_idx;
                    let sub = self.col_sub;
                    self.columns.get_mut(idx).and_then(|c| match sub {
                        ColSub::Name => Some(&mut c.name),
                        _ => None,
                    })
                }
                TableZone::AddBtn => None,
            },
        }
    }

    pub fn push_char(&mut self, c: char) {
        if let Some(s) = self.active_mut() {
            s.push(c);
        }
    }

    pub fn backspace(&mut self) {
        if let Some(s) = self.active_mut() {
            s.pop();
        }
    }

    pub fn to_sql(&self) -> Result<String, String> {
        create_dialect::emit(self)
    }

    pub fn on_flag_like_field(&self) -> bool {
        match self.kind {
            CreateObjectKind::Index => matches!(self.field, 3 | 4),
            CreateObjectKind::Role => match self.driver {
                DbDriver::Mysql => matches!(self.field, 3 | 4),
                DbDriver::Mssql => self.field == 2,
                _ => matches!(self.field, 1 | 3 | 4 | 5 | 6 | 7 | 8),
            },
            CreateObjectKind::Schema => self.field == 2,
            CreateObjectKind::Policy => self.field == 2,
            CreateObjectKind::View | CreateObjectKind::MaterializedView => self.field == 3,
            CreateObjectKind::Table => match self.table_zone() {
                TableZone::Acl => self.field == 3,
                TableZone::Column => matches!(
                    self.col_sub,
                    ColSub::Type | ColSub::Null | ColSub::Pk | ColSub::Ident
                ),
                _ => false,
            },
            _ => false,
        }
    }

    pub fn on_add_button(&self) -> bool {
        self.kind == CreateObjectKind::Table && self.table_zone() == TableZone::AddBtn
    }

    /// Absolute yes/no for Role/Index toggles (used by y/n keys).
    pub fn set_bool_field(&mut self, yes: bool) {
        match self.kind {
            CreateObjectKind::Index => match self.field {
                3 => self.flag = yes,
                4 => self.concurrent = yes,
                _ => {}
            },
            CreateObjectKind::Role => match self.driver {
                DbDriver::Mysql => match self.field {
                    3 => self.superuser = yes,
                    4 => self.can_create_db = yes,
                    _ => {}
                },
                DbDriver::Mssql => {
                    if self.field == 2 {
                        self.superuser = yes;
                    }
                }
                _ => match self.field {
                    1 => self.flag = yes,
                    3 => self.inherit = yes,
                    4 => self.can_create_db = yes,
                    5 => self.can_create_role = yes,
                    6 => self.superuser = yes,
                    7 => self.replication = yes,
                    8 => self.bypass_rls = yes,
                    _ => {}
                },
            },
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableZone {
    Header,
    Acl,
    Column,
    AddBtn,
}

fn cycle_policy_cmd(cmd: &mut String, forward: bool) {
    const CMDS: &[&str] = &["ALL", "SELECT", "INSERT", "UPDATE", "DELETE"];
    let cur = if cmd.is_empty() { "ALL" } else { cmd.as_str() };
    let i = CMDS.iter().position(|c| *c == cur).unwrap_or(0);
    let n = if forward { (i + 1) % CMDS.len() } else { (i + CMDS.len() - 1) % CMDS.len() };
    *cmd = CMDS[n].to_string();
}

pub fn render_create_object_form(frame: &mut Frame, screen: Rect, form: &CreateObjectForm) {
    let width = screen.width.min(84).max(56);
    let height = match form.kind {
        CreateObjectKind::Table => (16 + form.columns.len() as u16)
            .min(screen.height.saturating_sub(2))
            .max(16),
        CreateObjectKind::Role => 22u16.min(screen.height.saturating_sub(2)).max(16),
        CreateObjectKind::Database => 16u16.min(screen.height.saturating_sub(2)).max(12),
        _ => 16u16.min(screen.height.saturating_sub(2)).max(12),
    };
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 6;
    let area = Rect {
        x,
        y,
        width,
        height,
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::border_focus()))
            .style(Style::default().bg(theme::bg()))
            .title(Span::styled(
                format!(" {} ", form.kind.title()),
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::text())
                    .add_modifier(Modifier::BOLD),
            )),
        area,
    );
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(area);

    let lines = if form.kind == CreateObjectKind::Table {
        render_table_builder_lines(form)
    } else {
        render_simple_lines(form)
    };
    frame.render_widget(Paragraph::new(lines), inner[0]);

    if let Some((msg, err)) = &form.feedback {
        let style = if *err {
            theme::style_err_badge()
        } else {
            theme::style_ok()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(format!(" {msg} "), style)),
            inner[1],
        );
    } else {
        let hint = create_dialect::footer_hint(form);
        frame.render_widget(
            Paragraph::new(Span::styled(
                hint,
                theme::style_faint().add_modifier(Modifier::ITALIC),
            )),
            inner[1],
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Enter ",
                Style::default()
                    .fg(theme::bg())
                    .bg(theme::text())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" create ", theme::style_faint()),
            Span::styled(" · Tab · Esc", theme::style_faint()),
        ])),
        inner[2],
    );
}

fn yn(v: bool) -> &'static str {
    if v {
        "yes"
    } else {
        "no"
    }
}

fn render_simple_lines(form: &CreateObjectForm) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let fields: Vec<(&str, String, bool)> = match form.kind {
        CreateObjectKind::Database => vec![
            ("Name", form.name.clone(), false),
            ("Owner", form.owner.clone(), false),
            ("Template", form.template.clone(), false),
            ("Encoding", form.encoding.clone(), false),
            ("Conn limit", form.conn_limit.clone(), false),
        ],
        CreateObjectKind::Schema => vec![
            ("Name", form.name.clone(), false),
            ("Owner", form.owner.clone(), false),
            ("Grants", form.grant_preset.label().into(), true),
            ("Grant to", form.grant_role.clone(), false),
        ],
        CreateObjectKind::View | CreateObjectKind::MaterializedView => vec![
            ("Schema", form.schema.clone(), false),
            ("Name", form.name.clone(), false),
            ("Owner", form.owner.clone(), false),
            ("Grants", form.grant_preset.label().into(), true),
            ("Grant to", form.grant_role.clone(), false),
            ("SELECT", form.body.clone(), false),
        ],
        CreateObjectKind::Index => vec![
            ("On", form.schema.clone(), false),
            ("Name", form.name.clone(), false),
            ("Columns", form.body.clone(), false),
            ("Unique", yn(form.flag).into(), true),
            ("Concurrent", yn(form.concurrent).into(), true),
        ],
        CreateObjectKind::Sequence => vec![
            ("Schema", form.schema.clone(), false),
            ("Name", form.name.clone(), false),
            ("Owner", form.owner.clone(), false),
            ("Start", form.body.clone(), false),
        ],
        CreateObjectKind::Extension => vec![("Name", form.name.clone(), false)],
        CreateObjectKind::Policy => vec![
            ("Name", form.name.clone(), false),
            ("On table", form.schema.clone(), false),
            ("For", if form.password.is_empty() { "ALL".into() } else { form.password.clone() }, true),
            ("USING", form.body.clone(), false),
        ],
        CreateObjectKind::Function => vec![
            ("Schema", form.schema.clone(), false),
            ("Name", form.name.clone(), false),
            ("Body", form.body.clone(), false),
        ],
        CreateObjectKind::Publication => vec![
            ("Name", form.name.clone(), false),
            ("Tables", form.body.clone(), false),
        ],
        CreateObjectKind::Tablespace => vec![
            ("Name", form.name.clone(), false),
            ("Location", form.body.clone(), false),
        ],
        CreateObjectKind::Role => match form.driver {
            DbDriver::Mysql => vec![
                ("Name", form.name.clone(), false),
                ("Host", form.template.clone(), false),
                ("Password", form.password.clone(), false),
                ("Superuser", yn(form.superuser).into(), true),
                ("CreateDB", yn(form.can_create_db).into(), true),
                ("Member of", form.body.clone(), false),
            ],
            DbDriver::Mssql => vec![
                ("Name", form.name.clone(), false),
                ("Password", form.password.clone(), false),
                ("Sysadmin", yn(form.superuser).into(), true),
            ],
            _ => vec![
                ("Name", form.name.clone(), false),
                ("Login", yn(form.flag).into(), true),
                ("Password", form.password.clone(), false),
                ("Inherit", yn(form.inherit).into(), true),
                ("CreateDB", yn(form.can_create_db).into(), true),
                ("CreateRole", yn(form.can_create_role).into(), true),
                ("Superuser", yn(form.superuser).into(), true),
                ("Replication", yn(form.replication).into(), true),
                ("BypassRLS", yn(form.bypass_rls).into(), true),
                ("Conn limit", form.conn_limit.clone(), false),
                ("Valid until", form.valid_until.clone(), false),
                ("Member of", form.body.clone(), false),
            ],
        },
        CreateObjectKind::Table => Vec::new(),
    };
    for (i, (label, value, _)) in fields.iter().enumerate() {
        let focused = i == form.field;
        let cursor = if focused { "›" } else { " " };
        let label_style = if focused {
            theme::style_text().add_modifier(Modifier::BOLD)
        } else {
            theme::style_faint()
        };
        let value_style = if focused {
            if (*label == "Superuser" || *label == "Sysadmin") && form.superuser {
                theme::style_err_badge()
            } else {
                theme::style_text().add_modifier(Modifier::BOLD)
            }
        } else if (*label == "Superuser" || *label == "Sysadmin") && form.superuser {
            theme::style_err_badge()
        } else {
            theme::style_muted()
        };
        let shown = if value.chars().count() > 42 {
            format!("{}…", value.chars().take(41).collect::<String>())
        } else {
            value.clone()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{cursor} "), theme::style_text()),
            Span::styled(format!("{label:<12}"), label_style),
            Span::styled(shown, value_style),
        ]));
    }
    lines
}

fn render_table_builder_lines(form: &CreateObjectForm) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (i, (label, value)) in [
        ("Schema", form.schema.as_str()),
        ("Name", form.name.as_str()),
        ("Owner", form.owner.as_str()),
        ("Grants", form.grant_preset.label()),
        ("Grant to", form.grant_role.as_str()),
    ]
    .iter()
    .enumerate()
    {
        let focused = form.field == i;
        let cursor = if focused { "›" } else { " " };
        let ls = if focused {
            theme::style_text().add_modifier(Modifier::BOLD)
        } else {
            theme::style_faint()
        };
        let vs = if focused {
            theme::style_text().add_modifier(Modifier::BOLD)
        } else {
            theme::style_muted()
        };
        let hint = if *label == "Schema" && focused {
            Span::styled("  · creates if missing", theme::style_faint())
        } else if *label == "Owner" && focused && value.is_empty() {
            Span::styled("  · session user", theme::style_faint())
        } else {
            Span::raw("")
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{cursor} "), theme::style_text()),
            Span::styled(format!("{label:<9}"), ls),
            Span::styled((*value).to_string(), vs),
            hint,
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  COLUMNS",
        theme::style_muted().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "     name            type           null  pk  ident",
        theme::style_faint(),
    )));

    for (ci, col) in form.columns.iter().enumerate() {
        let row_focus = form.table_zone() == TableZone::Column && form.col_idx == ci;
        let mark = if row_focus { "›" } else { " " };
        let cell = |sub: ColSub, text: String| {
            let on = row_focus && form.col_sub == sub;
            let style = if on {
                theme::style_selected()
            } else if row_focus {
                theme::style_text()
            } else {
                theme::style_muted()
            };
            Span::styled(text, style)
        };
        let null_s = if col.nullable { " yes" } else { "  no" };
        let pk_s = if col.primary_key { " yes" } else { "  no" };
        let id_s = if col.identity { " yes" } else { "  no" };
        lines.push(Line::from(vec![
            Span::styled(format!("{mark} "), theme::style_text()),
            cell(ColSub::Name, format!(" {:<14}", truncate(&col.name, 14))),
            cell(ColSub::Type, format!(" {:<14}", truncate(&col.typ, 14))),
            cell(ColSub::Null, format!(" {null_s}")),
            cell(ColSub::Pk, format!(" {pk_s}")),
            cell(ColSub::Ident, format!(" {id_s}")),
        ]));
    }

    let add_focus = form.table_zone() == TableZone::AddBtn;
    let add_style = if add_focus {
        theme::style_selected()
    } else {
        theme::style_faint()
    };
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(if add_focus { "› " } else { "  " }, theme::style_text()),
        Span::styled(" + add column ", add_style),
    ]));
    lines
}

fn truncate(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        s.to_string()
    } else if max <= 1 {
        "…".into()
    } else {
        let mut o: String = s.chars().take(max - 1).collect();
        o.push('…');
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_builder_ddl_per_column() {
        let mut form = CreateObjectForm::fresh(CreateObjectKind::Table, "hello");
        form.add_column();
        form.columns[2].name = "email".into();
        form.columns[2].typ = "text".into();
        form.columns[2].nullable = true;
        let sql = form.to_sql().unwrap();
        assert!(sql.contains("CREATE TABLE hello.new_table"));
        assert!(sql.contains("REVOKE ALL ON TABLE hello.new_table FROM PUBLIC;"));
    }

    #[test]
    fn table_creates_missing_schema_with_owner_and_grants() {
        let mut form =
            CreateObjectForm::fresh_with_owner(CreateObjectKind::Table, "publicsdsad", "app_owner", DbDriver::Postgres);
        form.name = "t1".into();
        form.grant_preset = GrantPreset::Readonly;
        form.grant_role = "app_ro".into();
        let sql = form.to_sql().unwrap();
        assert!(sql.contains("AUTHORIZATION app_owner"));
        assert!(sql.contains("GRANT SELECT ON TABLE publicsdsad.t1 TO app_ro;"));
    }

    #[test]
    fn role_safe_defaults_and_superuser_opt_in() {
        let mut form = CreateObjectForm::fresh(CreateObjectKind::Role, "public");
        form.name = "app_user".into();
        form.flag = true;
        form.password = "s3cret".into();
        form.body = "app_rw".into();
        let sql = form.to_sql().unwrap();
        assert!(sql.contains("NOSUPERUSER"));
        assert!(sql.contains("NOREPLICATION"));
        assert!(sql.contains("NOBYPASSRLS"));
        assert!(!sql.contains(" SUPERUSER"));
        assert!(sql.contains("GRANT app_rw TO app_user;"));

        form.superuser = true;
        form.replication = true;
        form.bypass_rls = true;
        form.conn_limit = "3".into();
        form.valid_until = "2030-01-01".into();
        let sql2 = form.to_sql().unwrap();
        assert!(sql2.contains(" SUPERUSER"));
        assert!(sql2.contains(" REPLICATION"));
        assert!(sql2.contains(" BYPASSRLS"));
        assert!(sql2.contains(" CONNECTION LIMIT 3"));
        assert!(sql2.contains("VALID UNTIL '2030-01-01'"));
    }

    #[test]
    fn schema_authorization_and_usage() {
        let mut form =
            CreateObjectForm::fresh_with_owner(CreateObjectKind::Schema, "public", "app_owner", DbDriver::Postgres);
        form.name = "billing".into();
        form.grant_preset = GrantPreset::Readwrite;
        form.grant_role = "app_rw".into();
        let sql = form.to_sql().unwrap();
        assert!(sql.contains("CREATE SCHEMA billing AUTHORIZATION app_owner;"));
        assert!(sql.contains("GRANT CREATE ON SCHEMA billing TO app_rw;"));
    }

    #[test]
    fn index_concurrent_and_extension_sequence_matview() {
        let mut idx = CreateObjectForm::fresh(CreateObjectKind::Index, "public");
        idx.schema = "public.t".into();
        idx.name = "t_id_idx".into();
        idx.body = "id".into();
        idx.concurrent = true;
        assert!(idx
            .to_sql()
            .unwrap()
            .contains("CREATE INDEX CONCURRENTLY t_id_idx ON public.t (id);"));

        let ext = CreateObjectForm::fresh(CreateObjectKind::Extension, "public");
        assert_eq!(
            ext.to_sql().unwrap(),
            "CREATE EXTENSION IF NOT EXISTS pgcrypto;"
        );

        let mut seq =
            CreateObjectForm::fresh_with_owner(CreateObjectKind::Sequence, "hello", "app_owner", DbDriver::Postgres);
        seq.name = "ord_seq".into();
        seq.body = "100".into();
        let s = seq.to_sql().unwrap();
        assert!(s.contains("CREATE SEQUENCE hello.ord_seq START WITH 100;"));
        assert!(s.contains("OWNER TO app_owner"));

        let mut mv = CreateObjectForm::fresh(CreateObjectKind::MaterializedView, "hello");
        mv.name = "mv1".into();
        let m = mv.to_sql().unwrap();
        assert!(m.contains("CREATE MATERIALIZED VIEW hello.mv1 AS"));
        assert!(m.contains("WITH DATA;"));
    }


    #[test]
    fn mysql_table_and_user_ddl() {
        let mut form = CreateObjectForm::fresh_with_owner(
            CreateObjectKind::Table, "appdb", "root", DbDriver::Mysql,
        );
        form.name = "t1".into();
        form.schema = "appdb".into();
        let sql = form.to_sql().unwrap();
        assert!(sql.contains("ENGINE=InnoDB"));
        assert!(sql.contains("AUTO_INCREMENT"));
        let mut role = CreateObjectForm::fresh_with_owner(
            CreateObjectKind::Role, "", "", DbDriver::Mysql,
        );
        role.name = "app".into();
        role.password = "x".into();
        role.template = "%".into();
        let r = role.to_sql().unwrap();
        assert!(r.contains("CREATE USER"));
        assert!(r.contains("IDENTIFIED BY"));
    }

    #[test]
    fn mssql_sqlite_clickhouse_table_ddl() {
        let mut ms = CreateObjectForm::fresh_with_owner(
            CreateObjectKind::Table, "dbo", "sa", DbDriver::Mssql,
        );
        ms.name = "t1".into();
        ms.grant_preset = GrantPreset::Readonly;
        ms.grant_role = "app_ro".into();
        let s = ms.to_sql().unwrap();
        assert!(s.contains("IDENTITY(1,1)") || s.contains("CREATE TABLE"));
        assert!(s.contains("GRANT SELECT ON") && s.contains("TO [app_ro]"));
        let mut sq = CreateObjectForm::fresh_with_owner(
            CreateObjectKind::Table, "main", "", DbDriver::Sqlite,
        );
        sq.name = "t1".into();
        assert!(sq.to_sql().unwrap().contains("AUTOINCREMENT"));
        let mut ch = CreateObjectForm::fresh_with_owner(
            CreateObjectKind::Table, "default", "", DbDriver::Clickhouse,
        );
        ch.name = "t1".into();
        ch.grant_preset = GrantPreset::Readwrite;
        ch.grant_role = "analyst".into();
        let c = ch.to_sql().unwrap();
        assert!(c.contains("MergeTree"));
        assert!(c.contains("ORDER BY"));
        assert!(c.contains("GRANT SELECT, INSERT, ALTER, DELETE ON") && c.contains("TO `analyst`"));

        let mut chu = CreateObjectForm::fresh_with_owner(
            CreateObjectKind::Role, "", "", DbDriver::Clickhouse,
        );
        chu.name = "svc".into();
        chu.password = "pw".into();
        let u = chu.to_sql().unwrap();
        assert!(u.contains("CREATE USER IF NOT EXISTS"));
        assert!(u.contains("sha256_password"));
    }

    #[test]
    fn cycle_type_left_right() {
        let mut form = CreateObjectForm::fresh(CreateObjectKind::Table, "hello");
        form.field = 5;
        form.col_idx = 0;
        form.col_sub = ColSub::Type;
        form.nudge_flag(true);
        assert_eq!(form.columns[0].typ, "integer");
    }

    #[test]
    fn policy_function_publication_tablespace_ddl() {
        let mut pol = CreateObjectForm::fresh_with_owner(
            CreateObjectKind::Policy,
            "public",
            "",
            DbDriver::Postgres,
        );
        pol.name = "tenant_iso".into();
        pol.schema = "app.orders".into();
        pol.password = "SELECT".into();
        pol.body = "tenant_id = current_setting('app.tenant')::uuid".into();
        let p = pol.to_sql().unwrap();
        assert!(p.contains("ENABLE ROW LEVEL SECURITY"));
        assert!(p.contains("CREATE POLICY tenant_iso ON app.orders"));
        assert!(p.contains("FOR SELECT"));

        let mut fun = CreateObjectForm::fresh_with_owner(
            CreateObjectKind::Function,
            "public",
            "",
            DbDriver::Postgres,
        );
        fun.name = "ping".into();
        fun.body = "SELECT 1".into();
        assert!(fun.to_sql().unwrap().contains("CREATE OR REPLACE FUNCTION public.ping()"));

        let mut pub_ = CreateObjectForm::fresh(CreateObjectKind::Publication, "public");
        pub_.name = "pub1".into();
        pub_.body = "TABLE public.t".into();
        assert!(pub_.to_sql().unwrap().contains("CREATE PUBLICATION pub1 FOR TABLE public.t"));

        let mut ts = CreateObjectForm::fresh(CreateObjectKind::Tablespace, "");
        ts.name = "fast".into();
        ts.body = "/data/pg_tspace".into();
        assert!(ts
            .to_sql()
            .unwrap()
            .contains("CREATE TABLESPACE fast LOCATION '/data/pg_tspace'"));
    }
}
