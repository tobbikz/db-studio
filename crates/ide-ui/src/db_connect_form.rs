//! Interactive DataGrip/DBeaver-style connection form.

use crate::theme;
use ide_db::{DataSource, DbDriver, SourceRole, password_display_hint};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnField {
    Name,
    Driver,
    Host,
    Port,
    User,
    Password,
    Database,
    File,
    SslMode,
    SslTrustCert,
    SslCa,
    ReadOnly,
    Role,
    SshHost,
    SshUser,
    SshPort,
    SshKey,
    Group,
    Accent,
}

impl ConnField {
    pub const ALL: [ConnField; 19] = [
        Self::Name,
        Self::Group,
        Self::Accent,
        Self::Driver,
        Self::Host,
        Self::Port,
        Self::User,
        Self::Password,
        Self::Database,
        Self::File,
        Self::SslMode,
        Self::SslTrustCert,
        Self::SslCa,
        Self::ReadOnly,
        Self::Role,
        Self::SshHost,
        Self::SshUser,
        Self::SshPort,
        Self::SshKey,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Group => "Group",
            Self::Accent => "Accent",
            Self::Driver => "Driver",
            Self::Host => "Host",
            Self::Port => "Port",
            Self::User => "User",
            Self::Password => "Password",
            Self::Database => "Database",
            Self::File => "File",
            Self::SslMode => "SSL mode",
            Self::SslTrustCert => "Trust cert",
            Self::SslCa => "SSL CA",
            Self::ReadOnly => "Read-only",
            Self::Role => "Role",
            Self::SshHost => "SSH host",
            Self::SshUser => "SSH user",
            Self::SshPort => "SSH port",
            Self::SshKey => "SSH key",
        }
    }

    pub fn hint(self, driver: DbDriver) -> &'static str {
        match self {
            Self::Name => "display name",
            Self::Group => "Explorer folder (optional)",
            Self::Accent => "#RRGGBB or cyan/green/magenta/yellow",
            Self::Driver => "← → PG · MySQL · SQLite · … · ES · Trino",
            Self::Host => "DB host (as seen from jump / local)",
            Self::Port => "5432 / 3306 / 1433 / 8123 / 9200 / 8080",
            Self::User => "login user",
            Self::Password => "env:VAR · vault · or type new",
            Self::Database => "database / schema name",
            Self::File => {
                if driver == DbDriver::Sqlite {
                    "path.db or :memory:"
                } else {
                    "SQLite only"
                }
            }
            Self::SslMode => "← → disable · prefer · require · verify-full",
            Self::SslTrustCert => "← → yes/no (MSSQL server cert)",
            Self::SslCa => "optional PEM path (verify modes)",
            Self::ReadOnly => "← → block writes on this source",
            Self::Role => "← → viewer · editor · admin",
            Self::SshHost => "optional jump host (empty = direct)",
            Self::SshUser => "optional (default: local user)",
            Self::SshPort => "22",
            Self::SshKey => "optional -i identity file",
        }
    }

    fn next(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone)]
pub struct DbConnectForm {
    pub is_edit: bool,
    pub name: String,
    pub group: String,
    pub accent: String,
    pub driver: DbDriver,
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub database: String,
    pub file: String,
    pub ssl_mode: String,
    pub ssl_trust_cert: bool,
    pub ssl_ca: String,
    pub read_only: bool,
    pub role: SourceRole,
    /// Source id when editing — for vault hint without replaying secrets.
    pub source_id: String,
    pub ssh_host: String,
    pub ssh_user: String,
    pub ssh_port: String,
    pub ssh_key: String,
    pub focus: ConnField,
}

impl Default for DbConnectForm {
    fn default() -> Self {
        Self {
            is_edit: false,
            name: String::new(),
            group: String::new(),
            accent: String::new(),
            driver: DbDriver::Postgres,
            host: "127.0.0.1".into(),
            port: "5432".into(),
            user: String::new(),
            password: String::new(),
            database: String::new(),
            file: String::new(),
            ssl_mode: "prefer".into(),
            ssl_trust_cert: true,
            ssl_ca: String::new(),
            read_only: false,
            role: SourceRole::Admin,
            source_id: String::new(),
            ssh_host: String::new(),
            ssh_user: String::new(),
            ssh_port: "22".into(),
            ssh_key: String::new(),
            focus: ConnField::Name,
        }
    }
}

impl DbConnectForm {
    pub fn fresh() -> Self {
        let mut f = Self::default();
        f.user = whoami_user();
        f
    }

    pub fn from_source(src: &DataSource) -> Self {
        Self {
            is_edit: true,
            name: src.name.clone(),
            group: src.group.clone(),
            accent: src.accent.clone(),
            driver: src.driver,
            host: src.host.clone(),
            port: if src.port == 0 {
                String::new()
            } else {
                src.port.to_string()
            },
            user: src.user.clone(),
            // Never replay plaintext — env: kept; vault shown via hint.
            password: if src.password.trim().starts_with("env:") {
                src.password.clone()
            } else {
                String::new()
            },
            database: src.database.clone(),
            file: src.file.clone(),
            ssl_mode: if src.ssl_mode.is_empty() {
                "prefer".into()
            } else {
                src.ssl_mode.clone()
            },
            ssl_trust_cert: src.ssl_trust_cert,
            ssl_ca: src.ssl_ca.clone(),
            read_only: src.read_only,
            role: src.role,
            source_id: src.id.clone(),
            ssh_host: src.ssh_host.clone(),
            ssh_user: src.ssh_user.clone(),
            ssh_port: src.ssh_port.to_string(),
            ssh_key: src.ssh_key.clone(),
            focus: ConnField::Name,
        }
    }

    pub fn next_field(&mut self) {
        self.focus = self.focus.next();
        self.skip_irrelevant_forward();
    }

    pub fn prev_field(&mut self) {
        self.focus = self.focus.prev();
        self.skip_irrelevant_backward();
    }

    fn skip_irrelevant_forward(&mut self) {
        for _ in 0..ConnField::ALL.len() {
            if self.field_enabled(self.focus) {
                return;
            }
            self.focus = self.focus.next();
        }
    }

    fn skip_irrelevant_backward(&mut self) {
        for _ in 0..ConnField::ALL.len() {
            if self.field_enabled(self.focus) {
                return;
            }
            self.focus = self.focus.prev();
        }
    }

    pub fn field_enabled(&self, field: ConnField) -> bool {
        match self.driver {
            DbDriver::Sqlite | DbDriver::Duckdb => matches!(
                field,
                ConnField::Name
                    | ConnField::Group
                    | ConnField::Accent
                    | ConnField::Driver
                    | ConnField::File
                    | ConnField::ReadOnly
                    | ConnField::Role
            ),
            DbDriver::Redis => matches!(
                field,
                ConnField::Name
                    | ConnField::Group
                    | ConnField::Accent
                    | ConnField::Driver
                    | ConnField::Host
                    | ConnField::Port
                    | ConnField::Password
                    | ConnField::Database
                    | ConnField::ReadOnly
                    | ConnField::Role
            ),
            DbDriver::Mongo
            | DbDriver::Elasticsearch
            | DbDriver::Trino => matches!(
                field,
                ConnField::Name
                    | ConnField::Group
                    | ConnField::Accent
                    | ConnField::Driver
                    | ConnField::Host
                    | ConnField::Port
                    | ConnField::User
                    | ConnField::Password
                    | ConnField::Database
                    | ConnField::SslMode
                    | ConnField::ReadOnly
                    | ConnField::Role
            ),
            DbDriver::Mssql => !matches!(
                field,
                ConnField::File | ConnField::SslMode | ConnField::SslCa
            ),
            DbDriver::Postgres
            | DbDriver::Mysql
            | DbDriver::Clickhouse
            | DbDriver::Snowflake
            | DbDriver::Bigquery => !matches!(field, ConnField::File | ConnField::SslTrustCert),
        }
    }

    pub fn on_driver_field(&self) -> bool {
        self.focus == ConnField::Driver
    }

    pub fn on_ssl_mode_field(&self) -> bool {
        self.focus == ConnField::SslMode
    }

    pub fn on_cycle_bool_field(&self) -> bool {
        matches!(
            self.focus,
            ConnField::SslTrustCert | ConnField::ReadOnly
        )
    }

    pub fn on_role_field(&self) -> bool {
        self.focus == ConnField::Role
    }

    pub fn cycle_driver(&mut self, forward: bool) {
        const ORDER: [DbDriver; 12] = [
            DbDriver::Postgres,
            DbDriver::Mysql,
            DbDriver::Sqlite,
            DbDriver::Duckdb,
            DbDriver::Mssql,
            DbDriver::Clickhouse,
            DbDriver::Redis,
            DbDriver::Mongo,
            DbDriver::Snowflake,
            DbDriver::Bigquery,
            DbDriver::Elasticsearch,
            DbDriver::Trino,
        ];
        let idx = ORDER
            .iter()
            .position(|d| *d == self.driver)
            .unwrap_or(0);
        let next = if forward {
            (idx + 1) % ORDER.len()
        } else {
            (idx + ORDER.len() - 1) % ORDER.len()
        };
        self.driver = ORDER[next];
        match self.driver {
            DbDriver::Postgres => self.port = "5432".into(),
            DbDriver::Mysql => self.port = "3306".into(),
            DbDriver::Mssql => self.port = "1433".into(),
            DbDriver::Clickhouse => self.port = "8123".into(),
            DbDriver::Redis => self.port = "6379".into(),
            DbDriver::Mongo => self.port = "27017".into(),
            DbDriver::Elasticsearch => self.port = "9200".into(),
            DbDriver::Trino => self.port = "8080".into(),
            DbDriver::Sqlite | DbDriver::Duckdb => {}
            DbDriver::Snowflake | DbDriver::Bigquery => {}
        }
        if !self.field_enabled(self.focus) {
            self.focus = ConnField::Name;
        }
    }

    pub fn cycle_role(&mut self, forward: bool) {
        self.role = self.role.cycle(forward);
    }

    pub fn cycle_ssl_mode(&mut self, forward: bool) {
        const MODES: [&str; 4] = ["disable", "prefer", "require", "verify-full"];
        let idx = MODES
            .iter()
            .position(|m| *m == self.ssl_mode.as_str())
            .unwrap_or(1);
        let next = if forward {
            (idx + 1) % MODES.len()
        } else {
            (idx + MODES.len() - 1) % MODES.len()
        };
        self.ssl_mode = MODES[next].into();
    }

    pub fn cycle_bool_field(&mut self, forward: bool) {
        match self.focus {
            ConnField::SslTrustCert => self.ssl_trust_cert = forward,
            ConnField::ReadOnly => self.read_only = forward,
            _ => {}
        }
    }

    pub fn active_value_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            ConnField::Name => Some(&mut self.name),
            ConnField::Group => Some(&mut self.group),
            ConnField::Accent => Some(&mut self.accent),
            ConnField::Driver => None,
            ConnField::Host => Some(&mut self.host),
            ConnField::Port => Some(&mut self.port),
            ConnField::User => Some(&mut self.user),
            ConnField::Password => Some(&mut self.password),
            ConnField::Database => Some(&mut self.database),
            ConnField::File => Some(&mut self.file),
            ConnField::SslMode | ConnField::SslTrustCert | ConnField::ReadOnly | ConnField::Role => {
                None
            }
            ConnField::SslCa => Some(&mut self.ssl_ca),
            ConnField::SshHost => Some(&mut self.ssh_host),
            ConnField::SshUser => Some(&mut self.ssh_user),
            ConnField::SshPort => Some(&mut self.ssh_port),
            ConnField::SshKey => Some(&mut self.ssh_key),
        }
    }

    pub fn push_char(&mut self, c: char) {
        if matches!(
            self.focus,
            ConnField::Driver
                | ConnField::SslMode
                | ConnField::SslTrustCert
                | ConnField::ReadOnly
                | ConnField::Role
        ) {
            return;
        }
        if let Some(v) = self.active_value_mut() {
            v.push(c);
        }
    }

    pub fn backspace(&mut self) {
        if let Some(v) = self.active_value_mut() {
            v.pop();
        }
    }

    fn display_value(&self, field: ConnField) -> String {
        match field {
            ConnField::Name => self.name.clone(),
            ConnField::Group => self.group.clone(),
            ConnField::Accent => self.accent.clone(),
            ConnField::Driver => self.driver.label().to_string(),
            ConnField::Host => self.host.clone(),
            ConnField::Port => self.port.clone(),
            ConnField::User => self.user.clone(),
            ConnField::Password => {
                let typed = self.password.trim();
                if typed.starts_with("env:") {
                    typed.to_string()
                } else if !typed.is_empty() {
                    "•".repeat(typed.chars().count())
                } else {
                    password_display_hint(&self.source_id, "")
                }
            }
            ConnField::Database => self.database.clone(),
            ConnField::File => self.file.clone(),
            ConnField::SslMode => self.ssl_mode.clone(),
            ConnField::SslTrustCert => {
                if self.ssl_trust_cert {
                    "yes".into()
                } else {
                    "no".into()
                }
            }
            ConnField::SslCa => self.ssl_ca.clone(),
            ConnField::ReadOnly => {
                if self.read_only {
                    "yes".into()
                } else {
                    "no".into()
                }
            }
            ConnField::Role => self.role.label().to_string(),
            ConnField::SshHost => self.ssh_host.clone(),
            ConnField::SshUser => self.ssh_user.clone(),
            ConnField::SshPort => self.ssh_port.clone(),
            ConnField::SshKey => self.ssh_key.clone(),
        }
    }
}

fn whoami_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into())
}

pub fn render_db_connect_form(frame: &mut Frame, screen: Rect, form: &DbConnectForm) {
    let width = screen.width.min(68).max(48);
    let height = screen.height.min(26).max(18);
    let x = screen.x + (screen.width.saturating_sub(width)) / 2;
    let y = screen.y + (screen.height.saturating_sub(height)) / 5;
    let area = Rect {
        x,
        y,
        width,
        height,
    };

    let title = if form.is_edit {
        " DB Studio · edit connection "
    } else {
        " DB Studio · new connection · Tab fields · Enter save "
    };

    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme::border_idle()))
            .style(Style::default().bg(theme::surface()))
            .title(title),
        area,
    );

    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(area);

    frame.render_widget(
        Paragraph::new(Span::styled(
            "  Data source  ·  TLS + read-only  ·  SSH jump optional",
            theme::style_muted(),
        )),
        inner[0],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "  ─────────────────────────────────────────────",
            theme::style_faint(),
        )),
        inner[1],
    );

    let mut lines: Vec<Line> = Vec::new();
    for field in ConnField::ALL {
        if !form.field_enabled(field) {
            continue;
        }
        let focused = form.focus == field;
        let marker = if focused { "▸" } else { " " };
        let label = format!("{:<12}", field.label());
        let value = form.display_value(field);
        let value_shown = if value.is_empty() {
            field.hint(form.driver).to_string()
        } else if focused {
            format!("{value}▌")
        } else {
            value
        };

        let label_style = if focused {
            Style::default()
                .fg(theme::text())
                .add_modifier(Modifier::BOLD)
        } else {
            theme::style_muted()
        };
        let value_style = if focused {
            Style::default()
                .fg(theme::text())
                .bg(theme::surface_2())
                .add_modifier(Modifier::BOLD)
        } else if form.display_value(field).is_empty() {
            theme::style_faint()
        } else {
            theme::style_text()
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!(" {marker} "),
                if focused {
                    theme::style_accent()
                } else {
                    theme::style_faint()
                },
            ),
            Span::styled(label, label_style),
            Span::styled(" ", theme::style_faint()),
            Span::styled(format!(" {value_shown} "), value_style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner[2]);

    frame.render_widget(
        Paragraph::new(Span::styled(
            "  ─────────────────────────────────────────────",
            theme::style_faint(),
        )),
        inner[3],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  ", theme::style_faint()),
            Span::styled("Tab", theme::style_pill_off()),
            Span::styled(" field  ", theme::style_faint()),
            Span::styled("←→", theme::style_pill_off()),
            Span::styled(" cycle  ", theme::style_faint()),
            Span::styled("Enter", theme::style_pill_off()),
            Span::styled(" save  ", theme::style_faint()),
            Span::styled("Ctrl-T", theme::style_pill_off()),
            Span::styled("/F6 test  ", theme::style_faint()),
            Span::styled("Esc", theme::style_pill_off()),
            Span::styled(" cancel", theme::style_faint()),
        ])),
        inner[4],
    );
}
